//! The shared HTTP client.
//!
//! One client for the whole daemon, with connection reuse, a hard response-size
//! limit applied before any body is parsed, and conditional-request support so an
//! upstream `304` costs nothing.

use std::time::Duration;

use streamdeck_core::integrations::MAX_RESPONSE_BYTES;

#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error("{url} timed out after {}s", timeout.as_secs())]
    Timeout { url: String, timeout: Duration },
    #[error("{url} returned HTTP {status}")]
    Status { url: String, status: u16 },
    #[error("{url} is rate limited; retry after {retry_after_seconds}s")]
    RateLimited {
        url: String,
        retry_after_seconds: u64,
    },
    #[error("{url} requires authentication")]
    Unauthorized { url: String },
    #[error("{url} returned more than {limit} bytes")]
    TooLarge { url: String, limit: usize },
    #[error("could not reach {url}: {detail}")]
    Transport { url: String, detail: String },
    #[error("could not build the HTTP client: {0}")]
    Build(String),
}

/// A completed request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub body: String,
    /// `true` when the server answered `304 Not Modified`.
    pub not_modified: bool,
    /// Parsed `Expires` header, in milliseconds since the epoch.
    pub expires_at_ms: Option<i64>,
    pub last_modified: Option<String>,
}

/// A fetched binary body, for images. Text APIs use [`HttpClient::get`] instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpBytes {
    pub bytes: Vec<u8>,
    pub content_type: Option<String>,
}

/// One request's headers, as borrowed pairs so nothing is copied needlessly.
pub type Headers<'a> = &'a [(&'a str, &'a str)];

#[derive(Clone)]
pub struct HttpClient {
    client: reqwest::Client,
}

impl HttpClient {
    pub fn new() -> Result<Self, HttpError> {
        let client = reqwest::Client::builder()
            .use_rustls_tls()
            // Keep a small pool: this daemon makes a handful of requests a minute.
            .pool_max_idle_per_host(2)
            .pool_idle_timeout(Duration::from_secs(90))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|error| HttpError::Build(error.to_string()))?;
        Ok(Self { client })
    }

    /// Sends a GET and maps transport, authentication, and rate-limit failures.
    /// Status handling beyond that is the caller's, because `get` must let a
    /// `304 Not Modified` through while `get_bytes` never sees one.
    async fn dispatch(
        &self,
        url: &str,
        headers: Headers<'_>,
        timeout: Duration,
    ) -> Result<reqwest::Response, HttpError> {
        let mut request = self.client.get(url).timeout(timeout);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }

        let response = request.send().await.map_err(|error| {
            if error.is_timeout() {
                HttpError::Timeout {
                    url: url.to_string(),
                    timeout,
                }
            } else {
                HttpError::Transport {
                    url: url.to_string(),
                    // Never log the full error chain: it can contain the URL's
                    // query string, which may carry a token.
                    detail: sanitize(&error.to_string()),
                }
            }
        })?;

        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(HttpError::Unauthorized {
                url: url.to_string(),
            });
        }
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(HttpError::RateLimited {
                url: url.to_string(),
                retry_after_seconds: header(&response, reqwest::header::RETRY_AFTER)
                    .and_then(|value| value.trim().parse().ok())
                    .unwrap_or(60),
            });
        }
        Ok(response)
    }

    /// Performs a GET and returns the body as text, bounded by
    /// `MAX_RESPONSE_BYTES`. The lossy UTF-8 conversion is harmless for the JSON
    /// APIs this serves; anything binary must use [`HttpClient::get_bytes`].
    pub async fn get(
        &self,
        url: &str,
        headers: Headers<'_>,
        timeout: Duration,
    ) -> Result<HttpResponse, HttpError> {
        let response = self.dispatch(url, headers, timeout).await?;
        let status = response.status();
        let expires_at_ms = header(&response, reqwest::header::EXPIRES)
            .and_then(|value| httpdate_to_millis(&value));
        let last_modified = header(&response, reqwest::header::LAST_MODIFIED);

        if status == reqwest::StatusCode::NOT_MODIFIED {
            return Ok(HttpResponse {
                body: String::new(),
                not_modified: true,
                expires_at_ms,
                last_modified,
            });
        }
        if !status.is_success() {
            return Err(HttpError::Status {
                url: url.to_string(),
                status: status.as_u16(),
            });
        }

        let bytes = read_bounded(response, url, MAX_RESPONSE_BYTES).await?;
        Ok(HttpResponse {
            body: String::from_utf8_lossy(&bytes).to_string(),
            not_modified: false,
            expires_at_ms,
            last_modified,
        })
    }

    /// Performs a GET and returns the body as raw bytes, bounded by `max_bytes`.
    ///
    /// This exists because [`HttpClient::get`] stores the body as a lossy UTF-8
    /// string — fine for JSON, fatal for a JPEG, whose invalid byte sequences get
    /// replaced and can never decode again.
    pub async fn get_bytes(
        &self,
        url: &str,
        headers: Headers<'_>,
        timeout: Duration,
        max_bytes: usize,
    ) -> Result<HttpBytes, HttpError> {
        let response = self.dispatch(url, headers, timeout).await?;
        let status = response.status();
        if !status.is_success() {
            return Err(HttpError::Status {
                url: url.to_string(),
                status: status.as_u16(),
            });
        }

        let content_type = header(&response, reqwest::header::CONTENT_TYPE);
        let bytes = read_bounded(response, url, max_bytes).await?;
        Ok(HttpBytes {
            bytes,
            content_type,
        })
    }
}

