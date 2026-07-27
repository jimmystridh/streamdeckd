//! Everything the pages need in order to render, in one immutable value.
//!
//! Services publish snapshots; the coordinator assembles them into a `WorldView`
//! and hands it to the page layer. Pages never call a service, so rendering is a
//! pure function of this struct and cannot block on I/O.

use chrono::{DateTime, Utc};
use chrono_tz::Tz;

use crate::integrations::{
    audio::AudioSnapshot,
    claude::ClaudeUsage,
    codex::CodexUsage,
    github::GitHubSnapshot,
    lake::{LakeHistory, LakeReading},
    media::MediaStatus,
    meetings::Meeting,
    spotify::SpotifyStatus,
    weather::WeatherSnapshot,
};
use crate::pomodoro::PomodoroSnapshot;

/// A published integration value with the freshness the tile must reflect.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum Feed<T> {
    /// No attempt has completed yet.
    #[default]
    Loading,
    /// Current data.
    Ready(T),
    /// Cached data shown after a failed refresh.
    Stale(T),
    /// No usable data, with a short sanitized reason for the tile footer.
    Failed(String),
}

impl<T> Feed<T> {
    pub fn value(&self) -> Option<&T> {
        match self {
            Feed::Ready(value) | Feed::Stale(value) => Some(value),
            _ => None,
        }
    }

    pub fn is_stale(&self) -> bool {
        matches!(self, Feed::Stale(_))
    }

    pub fn error(&self) -> Option<&str> {
        match self {
            Feed::Failed(reason) => Some(reason.as_str()),
            _ => None,
        }
    }

    /// The key status a tile should carry for this feed, given that its own data
    /// is otherwise fine.
    pub fn status(&self) -> crate::view::KeyStatus {
        match self {
            Feed::Loading => crate::view::KeyStatus::Loading,
            Feed::Ready(_) => crate::view::KeyStatus::Ok,
            Feed::Stale(_) => crate::view::KeyStatus::Stale,
            Feed::Failed(_) => crate::view::KeyStatus::Error,
        }
    }

    pub fn map<U>(&self, transform: impl FnOnce(&T) -> U) -> Feed<U> {
        match self {
            Feed::Loading => Feed::Loading,
            Feed::Ready(value) => Feed::Ready(transform(value)),
            Feed::Stale(value) => Feed::Stale(transform(value)),
            Feed::Failed(reason) => Feed::Failed(reason.clone()),
        }
    }
}

/// The rendering inputs for one frame.
#[derive(Debug, Clone)]
pub struct WorldView {
    pub now: DateTime<Utc>,
    /// Monotonic milliseconds, for press and panel timing.
    pub now_ms: u64,
    pub timezone: Tz,
    pub location_name: String,
    pub usage_thresholds: (u8, u8),
    pub repository_aliases: Vec<(String, String)>,

    pub pomodoro: PomodoroSnapshot,
    /// Set while a completion is unacknowledged and the alert is being shown.
    pub pomodoro_alert_flashing: bool,
    pub wispr_hands_free: bool,

    pub audio: Feed<AudioSnapshot>,
    pub meetings: Feed<Vec<Meeting>>,
    pub weather: Feed<WeatherSnapshot>,
    pub lake_current: Feed<LakeReading>,
    pub lake_history: Feed<LakeHistory>,
    pub github: Feed<GitHubSnapshot>,
    pub claude: Feed<ClaudeUsage>,
    pub codex: Feed<CodexUsage>,
    pub spotify: Feed<SpotifyStatus>,
    pub media: Feed<MediaStatus>,

    /// The weather tile currently showing its expanded reading, if any.
    pub weather_detail: Option<crate::model::WeatherTile>,

    /// Countdown shown by the Stensjön panel's auto-close tile, when it is open.
    pub panel_seconds_remaining: Option<u64>,
    pub panel_total_seconds: u64,
}

impl WorldView {
    /// A view with no integration data yet, used at startup and in tests.
    pub fn empty(now: DateTime<Utc>, now_ms: u64, timezone: Tz) -> Self {
        Self {
            now,
            now_ms,
            timezone,
            location_name: "Stensjön".to_string(),
            usage_thresholds: (50, 80),
            repository_aliases: Vec::new(),
            pomodoro: crate::pomodoro::snapshot(
                &crate::pomodoro::PomodoroState::default(),
                now.timestamp_millis(),
                timezone,
            ),
            pomodoro_alert_flashing: false,
            wispr_hands_free: false,
            audio: Feed::Loading,
            meetings: Feed::Loading,
            weather: Feed::Loading,
            lake_current: Feed::Loading,
            lake_history: Feed::Loading,
            github: Feed::Loading,
            claude: Feed::Loading,
            codex: Feed::Loading,
            spotify: Feed::Loading,
            media: Feed::Loading,
            weather_detail: None,
            panel_seconds_remaining: None,
            panel_total_seconds: 10,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::KeyStatus;

    #[test]
    fn feeds_report_their_value_and_freshness() {
        assert_eq!(Feed::<u32>::Loading.value(), None);
        assert_eq!(Feed::Ready(4u32).value(), Some(&4));
        assert_eq!(Feed::Stale(4u32).value(), Some(&4));
        assert_eq!(Feed::<u32>::Failed("boom".into()).value(), None);

        assert!(!Feed::Ready(4u32).is_stale());
        assert!(Feed::Stale(4u32).is_stale());
        assert_eq!(Feed::<u32>::Failed("boom".into()).error(), Some("boom"));
    }

    #[test]
    fn feed_statuses_map_onto_distinct_key_treatments() {
        assert_eq!(Feed::<u32>::Loading.status(), KeyStatus::Loading);
        assert_eq!(Feed::Ready(1u32).status(), KeyStatus::Ok);
        assert_eq!(Feed::Stale(1u32).status(), KeyStatus::Stale);
        assert_eq!(Feed::<u32>::Failed("x".into()).status(), KeyStatus::Error);
    }

    #[test]
    fn mapping_preserves_freshness_and_errors() {
        assert_eq!(Feed::Stale(2u32).map(|value| value * 2), Feed::Stale(4u32));
        assert_eq!(
            Feed::<u32>::Failed("x".into()).map(|value| value * 2),
            Feed::<u32>::Failed("x".into())
        );
        assert_eq!(
            Feed::<u32>::Loading.map(|value| value * 2),
            Feed::<u32>::Loading
        );
    }

    #[test]
    fn an_empty_world_view_is_renderable() {
        let now = Utc::now();
        let view = WorldView::empty(now, 1_000, chrono_tz::Europe::Stockholm);

        assert_eq!(view.now_ms, 1_000);
        assert!(view.github.value().is_none());
        assert_eq!(view.pomodoro.status, crate::pomodoro::Status::Ready);
        assert_eq!(view.panel_seconds_remaining, None);
    }
}
