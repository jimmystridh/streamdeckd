//! Google Calendar event extraction and meeting countdown formatting.
//!
//! Only events with a valid `https://meet.google.com/...` URL become meetings.
//! Cancelled and all-day events are ignored, in-progress meetings are kept, and
//! duplicates across the two accounts collapse on their Meet URL.

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

use super::{parse_json, ParseError};
use crate::text::{format_duration_minutes, sanitize_single_line};

const INTEGRATION: &str = "meetings";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Meeting {
    pub account: String,
    pub title: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    /// Always normalised to `https://meet.google.com/<path>`.
    pub meet_url: String,
}

/// How a meeting tile should read right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeetingUrgency {
    /// Already started and not yet finished.
    Now,
    /// Starts within five minutes today.
    Imminent,
    /// Starts within fifteen minutes today.
    Soon,
    /// Later today.
    Today,
    /// Tomorrow or later.
    Later,
}

impl Meeting {
    pub fn urgency(&self, now: DateTime<Utc>, timezone: Tz) -> MeetingUrgency {
        if self.start <= now {
            return MeetingUrgency::Now;
        }
        if !is_same_local_day(self.start, now, timezone) {
            return MeetingUrgency::Later;
        }
        match minutes_until(self.start, now) {
            minutes if minutes <= 5 => MeetingUrgency::Imminent,
            minutes if minutes <= 15 => MeetingUrgency::Soon,
            _ => MeetingUrgency::Today,
        }
    }

    /// `HH:MM` start time in the configured timezone.
    pub fn start_label(&self, timezone: Tz) -> String {
        self.start
            .with_timezone(&timezone)
            .format("%H:%M")
            .to_string()
    }

    /// The countdown or day hint: `NOW`, `IN 42M`, `IN 2H`, `TOMORROW`, `THU`.
    pub fn status_label(&self, now: DateTime<Utc>, timezone: Tz) -> String {
        if self.start <= now {
            return "NOW".to_string();
        }
        if is_same_local_day(self.start, now, timezone) {
            return format!(
                "IN {}",
                format_duration_minutes(minutes_until(self.start, now).max(1))
            );
        }
        if is_next_local_day(self.start, now, timezone) {
            return "TOMORROW".to_string();
        }
        self.start
            .with_timezone(&timezone)
            .format("%a")
            .to_string()
            .to_uppercase()
    }
}

fn minutes_until(start: DateTime<Utc>, now: DateTime<Utc>) -> u32 {
    let seconds = (start - now).num_seconds().max(0);
    ((seconds + 59) / 60) as u32
}

fn is_same_local_day(left: DateTime<Utc>, right: DateTime<Utc>, timezone: Tz) -> bool {
    left.with_timezone(&timezone).date_naive() == right.with_timezone(&timezone).date_naive()
}

fn is_next_local_day(candidate: DateTime<Utc>, now: DateTime<Utc>, timezone: Tz) -> bool {
    candidate.with_timezone(&timezone).date_naive()
        == now.with_timezone(&timezone).date_naive() + chrono::Duration::days(1)
}

/// Parses one account's `gog calendar events --json --results-only` output.
pub fn parse_events(
    account: &str,
    body: &str,
    now: DateTime<Utc>,
) -> Result<Vec<Meeting>, ParseError> {
    let value = parse_json(INTEGRATION, body)?;
    let array = value.as_array().ok_or_else(|| {
        ParseError::shape(
            INTEGRATION,
            format!("calendar response for {account} is not an array"),
        )
    })?;

    Ok(array
        .iter()
        .filter_map(|event| to_meeting(account, event))
        .filter(|meeting| meeting.end > now)
        .collect())
}

/// Merges per-account meetings: sorted by start, deduplicated on Meet URL, with
/// the earliest occurrence of a shared meeting winning.
pub fn merge(mut meetings: Vec<Meeting>) -> Vec<Meeting> {
    meetings.sort_by_key(|meeting| meeting.start);
    let mut seen = std::collections::HashSet::new();
    meetings.retain(|meeting| seen.insert(meeting.meet_url.clone()));
    meetings
}

fn to_meeting(account: &str, event: &serde_json::Value) -> Option<Meeting> {
    if event.get("status").and_then(serde_json::Value::as_str) == Some("cancelled") {
        return None;
    }
    // An all-day event has `start.date` rather than `start.dateTime`.
    let start = parse_timestamp(event.get("start")?.get("dateTime")?)?;
    let end = parse_timestamp(event.get("end")?.get("dateTime")?)?;
    if end <= start {
        return None;
    }
    let meet_url = find_meet_url(event)?;
    let title = sanitize_single_line(
        event
            .get("summary")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default(),
    );

    Some(Meeting {
        account: account.to_string(),
        title: if title.is_empty() {
            "Google Meet".to_string()
        } else {
            title
        },
        start,
        end,
        meet_url,
    })
}

