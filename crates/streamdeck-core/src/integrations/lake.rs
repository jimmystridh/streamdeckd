//! Mölndal Energi lake temperature parsing.
//!
//! Both endpoints return every lake in the region; the configured lake id selects
//! one. Readings are validated against a plausible water-temperature range and a
//! plausible timestamp before anything reaches a tile.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{parse_json, ParseError};

const INTEGRATION: &str = "lake";

pub const CURRENT_ENDPOINT: &str =
    "https://me-web-integration-linux.azurewebsites.net/api/temperatures/getAllCurrent";
pub const HISTORY_ENDPOINT: &str =
    "https://me-web-integration-linux.azurewebsites.net/api/temperatures/getAllHistoric";
/// The site rejects requests without its own origin.
pub const ORIGIN: &str = "https://www.molndalenergi.se";
pub const REFERER: &str = "https://www.molndalenergi.se/badtemperatur";

/// Water temperatures outside this range mean a broken sensor, not weather.
pub const VALID_RANGE: (f64, f64) = (-5.0, 50.0);
/// Days of history kept for the Stensjön panel.
pub const HISTORY_DAYS: usize = 7;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LakeReading {
    pub measured_at: DateTime<Utc>,
    pub temperature: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LakeHistory {
    /// Newest first, at most [`HISTORY_DAYS`] entries.
    pub days: Vec<LakeReading>,
}

impl LakeHistory {
    pub fn newest(&self) -> Option<&LakeReading> {
        self.days.first()
    }

    pub fn oldest(&self) -> Option<&LakeReading> {
        self.days.last()
    }

    /// Change across the retained window, for the seven-day trend tile.
    pub fn trend(&self) -> Option<f64> {
        Some(self.newest()?.temperature - self.oldest()?.temperature)
    }

    pub fn day(&self, index: usize) -> Option<&LakeReading> {
        self.days.get(index)
    }
}

pub fn parse_current(
    body: &str,
    lake_id: &str,
    now: DateTime<Utc>,
) -> Result<LakeReading, ParseError> {
    let value = parse_json(INTEGRATION, body)?;
    let array = value
        .as_array()
        .ok_or_else(|| ParseError::shape(INTEGRATION, "current response is not an array"))?;

    let entry = array
        .iter()
        .find(|entry| entry.get("lakeId").and_then(serde_json::Value::as_str) == Some(lake_id))
        .ok_or_else(|| {
            ParseError::shape(
                INTEGRATION,
                format!("lake {lake_id} is not in the response"),
            )
        })?;

    reading_from(entry, now).ok_or_else(|| {
        ParseError::range(
            INTEGRATION,
            format!("lake {lake_id} reported an implausible reading"),
        )
    })
}

pub fn parse_history(
    body: &str,
    lake_id: &str,
    now: DateTime<Utc>,
) -> Result<LakeHistory, ParseError> {
    let value = parse_json(INTEGRATION, body)?;
    let temperatures = value
        .get("daily")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| ParseError::shape(INTEGRATION, "history response has no `daily` array"))?
        .iter()
        .find(|entry| entry.get("lakeId").and_then(serde_json::Value::as_str) == Some(lake_id))
        .and_then(|entry| entry.get("temperatures"))
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            ParseError::shape(
                INTEGRATION,
                format!("lake {lake_id} has no daily history in the response"),
            )
        })?;

    let mut days: Vec<LakeReading> = temperatures
        .iter()
        .filter_map(|entry| reading_from(entry, now))
        .collect();
    days.sort_by(|left, right| right.measured_at.cmp(&left.measured_at));
    days.truncate(HISTORY_DAYS);

    if days.is_empty() {
        return Err(ParseError::shape(
            INTEGRATION,
            format!("lake {lake_id} history contained no valid readings"),
        ));
    }
    Ok(LakeHistory { days })
}

fn reading_from(entry: &serde_json::Value, now: DateTime<Utc>) -> Option<LakeReading> {
    let temperature = number(entry.get("temperature")?)?;
    if !(VALID_RANGE.0..=VALID_RANGE.1).contains(&temperature) {
        return None;
    }
    let timestamp = number(entry.get("timestamp")?)? as i64;
    let measured_at = DateTime::from_timestamp(timestamp, 0)?;
    // Reject clearly bogus clocks: before 2000 or more than a day in the future.
    if timestamp < 946_684_800 || timestamp > now.timestamp() + 86_400 {
        return None;
    }
    Some(LakeReading {
        measured_at,
        temperature,
    })
}

