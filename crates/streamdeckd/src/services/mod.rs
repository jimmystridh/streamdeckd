//! Integration services.
//!
//! Each service is a plain async function that fetches and parses one
//! integration. Caching, staleness, single-flight coalescing, and retry backoff
//! all live in the runtime, so a service has exactly one job and is trivial to
//! test with a fake command runner or a local HTTP endpoint.

pub mod ci;
pub mod github;
pub mod http;
pub mod lake;
pub mod meetings;
pub mod system;
pub mod usage;
pub mod vasttrafik;
pub mod walkingpad;
pub mod weather;

use std::time::Duration;

/// A fetched value plus the cache metadata the runtime needs.
#[derive(Debug, Clone, PartialEq)]
pub struct Fetched<T> {
    pub value: T,
    /// Server-declared expiry, in epoch milliseconds, when one was given.
    pub expires_at_ms: Option<i64>,
    /// `Last-Modified`, echoed back on the next request.
    pub last_modified: Option<String>,
}

impl<T> Fetched<T> {
    pub fn new(value: T) -> Self {
        Self {
            value,
            expires_at_ms: None,
            last_modified: None,
        }
    }
}

/// Either a new value or confirmation that the cached one is still current.
#[derive(Debug, Clone, PartialEq)]
pub enum Refreshed<T> {
    Updated(Fetched<T>),
    /// The server answered `304`; keep the cached value and extend its lifetime.
    Unchanged {
        expires_at_ms: Option<i64>,
    },
}

/// Per-integration refresh intervals, from the plan's visibility-aware table.
pub mod intervals {
    use std::time::Duration;

    pub const AUDIO_STATUS: Duration = Duration::from_secs(30);
    pub const AUDIO_INVENTORY: Duration = Duration::from_secs(5 * 60);
    /// Calendars are fetched at most this often; labels recompute every minute.
    pub const MEETINGS: Duration = Duration::from_secs(5 * 60);
    pub const MEETING_LABELS: Duration = Duration::from_secs(60);
    pub const LAKE_CURRENT: Duration = Duration::from_secs(5 * 60);
    pub const LAKE_HISTORY: Duration = Duration::from_secs(15 * 60);
    /// Default when MET Norway sends no `Expires`.
    pub const WEATHER: Duration = Duration::from_secs(30 * 60);
    pub const GITHUB: Duration = Duration::from_secs(5 * 60);
    pub const CI: Duration = Duration::from_secs(5 * 60);
    pub const MAC_HEALTH: Duration = Duration::from_secs(60);
    pub const NETWORK: Duration = Duration::from_secs(30);
    pub const DEPARTURES: Duration = Duration::from_secs(30);
    pub const USAGE: Duration = Duration::from_secs(5 * 60);
    /// On the Spotify page the transport state must track closely.
    pub const SPOTIFY_PAGE: Duration = Duration::from_secs(2);
    /// The Home glance only shows track, play state, and artwork; every status
    /// poll is an `osascript` spawn, and at two seconds those spawns were the
    /// daemon's dominant idle cost. Ten seconds is imperceptible on a glance —
    /// and a press refreshes immediately anyway, so interaction stays instant.
    pub const SPOTIFY_GLANCE: Duration = Duration::from_secs(10);
    /// Native MediaRemote lookup while the generic media page is visible.
    pub const MEDIA_SESSION: Duration = Duration::from_secs(5);
    /// Native NSWorkspace lookup; no subprocess or AppleScript is involved.
    pub const FRONTMOST_APPLICATION: Duration = Duration::from_secs(1);
    /// How long to wait before retrying after a failure, while stale data shows.
    pub const ERROR_RETRY: Duration = Duration::from_secs(5 * 60);
}

/// Per-request timeouts.
pub mod timeouts {
    use std::time::Duration;

    pub const WEATHER: Duration = Duration::from_secs(10);
    pub const LAKE: Duration = Duration::from_secs(10);
    pub const USAGE: Duration = Duration::from_secs(10);
    pub const VASTTRAFIK: Duration = Duration::from_secs(10);
}

/// Turns a duration into the milliseconds the cache policy uses.
pub const fn millis(duration: Duration) -> u64 {
    duration.as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fetched_value_starts_with_no_cache_metadata() {
        let fetched = Fetched::new(42u32);
        assert_eq!(fetched.value, 42);
        assert_eq!(fetched.expires_at_ms, None);
        assert_eq!(fetched.last_modified, None);
    }

    #[test]
    fn the_documented_intervals_match_the_plan() {
        assert_eq!(intervals::AUDIO_STATUS.as_secs(), 30);
        assert_eq!(intervals::MEETINGS.as_secs(), 300);
        assert_eq!(intervals::MEETING_LABELS.as_secs(), 60);
        assert_eq!(intervals::LAKE_CURRENT.as_secs(), 300);
        assert_eq!(intervals::LAKE_HISTORY.as_secs(), 900);
        assert_eq!(intervals::WEATHER.as_secs(), 1800);
        assert_eq!(intervals::GITHUB.as_secs(), 300);
        assert_eq!(intervals::CI.as_secs(), 300);
        assert_eq!(intervals::MAC_HEALTH.as_secs(), 60);
        assert_eq!(intervals::NETWORK.as_secs(), 30);
        assert_eq!(intervals::DEPARTURES.as_secs(), 30);
        assert_eq!(intervals::USAGE.as_secs(), 300);
        assert_eq!(intervals::SPOTIFY_PAGE.as_secs(), 2);
        assert_eq!(intervals::SPOTIFY_GLANCE.as_secs(), 10);
        assert_eq!(intervals::MEDIA_SESSION.as_secs(), 5);
        assert_eq!(intervals::FRONTMOST_APPLICATION.as_secs(), 1);
        assert_eq!(intervals::ERROR_RETRY.as_secs(), 300);
    }

    #[test]
    fn millisecond_conversion_is_exact() {
        assert_eq!(millis(Duration::from_secs(300)), 300_000);
        assert_eq!(millis(Duration::from_millis(1_500)), 1_500);
    }
}
