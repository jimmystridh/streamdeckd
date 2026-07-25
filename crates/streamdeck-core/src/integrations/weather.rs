//! MET Norway Locationforecast parsing and day aggregation.
//!
//! Hourly points are grouped into days in the configured timezone, so a forecast
//! day means a local calendar day. Each day reports its high, low, total
//! precipitation, and the symbol from the point closest to local noon.

use chrono::{DateTime, Timelike, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

use super::{parse_json, ParseError};

const INTEGRATION: &str = "weather";

/// The descriptive User-Agent MET Norway's terms of service require.
pub const USER_AGENT: &str =
    "streamdeckd/0.1 (+https://github.com/jimmystridh/streamdeckd) contact via GitHub issues";

pub const ENDPOINT: &str = "https://api.met.no/weatherapi/locationforecast/2.0/compact";

/// The symbol families the renderer draws. Mapping MET's ~90 symbol codes onto
/// this small set keeps the icon inventory project-owned and reviewable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SymbolFamily {
    Clear,
    PartlyCloudy,
    Cloudy,
    Rain,
    Sleet,
    Snow,
    Thunder,
    Fog,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WeatherSymbol {
    pub family: SymbolFamily,
    pub night: bool,
}

impl WeatherSymbol {
    /// Maps a MET `symbol_code` such as `partlycloudy_night` or `heavysleetshowers_day`.
    pub fn from_code(code: &str) -> Self {
        let lower = code.to_ascii_lowercase();
        let night = lower.ends_with("_night");
        // Order matters: `sleet` must win over `rain`, and `thunder` over both.
        let family = if lower.contains("thunder") {
            SymbolFamily::Thunder
        } else if lower.contains("sleet") {
            SymbolFamily::Sleet
        } else if lower.contains("snow") {
            SymbolFamily::Snow
        } else if lower.contains("rain") || lower.contains("drizzle") {
            SymbolFamily::Rain
        } else if lower.contains("fog") {
            SymbolFamily::Fog
        } else if lower.contains("partlycloudy") {
            SymbolFamily::PartlyCloudy
        } else if lower.starts_with("clearsky") || lower.starts_with("fair") {
            SymbolFamily::Clear
        } else {
            SymbolFamily::Cloudy
        };
        Self { family, night }
    }

    pub const fn condition_label(self) -> &'static str {
        match self.family {
            SymbolFamily::Clear => "CLEAR",
            SymbolFamily::PartlyCloudy => "PART CLOUD",
            SymbolFamily::Cloudy => "CLOUDY",
            SymbolFamily::Rain => "RAIN",
            SymbolFamily::Sleet => "SLEET",
            SymbolFamily::Snow => "SNOW",
            SymbolFamily::Thunder => "THUNDER",
            SymbolFamily::Fog => "FOG",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ForecastPoint {
    pub time: DateTime<Utc>,
    pub temperature: f64,
    pub humidity: f64,
    pub wind_speed: f64,
    pub wind_direction: f64,
    pub precipitation: f64,
    pub symbol: WeatherSymbol,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WeatherDay {
    /// Local date key, `YYYY-MM-DD`.
    pub date_key: String,
    /// The representative point's instant, used for weekday and date labels.
    pub representative: DateTime<Utc>,
    pub high: f64,
    pub low: f64,
    pub precipitation: f64,
    pub symbol: WeatherSymbol,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WeatherSnapshot {
    pub location: String,
    pub updated_at: DateTime<Utc>,
    pub current: ForecastPoint,
    /// Today first, then each following day.
    pub days: Vec<WeatherDay>,
}

impl WeatherSnapshot {
    pub fn today(&self) -> Option<&WeatherDay> {
        self.days.first()
    }

    pub fn day(&self, offset: usize) -> Option<&WeatherDay> {
        self.days.get(offset).or_else(|| self.days.last())
    }
}

/// Validates coordinates and formats them the way the endpoint expects. MET asks
/// clients to truncate to four decimals so responses stay cacheable.
pub fn format_coordinates(latitude: f64, longitude: f64) -> Result<(String, String), ParseError> {
    if !latitude.is_finite()
        || !longitude.is_finite()
        || !(-90.0..=90.0).contains(&latitude)
        || !(-180.0..=180.0).contains(&longitude)
    {
        return Err(ParseError::range(
            INTEGRATION,
            format!("coordinates {latitude},{longitude} are out of range"),
        ));
    }
    Ok((format!("{latitude:.4}"), format!("{longitude:.4}")))
}

pub fn parse_forecast(
    body: &str,
    location: &str,
    timezone: Tz,
) -> Result<WeatherSnapshot, ParseError> {
    let value = parse_json(INTEGRATION, body)?;
    let properties = value
        .get("properties")
        .ok_or_else(|| ParseError::shape(INTEGRATION, "payload has no `properties`"))?;
    let timeseries = properties
        .get("timeseries")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| ParseError::shape(INTEGRATION, "payload has no `timeseries` array"))?;

    let mut points: Vec<ForecastPoint> = timeseries.iter().filter_map(parse_point).collect();
    points.sort_by_key(|point| point.time);
    let current = *points
        .first()
        .ok_or_else(|| ParseError::shape(INTEGRATION, "no usable forecast points"))?;

    let days = aggregate_days(&points, timezone);
    if days.is_empty() {
        return Err(ParseError::shape(INTEGRATION, "no forecast days"));
    }

    let updated_at = properties
        .get("meta")
        .and_then(|meta| meta.get("updated_at"))
        .and_then(serde_json::Value::as_str)
        .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
        .map(|time| time.with_timezone(&Utc))
        .unwrap_or(current.time);

    Ok(WeatherSnapshot {
        location: location.to_string(),
        updated_at,
        current,
        days,
    })
}

fn parse_point(value: &serde_json::Value) -> Option<ForecastPoint> {
    let time = DateTime::parse_from_rfc3339(value.get("time")?.as_str()?)
        .ok()?
        .with_timezone(&Utc);
    let data = value.get("data")?;
    let details = data.get("instant")?.get("details")?;
    let number = |key: &str| details.get(key).and_then(serde_json::Value::as_f64);

    let temperature = number("air_temperature")?;
    if !(-90.0..=60.0).contains(&temperature) {
        return None;
    }

    // Prefer the shortest available forecast period for the symbol and rain total.
    let period = ["next_1_hours", "next_6_hours", "next_12_hours"]
        .into_iter()
        .find_map(|key| data.get(key));
    let symbol = period
        .and_then(|period| period.get("summary"))
        .and_then(|summary| summary.get("symbol_code"))
        .and_then(serde_json::Value::as_str)
        .map(WeatherSymbol::from_code)
        .unwrap_or(WeatherSymbol {
            family: SymbolFamily::Cloudy,
            night: false,
        });
    let precipitation = period
        .and_then(|period| period.get("details"))
        .and_then(|details| details.get("precipitation_amount"))
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(0.0)
        .max(0.0);

    Some(ForecastPoint {
        time,
        temperature,
        humidity: number("relative_humidity").unwrap_or(0.0).clamp(0.0, 100.0),
        wind_speed: number("wind_speed").unwrap_or(0.0).max(0.0),
        wind_direction: number("wind_from_direction").unwrap_or(0.0),
        precipitation,
        symbol,
    })
}

fn aggregate_days(points: &[ForecastPoint], timezone: Tz) -> Vec<WeatherDay> {
    let mut days: Vec<WeatherDay> = Vec::new();
    // Points are already sorted, so a day's group is contiguous.
    for point in points {
        let local = point.time.with_timezone(&timezone);
        let date_key = local.format("%Y-%m-%d").to_string();
        match days.last_mut() {
            Some(day) if day.date_key == date_key => {
                day.high = day.high.max(point.temperature);
                day.low = day.low.min(point.temperature);
                day.precipitation += point.precipitation;
                // Keep the symbol from whichever point sits closest to local noon.
                let current_distance =
                    (day.representative.with_timezone(&timezone).hour() as i64 - 12).abs();
                let candidate_distance = (local.hour() as i64 - 12).abs();
                if candidate_distance < current_distance {
                    day.representative = point.time;
                    day.symbol = point.symbol;
                }
            }
            _ => days.push(WeatherDay {
                date_key,
                representative: point.time,
                high: point.temperature,
                low: point.temperature,
                precipitation: point.precipitation,
                symbol: point.symbol,
            }),
        }
    }
    days
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono_tz::Europe::Stockholm;

    const FORECAST: &str = include_str!("../../../../tests/fixtures/met-locationforecast.json");

    #[test]
    fn symbol_codes_map_onto_the_drawable_families() {
        let cases = [
            ("clearsky_day", SymbolFamily::Clear, false),
            ("clearsky_night", SymbolFamily::Clear, true),
            ("fair_day", SymbolFamily::Clear, false),
            ("partlycloudy_night", SymbolFamily::PartlyCloudy, true),
            ("cloudy", SymbolFamily::Cloudy, false),
            ("lightrainshowers_day", SymbolFamily::Rain, false),
            ("heavydrizzle", SymbolFamily::Rain, false),
            ("sleetshowers_day", SymbolFamily::Sleet, false),
            ("heavysnowshowers_night", SymbolFamily::Snow, true),
            ("rainshowersandthunder_day", SymbolFamily::Thunder, false),
            ("heavysleetandthunder", SymbolFamily::Thunder, false),
            ("fog", SymbolFamily::Fog, false),
            ("somethingnew_day", SymbolFamily::Cloudy, false),
        ];

        for (code, family, night) in cases {
            let symbol = WeatherSymbol::from_code(code);
            assert_eq!(symbol.family, family, "family for {code}");
            assert_eq!(symbol.night, night, "night for {code}");
        }
    }

    #[test]
    fn condition_labels_fit_a_detail_row() {
        for code in ["clearsky_day", "partlycloudy_day", "heavysnow", "fog"] {
            let label = WeatherSymbol::from_code(code).condition_label();
            assert!(label.len() <= 10, "{label} is too long for a row");
        }
    }

    #[test]
    fn coordinates_are_validated_and_truncated_to_four_decimals() {
        assert_eq!(
            format_coordinates(57.66271234, 12.03415678).expect("valid"),
            ("57.6627".to_string(), "12.0342".to_string())
        );
        assert!(format_coordinates(91.0, 0.0).is_err());
        assert!(format_coordinates(0.0, -181.0).is_err());
        assert!(format_coordinates(f64::NAN, 0.0).is_err());
    }

    #[test]
    fn the_fixture_forecast_parses_into_current_conditions_and_days() {
        let snapshot = parse_forecast(FORECAST, "Stensjön", Stockholm).expect("parsed");

        assert_eq!(snapshot.location, "Stensjön");
        assert_eq!(snapshot.current.temperature, 19.4);
        assert_eq!(snapshot.current.symbol.family, SymbolFamily::PartlyCloudy);
        assert_eq!(snapshot.days.len(), 3);
    }

    #[test]
    fn days_are_grouped_in_the_configured_timezone() {
        let snapshot = parse_forecast(FORECAST, "Stensjön", Stockholm).expect("parsed");
        let keys: Vec<&str> = snapshot
            .days
            .iter()
            .map(|day| day.date_key.as_str())
            .collect();
        assert_eq!(keys, vec!["2026-07-24", "2026-07-25", "2026-07-26"]);

        // 22:30 UTC on the 24th is already the 25th in Stockholm.
        let utc = parse_forecast(FORECAST, "Stensjön", chrono_tz::UTC).expect("parsed");
        let utc_keys: Vec<&str> = utc.days.iter().map(|day| day.date_key.as_str()).collect();
        assert_eq!(utc_keys, vec!["2026-07-24", "2026-07-25", "2026-07-26"]);
        // The 22:30Z point is already tomorrow in Stockholm but still today in UTC.
        assert_eq!(snapshot.days[0].low, 15.2);
        assert_eq!(utc.days[0].low, 11.0);
    }

    #[test]
    fn each_day_reports_its_high_low_and_total_precipitation() {
        let snapshot = parse_forecast(FORECAST, "Stensjön", Stockholm).expect("parsed");
        let today = snapshot.today().expect("today");

        assert_eq!(today.high, 23.1);
        assert_eq!(today.low, 15.2);
        assert!(
            (today.precipitation - 1.4).abs() < 1e-9,
            "{}",
            today.precipitation
        );
    }

    #[test]
    fn the_representative_symbol_comes_from_the_point_nearest_local_noon() {
        let snapshot = parse_forecast(FORECAST, "Stensjön", Stockholm).expect("parsed");
        let today = snapshot.today().expect("today");

        // 10:00 UTC is 12:00 in Stockholm and carries the thunder symbol.
        assert_eq!(today.symbol.family, SymbolFamily::Thunder);
        assert_eq!(
            today.representative.to_rfc3339(),
            "2026-07-24T10:00:00+00:00"
        );
    }

    #[test]
    fn negative_and_two_digit_temperatures_survive_aggregation() {
        let body = r#"{"properties":{"meta":{"updated_at":"2026-01-05T06:00:00Z"},"timeseries":[
            {"time":"2026-01-05T07:00:00Z","data":{"instant":{"details":{"air_temperature":-14.6,
              "relative_humidity":90,"wind_speed":3.0,"wind_from_direction":10}},
              "next_1_hours":{"summary":{"symbol_code":"heavysnow"},"details":{"precipitation_amount":2.0}}}},
            {"time":"2026-01-05T11:00:00Z","data":{"instant":{"details":{"air_temperature":-3.1,
              "relative_humidity":85,"wind_speed":4.0,"wind_from_direction":20}},
              "next_1_hours":{"summary":{"symbol_code":"snow"},"details":{"precipitation_amount":0.5}}}}
        ]}}"#;

        let snapshot = parse_forecast(body, "Stensjön", Stockholm).expect("parsed");
        let today = snapshot.today().expect("today");
        assert_eq!(today.low, -14.6);
        assert_eq!(today.high, -3.1);
        assert_eq!(crate::text::format_temperature(today.low), "-15°");
    }

    #[test]
    fn a_point_with_an_impossible_temperature_is_dropped() {
        let body = r#"{"properties":{"meta":{"updated_at":"2026-07-24T06:00:00Z"},"timeseries":[
            {"time":"2026-07-24T07:00:00Z","data":{"instant":{"details":{"air_temperature":999.0}}}},
            {"time":"2026-07-24T08:00:00Z","data":{"instant":{"details":{"air_temperature":18.0,
              "relative_humidity":50,"wind_speed":1.0,"wind_from_direction":90}},
              "next_1_hours":{"summary":{"symbol_code":"cloudy"},"details":{"precipitation_amount":0}}}}
        ]}}"#;

        let snapshot = parse_forecast(body, "x", Stockholm).expect("parsed");
        assert_eq!(snapshot.current.temperature, 18.0);
        assert_eq!(snapshot.days.len(), 1);
    }

    #[test]
    fn negative_precipitation_is_clamped_to_zero() {
        let body = r#"{"properties":{"meta":{"updated_at":"2026-07-24T06:00:00Z"},"timeseries":[
            {"time":"2026-07-24T07:00:00Z","data":{"instant":{"details":{"air_temperature":18.0,
              "relative_humidity":50,"wind_speed":1.0,"wind_from_direction":90}},
              "next_1_hours":{"summary":{"symbol_code":"cloudy"},"details":{"precipitation_amount":-3.0}}}}
        ]}}"#;
        let snapshot = parse_forecast(body, "x", Stockholm).expect("parsed");
        assert_eq!(snapshot.today().expect("today").precipitation, 0.0);
    }

    #[test]
    fn a_payload_with_no_usable_points_is_rejected_rather_than_rendered_empty() {
        for body in [
            r#"{"properties":{"timeseries":[]}}"#,
            r#"{"properties":{}}"#,
            r#"{}"#,
        ] {
            assert!(parse_forecast(body, "x", Stockholm).is_err(), "{body}");
        }
    }

    #[test]
    fn a_day_offset_past_the_forecast_horizon_falls_back_to_the_last_day() {
        let snapshot = parse_forecast(FORECAST, "Stensjön", Stockholm).expect("parsed");
        assert_eq!(
            snapshot.day(99).expect("fallback").date_key,
            snapshot.days.last().expect("last").date_key
        );
    }

    #[test]
    fn a_missing_updated_at_falls_back_to_the_first_point() {
        let body = r#"{"properties":{"timeseries":[
            {"time":"2026-07-24T07:00:00Z","data":{"instant":{"details":{"air_temperature":18.0,
              "relative_humidity":50,"wind_speed":1.0,"wind_from_direction":90}}}}
        ]}}"#;
        let snapshot = parse_forecast(body, "x", Stockholm).expect("parsed");
        assert_eq!(snapshot.updated_at, snapshot.current.time);
    }
}