/// The API sends temperatures as numbers in some responses and strings in others.
fn number(value: &serde_json::Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str()?.trim().replace(',', ".").parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CURRENT: &str = include_str!("../../../../tests/fixtures/lake-current.json");
    const HISTORY: &str = include_str!("../../../../tests/fixtures/lake-historic.json");
    const STENSJON: &str = "A84041BDC1864B41";

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-07-24T18:00:00Z")
            .expect("timestamp")
            .with_timezone(&Utc)
    }

    #[test]
    fn the_configured_lake_is_selected_out_of_the_regional_response() {
        let reading = parse_current(CURRENT, STENSJON, now()).expect("parsed");
        assert_eq!(reading.temperature, 21.3);
        assert_eq!(
            reading.measured_at.to_rfc3339(),
            "2026-07-24T16:00:00+00:00"
        );
    }

    #[test]
    fn a_string_encoded_temperature_is_accepted() {
        let body = r#"[{"lakeId":"A84041BDC1864B41","temperature":"18,7","timestamp":1784908800}]"#;
        assert_eq!(
            parse_current(body, STENSJON, now())
                .expect("parsed")
                .temperature,
            18.7
        );
    }

    #[test]
    fn a_missing_lake_is_reported_rather_than_defaulted() {
        let error = parse_current(CURRENT, "NOT-A-LAKE", now()).expect_err("rejected");
        assert!(error.to_string().contains("NOT-A-LAKE"), "{error}");
    }

    #[test]
    fn implausible_temperatures_and_timestamps_are_refused() {
        for body in [
            r#"[{"lakeId":"A84041BDC1864B41","temperature":-40,"timestamp":1784908800}]"#,
            r#"[{"lakeId":"A84041BDC1864B41","temperature":80,"timestamp":1784908800}]"#,
            r#"[{"lakeId":"A84041BDC1864B41","temperature":20,"timestamp":0}]"#,
            r#"[{"lakeId":"A84041BDC1864B41","temperature":"warm","timestamp":1784908800}]"#,
            r#"[{"lakeId":"A84041BDC1864B41","temperature":20}]"#,
        ] {
            assert!(parse_current(body, STENSJON, now()).is_err(), "{body}");
        }
    }

    #[test]
    fn a_non_array_current_response_is_rejected() {
        assert!(parse_current(r#"{"error":"nope"}"#, STENSJON, now()).is_err());
    }

    #[test]
    fn history_is_sorted_newest_first_and_capped_at_seven_days() {
        let history = parse_history(HISTORY, STENSJON, now()).expect("parsed");

        assert_eq!(history.days.len(), HISTORY_DAYS);
        let timestamps: Vec<i64> = history
            .days
            .iter()
            .map(|day| day.measured_at.timestamp())
            .collect();
        let mut sorted = timestamps.clone();
        sorted.sort_by(|left, right| right.cmp(left));
        assert_eq!(timestamps, sorted, "newest first");
        assert_eq!(history.newest().expect("newest").temperature, 21.3);
    }

    #[test]
    fn the_trend_is_the_change_across_the_retained_window() {
        let history = parse_history(HISTORY, STENSJON, now()).expect("parsed");
        let trend = history.trend().expect("trend");
        let expected = history.newest().expect("newest").temperature
            - history.oldest().expect("oldest").temperature;
        assert!((trend - expected).abs() < 1e-9);
        assert!(trend > 0.0, "the fixture warms over the week");
    }

    #[test]
    fn invalid_history_readings_are_skipped_rather_than_failing_the_whole_day_list() {
        let body = r#"{"daily":[{"lakeId":"A84041BDC1864B41","temperatures":[
            {"temperature":19.0,"timestamp":1784908800},
            {"temperature":900.0,"timestamp":1784822400},
            {"temperature":18.0,"timestamp":1784736000}
        ]}]}"#;

        let history = parse_history(body, STENSJON, now()).expect("parsed");
        assert_eq!(history.days.len(), 2);
        assert_eq!(history.newest().expect("newest").temperature, 19.0);
    }

    #[test]
    fn a_history_with_no_valid_readings_is_an_error() {
        let body = r#"{"daily":[{"lakeId":"A84041BDC1864B41","temperatures":[
            {"temperature":900.0,"timestamp":1784822400}
        ]}]}"#;
        assert!(parse_history(body, STENSJON, now()).is_err());
    }

    #[test]
    fn a_history_response_without_the_lake_is_an_error() {
        assert!(parse_history(HISTORY, "NOT-A-LAKE", now()).is_err());
        assert!(parse_history(r#"{"unexpected":true}"#, STENSJON, now()).is_err());
    }

    #[test]
    fn a_reading_from_the_far_future_is_refused() {
        let body = r#"[{"lakeId":"A84041BDC1864B41","temperature":20,"timestamp":2000000000}]"#;
        assert!(parse_current(body, STENSJON, now()).is_err());
    }

    #[test]
    fn day_lookups_stay_inside_the_retained_window() {
        let history = parse_history(HISTORY, STENSJON, now()).expect("parsed");
        assert!(history.day(0).is_some());
        assert!(history.day(HISTORY_DAYS - 1).is_some());
        assert!(history.day(HISTORY_DAYS).is_none());
    }
}
