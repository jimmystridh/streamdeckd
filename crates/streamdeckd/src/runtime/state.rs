//! The coordinator's owned state.
//!
//! Split out from the event loop so the parts that are pure decisions — which
//! integrations are due, what the world view looks like, what to persist — can be
//! tested directly.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Datelike, Days, TimeZone, Timelike, Utc};
use chrono_tz::Tz;
use streamdeck_core::cache::{CachePolicy, Cached};
use streamdeck_core::config::Config;
use streamdeck_core::deadline::{Backoff, DeadlineId, DeadlineQueue};
use streamdeck_core::integrations::application::ApplicationInfo;
use streamdeck_core::integrations::audio::{AudioSnapshot, AudioTarget};
use streamdeck_core::integrations::ci::CiSnapshot;
use streamdeck_core::integrations::claude::ClaudeUsage;
use streamdeck_core::integrations::codex::CodexUsage;
use streamdeck_core::integrations::departures::DepartureBoard;
use streamdeck_core::integrations::github::GitHubSnapshot;
use streamdeck_core::integrations::lake::{LakeHistory, LakeReading};
use streamdeck_core::integrations::media::MediaStatus;
use streamdeck_core::integrations::meetings::Meeting;
use streamdeck_core::integrations::spotify::SpotifyStatus;
use streamdeck_core::integrations::system::{MacHealth, NetworkStatus};
use streamdeck_core::integrations::weather::WeatherSnapshot;
use streamdeck_core::model::{IntegrationId, PageId, WeatherTile};
use streamdeck_core::nav::Navigator;
use streamdeck_core::pages;
use streamdeck_core::pomodoro;
use streamdeck_core::press::{PressConfig, PressTracker};
use streamdeck_core::snapshot::{Feed, WorldView};
use streamdeck_core::state::{PersistentState, StateStore};

use crate::services::{intervals, millis};

/// Every cached integration value, each with its own lifetime policy.
pub struct Feeds {
    pub audio: Cached<AudioSnapshot>,
    pub meetings: Cached<Vec<Meeting>>,
    pub weather: Cached<WeatherSnapshot>,
    /// Kept so the next request can send `If-Modified-Since`.
    pub weather_last_modified: Option<String>,
    pub lake_current: Cached<LakeReading>,
    pub lake_history: Cached<LakeHistory>,
    pub github: Cached<GitHubSnapshot>,
    pub ci: Cached<CiSnapshot>,
    pub mac_health: Cached<MacHealth>,
    pub network: Cached<NetworkStatus>,
    pub departures: Cached<DepartureBoard>,
    pub claude: Cached<ClaudeUsage>,
    pub codex: Cached<CodexUsage>,
    pub spotify: Cached<SpotifyStatus>,
    pub media: Cached<MediaStatus>,
    pub application: Cached<ApplicationInfo>,
}

impl Default for Feeds {
    fn default() -> Self {
        let policy =
            |interval: Duration| CachePolicy::new(millis(interval), millis(intervals::ERROR_RETRY));
        Self {
            audio: Cached::new(policy(intervals::AUDIO_STATUS)),
            meetings: Cached::new(policy(intervals::MEETINGS)),
            weather: Cached::new(policy(intervals::WEATHER)),
            weather_last_modified: None,
            lake_current: Cached::new(policy(intervals::LAKE_CURRENT)),
            lake_history: Cached::new(policy(intervals::LAKE_HISTORY)),
            github: Cached::new(policy(intervals::GITHUB)),
            ci: Cached::new(policy(intervals::CI)),
            mac_health: Cached::new(policy(intervals::MAC_HEALTH)),
            network: Cached::new(policy(intervals::NETWORK)),
            departures: Cached::new(policy(intervals::DEPARTURES)),
            claude: Cached::new(policy(intervals::USAGE)),
            codex: Cached::new(policy(intervals::USAGE)),
            // Spotify polls fast but only while visible, and never retries slowly:
            // a closed application is normal, not a failure to back off from. The
            // cadence is per page; the glance rate is the safe default.
            spotify: Cached::new(CachePolicy::new(
                millis(intervals::SPOTIFY_GLANCE),
                millis(intervals::SPOTIFY_GLANCE),
            )),
            media: Cached::new(CachePolicy::new(
                millis(intervals::MEDIA_SESSION),
                millis(intervals::MEDIA_SESSION),
            )),
            application: Cached::new(CachePolicy::new(
                millis(intervals::FRONTMOST_APPLICATION),
                millis(intervals::FRONTMOST_APPLICATION),
            )),
        }
    }
}

