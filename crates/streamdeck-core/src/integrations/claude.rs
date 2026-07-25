//! Claude Code usage parsing.
//!
//! Reproduces the combined projection, five-hour window, and seven-day window
//! tiles. Nothing here ever touches a token: credential resolution lives in the
//! macOS crate and only the parsed percentages reach the domain.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{parse_json, ParseError};

const INTEGRATION: &str = "claude-usage";

pub const ENDPOINT: &str = "https://api.anthropic.com/api/oauth/usage";
/// The beta header the OAuth usage endpoint requires.
pub const BETA_HEADER: &str = "oauth-2025-04-20";

/// One rate-limit window.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct UsageWindow {
    /// Percentage of the window consumed, `0..=100`.
    pub percent: f64,
    pub resets_at: Option<DateTime<Utc>>,
}

impl UsageWindow {
    /// Time until the window resets, as a compact label such as `4H 12M` or `RESET`.
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

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ClaudeUsage {
    pub five_hour: Option<UsageWindow>,
    pub seven_day: Option<UsageWindow>,
}

impl ClaudeUsage {
    /// The combined projection tile: the tighter of the two windows, because that
    /// is the one that will actually stop work.
    pub fn combined_percent(&self) -> Option<f64> {
        [self.five_hour, self.seven_day]
            .into_iter()
            .flatten()
            .map(|window| window.percent)
            .reduce(f64::max)
    }

    /// Which window the combined tile is currently reporting.
    pub fn binding_window(&self) -> Option<(&'static str, UsageWindow)> {
        match (self.five_hour, self.seven_day) {
            (Some(five), Some(seven)) if five.percent >= seven.percent => Some(("5H", five)),
            (Some(_), Some(seven)) => Some(("7D", seven)),
            (Some(five), None) => Some(("5H", five)),
            (None, Some(seven)) => Some(("7D", seven)),
            (None, None) => None,
        }
    }
}

/// Severity used to pick tile colours, shared by the Claude and Codex tiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageSeverity {
    Normal,
    Warning,
    Critical,
}

impl UsageSeverity {
    pub fn of(percent: f64, warning: u8, critical: u8) -> Self {
        if percent >= f64::from(critical) {
            UsageSeverity::Critical
        } else if percent >= f64::from(warning) {
            UsageSeverity::Warning
        } else {
            UsageSeverity::Normal
        }
    }
}

pub fn parse_usage(body: &str) -> Result<ClaudeUsage, ParseError> {
    let value = parse_json(INTEGRATION, body)?;
    if !value.is_object() {
        return Err(ParseError::shape(INTEGRATION, "payload is not an object"));
    }
    let usage = ClaudeUsage {
        five_hour: window(value.get("five_hour")),
        seven_day: window(value.get("seven_day")),
    };
    if usage.five_hour.is_none() && usage.seven_day.is_none() {
        return Err(ParseError::shape(
            INTEGRATION,
            "payload contained neither a five_hour nor a seven_day window",
        ));
    }
    Ok(usage)
}

