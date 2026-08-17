//! Action application and refresh dispatch.
//!
//! `apply` is a pure function from an action to a set of effects, so every press
//! semantic is testable without a device, a network, or a clock. Anything that has
//! to talk to the system comes back as a `Task` the coordinator spawns.

use std::sync::Arc;

use chrono::{DateTime, Datelike, Timelike, Utc, Weekday};
use chrono_tz::Tz;
use streamdeck_core::config::QuickCaptureConfig;
use streamdeck_core::integrations::application::{
    ApplicationInfo, ContextAction, CustomApplicationAction,
};
use streamdeck_core::integrations::audio::AudioSnapshot;
use streamdeck_core::integrations::ci::CiSnapshot;
use streamdeck_core::integrations::claude::ClaudeUsage;
use streamdeck_core::integrations::codex::CodexUsage;
use streamdeck_core::integrations::departures::DepartureBoard;
use streamdeck_core::integrations::github::{GitHubSnapshot, MetricKind};
use streamdeck_core::integrations::lake::{LakeHistory, LakeReading};
use streamdeck_core::integrations::media::MediaStatus;
use streamdeck_core::integrations::meetings::Meeting;
use streamdeck_core::integrations::spotify::SpotifyStatus;
use streamdeck_core::integrations::system::{MacHealth, NetworkStatus};
use streamdeck_core::integrations::walkingpad::WalkingPadRequest;
use streamdeck_core::integrations::weather::WeatherSnapshot;
use streamdeck_core::model::{AudioKind, IntegrationId, PageId};
use streamdeck_core::pages::{
    Action, ApplicationCommand, AudioCommand, CaptureDestination, DashboardCommand, MediaCommand,
    PomodoroCommand, SpotifyCommand, WisprCommand,
};
use streamdeck_core::pomodoro;
use streamdeck_core::state::Durability;
use streamdeck_macos::application::ApplicationControl;
use streamdeck_macos::media::Control as MediaControl;
use streamdeck_macos::spotify::Control;
use tokio::sync::mpsc;

use super::state::RuntimeState;
use super::{RuntimeEvent, Services};
use crate::metrics::Metrics;
use crate::services::{self, http::HttpClient, Refreshed};

/// How long a pressed weather tile shows its expanded reading, matching the
/// previous plugin's behaviour.
pub const WEATHER_DETAIL_MS: u64 = 6_000;

fn quick_capture_vault<'a>(
    destination: CaptureDestination,
    config: &'a QuickCaptureConfig,
    local_now: &DateTime<Tz>,
) -> &'a str {
    let weekday = matches!(
        local_now.weekday(),
        Weekday::Mon | Weekday::Tue | Weekday::Wed | Weekday::Thu | Weekday::Fri
    );
    let automatic_work_time = weekday && (6..18).contains(&local_now.hour());

    match destination {
        CaptureDestination::Automatic if automatic_work_time => &config.work_vault,
        CaptureDestination::Automatic | CaptureDestination::Personal => &config.personal_vault,
        CaptureDestination::Work => &config.work_vault,
    }
}

/// Something the coordinator must do after applying an action.
#[derive(Debug, Default)]
pub struct Effects {
    pub page_changed: bool,
    pub pomodoro_changed: bool,
    /// A weather tile opened or re-armed its detail window.
    pub weather_detail_changed: bool,
    pub persist: Option<Durability>,
    pub invalidate: Vec<IntegrationId>,
    pub spawn: Vec<Task>,
    pub walkingpad_request: Option<WalkingPadRequest>,
    pub walkingpad_feedback_changed: bool,
}

/// Work that has to leave the coordinator's thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Task {
    SelectAudio {
        kind: AudioKind,
        index: usize,
    },
    AdjustVolume {
        kind: AudioKind,
        delta: i32,
    },
    ToggleMute(AudioKind),
    Spotify(SpotifyCommand),
    Media(MediaCommand),
    Application {
        application: ApplicationInfo,
        control: ApplicationControl,
    },
    CustomApplication {
        application: ApplicationInfo,
        action: CustomApplicationAction,
    },
    SetWisprHandsFree(bool),
    SelectWisprMicrophone(usize),
    OpenMeeting(usize),
    QuickCapture(CaptureDestination),
    OpenActivityMonitor,
    OpenVpn,
    OpenNetworkSettings,
    OpenUrl(String),
}

/// The result of a spawned task.
#[derive(Debug, Default)]
pub struct ActionOutcome {
    pub error: Option<String>,
    pub invalidate: Vec<IntegrationId>,
    /// Set when a microphone mute captured the level to restore later.
    pub remembered_input_volume: Option<u8>,
    pub wispr_hands_free: Option<bool>,
}

/// The result of a spawned refresh.
#[derive(Debug)]
pub enum RefreshResult {
    Audio(Result<AudioSnapshot, String>),
    Meetings(Result<Vec<Meeting>, String>),
    Weather(Result<Refreshed<WeatherSnapshot>, String>),
    LakeCurrent(Result<LakeReading, String>),
    LakeHistory(Result<LakeHistory, String>),
    GitHub(Result<GitHubSnapshot, String>),
    Ci(Result<CiSnapshot, String>),
    MacHealth(Result<MacHealth, String>),
    Network(Result<NetworkStatus, String>),
    Departures(Result<DepartureBoard, String>),
    Claude(Result<ClaudeUsage, String>),
    Codex(Result<CodexUsage, String>),
    Spotify(Result<SpotifyStatus, String>),
    Media(Result<MediaStatus, String>),
    Application(Result<ApplicationInfo, String>),
}

