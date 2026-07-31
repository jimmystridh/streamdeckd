//! Västtrafik departures with real-time times preferred over timetable times.

use chrono::{DateTime, Duration, FixedOffset, Utc};
use serde::Deserialize;

use super::ParseError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Departure {
    pub line: String,
    pub direction: String,
    pub platform: Option<String>,
    pub planned_at: DateTime<FixedOffset>,
    pub departure_at: DateTime<FixedOffset>,
    pub cancelled: bool,
}

impl Departure {
    pub fn countdown(&self, now: DateTime<Utc>) -> String {
        let seconds = (self.departure_at.with_timezone(&Utc) - now).num_seconds();
        if seconds <= 60 {
            "NU".to_string()
        } else {
            format!("{}m", (seconds + 59) / 60)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StopDepartures {
    pub label: String,
    pub gid: String,
    pub line: Option<String>,
    pub direction: Option<String>,
    pub departures: Vec<Departure>,
}

impl StopDepartures {
    pub fn retain_route(&mut self, line: &str, direction: &str) {
        if line.is_empty() && direction.is_empty() {
            return;
        }
        let wanted_direction = direction.to_lowercase();
        self.departures.retain(|departure| {
            departure.line == line && departure.direction.to_lowercase() == wanted_direction
        });
        self.line = Some(line.to_string());
        self.direction = Some(direction.to_string());
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DepartureBoard {
    pub stops: Vec<StopDepartures>,
}

#[derive(Deserialize)]
struct Response {
    results: Vec<RawDeparture>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawDeparture {
    service_journey: ServiceJourney,
    stop_point: StopPoint,
    planned_time: DateTime<FixedOffset>,
    estimated_otherwise_planned_time: DateTime<FixedOffset>,
    #[serde(default)]
    is_cancelled: bool,
    #[serde(default)]
    is_departure_cancelled: bool,
}

#[derive(Deserialize)]
struct ServiceJourney {
    direction: String,
    #[serde(rename = "directionDetails")]
    direction_details: Option<DirectionDetails>,
    line: Line,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DirectionDetails {
    short_direction: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Line {
    short_name: String,
}

#[derive(Deserialize)]
struct StopPoint {
    platform: Option<String>,
}

pub fn parse(
    label: &str,
    gid: &str,
    body: &str,
    now: DateTime<Utc>,
) -> Result<StopDepartures, ParseError> {
    let response: Response = serde_json::from_str(body).map_err(|source| ParseError::Json {
        integration: "vasttrafik",
        source,
    })?;
    let cutoff = now - Duration::minutes(2);
    let departures = response
        .results
        .into_iter()
        .filter(|departure| {
            departure
                .estimated_otherwise_planned_time
                .with_timezone(&Utc)
                >= cutoff
        })
        .take(8)
        .map(|departure| Departure {
            line: clean(&departure.service_journey.line.short_name, 8),
            direction: clean(
                departure
                    .service_journey
                    .direction_details
                    .as_ref()
                    .map(|details| details.short_direction.as_str())
                    .unwrap_or(&departure.service_journey.direction),
                40,
            ),
            platform: departure.stop_point.platform.map(|value| clean(&value, 6)),
            planned_at: departure.planned_time,
            departure_at: departure.estimated_otherwise_planned_time,
            cancelled: departure.is_cancelled || departure.is_departure_cancelled,
        })
        .collect();

    Ok(StopDepartures {
        label: label.to_string(),
        gid: gid.to_string(),
        line: None,
        direction: None,
        departures,
    })
}

fn clean(value: &str, limit: usize) -> String {
    value
        .chars()
        .filter(|character| !character.is_control())
        .take(limit)
        .collect::<String>()
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &str = r#"{"results":[{
      "serviceJourney":{"direction":"Heden, Påstigning fram","directionDetails":{"shortDirection":"Heden"},"line":{"shortName":"754"}},
      "stopPoint":{"platform":"B"},
      "plannedTime":"2026-07-31T15:42:00+02:00",
      "estimatedOtherwisePlannedTime":"2026-07-31T15:43:00+02:00",
      "isCancelled":false,"isDepartureCancelled":false
    }]}"#;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-07-31T13:40:00Z")
            .expect("time")
            .with_timezone(&Utc)
    }

    #[test]
    fn real_time_departures_parse_with_line_direction_and_countdown() {
        let stop = parse("Tallkottegatan", "9021014012521000", BODY, now()).expect("stop");
        assert_eq!(stop.departures[0].line, "754");
        assert_eq!(stop.departures[0].direction, "Heden");
        assert_eq!(stop.departures[0].countdown(now()), "3m");
    }

    #[test]
    fn malformed_payloads_are_named_as_vasttrafik_errors() {
        let error = parse("x", "1", "not json", now()).expect_err("bad JSON");
        assert!(error.to_string().contains("vasttrafik"), "{error}");
    }

    #[test]
    fn route_filter_keeps_only_the_requested_line_and_direction() {
        let mut stop = parse("Tallkotten", "9021014012521000", BODY, now()).expect("stop");
        stop.departures.push(Departure {
            line: "754".to_string(),
            direction: "Mölndal resecentrum".to_string(),
            platform: Some("A".to_string()),
            planned_at: stop.departures[0].planned_at,
            departure_at: stop.departures[0].departure_at,
            cancelled: false,
        });

        stop.retain_route("754", "HEDEN");

        assert_eq!(stop.departures.len(), 1);
        assert_eq!(stop.line.as_deref(), Some("754"));
        assert_eq!(stop.direction.as_deref(), Some("HEDEN"));
    }
}