fn window(value: Option<&serde_json::Value>) -> Option<UsageWindow> {
    let value = value?;
    if value.is_null() {
        return None;
    }
    let percent = value.get("utilization")?.as_f64()?;
    if !percent.is_finite() {
        return None;
    }
    Some(UsageWindow {
        percent: percent.clamp(0.0, 100.0),
        resets_at: value
            .get("resets_at")
            .and_then(serde_json::Value::as_str)
            .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
            .map(|time| time.with_timezone(&Utc)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const USAGE: &str = include_str!("../../../../tests/fixtures/claude-usage.json");

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-07-24T18:00:00Z")
            .expect("timestamp")
            .with_timezone(&Utc)
    }

    #[test]
    fn both_windows_parse_out_of_the_live_payload_shape() {
        let usage = parse_usage(USAGE).expect("parsed");

        assert_eq!(usage.five_hour.expect("five hour").percent, 1.0);
        assert_eq!(usage.seven_day.expect("seven day").percent, 33.0);
        assert_eq!(
            usage
                .five_hour
                .expect("five hour")
                .resets_at
                .expect("reset")
                .to_rfc3339(),
            "2026-07-24T22:29:59.704918+00:00"
        );
    }

    #[test]
    fn the_combined_projection_reports_the_tighter_window() {
        let usage = parse_usage(USAGE).expect("parsed");
        assert_eq!(usage.combined_percent(), Some(33.0));

        let (label, window) = usage.binding_window().expect("binding");
        assert_eq!(label, "7D");
        assert_eq!(window.percent, 33.0);
    }

    #[test]
    fn the_five_hour_window_binds_when_it_is_the_tighter_one() {
        let usage = ClaudeUsage {
            five_hour: Some(UsageWindow {
                percent: 91.0,
                resets_at: None,
            }),
            seven_day: Some(UsageWindow {
                percent: 40.0,
                resets_at: None,
            }),
        };
        assert_eq!(usage.combined_percent(), Some(91.0));
        assert_eq!(usage.binding_window().expect("binding").0, "5H");
    }

    #[test]
    fn a_single_available_window_still_produces_a_projection() {
        let usage =
            parse_usage(r#"{"five_hour":{"utilization":12.0},"seven_day":null}"#).expect("parsed");
        assert_eq!(usage.combined_percent(), Some(12.0));
        assert_eq!(usage.binding_window().expect("binding").0, "5H");

        let usage =
            parse_usage(r#"{"five_hour":null,"seven_day":{"utilization":80.0}}"#).expect("parsed");
        assert_eq!(usage.binding_window().expect("binding").0, "7D");
    }

    #[test]
    fn reset_labels_count_down_and_then_say_reset() {
        let window = UsageWindow {
            percent: 50.0,
            resets_at: Some(
                DateTime::parse_from_rfc3339("2026-07-24T22:30:00Z")
                    .expect("timestamp")
                    .with_timezone(&Utc),
            ),
        };
        assert_eq!(window.reset_label(now()), "4H 30M");

        let far_out = UsageWindow {
            resets_at: Some(now() + chrono::Duration::hours(92)),
            ..window
        };
        assert_eq!(far_out.reset_label(now()), "3D 20H");

        let elapsed = UsageWindow {
            resets_at: Some(now() - chrono::Duration::minutes(5)),
            ..window
        };
        assert_eq!(elapsed.reset_label(now()), "RESET");

        let unknown = UsageWindow {
            resets_at: None,
            ..window
        };
        assert_eq!(unknown.reset_label(now()), "—");
    }

    #[test]
    fn severity_thresholds_drive_the_tile_colour() {
        assert_eq!(UsageSeverity::of(10.0, 50, 80), UsageSeverity::Normal);
        assert_eq!(UsageSeverity::of(50.0, 50, 80), UsageSeverity::Warning);
        assert_eq!(UsageSeverity::of(79.9, 50, 80), UsageSeverity::Warning);
        assert_eq!(UsageSeverity::of(80.0, 50, 80), UsageSeverity::Critical);
        assert_eq!(UsageSeverity::of(100.0, 50, 80), UsageSeverity::Critical);
    }

    #[test]
    fn percentages_are_clamped_into_range() {
        let usage = parse_usage(r#"{"five_hour":{"utilization":140.0}}"#).expect("parsed");
        assert_eq!(usage.five_hour.expect("window").percent, 100.0);

        let usage = parse_usage(r#"{"five_hour":{"utilization":-5.0}}"#).expect("parsed");
        assert_eq!(usage.five_hour.expect("window").percent, 0.0);
    }

    #[test]
    fn a_payload_with_no_recognisable_window_is_rejected() {
        for body in [
            r#"{"five_hour":null,"seven_day":null}"#,
            r#"{"something_else":{"utilization":10}}"#,
            r#"{"five_hour":{"no_utilization":1}}"#,
            r#"[]"#,
        ] {
            assert!(parse_usage(body).is_err(), "{body}");
        }
    }

    #[test]
    fn an_unparseable_reset_timestamp_does_not_lose_the_percentage() {
        let usage = parse_usage(r#"{"five_hour":{"utilization":22.0,"resets_at":"soon"}}"#)
            .expect("parsed");
        let window = usage.five_hour.expect("window");
        assert_eq!(window.percent, 22.0);
        assert_eq!(window.resets_at, None);
    }

    #[test]
    fn future_payload_additions_do_not_break_parsing() {
        let usage = parse_usage(
            r#"{"five_hour":{"utilization":5.0},"seven_day":{"utilization":10.0},
                "brand_new_window":{"utilization":99.0},"limits":[{"kind":"session"}]}"#,
        )
        .expect("parsed");
        assert_eq!(usage.combined_percent(), Some(10.0));
    }
}