/// Applies an action to the runtime's own state and reports what else must happen.
pub fn apply(state: &mut RuntimeState, action: Action, now: DateTime<Utc>, now_ms: u64) -> Effects {
    let mut effects = Effects::default();
    let now_wall = now.timestamp_millis();

    match action {
        Action::None => {}
        Action::Navigate(page) => {
            effects.page_changed = state.navigator.go_to(page);
        }
        Action::OpenPanel(page) => {
            effects.page_changed = state.navigator.open_panel(page, now_ms);
        }
        Action::DismissPanel => {
            effects.page_changed = state.navigator.dismiss_panel();
        }
        Action::Acknowledge => {}
        Action::Pomodoro(command) => {
            apply_pomodoro(state, command, now_wall, &mut effects);
        }
        Action::Audio(command) => {
            effects.spawn.push(match command {
                AudioCommand::Select { kind, index } => Task::SelectAudio { kind, index },
                AudioCommand::Volume { kind, delta } => Task::AdjustVolume { kind, delta },
                AudioCommand::ToggleMute(kind) => Task::ToggleMute(kind),
            });
        }
        Action::Spotify(command) => effects.spawn.push(Task::Spotify(command)),
        Action::Media(command) => effects.spawn.push(Task::Media(command)),
        Action::Application(command) => apply_application(state, command, &mut effects),
        Action::Dashboard(command) => match command {
            DashboardCommand::QuickCapture(destination) => {
                effects.spawn.push(Task::QuickCapture(destination));
            }
            DashboardCommand::OpenCiRun => {
                if let Some(url) = state
                    .feeds
                    .ci
                    .peek()
                    .and_then(|snapshot| snapshot.actionable())
                    .map(|run| run.url.clone())
                {
                    effects.spawn.push(Task::OpenUrl(url));
                }
            }
            DashboardCommand::OpenDepartureBoard(index) => {
                if let Some(stop) = state.config.vasttrafik.stops.get(index) {
                    effects.spawn.push(Task::OpenUrl(format!(
                        "https://avgangstavla.vasttrafik.se/?source=streamdeckd&board1Gids={}&board1Modules=departures&board1Modules=trafficSituations",
                        stop.gid
                    )));
                }
            }
            DashboardCommand::OpenActivityMonitor => {
                effects.spawn.push(Task::OpenActivityMonitor);
            }
            DashboardCommand::OpenVpn => effects.spawn.push(Task::OpenVpn),
            DashboardCommand::OpenNetworkSettings => {
                effects.spawn.push(Task::OpenNetworkSettings);
            }
        },
        Action::Wispr(command) => match command {
            WisprCommand::ToggleHandsFree => {
                state.wispr_hands_free = !state.wispr_hands_free;
                effects
                    .spawn
                    .push(Task::SetWisprHandsFree(state.wispr_hands_free));
            }
            WisprCommand::SelectMicrophone(index) => {
                effects.page_changed = state.navigator.go_to(PageId::Home);
                effects.spawn.push(Task::SelectWisprMicrophone(index));
            }
        },
        Action::WalkingPad(command) => match state.walkingpad.prepare(command, now_wall) {
            Ok(request) => effects.walkingpad_request = request,
            Err(message) => {
                state.walkingpad.reject(command, message);
                effects.walkingpad_feedback_changed = true;
            }
        },
        Action::OpenMeeting(index) => effects.spawn.push(Task::OpenMeeting(index)),
        Action::OpenGitHubMetric(kind) => {
            if let Some(snapshot) = state.feeds.github.peek() {
                effects.spawn.push(Task::OpenUrl(snapshot.url(kind)));
            }
        }
        Action::OpenGitHubItem(index) => {
            if let Some(url) = state
                .feeds
                .github
                .peek()
                .and_then(|snapshot| snapshot.item(index))
                .map(|item| item.url.clone())
            {
                effects.spawn.push(Task::OpenUrl(url));
            }
        }
        Action::Refresh(id) => effects.invalidate.push(id),
        Action::RefreshLake => {
            effects.invalidate.push(IntegrationId::LakeCurrent);
            effects.invalidate.push(IntegrationId::LakeHistory);
        }
        Action::WeatherDetail(tile) => {
            // Shows the already-cached reading; MET's Expires header decides when
            // to refetch, so a press never becomes extra load on their API.
            state.weather_detail = Some((tile, now_ms + WEATHER_DETAIL_MS));
            effects.weather_detail_changed = true;
        }
    }
    effects
}

fn apply_application(state: &mut RuntimeState, command: ApplicationCommand, effects: &mut Effects) {
    let Some(application) = state.feeds.application.peek().cloned() else {
        return;
    };

    match command {
        ApplicationCommand::Activate => effects.spawn.push(Task::Application {
            application,
            control: ApplicationControl::Activate,
        }),
        ApplicationCommand::Hide => effects.spawn.push(Task::Application {
            application,
            control: ApplicationControl::Hide,
        }),
        ApplicationCommand::Quit => effects.spawn.push(Task::Application {
            application,
            control: ApplicationControl::Quit,
        }),
        ApplicationCommand::ForceQuit => effects.spawn.push(Task::Application {
            application,
            control: ApplicationControl::ForceQuit,
        }),
        ApplicationCommand::Context(slot) => {
            apply_context_action(state, application.context_action(slot), effects);
        }
        ApplicationCommand::Recent(index) => {
            if let Some(recent) = state.recent_application(index).cloned() {
                effects.spawn.push(Task::Application {
                    application: recent,
                    control: ApplicationControl::Activate,
                });
            }
        }
    }
}

fn apply_context_action(state: &mut RuntimeState, action: ContextAction, effects: &mut Effects) {
    match action {
        ContextAction::None => {}
        ContextAction::SpotifyPrevious => {
            effects.spawn.push(Task::Spotify(SpotifyCommand::Previous));
        }
        ContextAction::SpotifyPlayPause => {
            effects.spawn.push(Task::Spotify(SpotifyCommand::PlayPause));
        }
        ContextAction::SpotifyNext => {
            effects.spawn.push(Task::Spotify(SpotifyCommand::Next));
        }
        ContextAction::SpotifySeek(delta) => {
            effects
                .spawn
                .push(Task::Spotify(SpotifyCommand::Seek(delta)));
        }
        ContextAction::WisprToggle => {
            state.wispr_hands_free = !state.wispr_hands_free;
            effects
                .spawn
                .push(Task::SetWisprHandsFree(state.wispr_hands_free));
        }
        ContextAction::WisprMicrophone(index) => {
            effects.spawn.push(Task::SelectWisprMicrophone(index));
        }
        ContextAction::OpenMeeting(index) => effects.spawn.push(Task::OpenMeeting(index)),
        ContextAction::Navigate(page) => {
            effects.page_changed = state.navigator.go_to(page);
        }
        ContextAction::Custom(action) => {
            if let Some(application) = state.feeds.application.peek().cloned() {
                effects.spawn.push(Task::CustomApplication {
                    application,
                    action,
                });
            }
        }
    }
}

