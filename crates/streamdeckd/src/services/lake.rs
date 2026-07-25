//! Mölndal Energi lake temperatures.

use chrono::{DateTime, Utc};
use streamdeck_core::integrations::lake::{
    self, LakeHistory, LakeReading, CURRENT_ENDPOINT, HISTORY_ENDPOINT, ORIGIN, REFERER,
};

use super::http::{HttpClient, HttpError};
use super::timeouts;

#[derive(Debug, thiserror::Error)]
pub enum LakeError {
    #[error(transparent)]
    Http(#[from] HttpError),
    #[error(transparent)]
    Parse(#[from] streamdeck_core::integrations::ParseError),
}

/// The site refuses requests that do not present its own origin.
fn headers() -> Vec<(&'static str, &'static str)> {
    vec![
        ("Accept", "application/json"),
        ("Origin", ORIGIN),
        ("Referer", REFERER),
    ]
}

pub async fn fetch_current(
    client: &HttpClient,
    lake_id: &str,
    now: DateTime<Utc>,
) -> Result<LakeReading, LakeError> {
    let response = client
        .get(CURRENT_ENDPOINT, &headers(), timeouts::LAKE)
        .await?;
    Ok(lake::parse_current(&response.body, lake_id, now)?)
}

pub async fn fetch_history(
    client: &HttpClient,
    lake_id: &str,
    now: DateTime<Utc>,
) -> Result<LakeHistory, LakeError> {
    let response = client
        .get(HISTORY_ENDPOINT, &headers(), timeouts::LAKE)
        .await?;
    Ok(lake::parse_history(&response.body, lake_id, now)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_requests_carry_the_required_origin_and_referer() {
        let headers = headers();
        assert!(headers.contains(&("Origin", "https://www.molndalenergi.se")));
        assert!(headers.contains(&("Referer", "https://www.molndalenergi.se/badtemperatur")));
    }

    #[test]
    fn the_endpoints_match_the_plan() {
        assert!(CURRENT_ENDPOINT.ends_with("/getAllCurrent"));
        assert!(HISTORY_ENDPOINT.ends_with("/getAllHistoric"));
    }
}
