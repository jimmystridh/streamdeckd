//! MET Norway Locationforecast.
//!
//! Sends the descriptive User-Agent the terms of service require, honours
//! `Expires`, and uses `If-Modified-Since` so an unchanged forecast costs a `304`.

use chrono_tz::Tz;
use streamdeck_core::integrations::weather::{self, WeatherSnapshot, ENDPOINT, USER_AGENT};

use super::http::{HttpClient, HttpError};
use super::{timeouts, Fetched, Refreshed};

#[derive(Debug, thiserror::Error)]
pub enum WeatherError {
    #[error(transparent)]
    Http(#[from] HttpError),
    #[error(transparent)]
    Parse(#[from] streamdeck_core::integrations::ParseError),
}

pub async fn fetch(
    client: &HttpClient,
    latitude: f64,
    longitude: f64,
    location: &str,
    timezone: Tz,
    if_modified_since: Option<&str>,
) -> Result<Refreshed<WeatherSnapshot>, WeatherError> {
    let (latitude, longitude) = weather::format_coordinates(latitude, longitude)?;
    let url = format!("{ENDPOINT}?lat={latitude}&lon={longitude}");

    let mut headers = vec![
        ("Accept", "application/json"),
        ("User-Agent", USER_AGENT),
        ("Accept-Encoding", "gzip"),
    ];
    if let Some(since) = if_modified_since {
        headers.push(("If-Modified-Since", since));
    }

    let response = client.get(&url, &headers, timeouts::WEATHER).await?;
    if response.not_modified {
        return Ok(Refreshed::Unchanged {
            expires_at_ms: response.expires_at_ms,
        });
    }

    let snapshot = weather::parse_forecast(&response.body, location, timezone)?;
    Ok(Refreshed::Updated(Fetched {
        value: snapshot,
        expires_at_ms: response.expires_at_ms,
        last_modified: response.last_modified,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono_tz::Europe::Stockholm;

    #[test]
    fn the_user_agent_identifies_the_project_and_a_contact_route() {
        assert!(USER_AGENT.contains("streamdeckd"));
        assert!(USER_AGENT.contains("github.com/jimmystridh"));
    }

    #[test]
    fn invalid_coordinates_are_refused_before_any_request_is_made() {
        let client = HttpClient::new().expect("client");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");

        let error = runtime
            .block_on(fetch(&client, 999.0, 0.0, "Nowhere", Stockholm, None))
            .expect_err("refused");
        assert!(matches!(error, WeatherError::Parse(_)), "{error}");
    }

    #[test]
    fn the_endpoint_is_the_compact_locationforecast() {
        assert_eq!(
            ENDPOINT,
            "https://api.met.no/weatherapi/locationforecast/2.0/compact"
        );
    }
}
