//! Codex usage parsing.
//!
//! This endpoint is not a stable public API, so the parser is deliberately
//! isolated behind fixtures: a shape change must fail loudly in one place rather
//! than corrupting a tile.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{parse_json, ParseError};

const INTEGRATION: &str = "codex-usage";

pub const ENDPOINT: &str = "https://chatgpt.com/backend-api/wham/usage";

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CodexWindow {
    /// Percentage of the window consumed, `0..=100`.
    pub percent: f64,
    /// Length of the window in seconds, used to label it as `5H` or `7D`.
    pub window_seconds: u64,
    pub resets_at: Option<DateTime<Utc>>,
}

impl CodexWindow {
    /// A short label for the window length: `5H`, `7D`, `30D`.
    pub fn window_label(&self) -> String {
        let hours = self.window_seconds / 3_600;
        if hours >= 24 {
            format!("{}D", hours / 24)
        } else if hours >= 1 {
            format!("{hours}H")
        } else {
            format!("{}M", self.window_seconds / 60)
        }
    }

    pub fn reset_label(&self, now: DateTime<Utc>) -> String {
        match self.resets_at {
            Some(resets_at) if resets_at > now => {
                let minutes = ((resets_at - now).num_seconds().max(0) + 59) / 60;
                crate::text::format_long_duration_minutes(minutes as u32)
            }
            Some(_) => "RESET".to_string(),
            None => "—".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CodexUsage {
    pub plan: Option<String>,
    pub primary: Option<CodexWindow>,
    pub secondary: Option<CodexWindow>,
    /// The account has hit a limit and requests are being refused.
    pub limit_reached: bool,
}

impl CodexUsage {
    /// The short (five-hour) window, when the payload carries one.
    ///
    /// Classified by duration rather than position: the live API has been seen
    /// returning the weekly window as `primary_window`, and it omits the short
    /// window entirely when it is not currently applicable.
    pub fn five_hour(&self) -> Option<CodexWindow> {
        [self.primary, self.secondary]
            .into_iter()
            .flatten()
            .find(|window| window.window_seconds < 24 * 3600)
    }

    /// The window the tile shows: whichever is closest to its limit.
    pub fn binding(&self) -> Option<CodexWindow> {
        match (self.primary, self.secondary) {
            (Some(primary), Some(secondary)) if secondary.percent > primary.percent => {
                Some(secondary)
            }
            (Some(primary), _) => Some(primary),
            (None, secondary) => secondary,
        }
    }

    pub fn percent(&self) -> Option<f64> {
        self.binding().map(|window| window.percent)
    }
}

/// Distinguishes a broken payload from an expired credential, because the two need
/// different tiles and different diagnostics.
#[derive(Debug, thiserror::Error)]
pub enum CodexError {
    #[error("Codex authentication has expired; run `codex login`")]
    Unauthorized,
    #[error(transparent)]
    Parse(#[from] ParseError),
}

pub fn parse_usage(body: &str) -> Result<CodexUsage, CodexError> {
    let value = parse_json(INTEGRATION, body)?;

    // The endpoint reports auth failures as a JSON body, not only as a status code.
    if value
        .get("detail")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|detail| detail.to_ascii_lowercase().contains("token"))
    {
        return Err(CodexError::Unauthorized);
    }

    let rate_limit = value
        .get("rate_limit")
        .ok_or_else(|| ParseError::shape(INTEGRATION, "payload has no `rate_limit` object"))?;

    let usage = CodexUsage {
        plan: value
            .get("plan_type")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        primary: window(rate_limit.get("primary_window")),
        secondary: window(rate_limit.get("secondary_window")),
        limit_reached: rate_limit
            .get("limit_reached")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
    };

    if usage.primary.is_none() && usage.secondary.is_none() {
        return Err(ParseError::shape(INTEGRATION, "rate_limit contained no usable window").into());
    }
    Ok(usage)
}

fn window(value: Option<&serde_json::Value>) -> Option<CodexWindow> {
    let value = value?;
    if value.is_null() {
        return None;
    }
    let percent = value.get("used_percent")?.as_f64()?;
    if !percent.is_finite() {
        return None;
    }
    Some(CodexWindow {
        percent: percent.clamp(0.0, 100.0),
        window_seconds: value
            .get("limit_window_seconds")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0),
        resets_at: value
            .get("reset_at")
            .and_then(serde_json::Value::as_i64)
            .and_then(|seconds| DateTime::from_timestamp(seconds, 0)),
    })
}

/// Reads the bearer token and account id out of the Codex credential file. The
/// caller passes the file contents; nothing is logged.
pub fn parse_auth_file(contents: &str) -> Result<(String, Option<String>), CodexError> {
    let value = parse_json(INTEGRATION, contents)?;
    let tokens = value
        .get("tokens")
        .ok_or_else(|| ParseError::shape(INTEGRATION, "credential file has no `tokens` object"))?;
    let access_token = tokens
        .get("access_token")
        .and_then(serde_json::Value::as_str)
        .filter(|token| !token.is_empty())
        .ok_or(CodexError::Unauthorized)?
        .to_string();
    let account_id = tokens
        .get("account_id")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    Ok((access_token, account_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    const USAGE: &str = include_str!("../../../../tests/fixtures/codex-usage.json");

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-07-24T18:00:00Z")
            .expect("timestamp")
            .with_timezone(&Utc)
    }

    #[test]
    fn the_live_payload_shape_parses() {
        let usage = parse_usage(USAGE).expect("parsed");

        assert_eq!(usage.plan.as_deref(), Some("pro"));
        assert!(!usage.limit_reached);
        let primary = usage.primary.expect("primary window");
        assert_eq!(primary.percent, 43.0);
        assert_eq!(primary.window_seconds, 604_800);
        assert_eq!(usage.secondary, None);
    }

    #[test]
    fn window_lengths_get_short_labels() {
        let window = |seconds| CodexWindow {
            percent: 0.0,
            window_seconds: seconds,
            resets_at: None,
        };
        assert_eq!(window(604_800).window_label(), "7D");
        assert_eq!(window(18_000).window_label(), "5H");
        assert_eq!(window(2_592_000).window_label(), "30D");
        assert_eq!(window(900).window_label(), "15M");
    }

    #[test]
    fn the_binding_window_is_whichever_is_closest_to_its_limit() {
        let usage = CodexUsage {
            plan: None,
            primary: Some(CodexWindow {
                percent: 20.0,
                window_seconds: 604_800,
                resets_at: None,
            }),
            secondary: Some(CodexWindow {
                percent: 88.0,
                window_seconds: 18_000,
                resets_at: None,
            }),
            limit_reached: false,
        };
        assert_eq!(usage.percent(), Some(88.0));
        assert_eq!(usage.binding().expect("binding").window_label(), "5H");
    }

    #[test]
    fn the_five_hour_window_is_found_by_duration_in_either_position() {
        let five_hour = CodexWindow {
            percent: 61.0,
            window_seconds: 18_000,
            resets_at: None,
        };
        let weekly = CodexWindow {
            percent: 44.0,
            window_seconds: 604_800,
            resets_at: None,
        };

        let as_secondary = CodexUsage {
            plan: None,
            primary: Some(weekly),
            secondary: Some(five_hour),
            limit_reached: false,
        };
        assert_eq!(as_secondary.five_hour(), Some(five_hour));

        let as_primary = CodexUsage {
            primary: Some(five_hour),
            secondary: Some(weekly),
            ..as_secondary.clone()
        };
        assert_eq!(as_primary.five_hour(), Some(five_hour));

        // The live payload has been seen with only the weekly window.
        let weekly_only = CodexUsage {
            primary: Some(weekly),
            secondary: None,
            ..as_secondary.clone()
        };
        assert_eq!(weekly_only.five_hour(), None);
    }

    #[test]
    fn a_secondary_only_payload_still_reports_usage() {
        let body = r#"{"plan_type":"plus","rate_limit":{"primary_window":null,
            "secondary_window":{"used_percent":12,"limit_window_seconds":18000}}}"#;
        let usage = parse_usage(body).expect("parsed");
        assert_eq!(usage.percent(), Some(12.0));
    }

    #[test]
    fn reset_timestamps_become_countdowns() {
        let usage = parse_usage(USAGE).expect("parsed");
        let primary = usage.primary.expect("primary");
        let resets_at = primary.resets_at.expect("reset");
        assert!(resets_at > now(), "the fixture resets in the future");
        assert_ne!(primary.reset_label(now()), "—");
    }

    #[test]
    fn an_expired_credential_is_distinguished_from_a_broken_payload() {
        let error = parse_usage(r#"{"detail":"Your token has expired"}"#).expect_err("rejected");
        assert!(matches!(error, CodexError::Unauthorized), "{error}");

        let error = parse_usage(r#"{"unexpected":true}"#).expect_err("rejected");
        assert!(matches!(error, CodexError::Parse(_)), "{error}");
    }

    #[test]
    fn a_rate_limit_with_no_windows_is_a_shape_error() {
        let body = r#"{"rate_limit":{"primary_window":null,"secondary_window":null}}"#;
        assert!(matches!(
            parse_usage(body).expect_err("rejected"),
            CodexError::Parse(_)
        ));
    }

    #[test]
    fn a_reached_limit_is_surfaced() {
        let body = r#"{"rate_limit":{"limit_reached":true,
            "primary_window":{"used_percent":100,"limit_window_seconds":604800}}}"#;
        let usage = parse_usage(body).expect("parsed");
        assert!(usage.limit_reached);
        assert_eq!(usage.percent(), Some(100.0));
    }

    #[test]
    fn the_credential_file_yields_a_token_and_account_id() {
        let contents = r#"{"auth_mode":"chatgpt","OPENAI_API_KEY":null,
            "tokens":{"id_token":"a","access_token":"secret-token","refresh_token":"b",
            "account_id":"user-abc"},"last_refresh":"2026-07-19T21:22:00Z"}"#;

        let (token, account) = parse_auth_file(contents).expect("parsed");
        assert_eq!(token, "secret-token");
        assert_eq!(account.as_deref(), Some("user-abc"));
    }

    #[test]
    fn an_empty_or_missing_token_reports_unauthorized() {
        for contents in [
            r#"{"tokens":{"access_token":""}}"#,
            r#"{"tokens":{"refresh_token":"only"}}"#,
        ] {
            assert!(
                matches!(
                    parse_auth_file(contents).expect_err("rejected"),
                    CodexError::Unauthorized
                ),
                "{contents}"
            );
        }
        assert!(matches!(
            parse_auth_file(r#"{"no_tokens":true}"#).expect_err("rejected"),
            CodexError::Parse(_)
        ));
    }

    #[test]
    fn error_messages_never_contain_the_token() {
        let contents = r#"{"tokens":{"access_token":""}}"#;
        let error = parse_auth_file(contents).expect_err("rejected").to_string();
        assert!(!error.contains("access_token"), "{error}");
    }
}