impl Feeds {
    /// Whether the integration behind `id` needs fetching now.
    pub fn needs_fetch(&self, id: IntegrationId, now_ms: u64) -> bool {
        match id {
            IntegrationId::AudioStatus | IntegrationId::AudioInventory => {
                self.audio.needs_fetch(now_ms)
            }
            IntegrationId::Meetings => self.meetings.needs_fetch(now_ms),
            IntegrationId::Weather => self.weather.needs_fetch(now_ms),
            IntegrationId::LakeCurrent => self.lake_current.needs_fetch(now_ms),
            IntegrationId::LakeHistory => self.lake_history.needs_fetch(now_ms),
            IntegrationId::GitHub => self.github.needs_fetch(now_ms),
            IntegrationId::CiRadar => self.ci.needs_fetch(now_ms),
            IntegrationId::MacHealth => self.mac_health.needs_fetch(now_ms),
            IntegrationId::NetworkStatus => self.network.needs_fetch(now_ms),
            IntegrationId::Departures => self.departures.needs_fetch(now_ms),
            IntegrationId::ClaudeUsage => self.claude.needs_fetch(now_ms),
            IntegrationId::CodexUsage => self.codex.needs_fetch(now_ms),
            IntegrationId::Spotify => self.spotify.needs_fetch(now_ms),
            IntegrationId::MediaSession => self.media.needs_fetch(now_ms),
            IntegrationId::FrontmostApplication => self.application.needs_fetch(now_ms),
        }
    }

    /// The instant at which `id` next becomes due, when it has a value.
    pub fn next_due_ms(&self, id: IntegrationId) -> Option<u64> {
        match id {
            IntegrationId::AudioStatus | IntegrationId::AudioInventory => {
                self.audio.expires_at_ms()
            }
            IntegrationId::Meetings => self.meetings.expires_at_ms(),
            IntegrationId::Weather => self.weather.expires_at_ms(),
            IntegrationId::LakeCurrent => self.lake_current.expires_at_ms(),
            IntegrationId::LakeHistory => self.lake_history.expires_at_ms(),
            IntegrationId::GitHub => self.github.expires_at_ms(),
            IntegrationId::CiRadar => self.ci.expires_at_ms(),
            IntegrationId::MacHealth => self.mac_health.expires_at_ms(),
            IntegrationId::NetworkStatus => self.network.expires_at_ms(),
            IntegrationId::Departures => self.departures.expires_at_ms(),
            IntegrationId::ClaudeUsage => self.claude.expires_at_ms(),
            IntegrationId::CodexUsage => self.codex.expires_at_ms(),
            IntegrationId::Spotify => self.spotify.expires_at_ms(),
            IntegrationId::MediaSession => self.media.expires_at_ms(),
            IntegrationId::FrontmostApplication => self.application.expires_at_ms(),
        }
    }

    /// Forces `id` to refetch on the next pass, for a manual refresh press.
    pub fn invalidate(&mut self, id: IntegrationId) {
        match id {
            IntegrationId::AudioStatus | IntegrationId::AudioInventory => self.audio.invalidate(),
            IntegrationId::Meetings => self.meetings.invalidate(),
            IntegrationId::Weather => self.weather.invalidate(),
            IntegrationId::LakeCurrent => self.lake_current.invalidate(),
            IntegrationId::LakeHistory => self.lake_history.invalidate(),
            IntegrationId::GitHub => self.github.invalidate(),
            IntegrationId::CiRadar => self.ci.invalidate(),
            IntegrationId::MacHealth => self.mac_health.invalidate(),
            IntegrationId::NetworkStatus => self.network.invalidate(),
            IntegrationId::Departures => self.departures.invalidate(),
            IntegrationId::ClaudeUsage => self.claude.invalidate(),
            IntegrationId::CodexUsage => self.codex.invalidate(),
            IntegrationId::Spotify => self.spotify.invalidate(),
            IntegrationId::MediaSession => self.media.invalidate(),
            IntegrationId::FrontmostApplication => self.application.invalidate(),
        }
    }