fn parse_timestamp(value: &serde_json::Value) -> Option<DateTime<Utc>> {
    let text = value.as_str()?;
    Some(DateTime::parse_from_rfc3339(text).ok()?.with_timezone(&Utc))
}

/// Accepts a Meet link only from `hangoutLink` or a `video` conference entry
/// point, and only when it is really on `meet.google.com`.
fn find_meet_url(event: &serde_json::Value) -> Option<String> {
    let hangout = event.get("hangoutLink").and_then(serde_json::Value::as_str);
    let video = event
        .get("conferenceData")
        .and_then(|data| data.get("entryPoints"))
        .and_then(serde_json::Value::as_array)
        .and_then(|entries| {
            entries.iter().find_map(|entry| {
                (entry
                    .get("entryPointType")
                    .and_then(serde_json::Value::as_str)
                    == Some("video"))
                .then(|| entry.get("uri").and_then(serde_json::Value::as_str))
                .flatten()
            })
        });

    [hangout, video]
        .into_iter()
        .flatten()
        .find_map(normalize_meet_url)
}

/// Validates and canonicalises a Meet URL. Everything else is refused so a
/// malicious calendar invite cannot make the daemon open an arbitrary link.
pub fn normalize_meet_url(candidate: &str) -> Option<String> {
    let rest = candidate.strip_prefix("https://")?;
    let (host, path) = match rest.split_once('/') {
        Some((host, path)) => (host, path),
        None => return None,
    };
    if !host.eq_ignore_ascii_case("meet.google.com") {
        return None;
    }
    let path = path.split(['?', '#']).next().unwrap_or_default();
    if path.is_empty()
        || !path.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '-' || character == '/'
        })
    {
        return None;
    }
    Some(format!("https://meet.google.com/{path}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono_tz::Europe::Stockholm;

    const EVENTS: &str = include_str!("../../../../tests/fixtures/gog-calendar-events.json");

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .expect("timestamp")
            .with_timezone(&Utc)
    }

    fn meeting(start: &str, end: &str) -> Meeting {
        Meeting {
            account: "a@example.com".to_string(),
            title: "Standup".to_string(),
            start: at(start),
            end: at(end),
            meet_url: "https://meet.google.com/abc-defg-hij".to_string(),
        }
    }

    #[test]
    fn valid_events_with_meet_links_become_meetings() {
        let now = at("2026-07-24T06:00:00Z");
        let meetings = parse_events("jimmy.stridh@visma.com", EVENTS, now).expect("parsed");

        let titles: Vec<&str> = meetings.iter().map(|m| m.title.as_str()).collect();
        assert!(titles.contains(&"Sprint planning"), "{titles:?}");
        assert!(titles.contains(&"Architecture review"), "{titles:?}");
        assert!(titles.contains(&"Ongoing incident bridge"), "{titles:?}");
    }

    #[test]
    fn cancelled_all_day_and_link_free_events_are_ignored() {
        let now = at("2026-07-24T06:00:00Z");
        let meetings = parse_events("a@example.com", EVENTS, now).expect("parsed");
        let titles: Vec<&str> = meetings.iter().map(|m| m.title.as_str()).collect();

        assert!(!titles.contains(&"Cancelled sync"), "{titles:?}");
        assert!(!titles.contains(&"Vacation"), "{titles:?}");
        assert!(!titles.contains(&"Coffee, no link"), "{titles:?}");
        assert!(
            !titles.contains(&"Phishy offsite"),
            "non-Meet link rejected"
        );
    }

    #[test]
    fn an_in_progress_meeting_is_kept_and_a_finished_one_is_dropped() {
        let now = at("2026-07-24T09:15:00Z");
        let meetings = parse_events("a@example.com", EVENTS, now).expect("parsed");
        let titles: Vec<&str> = meetings.iter().map(|m| m.title.as_str()).collect();

        assert!(titles.contains(&"Ongoing incident bridge"), "{titles:?}");
        assert!(!titles.contains(&"Yesterday retro"), "{titles:?}");
    }

    #[test]
    fn a_conference_entry_point_is_used_when_there_is_no_hangout_link() {
        let now = at("2026-07-24T06:00:00Z");
        let meetings = parse_events("a@example.com", EVENTS, now).expect("parsed");
        let review = meetings
            .iter()
            .find(|meeting| meeting.title == "Architecture review")
            .expect("review is present");
        assert_eq!(review.meet_url, "https://meet.google.com/xyz-1234-abc");
    }

    #[test]
    fn merging_sorts_by_start_and_deduplicates_on_meet_url() {
        let shared = "https://meet.google.com/shared-room";
        let mut later = meeting("2026-07-24T12:00:00Z", "2026-07-24T13:00:00Z");
        later.meet_url = shared.to_string();
        later.account = "work@example.com".to_string();
        let mut earlier = meeting("2026-07-24T08:00:00Z", "2026-07-24T09:00:00Z");
        earlier.meet_url = shared.to_string();
        earlier.account = "personal@example.com".to_string();
        let other = meeting("2026-07-24T10:00:00Z", "2026-07-24T10:30:00Z");

        let merged = merge(vec![later, other.clone(), earlier.clone()]);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].start, earlier.start);
        assert_eq!(merged[0].account, "personal@example.com");
        assert_eq!(merged[1].meet_url, other.meet_url);
    }

    #[test]
    fn urgency_bands_follow_the_tile_colours() {
        let now = at("2026-07-24T10:00:00Z");

        let ongoing = meeting("2026-07-24T09:50:00Z", "2026-07-24T10:30:00Z");
        assert_eq!(ongoing.urgency(now, Stockholm), MeetingUrgency::Now);

        let imminent = meeting("2026-07-24T10:04:00Z", "2026-07-24T10:30:00Z");
        assert_eq!(imminent.urgency(now, Stockholm), MeetingUrgency::Imminent);

        let soon = meeting("2026-07-24T10:12:00Z", "2026-07-24T10:30:00Z");
        assert_eq!(soon.urgency(now, Stockholm), MeetingUrgency::Soon);

        let today = meeting("2026-07-24T15:00:00Z", "2026-07-24T16:00:00Z");
        assert_eq!(today.urgency(now, Stockholm), MeetingUrgency::Today);

        let later = meeting("2026-07-26T09:00:00Z", "2026-07-26T10:00:00Z");
        assert_eq!(later.urgency(now, Stockholm), MeetingUrgency::Later);
    }

    #[test]
    fn status_labels_read_as_the_plan_describes() {
        let now = at("2026-07-24T10:00:00Z");

        assert_eq!(
            meeting("2026-07-24T09:50:00Z", "2026-07-24T10:30:00Z").status_label(now, Stockholm),
            "NOW"
        );
        assert_eq!(
            meeting("2026-07-24T10:42:00Z", "2026-07-24T11:00:00Z").status_label(now, Stockholm),
            "IN 42M"
        );
        assert_eq!(
            meeting("2026-07-24T12:00:00Z", "2026-07-24T13:00:00Z").status_label(now, Stockholm),
            "IN 2H"
        );
        assert_eq!(
            meeting("2026-07-25T09:00:00Z", "2026-07-25T10:00:00Z").status_label(now, Stockholm),
            "TOMORROW"
        );
        assert_eq!(
            meeting("2026-07-30T09:00:00Z", "2026-07-30T10:00:00Z").status_label(now, Stockholm),
            "THU"
        );
    }

    #[test]
    fn a_meeting_seconds_away_still_reports_at_least_one_minute() {
        let now = at("2026-07-24T10:00:00Z");
        let almost = meeting("2026-07-24T10:00:20Z", "2026-07-24T10:30:00Z");
        assert_eq!(almost.status_label(now, Stockholm), "IN 1M");
    }

    #[test]
    fn start_labels_use_the_configured_timezone() {
        let meeting = meeting("2026-07-24T09:30:00Z", "2026-07-24T10:00:00Z");
        assert_eq!(meeting.start_label(Stockholm), "11:30");
        assert_eq!(meeting.start_label(chrono_tz::UTC), "09:30");
    }

    #[test]
    fn meet_urls_are_validated_and_canonicalised() {
        assert_eq!(
            normalize_meet_url("https://meet.google.com/abc-defg-hij?authuser=1"),
            Some("https://meet.google.com/abc-defg-hij".to_string())
        );
        assert_eq!(
            normalize_meet_url("https://MEET.GOOGLE.COM/abc-defg-hij"),
            Some("https://meet.google.com/abc-defg-hij".to_string())
        );
        assert_eq!(normalize_meet_url("https://meet.google.com/"), None);
        assert_eq!(
            normalize_meet_url("https://meet.google.com.evil.test/x"),
            None
        );
        assert_eq!(normalize_meet_url("http://meet.google.com/abc"), None);
        assert_eq!(normalize_meet_url("https://zoom.us/j/123"), None);
        assert_eq!(normalize_meet_url("not a url"), None);
        assert_eq!(
            normalize_meet_url("https://meet.google.com/abc\"><script>"),
            None
        );
    }

    #[test]
    fn a_non_array_calendar_response_is_rejected_so_one_account_can_fail_alone() {
        let error = parse_events(
            "a@example.com",
            r#"{"error":"nope"}"#,
            at("2026-07-24T10:00:00Z"),
        )
        .expect_err("rejected");
        assert!(error.to_string().contains("a@example.com"), "{error}");
    }

    #[test]
    fn an_event_with_no_summary_falls_back_to_a_neutral_title() {
        let body = r#"[{"start":{"dateTime":"2026-07-24T12:00:00Z"},
            "end":{"dateTime":"2026-07-24T13:00:00Z"},
            "hangoutLink":"https://meet.google.com/aaa-bbbb-ccc"}]"#;
        let meetings =
            parse_events("a@example.com", body, at("2026-07-24T10:00:00Z")).expect("parsed");
        assert_eq!(meetings[0].title, "Google Meet");
    }
}
