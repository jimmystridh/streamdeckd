#![allow(dead_code)]
//! A runtime harness with no hardware, no network, and no clock of its own.
//!
//! Every integration is pre-populated so nothing is due, which keeps the tests
//! deterministic and offline. Individual tests invalidate what they want to
//! exercise.

use std::sync::Arc;

use chrono::{Duration, Utc};
use streamdeck_core::config::Config;
use streamdeck_core::integrations::audio::{AudioInventory, AudioSnapshot, AudioStatus};
use streamdeck_core::integrations::claude::{ClaudeUsage, UsageWindow};
use streamdeck_core::integrations::codex::{CodexUsage, CodexWindow};
use streamdeck_core::integrations::github::GitHubSnapshot;
use streamdeck_core::integrations::lake::{parse_current, parse_history};
use streamdeck_core::integrations::meetings::Meeting;
use streamdeck_core::integrations::spotify::parse_status;
use streamdeck_core::integrations::weather::parse_forecast;
use streamdeck_core::model::PageId;
use streamdeck_core::state::{PersistentState, StateStore};
use streamdeck_macos::audio::CommandAudioAdapter;
use streamdeck_macos::fake::{FakeCommandRunner, Reply};
use streamdeck_macos::meet::SystemMeetLauncher;
use streamdeck_macos::notify::SystemNotifier;
use streamdeck_macos::spotify::AppleScriptSpotifyAdapter;
use streamdeck_macos::CommandRunner;
use streamdeck_render::Renderer;
use streamdeckd::device::recording::RecordingDeckDevice;
use streamdeckd::device::KeyEvent;
use streamdeckd::runtime::state::RuntimeState;
use streamdeckd::runtime::{Runtime, RuntimeEvent, Services};
use streamdeckd::services::http::HttpClient;
use tokio::sync::mpsc;

const MET: &str = include_str!("../../../../tests/fixtures/met-locationforecast.json");
const LAKE_CURRENT: &str = include_str!("../../../../tests/fixtures/lake-current.json");
const LAKE_HISTORY: &str = include_str!("../../../../tests/fixtures/lake-historic.json");
const STENSJON: &str = "A84041BDC1864B41";

pub struct Harness {
    pub runtime: Runtime,
    pub device: Arc<RecordingDeckDevice>,
    pub keys: mpsc::UnboundedSender<KeyEvent>,
    pub events: mpsc::UnboundedSender<RuntimeEvent>,
    pub commands: Arc<FakeCommandRunner>,
    pub store: StateStore,
    /// Kept alive so the state file's directory outlives the harness.
    _directory: tempfile::TempDir,
}

impl Harness {
    pub async fn new(page: PageId) -> Self {
        Self::with_state(page, PersistentState::default()).await
    }

    pub async fn with_state(page: PageId, mut persistent: PersistentState) -> Self {
        let directory = tempfile::tempdir().expect("temp dir");
        let store = StateStore::new(directory.path().join("state.json"));
        persistent.active_page = page;

        let config =
            Arc::new(Config::parse(streamdeck_core::config::TEMPLATE).expect("template parses"));

        let commands = Arc::new(FakeCommandRunner::new());
        script(&commands);
        let runner = Arc::clone(&commands) as Arc<dyn CommandRunner>;

        let services = Services {
            runner: Arc::clone(&runner),
            audio: Arc::new(CommandAudioAdapter::new(
                Arc::clone(&runner),
                config.tools.clone(),
            )),
            spotify: Arc::new(AppleScriptSpotifyAdapter::new(
                Arc::clone(&runner),
                config.tools.clone(),
            )),
            notifier: Arc::new(SystemNotifier::new(
                Arc::clone(&runner),
                config.tools.clone(),
            )),
            meet: Arc::new(SystemMeetLauncher::new(
                Arc::clone(&runner),
                config.tools.clone(),
                "/tmp/Google Meet.app",
            )),
            http: HttpClient::new().expect("client"),
            // No helper binary in tests: the deck's alert state stands alone.
            helper_path: None,
        };

        let mut state = RuntimeState::new(
            Arc::clone(&config),
            directory.path().join("config.toml"),
            StateStore::new(store.path()),
            persistent,
        );
        prefill(&mut state);

        let (sender, receiver) = mpsc::unbounded_channel();
        let (device, keys) = RecordingDeckDevice::new();
        let device = Arc::new(device);
        let mut runtime = Runtime::new(
            state,
            services,
            Renderer::new().expect("renderer"),
            receiver,
            sender.clone(),
        );
        runtime.attach_device(Arc::clone(&device) as Arc<dyn streamdeckd::device::DeckDevice>);
        runtime.start().await.expect("started");

        Self {
            runtime,
            device,
            keys,
            events: sender,
            commands,
            store,
            _directory: directory,
        }
    }

    /// Drives the loop for a default budget.
    pub async fn settle(&mut self) {
        self.settle_for(std::time::Duration::from_millis(250)).await;
    }

    /// Drives the loop for `budget`.
    ///
    /// The loop only returns on shutdown, so it is cancelled by a timeout. The
    /// budget therefore has to exceed whatever the coordinator is doing, or the
    /// cancellation lands in the middle of handling an event — something the real
    /// daemon never does to itself.
    pub async fn settle_for(&mut self, budget: std::time::Duration) {
        let _ = tokio::time::timeout(budget, self.runtime.run()).await;
    }