fn apply_pomodoro(
    state: &mut RuntimeState,
    command: PomodoroCommand,
    now_wall: i64,
    effects: &mut Effects,
) {
    let timer = &mut state.persistent.pomodoro;
    effects.pomodoro_changed = true;

    match command {
        PomodoroCommand::Toggle => {
            pomodoro::toggle(timer, now_wall, state.timezone);
            effects.persist = Some(Durability::Critical);
        }
        PomodoroCommand::Skip => {
            pomodoro::skip(timer);
            effects.persist = Some(Durability::Critical);
        }
        PomodoroCommand::Reset => {
            pomodoro::reset_session(timer);
            effects.persist = Some(Durability::Critical);
        }
        PomodoroCommand::Start(phase) => {
            pomodoro::start_phase(timer, phase, now_wall);
            effects.persist = Some(Durability::Critical);
        }
        PomodoroCommand::Adjust {
            duration,
            step_minutes,
        } => {
            pomodoro::adjust_duration(timer, duration, step_minutes, now_wall);
            // A duration tweak is cosmetic unless it moved a running deadline.
            effects.persist = Some(if timer.status == pomodoro::Status::Running {
                Durability::Critical
            } else {
                Durability::Normal
            });
        }
        PomodoroCommand::Acknowledge => {
            pomodoro::acknowledge(timer);
            effects.persist = Some(Durability::Critical);
        }
    }
}

/// Spawns a side-effecting task and reports its outcome back to the coordinator.
pub fn spawn(
    task: Task,
    services: &Services,
    state: &RuntimeState,
    events: mpsc::UnboundedSender<RuntimeEvent>,
) {
    let audio = Arc::clone(&services.audio);
    let application_service = Arc::clone(&services.application);
    let spotify = Arc::clone(&services.spotify);
    let media = Arc::clone(&services.media);
    let wispr = Arc::clone(&services.wispr);
    let meet = Arc::clone(&services.meet);
    let runner = Arc::clone(&services.runner);
    let open = state.config.tools.open.clone();
    let output_targets = state.audio_output.clone();
    let input_targets = state.audio_input.clone();
    let restore_volume = state.persistent.input_volume_before_mute;
    let meetings = state.feeds.meetings.peek().cloned().unwrap_or_default();
    let spotify_volume = state
        .feeds
        .spotify
        .peek()
        .map(|status| status.volume)
        .unwrap_or(50);
    let spotify_playlists = state.config.spotify.playlists.clone();
    let wispr_microphones = state.config.wispr.microphones.clone();
    let quick_capture = state.config.quick_capture.clone();
    let network = state.config.network.clone();
    let timezone = state.timezone;

    tokio::spawn(async move {
        let mut outcome = ActionOutcome::default();

        match task {
            Task::SelectAudio { kind, index } => {
                let targets = match kind {
                    AudioKind::Output => &output_targets,
                    AudioKind::Input => &input_targets,
                };
                match streamdeck_macos::audio::select_target(&*audio, kind, targets, index).await {
                    Ok(device) => tracing::info!(
                        component = "audio",
                        device = %device,
                        "selected audio device"
                    ),
                    Err(error) => outcome.error = Some(error.to_string()),
                }
                outcome.invalidate.push(IntegrationId::AudioStatus);
            }
            Task::AdjustVolume { kind, delta } => {
                match streamdeck_macos::audio::adjust_volume(&*audio, kind, delta).await {
                    Ok(volume) => tracing::info!(
                        component = "audio",
                        kind = ?kind,
                        delta,
                        volume,
                        "adjusted audio volume"
                    ),
                    Err(error) => outcome.error = Some(error.to_string()),
                }
                outcome.invalidate.push(IntegrationId::AudioStatus);
            }
            Task::ToggleMute(kind) => {
                match streamdeck_macos::audio::toggle_mute(&*audio, kind, restore_volume).await {
                    Ok((_, remembered)) => outcome.remembered_input_volume = remembered,
                    Err(error) => outcome.error = Some(error.to_string()),
                }
                outcome.invalidate.push(IntegrationId::AudioStatus);
            }
            Task::Spotify(command) => {
                let result = match command {
                    SpotifyCommand::OpenApp => spotify.open().await,
                    SpotifyCommand::PlayPause => spotify.control(Control::PlayPause).await,
                    SpotifyCommand::Next => spotify.control(Control::Next).await,
                    SpotifyCommand::Previous => spotify.control(Control::Previous).await,
                    SpotifyCommand::Seek(delta) => spotify.control(Control::Seek(delta)).await,
                    SpotifyCommand::PlayPlaylist(index) => match spotify_playlists.get(index) {
                        Some(playlist) => {
                            spotify
                                .control(Control::PlayPlaylist(playlist.uri.clone()))
                                .await
                        }
                        None => {
                            outcome.error = Some(format!(
                                "no Spotify playlist configured at position {index}"
                            ));
                            Ok(())
                        }
                    },
                    SpotifyCommand::Volume(delta) => {
                        let next = streamdeck_core::integrations::spotify::next_volume(
                            spotify_volume,
                            delta,
                        );
                        spotify.control(Control::SetVolume(next)).await
                    }
                };
                if let Err(error) = result {
                    outcome.error = Some(error.to_string());
                }
                outcome.invalidate.push(IntegrationId::Spotify);
            }
            Task::Media(command) => {
                let control = match command {
                    MediaCommand::PlayPause => MediaControl::PlayPause,
                    MediaCommand::Next => MediaControl::Next,
                    MediaCommand::Previous => MediaControl::Previous,
                };
                if let Err(error) = media.control(control).await {
                    outcome.error = Some(error.to_string());
                }
                outcome.invalidate.push(IntegrationId::MediaSession);
            }
            Task::Application {
                application,
                control,
            } => {
                if let Err(error) = application_service.control(&application, control).await {
                    outcome.error = Some(error.to_string());
                }
                outcome.invalidate.push(IntegrationId::FrontmostApplication);
            }
            Task::CustomApplication {
                application,
                action,
            } => {
                if let Err(error) = application_service.custom(&application, action).await {
                    outcome.error = Some(error.to_string());
                }
                outcome.invalidate.push(IntegrationId::FrontmostApplication);
            }
            Task::SetWisprHandsFree(enabled) => {
                if let Err(error) = wispr.set_hands_free(enabled).await {
                    outcome.error = Some(error.to_string());
                    outcome.wispr_hands_free = Some(!enabled);
                }
            }
            Task::SelectWisprMicrophone(index) => match wispr_microphones.get(index) {
                Some(microphone) => {
                    if let Err(error) = wispr.select_microphone(&microphone.name).await {
                        outcome.error = Some(error.to_string());
                    } else {
                        tracing::info!(
                            component = "wispr",
                            microphone = %microphone.label,
                            "selected Wispr Flow microphone"
                        );
                    }
                }
                None => {
                    outcome.error = Some(format!(
                        "no Wispr microphone configured at position {index}"
                    ));
                }
            },
            Task::OpenMeeting(index) => match meetings.get(index) {
                Some(meeting) => {
                    // Only the URL is used; the title never reaches a log here.
                    if let Err(error) = meet.focus_or_open(&meeting.meet_url).await {
                        outcome.error = Some(error.to_string());
                    }
                }
                None => outcome.error = Some(format!("no meeting at position {index}")),
            },
            Task::QuickCapture(destination) => {
                let local_now = Utc::now().with_timezone(&timezone);
                let vault = quick_capture_vault(destination, &quick_capture, &local_now);
                let timestamp = local_now
                    .format("%Y-%m-%d %H-%M-%S Quick Capture")
                    .to_string();
                let file = format!("{}/{}", quick_capture.folder, timestamp);
                let encode = |value: &str| {
                    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC)
                        .to_string()
                };
                let uri = format!(
                    "obsidian://new?vault={}&file={}",
                    encode(vault),
                    encode(&file)
                );
                if let Err(error) = runner
                    .run(&open, &[&uri], streamdeck_macos::timeouts::LOCAL)
                    .await
                {
                    outcome.error = Some(error.to_string());
                }
            }
            Task::OpenActivityMonitor => {
                if let Err(error) = runner
                    .run(
                        &open,
                        &["-a", "Activity Monitor"],
                        streamdeck_macos::timeouts::LOCAL,
                    )
                    .await
                {
                    outcome.error = Some(error.to_string());
                }
            }
            Task::OpenVpn => {
                if let Err(error) = runner
                    .run(
                        &open,
                        &["-a", &network.vpn_application],
                        streamdeck_macos::timeouts::LOCAL,
                    )
                    .await
                {
                    outcome.error = Some(error.to_string());
                }
            }
            Task::OpenNetworkSettings => {
                if let Err(error) = runner
                    .run(
                        &open,
                        &["x-apple.systempreferences:com.apple.Network-Settings.extension"],
                        streamdeck_macos::timeouts::LOCAL,
                    )
                    .await
                {
                    outcome.error = Some(error.to_string());
                }
            }
            Task::OpenUrl(url) => {
                if url.starts_with("https://github.com/")
                    || url.starts_with("https://avgangstavla.vasttrafik.se/")
                {
                    if let Err(error) = runner
                        .run(&open, &[&url], streamdeck_macos::timeouts::LOCAL)
                        .await
                    {
                        outcome.error = Some(error.to_string());
                    }
                } else {
                    outcome.error = Some(format!("refused to open {url}"));
                }
            }
        }

        let _ = events.send(RuntimeEvent::ActionFinished(outcome));
    });
}