    /// Turns a cache slot into the freshness-tagged feed the pages read.
    fn feed<T: Clone>(cache: &Cached<T>) -> Feed<T> {
        match cache.peek() {
            Some(value) if cache.is_stale() => Feed::Stale(value.clone()),
            Some(value) => Feed::Ready(value.clone()),
            None => match cache.last_error() {
                Some(error) => Feed::Failed(error.to_string()),
                None => Feed::Loading,
            },
        }
    }
}

/// Everything the coordinator owns between events.
pub struct RuntimeState {
    pub config: Arc<Config>,
    /// Where a reload re-reads configuration from.
    ///
    /// Held here rather than read from a process-global on each reload: two
    /// parallel tests pointing the same global at different files made one read
    /// the other's deliberately-broken config.
    pub config_path: std::path::PathBuf,
    pub timezone: Tz,
    pub store: StateStore,
    pub persistent: PersistentState,
    pub navigator: Navigator,
    pub presses: PressTracker,
    pub deadlines: DeadlineQueue,
    pub feeds: Feeds,
    /// Integrations with a refresh already running, so a second tile press or a
    /// second due deadline never starts a duplicate request.
    pub in_flight: HashSet<IntegrationId>,
    pub backoff: HashMap<IntegrationId, Backoff>,
    /// Most recently frontmost applications, newest first. Kept in memory only.
    pub application_history: Vec<ApplicationInfo>,
    pub audio_output: Vec<AudioTarget>,
    pub audio_input: Vec<AudioTarget>,
    /// Set while a Pomodoro completion is unacknowledged.
    pub alert_flashing: bool,
    pub wispr_hands_free: bool,
    /// The weather tile showing its expanded reading, and when it reverts.
    pub weather_detail: Option<(WeatherTile, u64)>,
}

impl RuntimeState {
    pub fn new(
        config: Arc<Config>,
        config_path: impl Into<std::path::PathBuf>,
        store: StateStore,
        persistent: PersistentState,
    ) -> Self {
        let timezone = config.location.timezone();
        let navigator = Navigator::new(
            persistent.active_page,
            config.temporary_panel_seconds * 1_000,
        );
        let presses = PressTracker::new(PressConfig {
            long_press_ms: config.long_press_ms,
        });
        let (audio_output, audio_input) = audio_targets(&config);

        Self {
            config,
            config_path: config_path.into(),
            timezone,
            store,
            persistent,
            navigator,
            presses,
            deadlines: DeadlineQueue::new(),
            feeds: Feeds::default(),
            in_flight: HashSet::new(),
            backoff: HashMap::new(),
            application_history: Vec::new(),
            audio_output,
            audio_input,
            alert_flashing: false,
            wispr_hands_free: false,
            weather_detail: None,
        }
    }

    /// Applies a validated configuration without dropping any cached data.
    pub fn apply_config(&mut self, config: Arc<Config>) {
        self.timezone = config.location.timezone();
        self.navigator
            .set_panel_duration_ms(config.temporary_panel_seconds * 1_000);
        self.presses.set_config(PressConfig {
            long_press_ms: config.long_press_ms,
        });
        let (output, input) = audio_targets(&config);
        self.audio_output = output;
        self.audio_input = input;
        self.config = config;
    }

    pub fn visible_page(&self) -> PageId {
        self.navigator.visible_page()
    }