    /// Sends a key down and up, then settles.
    pub async fn press(&mut self, row: u8, column: u8) {
        let position = streamdeck_core::model::KeyPosition::new(row, column);
        self.events
            .send(RuntimeEvent::Key(KeyEvent::Down(position)))
            .expect("sent");
        self.events
            .send(RuntimeEvent::Key(KeyEvent::Up(position)))
            .expect("sent");
        self.settle().await;
    }

    /// Sends a key down, waits past the long-press threshold, then releases.
    pub async fn hold(&mut self, row: u8, column: u8) {
        let position = streamdeck_core::model::KeyPosition::new(row, column);
        self.events
            .send(RuntimeEvent::Key(KeyEvent::Down(position)))
            .expect("sent");
        self.settle().await;
        tokio::time::sleep(std::time::Duration::from_millis(
            self.runtime.state().config.long_press_ms + 80,
        ))
        .await;
        self.settle().await;
        self.events
            .send(RuntimeEvent::Key(KeyEvent::Up(position)))
            .expect("sent");
        self.settle().await;
    }

    pub fn page(&self) -> PageId {
        self.runtime.state().visible_page()
    }
}

/// Scripts every external tool the harness might reach.
fn script(runner: &FakeCommandRunner) {
    runner
        .on("-c -t output", Reply::ok("Bose NC 700 Headphones\n"))
        .on("-c -t input", Reply::ok("MacBook Pro Microphone\n"))
        .on(
            "-a -t output",
            Reply::ok("MacBook Pro Speakers\nBose NC 700 Headphones\n"),
        )
        .on("-a -t input", Reply::ok("MacBook Pro Microphone\n"))
        .on(
            "get volume settings",
            Reply::ok("output volume:42, input volume:75, output muted:false"),
        )
        .on("set volume", Reply::ok(""))
        .on("-s ", Reply::ok(""))
        .on(
            "player state",
            Reply::ok(
                "playing\tTruth\tKamasi Washington\tThe Epic\t\tspotify:track:1\t72\tfalse\toff\n",
            ),
        )
        .on("playpause", Reply::ok("ok"))
        .on("next track", Reply::ok("ok"))
        .on("previous track", Reply::ok("ok"))
        .on("display notification", Reply::ok(""))
        .on("/usr/bin/afplay", Reply::ok(""))
        .on("/usr/bin/open", Reply::ok(""))
        .on("repeat with w in windows", Reply::ok("not-found\n"))
        .fallback(Reply::ok(""));
}

/// Fills every feed so no refresh is due and no test needs the network.
fn prefill(state: &mut RuntimeState) {
    let now = Utc::now();
    let now_ms = 0;

    state.feeds.audio.store(
        AudioSnapshot {
            status: Some(AudioStatus {
                current_output: "Bose NC 700 Headphones".to_string(),
                current_input: "MacBook Pro Microphone".to_string(),
                output_volume: 42,
                input_volume: 75,
                output_muted: false,
            }),
            inventory: AudioInventory {
                outputs: vec![
                    "MacBook Pro Speakers".to_string(),
                    "Bose NC 700 Headphones".to_string(),
                ],
                inputs: vec!["MacBook Pro Microphone".to_string()],
            },
        },
        now_ms,
    );

    state.feeds.meetings.store(
        vec![Meeting {
            account: "tester@example.com".to_string(),
            title: "Sprint planning".to_string(),
            start: now + Duration::minutes(42),
            end: now + Duration::minutes(102),
            meet_url: "https://meet.google.com/aaa-bbbb-ccc".to_string(),
        }],
        now_ms,
    );

    state.feeds.weather.store(
        parse_forecast(MET, "Stensjön", state.timezone).expect("weather"),
        now_ms,
    );
    state.feeds.lake_current.store(
        parse_current(LAKE_CURRENT, STENSJON, now + Duration::days(365)).expect("lake"),
        now_ms,
    );
    state.feeds.lake_history.store(
        parse_history(LAKE_HISTORY, STENSJON, now + Duration::days(365)).expect("history"),
        now_ms,
    );
    state.feeds.github.store(
        GitHubSnapshot {
            inbox_count: 3,
            updated_since: "2026-06-24".to_string(),
            ..Default::default()
        },
        now_ms,
    );
    state.feeds.claude.store(
        ClaudeUsage {
            five_hour: Some(UsageWindow {
                percent: 10.0,
                resets_at: None,
            }),
            seven_day: Some(UsageWindow {
                percent: 20.0,
                resets_at: None,
            }),
        },
        now_ms,
    );
    state.feeds.codex.store(
        CodexUsage {
            plan: Some("pro".to_string()),
            primary: Some(CodexWindow {
                percent: 30.0,
                window_seconds: 604_800,
                resets_at: None,
            }),
            secondary: None,
            limit_reached: false,
        },
        now_ms,
    );
    state.feeds.spotify.store(
        parse_status("paused\tTruth\tKamasi\tThe Epic\t\tspotify:track:1\t72\tfalse\toff")
            .expect("spotify"),
        now_ms,
    );
}