/// Spawns one integration refresh.
pub fn spawn_refresh(
    id: IntegrationId,
    services: &Services,
    state: &RuntimeState,
    events: mpsc::UnboundedSender<RuntimeEvent>,
) {
    let http = services.http.clone();
    let runner = Arc::clone(&services.runner);
    let audio = Arc::clone(&services.audio);
    let application = Arc::clone(&services.application);
    let spotify = Arc::clone(&services.spotify);
    let media = Arc::clone(&services.media);
    let config = Arc::clone(&state.config);
    let timezone = state.timezone;
    let last_modified = state.feeds.weather_last_modified.clone();
    let vasttrafik = services.vasttrafik.clone();

    tokio::spawn(async move {
        let now = Utc::now();
        let result = match id {
            IntegrationId::AudioStatus | IntegrationId::AudioInventory => {
                RefreshResult::Audio(audio.snapshot().await.map_err(|error| error.to_string()))
            }
            IntegrationId::Meetings => RefreshResult::Meetings(
                services::meetings::fetch(&runner, &config.tools.gog, &config.meetings, now)
                    .await
                    .map(|result| {
                        for (account, error) in &result.failures {
                            tracing::warn!(
                                component = "meetings",
                                account = %account,
                                error = %error,
                                "one calendar account failed"
                            );
                        }
                        result.meetings
                    })
                    .map_err(|error| error.to_string()),
            ),
            IntegrationId::Weather => RefreshResult::Weather(
                services::weather::fetch(
                    &http,
                    config.location.latitude,
                    config.location.longitude,
                    &config.location.name,
                    timezone,
                    last_modified.as_deref(),
                )
                .await
                .map_err(|error| error.to_string()),
            ),
            IntegrationId::LakeCurrent => RefreshResult::LakeCurrent(
                services::lake::fetch_current(&http, &config.lake.id, now)
                    .await
                    .map_err(|error| error.to_string()),
            ),
            IntegrationId::LakeHistory => RefreshResult::LakeHistory(
                services::lake::fetch_history(&http, &config.lake.id, now)
                    .await
                    .map_err(|error| error.to_string()),
            ),
            IntegrationId::GitHub => RefreshResult::GitHub(
                services::github::fetch(&runner, &config.tools.gh, &config.github)
                    .await
                    .map_err(|error| error.to_string()),
            ),
            IntegrationId::CiRadar => RefreshResult::Ci(
                services::ci::fetch(&runner, &config.tools.gh, &config.ci)
                    .await
                    .map(|result| {
                        for (repository, error) in result.failures {
                            tracing::warn!(
                                component = "ci-radar",
                                repository = %repository,
                                error = %error,
                                "one CI repository failed"
                            );
                        }
                        result.snapshot
                    })
                    .map_err(|error| error.to_string()),
            ),
            IntegrationId::MacHealth => RefreshResult::MacHealth(
                services::system::health(&runner)
                    .await
                    .map_err(|error| error.to_string()),
            ),
            IntegrationId::NetworkStatus => RefreshResult::Network(
                services::system::network(&runner, &config.network.vpn_name)
                    .await
                    .map_err(|error| error.to_string()),
            ),
            IntegrationId::Departures => RefreshResult::Departures(
                vasttrafik
                    .fetch(&config.vasttrafik, now)
                    .await
                    .map_err(|error| error.to_string()),
            ),
            IntegrationId::ClaudeUsage => RefreshResult::Claude(
                services::usage::fetch_claude(&http, now.timestamp_millis())
                    .await
                    .map_err(|error| error.to_string()),
            ),
            IntegrationId::CodexUsage => RefreshResult::Codex(
                services::usage::fetch_codex(&http, config.usage.codex_auth_path.as_deref())
                    .await
                    .map_err(|error| error.to_string()),
            ),
            IntegrationId::Spotify => {
                RefreshResult::Spotify(spotify.status().await.map_err(|error| error.to_string()))
            }
            IntegrationId::MediaSession => {
                RefreshResult::Media(media.status().await.map_err(|error| error.to_string()))
            }
            IntegrationId::FrontmostApplication => RefreshResult::Application(
                application
                    .frontmost()
                    .await
                    .map_err(|error| error.to_string()),
            ),
        };
        let _ = events.send(RuntimeEvent::Refreshed(id, result));
    });
}