    /// Builds the render inputs for one frame.
    pub fn world(&self, now: DateTime<Utc>, now_ms: u64) -> WorldView {
        let pomodoro = pomodoro::snapshot(
            &self.persistent.pomodoro,
            now.timestamp_millis(),
            self.timezone,
        );

        WorldView {
            now,
            now_ms,
            timezone: self.timezone,
            location_name: self.config.location.name.clone(),
            usage_thresholds: (
                self.config.usage.warning_percent,
                self.config.usage.critical_percent,
            ),
            repository_aliases: self
                .config
                .github
                .repository_aliases
                .iter()
                .map(|alias| (alias.prefix.clone(), alias.replacement.clone()))
                .collect(),
            pomodoro,
            pomodoro_alert_flashing: self.alert_flashing,
            wispr_hands_free: self.wispr_hands_free,
            audio: Feeds::feed(&self.feeds.audio),
            meetings: Feeds::feed(&self.feeds.meetings),
            weather: Feeds::feed(&self.feeds.weather),
            lake_current: Feeds::feed(&self.feeds.lake_current),
            lake_history: Feeds::feed(&self.feeds.lake_history),
            github: Feeds::feed(&self.feeds.github),
            ci: Feeds::feed(&self.feeds.ci),
            mac_health: Feeds::feed(&self.feeds.mac_health),
            network: Feeds::feed(&self.feeds.network),
            departures: Feeds::feed(&self.feeds.departures),
            claude: Feeds::feed(&self.feeds.claude),
            codex: Feeds::feed(&self.feeds.codex),
            spotify: Feeds::feed(&self.feeds.spotify),
            media: Feeds::feed(&self.feeds.media),
            application: Feeds::feed(&self.feeds.application),
            recent_applications: self.recent_applications(),
            weather_detail: self
                .weather_detail
                .filter(|(_, until)| now_ms < *until)
                .map(|(tile, _)| tile),
            panel_seconds_remaining: self.navigator.panel_seconds_remaining(now_ms),
            panel_total_seconds: self.navigator.panel_total_seconds(),
        }
    }

    pub fn record_frontmost_application(&mut self, application: &ApplicationInfo) {
        self.application_history
            .retain(|candidate| !candidate.same_application(application));
        self.application_history.insert(0, application.clone());
        self.application_history.truncate(6);
    }

    pub fn recent_application(&self, index: usize) -> Option<&ApplicationInfo> {
        let current = self.feeds.application.peek();
        self.application_history
            .iter()
            .filter(|candidate| {
                !current.is_some_and(|application| candidate.same_application(application))
            })
            .nth(index)
    }

    fn recent_applications(&self) -> Vec<ApplicationInfo> {
        (0..5)
            .filter_map(|index| self.recent_application(index).cloned())
            .collect()
    }

    /// The integrations that should be refreshed now: required by the visible
    /// page, due, and not already running.
    pub fn due_integrations(&self, now_ms: u64) -> Vec<IntegrationId> {
        pages::required_integrations(self.visible_page())
            .into_iter()
            .filter(|id| !self.in_flight.contains(id))
            .filter(|id| self.feeds.needs_fetch(*id, now_ms))
            .collect()
    }

    /// Matches the Spotify poll rate to what is on screen: the transport page
    /// needs two-second fidelity, the Home glance does not, and every poll is a
    /// process spawn. Entering the Spotify page also refreshes immediately so
    /// the transport controls never open showing ten-second-old state.
    fn apply_spotify_cadence(&mut self) {
        let on_spotify_page = self.visible_page() == PageId::Spotify
            || (self.visible_page() == PageId::Application
                && self.feeds.application.peek().is_some_and(|application| {
                    application.kind()
                        == streamdeck_core::integrations::application::ApplicationKind::Spotify
                }));
        let interval = if on_spotify_page {
            intervals::SPOTIFY_PAGE
        } else {
            intervals::SPOTIFY_GLANCE
        };
        let policy = CachePolicy::new(millis(interval), millis(interval));

        if self.feeds.spotify.policy() != policy {
            self.feeds.spotify.set_policy(policy);
            if on_spotify_page {
                self.feeds.spotify.invalidate();
            }
        }
    }