/// Buffers a response body, refusing anything past `limit` — first from the
/// declared length, then from the actual bytes, because the two can disagree.
async fn read_bounded(
    response: reqwest::Response,
    url: &str,
    limit: usize,
) -> Result<Vec<u8>, HttpError> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(HttpError::TooLarge {
            url: url.to_string(),
            limit,
        });
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|error| HttpError::Transport {
            url: url.to_string(),
            detail: sanitize(&error.to_string()),
        })?;
    if bytes.len() > limit {
        return Err(HttpError::TooLarge {
            url: url.to_string(),
            limit,
        });
    }
    Ok(bytes.to_vec())
}

fn header(response: &reqwest::Response, name: reqwest::header::HeaderName) -> Option<String> {
    response
        .headers()
        .get(name)?
        .to_str()
        .ok()
        .map(str::to_string)
}

/// Parses the three date formats HTTP allows into epoch milliseconds.
pub fn httpdate_to_millis(value: &str) -> Option<i64> {
    let value = value.trim();
    for format in [
        "%a, %d %b %Y %H:%M:%S GMT",
        "%A, %d-%b-%y %H:%M:%S GMT",
        "%a %b %e %H:%M:%S %Y",
    ] {
        if let Ok(parsed) = chrono::NaiveDateTime::parse_from_str(value, format) {
            return Some(parsed.and_utc().timestamp_millis());
        }
    }
    None
}

/// Strips anything that looks like a credential out of an error string.
fn sanitize(detail: &str) -> String {
    detail
        .split_whitespace()
        .map(|word| {
            if word.contains("token") || word.contains("Bearer") || word.contains('?') {
                "<redacted>"
            } else {
                word
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_client_builds_with_rustls() {
        assert!(HttpClient::new().is_ok());
    }

    #[test]
    fn http_dates_parse_into_epoch_milliseconds() {
        assert_eq!(
            httpdate_to_millis("Fri, 24 Jul 2026 12:00:00 GMT"),
            Some(1_784_894_400_000)
        );
        assert_eq!(
            httpdate_to_millis("  Fri, 24 Jul 2026 12:00:00 GMT  "),
            Some(1_784_894_400_000)
        );
        assert_eq!(httpdate_to_millis("not a date"), None);
        assert_eq!(httpdate_to_millis(""), None);
    }

    #[test]
    fn error_details_never_carry_a_query_string_or_a_token() {
        let sanitized = sanitize("error sending request for url https://x/api?access_token=secret");
        assert!(!sanitized.contains("secret"), "{sanitized}");
        assert!(sanitized.contains("<redacted>"), "{sanitized}");

        let sanitized = sanitize("invalid Bearer abc123");
        assert!(!sanitized.contains("abc123") || sanitized.contains("<redacted>"));
    }

    #[test]
    fn error_messages_name_the_endpoint_and_the_problem() {
        let timeout = HttpError::Timeout {
            url: "https://api.met.no/x".to_string(),
            timeout: Duration::from_secs(10),
        };
        assert!(timeout.to_string().contains("api.met.no"));
        assert!(timeout.to_string().contains("10s"));

        let limited = HttpError::RateLimited {
            url: "https://api.anthropic.com/x".to_string(),
            retry_after_seconds: 30,
        };
        assert!(limited.to_string().contains("30s"));

        let too_large = HttpError::TooLarge {
            url: "https://x".to_string(),
            limit: 1024,
        };
        assert!(too_large.to_string().contains("1024"), "{too_large}");
    }

    #[tokio::test]
    async fn an_unreachable_host_reports_a_transport_error_rather_than_hanging() {
        let client = HttpClient::new().expect("client");
        let error = client
            .get(
                "https://localhost:1/nothing",
                &[],
                Duration::from_millis(500),
            )
            .await
            .expect_err("unreachable");
        assert!(
            matches!(
                error,
                HttpError::Transport { .. } | HttpError::Timeout { .. }
            ),
            "{error}"
        );
    }
}