/// Stores a refresh result. Returns `true` when the integration now has current
/// data, and records the outcome in the metrics either way.
pub fn store_refresh(
    state: &mut RuntimeState,
    id: IntegrationId,
    result: RefreshResult,
    now_ms: u64,
    metrics: &mut Metrics,
) -> bool {
    macro_rules! store {
        ($cache:expr, $result:expr) => {
            match $result {
                Ok(value) => {
                    $cache.store(value, now_ms);
                    metrics.record_success(id, 0);
                    true
                }
                Err(error) => {
                    $cache.fail(error.clone(), now_ms);
                    let stale = $cache.is_stale();
                    metrics.record_failure(id, error, stale);
                    false
                }
            }
        };
    }

    match result {
        RefreshResult::Audio(result) => store!(state.feeds.audio, result),
        RefreshResult::Meetings(result) => store!(state.feeds.meetings, result),
        RefreshResult::Weather(result) => match result {
            Ok(Refreshed::Updated(fetched)) => {
                state.feeds.weather_last_modified = fetched.last_modified.clone();
                match fetched.expires_at_ms {
                    Some(expires) => state.feeds.weather.store_until(
                        fetched.value,
                        now_ms,
                        // The header is wall clock; convert into monotonic terms.
                        wall_to_monotonic(expires, now_ms),
                    ),
                    None => state.feeds.weather.store(fetched.value, now_ms),
                }
                metrics.record_success(id, 0);
                true
            }
            Ok(Refreshed::Unchanged { expires_at_ms }) => {
                let until = expires_at_ms
                    .map(|expires| wall_to_monotonic(expires, now_ms))
                    .unwrap_or(now_ms + services::millis(services::intervals::WEATHER));
                state.feeds.weather.revalidate(now_ms, until);
                metrics.record_success(id, 0);
                true
            }
            Err(error) => {
                state.feeds.weather.fail(error.clone(), now_ms);
                let stale = state.feeds.weather.is_stale();
                metrics.record_failure(id, error, stale);
                false
            }
        },
        RefreshResult::LakeCurrent(result) => store!(state.feeds.lake_current, result),
        RefreshResult::LakeHistory(result) => store!(state.feeds.lake_history, result),
        RefreshResult::GitHub(result) => store!(state.feeds.github, result),
        RefreshResult::Ci(result) => store!(state.feeds.ci, result),
        RefreshResult::MacHealth(result) => store!(state.feeds.mac_health, result),
        RefreshResult::Network(result) => store!(state.feeds.network, result),
        RefreshResult::Departures(result) => store!(state.feeds.departures, result),
        RefreshResult::Claude(result) => store!(state.feeds.claude, result),
        RefreshResult::Codex(result) => store!(state.feeds.codex, result),
        RefreshResult::Spotify(result) => store!(state.feeds.spotify, result),
        RefreshResult::Media(result) => store!(state.feeds.media, result),
        RefreshResult::Application(result) => match result {
            Ok(application) => {
                state.record_frontmost_application(&application);
                state.feeds.application.store(application, now_ms);
                metrics.record_success(id, 0);
                true
            }
            Err(error) => {
                state.feeds.application.fail(error.clone(), now_ms);
                let stale = state.feeds.application.is_stale();
                metrics.record_failure(id, error, stale);
                false
            }
        },
    }
}

/// Converts a wall-clock expiry into the runtime's monotonic milliseconds.
fn wall_to_monotonic(expires_at_wall_ms: i64, now_ms: u64) -> u64 {
    let remaining = expires_at_wall_ms - Utc::now().timestamp_millis();
    now_ms + remaining.max(0) as u64
}

/// Album art larger than this is not a thumbnail; refuse it.
const MAX_ARTWORK_BYTES: usize = 1024 * 1024;

/// Fetches album artwork under strict host, size, and content-type limits.
pub async fn fetch_artwork(client: &HttpClient, url: &str) -> Result<Vec<u8>, String> {
    if streamdeck_core::integrations::spotify::normalize_artwork_url(url).is_none() {
        return Err(format!("refused artwork host in {url}"));
    }
    let response = client
        .get_bytes(
            url,
            &[("Accept", "image/jpeg,image/png")],
            std::time::Duration::from_secs(5),
            MAX_ARTWORK_BYTES,
        )
        .await
        .map_err(|error| error.to_string())?;

    if let Some(content_type) = &response.content_type {
        if !content_type.starts_with("image/") {
            return Err(format!("refused artwork content type {content_type}"));
        }
    }
    Ok(response.bytes)
}

/// The GitHub metric a tile opens, exposed for the CLI's dry runs.
pub fn metric_url(snapshot: &GitHubSnapshot, kind: MetricKind) -> String {
    snapshot.url(kind)
}

#[cfg(test)]
mod tests {
    use super::*;
    use streamdeck_core::config::Config;
    use streamdeck_core::integrations::walkingpad::{
        WalkingPadCommand, WalkingPadConnection, WalkingPadCounters, WalkingPadMode,
        WalkingPadTelemetry,
    };
    use streamdeck_core::model::PageId;
    use streamdeck_core::pomodoro::{Phase, Status};
    use streamdeck_core::state::{PersistentState, StateStore};