    /// Re-derives every refresh deadline for the visible page. Called after a page
    /// change, a successful refresh, and a system wake.
    pub fn schedule_refresh_deadlines(&mut self, now_ms: u64) {
        self.apply_spotify_cadence();
        for id in IntegrationId::ALL {
            self.deadlines.clear(DeadlineId::Refresh(id));
        }
        for id in pages::required_integrations(self.visible_page()) {
            let at = match self.feeds.next_due_ms(id) {
                Some(due) => due,
                // Nothing cached yet: refresh immediately.
                None => now_ms,
            };
            self.deadlines.set(DeadlineId::Refresh(id), at.max(now_ms));
        }

        // Meeting labels recompute every minute without refetching calendars.
        if pages::required_integrations(self.visible_page()).contains(&IntegrationId::Meetings) {
            self.deadlines.set(
                DeadlineId::MeetingLabels,
                now_ms + millis(intervals::MEETING_LABELS),
            );
        } else {
            self.deadlines.clear(DeadlineId::MeetingLabels);
        }

        self.schedule_home_weather_boundary(Utc::now(), now_ms);
    }

    pub fn schedule_home_weather_boundary(&mut self, now: DateTime<Utc>, now_ms: u64) {
        self.deadlines.clear(DeadlineId::HomeWeatherBoundary);
        if self.visible_page() != PageId::Home {
            return;
        }
        let boundary = next_home_weather_boundary(now, self.timezone);
        let remaining = (boundary - now).num_milliseconds().max(1) as u64;
        self.deadlines
            .set(DeadlineId::HomeWeatherBoundary, now_ms + remaining);
    }

    /// Schedules the Pomodoro completion and, when a countdown is on screen, the
    /// once-a-second repaint.
    pub fn schedule_pomodoro_deadlines(&mut self, now: DateTime<Utc>, now_ms: u64) {
        self.deadlines.clear(DeadlineId::PomodoroCompletion);
        self.deadlines.clear(DeadlineId::CountdownTick);

        let state = &self.persistent.pomodoro;
        if state.status == pomodoro::Status::Running {
            if let Some(ends_at) = state.ends_at_ms {
                let remaining = (ends_at - now.timestamp_millis()).max(0) as u64;
                self.deadlines
                    .set(DeadlineId::PomodoroCompletion, now_ms + remaining);
            }
        }

        if self.shows_countdown() {
            // Align to the next whole second so the displayed value never skips.
            self.deadlines
                .set(DeadlineId::CountdownTick, now_ms + 1_000 - (now_ms % 1_000));
        }
    }

    /// Whether the visible page has a tile that changes every second.
    pub fn shows_countdown(&self) -> bool {
        let running = self.persistent.pomodoro.status == pomodoro::Status::Running;
        let pending = self.persistent.pomodoro.pending_completion_phase.is_some();
        let page = self.visible_page();
        let has_timer_tile = matches!(page, PageId::Home | PageId::Pomodoro);

        (has_timer_tile && (running || pending)) || self.navigator.panel_is_open()
    }

    pub fn backoff_for(&mut self, id: IntegrationId) -> &mut Backoff {
        self.backoff
            .entry(id)
            .or_insert_with(|| Backoff::new(millis(intervals::ERROR_RETRY), 30 * 60_000))
    }
}

fn next_home_weather_boundary(now: DateTime<Utc>, timezone: Tz) -> DateTime<Utc> {
    let local = now.with_timezone(&timezone);
    let (date, hour) = if local.hour() < 17 {
        (local.date_naive(), 17)
    } else {
        (
            local
                .date_naive()
                .checked_add_days(Days::new(1))
                .expect("the next calendar day is representable"),
            0,
        )
    };
    timezone
        .with_ymd_and_hms(date.year(), date.month(), date.day(), hour, 0, 0)
        .single()
        .expect("midnight and 17:00 are unambiguous in configured timezones")
        .with_timezone(&Utc)
}

fn audio_targets(config: &Config) -> (Vec<AudioTarget>, Vec<AudioTarget>) {
    let build = |targets: &[streamdeck_core::config::AudioTargetConfig]| {
        targets
            .iter()
            .filter_map(|target| AudioTarget::from_config(target).ok())
            .collect()
    };
    (build(&config.audio.output), build(&config.audio.input))
}

#[cfg(test)]
mod tests {
    use super::*;
    use streamdeck_core::pomodoro::{Phase, PomodoroState};

    fn state() -> RuntimeState {
        let config = Arc::new(Config::parse(streamdeck_core::config::TEMPLATE).expect("template"));
        let directory = tempfile::tempdir().expect("temp dir");
        let store = StateStore::new(directory.path().join("state.json"));
        // Leak the directory so the store's path stays valid for the test's life.
        std::mem::forget(directory);
        RuntimeState::new(
            config,
            "/nonexistent/streamdeckd-test.toml",
            store,
            PersistentState::default(),
        )
    }

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-07-24T10:00:00Z")
            .expect("timestamp")
            .with_timezone(&Utc)
    }

    #[test]
    fn a_fresh_runtime_starts_on_the_persisted_page_with_no_data() {
        let state = state();
        assert_eq!(state.visible_page(), PageId::Home);

        let world = state.world(now(), 1_000);
        assert!(world.github.value().is_none());
        assert_eq!(world.location_name, "Stensjön");
        assert_eq!(world.usage_thresholds, (50, 80));
        assert_eq!(
            world.repository_aliases,
            vec![("visma.administration.".to_string(), "admin.".to_string())]
        );
    }

    #[test]
    fn configured_audio_targets_are_compiled_once() {
        let state = state();
        assert_eq!(state.audio_output.len(), 4);
        assert_eq!(state.audio_input.len(), 3);
        assert_eq!(state.audio_output[0].label, "MacBook");
    }

    #[test]
    fn application_history_is_recent_unique_and_excludes_the_current_app() {
        let mut state = state();
        for index in 0..7 {
            state.record_frontmost_application(&ApplicationInfo {
                name: format!("App {index}"),
                bundle_id: Some(format!("test.app.{index}")),
                pid: index,
            });
        }
        let current = ApplicationInfo {
            name: "App 6".to_string(),
            bundle_id: Some("test.app.6".to_string()),
            pid: 99,
        };
        state.record_frontmost_application(&current);
        state.feeds.application.store(current, 0);

        let recent = state.recent_applications();
        assert_eq!(recent.len(), 5);
        assert_eq!(recent[0].name, "App 5");
        assert_eq!(recent[4].name, "App 1");
    }

    #[test]
    fn only_the_visible_pages_integrations_are_due() {
        let mut state = state();
        let due = state.due_integrations(1_000);
        assert!(due.contains(&IntegrationId::Weather));
        assert!(!due.contains(&IntegrationId::GitHub));

        state.navigator.go_to(PageId::Dashboard);
        let due = state.due_integrations(1_000);
        for integration in [
            IntegrationId::GitHub,
            IntegrationId::CiRadar,
            IntegrationId::MacHealth,
            IntegrationId::NetworkStatus,
            IntegrationId::Departures,
        ] {
            assert!(due.contains(&integration), "{integration} should be due");
        }

        state.navigator.go_to(PageId::Pomodoro);
        assert!(
            state.due_integrations(1_000).is_empty(),
            "the Pomodoro page needs no integration"
        );

        state.navigator.go_to(PageId::Spotify);
        assert_eq!(state.due_integrations(1_000), vec![IntegrationId::Spotify]);

        state.navigator.go_to(PageId::Media);
        assert_eq!(
            state.due_integrations(1_000),
            vec![IntegrationId::MediaSession, IntegrationId::AudioStatus]
        );
    }

    #[test]
    fn an_in_flight_refresh_is_not_started_again() {
        let mut state = state();
        state.navigator.go_to(PageId::Dashboard);
        state.in_flight.insert(IntegrationId::GitHub);
        assert!(!state
            .due_integrations(1_000)
            .contains(&IntegrationId::GitHub));
    }

    #[test]
    fn a_fresh_cache_entry_is_not_due_again_until_it_expires() {
        let mut state = state();
        state.navigator.go_to(PageId::Dashboard);
        state.feeds.github.store(GitHubSnapshot::default(), 1_000);

        assert!(!state
            .due_integrations(2_000)
            .contains(&IntegrationId::GitHub));
        assert!(state
            .due_integrations(1_000 + 300_001)
            .contains(&IntegrationId::GitHub));
    }

    #[test]
    fn invalidating_makes_an_integration_due_immediately() {
        let mut state = state();
        state.navigator.go_to(PageId::Dashboard);
        state.feeds.github.store(GitHubSnapshot::default(), 1_000);
        state.feeds.invalidate(IntegrationId::GitHub);

        assert!(state
            .due_integrations(2_000)
            .contains(&IntegrationId::GitHub));
    }

    #[test]
    fn refresh_deadlines_only_cover_the_visible_page() {
        let mut state = state();
        state.navigator.go_to(PageId::Spotify);
        state.schedule_refresh_deadlines(1_000);

        assert!(state
            .deadlines
            .get(DeadlineId::Refresh(IntegrationId::Spotify))
            .is_some());
        assert!(state
            .deadlines
            .get(DeadlineId::Refresh(IntegrationId::Weather))
            .is_none());
        assert!(
            state.deadlines.get(DeadlineId::MeetingLabels).is_none(),
            "the Spotify page has no meeting tile"
        );
    }

    #[test]
    fn the_home_page_schedules_a_meeting_label_recompute() {
        let mut state = state();
        state.schedule_refresh_deadlines(1_000);
        assert_eq!(state.deadlines.get(DeadlineId::MeetingLabels), Some(61_000));
    }

    #[test]
    fn a_running_timer_schedules_its_exact_completion() {
        let mut state = state();
        pomodoro::start_phase(
            &mut state.persistent.pomodoro,
            Phase::Focus,
            now().timestamp_millis(),
        );
        state.schedule_pomodoro_deadlines(now(), 5_000);

        assert_eq!(
            state.deadlines.get(DeadlineId::PomodoroCompletion),
            Some(5_000 + 25 * 60 * 1_000)
        );
    }

    #[test]
    fn a_paused_timer_has_no_completion_deadline() {
        let mut state = state();
        state.persistent.pomodoro = PomodoroState::default();
        state.schedule_pomodoro_deadlines(now(), 5_000);

        assert_eq!(state.deadlines.get(DeadlineId::PomodoroCompletion), None);
        assert_eq!(state.deadlines.get(DeadlineId::CountdownTick), None);
    }

    #[test]
    fn a_visible_running_countdown_ticks_once_a_second_aligned_to_the_clock() {
        let mut state = state();
        pomodoro::start_phase(
            &mut state.persistent.pomodoro,
            Phase::Focus,
            now().timestamp_millis(),
        );
        state.schedule_pomodoro_deadlines(now(), 5_250);

        assert_eq!(state.deadlines.get(DeadlineId::CountdownTick), Some(6_000));
    }

    #[test]
    fn a_running_timer_on_an_unrelated_page_does_not_tick() {
        let mut state = state();
        pomodoro::start_phase(
            &mut state.persistent.pomodoro,
            Phase::Focus,
            now().timestamp_millis(),
        );
        state.navigator.go_to(PageId::GitHub);
        state.schedule_pomodoro_deadlines(now(), 5_000);

        assert_eq!(state.deadlines.get(DeadlineId::CountdownTick), None);
        assert!(
            state
                .deadlines
                .get(DeadlineId::PomodoroCompletion)
                .is_some(),
            "the completion must still fire"
        );
    }

    #[test]
    fn a_pending_completion_keeps_the_timer_tiles_ticking() {
        let mut state = state();
        pomodoro::start_phase(
            &mut state.persistent.pomodoro,
            Phase::Focus,
            now().timestamp_millis(),
        );
        pomodoro::reconcile(
            &mut state.persistent.pomodoro,
            now().timestamp_millis() + 25 * 60 * 1_000,
            state.timezone,
        );
        assert!(state.shows_countdown());
    }

    #[test]
    fn an_open_panel_ticks_so_its_countdown_moves() {
        let mut state = state();
        state.navigator.open_panel(PageId::Stensjon, 1_000);
        assert!(state.shows_countdown());
    }

    #[test]
    fn reloaded_configuration_is_applied_without_dropping_cached_data() {
        let mut state = state();
        state.feeds.github.store(GitHubSnapshot::default(), 1_000);

        let mut config = Config::parse(streamdeck_core::config::TEMPLATE).expect("template");
        config.long_press_ms = 900;
        config.temporary_panel_seconds = 20;
        config.audio.output.clear();
        state.apply_config(Arc::new(config));

        assert_eq!(state.config.long_press_ms, 900);
        assert_eq!(state.navigator.panel_total_seconds(), 20);
        assert!(state.audio_output.is_empty());
        assert!(
            state.feeds.github.peek().is_some(),
            "a reload must not discard cached data"
        );
    }

    #[test]
    fn a_stale_cache_entry_renders_as_stale_and_a_failed_one_as_failed() {
        let mut stale = state();
        stale.feeds.github.store(GitHubSnapshot::default(), 1_000);
        stale.feeds.github.fail("timeout", 400_000);
        assert!(stale.world(now(), 400_000).github.is_stale());

        let mut failed = state();
        failed.feeds.weather.fail("no network", 1_000);
        assert_eq!(
            failed.world(now(), 1_000).weather.error(),
            Some("no network")
        );
    }

    #[test]
    fn the_spotify_cadence_follows_the_visible_page() {
        let mut state = state();

        // On Home only the glance is visible: the slow cadence applies.
        state.schedule_refresh_deadlines(1_000);
        assert_eq!(
            state.feeds.spotify.policy().ttl_ms,
            millis(intervals::SPOTIFY_GLANCE)
        );

        // Entering the Spotify page tightens the cadence and refreshes now,
        // so the transport controls never open showing ten-second-old state.
        state.feeds.spotify.store(
            streamdeck_core::integrations::spotify::SpotifyStatus::not_running(),
            1_000,
        );
        state.navigator.go_to(PageId::Spotify);
        state.schedule_refresh_deadlines(2_000);
        assert_eq!(
            state.feeds.spotify.policy().ttl_ms,
            millis(intervals::SPOTIFY_PAGE)
        );
        assert!(
            state.feeds.spotify.needs_fetch(2_000),
            "entering the page must refresh immediately"
        );

        // Returning to Home relaxes it again without discarding the data.
        state.feeds.spotify.store(
            streamdeck_core::integrations::spotify::SpotifyStatus::not_running(),
            3_000,
        );
        state.navigator.go_to(PageId::Home);
        state.schedule_refresh_deadlines(4_000);
        assert_eq!(
            state.feeds.spotify.policy().ttl_ms,
            millis(intervals::SPOTIFY_GLANCE)
        );
        assert!(state.feeds.spotify.peek().is_some());
        assert!(
            !state.feeds.spotify.needs_fetch(4_000),
            "leaving the page must not force a refetch"
        );
    }

    #[test]
    fn the_weather_detail_window_expires_by_clock_even_without_its_deadline() {
        let mut state = state();
        state.weather_detail = Some((WeatherTile::Current, 7_000));

        assert_eq!(
            state.world(now(), 6_999).weather_detail,
            Some(WeatherTile::Current)
        );
        assert_eq!(
            state.world(now(), 7_000).weather_detail,
            None,
            "a stale window must never survive a missed deadline"
        );
    }

    #[test]
    fn the_home_weather_boundary_is_seventeen_then_midnight_in_local_time() {
        let before = DateTime::parse_from_rfc3339("2026-07-24T14:59:00Z")
            .expect("timestamp")
            .with_timezone(&Utc);
        assert_eq!(
            next_home_weather_boundary(before, chrono_tz::Europe::Stockholm).to_rfc3339(),
            "2026-07-24T15:00:00+00:00"
        );

        let after = DateTime::parse_from_rfc3339("2026-07-24T15:00:00Z")
            .expect("timestamp")
            .with_timezone(&Utc);
        assert_eq!(
            next_home_weather_boundary(after, chrono_tz::Europe::Stockholm).to_rfc3339(),
            "2026-07-24T22:00:00+00:00"
        );
    }

    #[test]
    fn backoff_is_tracked_per_integration() {
        let mut state = state();
        assert_eq!(state.backoff_for(IntegrationId::GitHub).fail(), 300_000);
        assert_eq!(state.backoff_for(IntegrationId::GitHub).fail(), 600_000);
        assert_eq!(
            state.backoff_for(IntegrationId::Weather).failures(),
            0,
            "one integration failing must not back another off"
        );
    }
}