    fn state() -> RuntimeState {
        let config = Arc::new(Config::parse(streamdeck_core::config::TEMPLATE).expect("template"));
        let directory = tempfile::tempdir().expect("temp dir");
        let store = StateStore::new(directory.path().join("state.json"));
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

    fn connected_walkingpad(state: &mut RuntimeState, speed_tenths: u8) {
        state.walkingpad.connection = WalkingPadConnection::Connected;
        state.walkingpad.telemetry = Some(WalkingPadTelemetry {
            counters: WalkingPadCounters::default(),
            speed_tenths,
            target_speed_tenths: speed_tenths,
            belt_state: u8::from(speed_tenths > 0),
            mode: WalkingPadMode::Manual,
        });
        state.walkingpad.last_status_at_ms = Some(now().timestamp_millis());
    }

    #[test]
    fn navigation_reports_a_page_change() {
        let mut state = state();
        let effects = apply(&mut state, Action::Navigate(PageId::Mixer), now(), 1_000);

        assert!(effects.page_changed);
        assert_eq!(state.visible_page(), PageId::Mixer);
        assert!(effects.spawn.is_empty());
    }

    #[test]
    fn opening_and_dismissing_the_panel_both_change_the_page() {
        let mut state = state();
        assert!(
            apply(
                &mut state,
                Action::OpenPanel(PageId::Stensjon),
                now(),
                1_000
            )
            .page_changed
        );
        assert_eq!(state.visible_page(), PageId::Stensjon);

        assert!(apply(&mut state, Action::DismissPanel, now(), 2_000).page_changed);
        assert_eq!(state.visible_page(), PageId::Home);
    }

    #[test]
    fn a_pomodoro_toggle_starts_the_timer_and_asks_for_a_critical_write() {
        let mut state = state();
        let effects = apply(
            &mut state,
            Action::Pomodoro(PomodoroCommand::Toggle),
            now(),
            1_000,
        );

        assert!(effects.pomodoro_changed);
        assert_eq!(effects.persist, Some(Durability::Critical));
        assert_eq!(state.persistent.pomodoro.status, Status::Running);
    }

    #[test]
    fn a_duration_tweak_on_a_stopped_timer_is_only_a_normal_write() {
        let mut state = state();
        let effects = apply(
            &mut state,
            Action::Pomodoro(PomodoroCommand::Adjust {
                duration: Phase::Focus,
                step_minutes: 5,
            }),
            now(),
            1_000,
        );

        assert_eq!(effects.persist, Some(Durability::Normal));
        assert_eq!(state.persistent.pomodoro.focus_minutes, 30);
    }

    #[test]
    fn a_duration_tweak_on_a_running_timer_is_a_critical_write() {
        let mut state = state();
        apply(
            &mut state,
            Action::Pomodoro(PomodoroCommand::Toggle),
            now(),
            1_000,
        );
        let effects = apply(
            &mut state,
            Action::Pomodoro(PomodoroCommand::Adjust {
                duration: Phase::Focus,
                step_minutes: 5,
            }),
            now(),
            2_000,
        );
        assert_eq!(effects.persist, Some(Durability::Critical));
    }

    #[test]
    fn audio_actions_become_tasks_rather_than_blocking_the_loop() {
        let mut state = state();
        let effects = apply(
            &mut state,
            Action::Audio(AudioCommand::Select {
                kind: AudioKind::Output,
                index: 1,
            }),
            now(),
            1_000,
        );

        assert_eq!(
            effects.spawn,
            vec![Task::SelectAudio {
                kind: AudioKind::Output,
                index: 1
            }]
        );
        assert!(effects.persist.is_none());
    }

    #[test]
    fn walkingpad_actions_preserve_exact_tenths_and_only_start_explicitly() {
        let cases = [
            (
                WalkingPadCommand::Increase,
                30,
                WalkingPadRequest::SetSpeed(32),
            ),
            (
                WalkingPadCommand::Decrease,
                30,
                WalkingPadRequest::SetSpeed(28),
            ),
            (
                WalkingPadCommand::SetSpeed(26),
                30,
                WalkingPadRequest::SetSpeed(26),
            ),
            (
                WalkingPadCommand::SetSpeed(30),
                26,
                WalkingPadRequest::SetSpeed(30),
            ),
            (
                WalkingPadCommand::SetSpeed(34),
                30,
                WalkingPadRequest::SetSpeed(34),
            ),
            (
                WalkingPadCommand::SetSpeed(42),
                30,
                WalkingPadRequest::SetSpeed(42),
            ),
            (
                WalkingPadCommand::SetSpeed(45),
                30,
                WalkingPadRequest::SetSpeed(45),
            ),
            (WalkingPadCommand::Start, 0, WalkingPadRequest::Start),
        ];

        for (command, current, expected) in cases {
            let mut state = state();
            connected_walkingpad(&mut state, current);
            let effects = apply(&mut state, Action::WalkingPad(command), now(), 1_000);
            assert_eq!(effects.walkingpad_request, Some(expected), "{command:?}");
        }

        let mut stopped = state();
        connected_walkingpad(&mut stopped, 0);
        let effects = apply(
            &mut stopped,
            Action::WalkingPad(WalkingPadCommand::SetSpeed(34)),
            now(),
            1_000,
        );
        assert_eq!(effects.walkingpad_request, None);
        assert!(effects.walkingpad_feedback_changed);
        assert_eq!(
            stopped.walkingpad.feedback.expect("feedback").message,
            "START FIRST"
        );
    }

    #[test]
    fn walkingpad_stop_remains_available_when_status_is_stale() {
        let mut state = state();
        connected_walkingpad(&mut state, 30);
        state.walkingpad.last_status_at_ms = Some(0);

        let effects = apply(
            &mut state,
            Action::WalkingPad(WalkingPadCommand::Stop),
            now(),
            1_000,
        );

        assert_eq!(effects.walkingpad_request, Some(WalkingPadRequest::Stop));
    }

    #[test]
    fn every_spotify_control_becomes_exactly_one_task() {
        for command in [
            SpotifyCommand::PlayPause,
            SpotifyCommand::Next,
            SpotifyCommand::Previous,
            SpotifyCommand::Seek(-15),
            SpotifyCommand::Volume(5),
            SpotifyCommand::PlayPlaylist(0),
            SpotifyCommand::OpenApp,
        ] {
            let mut state = state();
            let effects = apply(&mut state, Action::Spotify(command), now(), 1_000);
            assert_eq!(effects.spawn, vec![Task::Spotify(command)], "{command:?}");
        }
    }

    #[test]
    fn every_system_media_control_becomes_exactly_one_task() {
        for command in [
            MediaCommand::PlayPause,
            MediaCommand::Next,
            MediaCommand::Previous,
        ] {
            let mut state = state();
            let effects = apply(&mut state, Action::Media(command), now(), 1_000);
            assert_eq!(effects.spawn, vec![Task::Media(command)], "{command:?}");
        }
    }

    #[test]
    fn application_controls_target_the_snapshotted_process_and_use_custom_context() {
        let mut state = state();
        let application = ApplicationInfo {
            name: "Spotify".to_string(),
            bundle_id: Some("com.spotify.client".to_string()),
            pid: 123,
        };
        state.feeds.application.store(application.clone(), 0);

        let hide = apply(
            &mut state,
            Action::Application(ApplicationCommand::Hide),
            now(),
            1_000,
        );
        assert_eq!(
            hide.spawn,
            vec![Task::Application {
                application,
                control: ApplicationControl::Hide,
            }]
        );

        let seek = apply(
            &mut state,
            Action::Application(ApplicationCommand::Context(3)),
            now(),
            1_000,
        );
        assert_eq!(seek.spawn, vec![Task::Spotify(SpotifyCommand::Seek(-15))]);
    }

    #[test]
    fn wispr_tap_toggles_hands_free_and_microphone_selection_returns_home() {
        let mut state = state();

        let start = apply(
            &mut state,
            Action::Wispr(WisprCommand::ToggleHandsFree),
            now(),
            1_000,
        );
        assert!(state.wispr_hands_free);
        assert_eq!(start.spawn, vec![Task::SetWisprHandsFree(true)]);

        let stop = apply(
            &mut state,
            Action::Wispr(WisprCommand::ToggleHandsFree),
            now(),
            2_000,
        );
        assert!(!state.wispr_hands_free);
        assert_eq!(stop.spawn, vec![Task::SetWisprHandsFree(false)]);

        state.navigator.go_to(PageId::Wispr);
        let select = apply(
            &mut state,
            Action::Wispr(WisprCommand::SelectMicrophone(2)),
            now(),
            3_000,
        );
        assert_eq!(state.navigator.visible_page(), PageId::Home);
        assert!(select.page_changed);
        assert_eq!(select.spawn, vec![Task::SelectWisprMicrophone(2)]);
    }

    #[test]
    fn a_github_metric_press_without_data_opens_nothing() {
        let mut state = state();
        let effects = apply(
            &mut state,
            Action::OpenGitHubMetric(MetricKind::Reviews),
            now(),
            1_000,
        );
        assert!(effects.spawn.is_empty());
    }

    #[test]
    fn a_github_metric_press_with_data_opens_its_filter() {
        let mut state = state();
        state.feeds.github.store(
            GitHubSnapshot {
                updated_since: "2026-06-24".to_string(),
                ..Default::default()
            },
            1_000,
        );
        let effects = apply(
            &mut state,
            Action::OpenGitHubMetric(MetricKind::Reviews),
            now(),
            1_000,
        );

        match effects.spawn.first() {
            Some(Task::OpenUrl(url)) => {
                assert!(url.contains("review-requested"), "{url}");
                assert!(url.starts_with("https://github.com/"), "{url}");
            }
            other => panic!("expected a URL task, got {other:?}"),
        }
    }

    #[test]
    fn a_github_item_press_opens_the_item_url() {
        let mut state = state();
        let prs = streamdeck_core::integrations::github::parse_search(
            include_str!("../../../../tests/fixtures/github-search-prs.json"),
            100,
        )
        .expect("parsed");
        let expected = prs[0].url.clone();
        state.feeds.github.store(
            GitHubSnapshot {
                prs,
                ..Default::default()
            },
            1_000,
        );

        let effects = apply(&mut state, Action::OpenGitHubItem(0), now(), 1_000);
        assert_eq!(effects.spawn, vec![Task::OpenUrl(expected)]);

        let empty = apply(&mut state, Action::OpenGitHubItem(9), now(), 1_000);
        assert!(empty.spawn.is_empty());
    }

    #[test]
    fn a_refresh_action_invalidates_only_its_own_integration() {
        let mut state = state();
        let effects = apply(
            &mut state,
            Action::Refresh(IntegrationId::ClaudeUsage),
            now(),
            1_000,
        );
        assert_eq!(effects.invalidate, vec![IntegrationId::ClaudeUsage]);
    }

    #[test]
    fn a_weather_press_opens_the_detail_window_without_refetching() {
        let mut state = state();
        let effects = apply(
            &mut state,
            Action::WeatherDetail(streamdeck_core::model::WeatherTile::Current),
            now(),
            1_000,
        );

        assert!(effects.weather_detail_changed);
        assert!(
            effects.invalidate.is_empty(),
            "showing the cached reading must not become load on MET's API"
        );
        assert_eq!(
            state.weather_detail,
            Some((
                streamdeck_core::model::WeatherTile::Current,
                1_000 + WEATHER_DETAIL_MS
            ))
        );

        // Pressing the other tile replaces the window rather than stacking.
        apply(
            &mut state,
            Action::WeatherDetail(streamdeck_core::model::WeatherTile::Forecast),
            now(),
            2_000,
        );
        assert_eq!(
            state.weather_detail,
            Some((
                streamdeck_core::model::WeatherTile::Forecast,
                2_000 + WEATHER_DETAIL_MS
            ))
        );
    }

    #[test]
    fn the_home_water_tile_refreshes_both_lake_feeds() {
        let mut state = state();
        let effects = apply(&mut state, Action::RefreshLake, now(), 1_000);
        assert_eq!(
            effects.invalidate,
            vec![IntegrationId::LakeCurrent, IntegrationId::LakeHistory]
        );
    }

    #[test]
    fn dashboard_actions_become_scoped_safe_tasks() {
        let mut state = state();

        let automatic = apply(
            &mut state,
            Action::Dashboard(DashboardCommand::QuickCapture(
                CaptureDestination::Automatic,
            )),
            now(),
            1_000,
        );
        assert_eq!(
            automatic.spawn,
            vec![Task::QuickCapture(CaptureDestination::Automatic)]
        );

        let health = apply(
            &mut state,
            Action::Dashboard(DashboardCommand::OpenActivityMonitor),
            now(),
            1_000,
        );
        assert_eq!(health.spawn, vec![Task::OpenActivityMonitor]);

        let vpn = apply(
            &mut state,
            Action::Dashboard(DashboardCommand::OpenVpn),
            now(),
            1_000,
        );
        assert_eq!(vpn.spawn, vec![Task::OpenVpn]);

        let board = apply(
            &mut state,
            Action::Dashboard(DashboardCommand::OpenDepartureBoard(0)),
            now(),
            1_000,
        );
        assert!(matches!(
            board.spawn.as_slice(),
            [Task::OpenUrl(url)] if url == "https://avgangstavla.vasttrafik.se/?source=streamdeckd&board1Gids=9021014002140000&board1Modules=departures&board1Modules=trafficSituations"
        ));
    }

    #[test]
    fn automatic_quick_capture_uses_local_weekday_work_hours() {
        use chrono::TimeZone;

        let config = QuickCaptureConfig::default();
        let stockholm = chrono_tz::Europe::Stockholm;
        let local = |year, month, day, hour, minute| {
            stockholm
                .with_ymd_and_hms(year, month, day, hour, minute, 0)
                .single()
                .expect("unambiguous local time")
        };

        for time in [
            local(2026, 7, 27, 6, 0),
            local(2026, 7, 27, 12, 0),
            local(2026, 7, 27, 17, 59),
        ] {
            assert_eq!(
                quick_capture_vault(CaptureDestination::Automatic, &config, &time),
                config.work_vault
            );
        }
        for time in [
            local(2026, 7, 27, 5, 59),
            local(2026, 7, 27, 18, 0),
            local(2026, 8, 1, 12, 0),
        ] {
            assert_eq!(
                quick_capture_vault(CaptureDestination::Automatic, &config, &time),
                config.personal_vault
            );
        }
    }

    #[test]
    fn a_blank_key_press_does_nothing_at_all() {
        let mut state = state();
        let effects = apply(&mut state, Action::None, now(), 1_000);

        assert!(!effects.page_changed);
        assert!(!effects.pomodoro_changed);
        assert!(effects.persist.is_none());
        assert!(effects.spawn.is_empty());
        assert!(effects.invalidate.is_empty());
    }

    #[test]
    fn storing_a_successful_refresh_makes_the_feed_current() {
        let mut state = state();
        let mut metrics = Metrics::new();
        let stored = store_refresh(
            &mut state,
            IntegrationId::GitHub,
            RefreshResult::GitHub(Ok(GitHubSnapshot::default())),
            1_000,
            &mut metrics,
        );

        assert!(stored);
        assert!(state.feeds.github.peek().is_some());
        assert!(!state.feeds.github.is_stale());
        assert_eq!(metrics.integrations()[&IntegrationId::GitHub].failures, 0);
    }

    #[test]
    fn storing_a_failure_keeps_the_previous_value_and_marks_it_stale() {
        let mut state = state();
        let mut metrics = Metrics::new();
        state.feeds.github.store(GitHubSnapshot::default(), 1_000);

        let stored = store_refresh(
            &mut state,
            IntegrationId::GitHub,
            RefreshResult::GitHub(Err("gh timed out".to_string())),
            400_000,
            &mut metrics,
        );

        assert!(!stored);
        assert!(state.feeds.github.peek().is_some());
        assert!(state.feeds.github.is_stale());
        let entry = &metrics.integrations()[&IntegrationId::GitHub];
        assert_eq!(entry.failures, 1);
        assert!(entry.stale);
    }

    #[test]
    fn a_weather_304_extends_the_lifetime_without_replacing_the_value() {
        let mut state = state();
        let mut metrics = Metrics::new();
        let snapshot = streamdeck_core::integrations::weather::parse_forecast(
            include_str!("../../../../tests/fixtures/met-locationforecast.json"),
            "Stensjön",
            state.timezone,
        )
        .expect("parsed");
        state.feeds.weather.store(snapshot.clone(), 1_000);
        state.feeds.weather.fail("timeout", 2_000);

        let stored = store_refresh(
            &mut state,
            IntegrationId::Weather,
            RefreshResult::Weather(Ok(Refreshed::Unchanged {
                expires_at_ms: None,
            })),
            3_000,
            &mut metrics,
        );

        assert!(stored);
        assert!(!state.feeds.weather.is_stale());
        assert_eq!(state.feeds.weather.peek(), Some(&snapshot));
    }

    #[test]
    fn a_weather_response_remembers_last_modified_for_the_next_request() {
        let mut state = state();
        let mut metrics = Metrics::new();
        let snapshot = streamdeck_core::integrations::weather::parse_forecast(
            include_str!("../../../../tests/fixtures/met-locationforecast.json"),
            "Stensjön",
            state.timezone,
        )
        .expect("parsed");

        store_refresh(
            &mut state,
            IntegrationId::Weather,
            RefreshResult::Weather(Ok(Refreshed::Updated(services::Fetched {
                value: snapshot,
                expires_at_ms: None,
                last_modified: Some("Fri, 24 Jul 2026 05:41:19 GMT".to_string()),
            }))),
            1_000,
            &mut metrics,
        );

        assert_eq!(
            state.feeds.weather_last_modified.as_deref(),
            Some("Fri, 24 Jul 2026 05:41:19 GMT")
        );
    }

    #[test]
    fn a_wall_clock_expiry_in_the_past_does_not_underflow() {
        let now_ms = 5_000;
        let past = Utc::now().timestamp_millis() - 60_000;
        assert_eq!(wall_to_monotonic(past, now_ms), now_ms);
    }

    #[tokio::test]
    async fn artwork_from_a_foreign_host_is_refused_without_a_request() {
        let client = HttpClient::new().expect("client");
        let error = fetch_artwork(&client, "https://evil.example/image.jpg")
            .await
            .expect_err("refused");
        assert!(error.contains("refused artwork host"), "{error}");
    }
}
