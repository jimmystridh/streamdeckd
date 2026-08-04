//! The runtime coordinator.
//!
//! Every external event becomes a typed message on one channel, and this loop is
//! the sole owner of navigation, press, and timer state. Services publish
//! snapshots; they never touch a page. When nothing is due, the loop sleeps.

pub mod actions;
pub mod state;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use streamdeck_core::config::AmbientBrightnessConfig;
use streamdeck_core::control::{PomodoroAction, Request, Response};
use streamdeck_core::deadline::DeadlineId;
use streamdeck_core::integrations::walkingpad::{WalkingPadConnection, WalkingPadUpdate};
use streamdeck_core::model::{IntegrationId, KeyPosition, PageId};
use streamdeck_core::pages::views::{render, RenderContext};
use streamdeck_core::pages::{self, Action, KeyBinding};
use streamdeck_core::pomodoro;
use streamdeck_core::press::PressOutcome;
use streamdeck_core::state::Durability;
use streamdeck_macos::application::ApplicationAdapter;
use streamdeck_macos::audio::AudioAdapter;
use streamdeck_macos::media::MediaAdapter;
use streamdeck_macos::meet::MeetLauncher;
use streamdeck_macos::notify::Notifier;
use streamdeck_macos::spotify::SpotifyAdapter;
use streamdeck_macos::wispr::WisprAdapter;
use streamdeck_macos::CommandRunner;
use streamdeck_render::Renderer;
use tokio::sync::{mpsc, oneshot};

use crate::alert::{self, AlertContext, AlertState, HelperOutcome};
use crate::aurora::{ScreensaverCanvas, ScreensaverScene};
use crate::device::{DeckDevice, DeviceError, FrameCache, KeyEvent};
use crate::logging::LevelControl;
use crate::metrics::Metrics;
use crate::services::http::HttpClient;
use crate::services::walkingpad::WalkingPadCommander;
use actions::{ActionOutcome, Effects};
use state::RuntimeState;

/// A newly opened HID handle passed from the device supervisor to the runtime.
///
/// The wrapper keeps [`RuntimeEvent`] debuggable without requiring the hardware
/// trait object itself to implement `Debug`.
pub struct ReconnectedDevice(pub Arc<dyn DeckDevice>);

impl std::fmt::Debug for ReconnectedDevice {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("ReconnectedDevice")
            .field(&self.0.descriptor())
            .finish()
    }
}

/// Everything that can wake the coordinator.
#[derive(Debug)]
pub enum RuntimeEvent {
    Key(KeyEvent),
    DeviceDisconnected,
    DeviceReconnected(ReconnectedDevice),
    /// A native reading from the Mac's ambient-light sensor, in lux.
    AmbientLight(f64),
    WalkingPad(WalkingPadUpdate),
    /// A spawned refresh finished.
    Refreshed(IntegrationId, actions::RefreshResult),
    /// A spawned side effect finished.
    ActionFinished(ActionOutcome),
    /// A spawned album-artwork download finished.
    ArtworkFetched {
        key: String,
        result: Result<Vec<u8>, String>,
    },
    ConfigChanged,
    SystemWoke,
    ScreenLockChanged(bool),
    HelperOutcome(HelperOutcome),
    Control {
        request: Request,
        reply: oneshot::Sender<Response>,
    },
    Shutdown,
}

/// The macOS adapters and clients the runtime drives.
pub struct Services {
    pub runner: Arc<dyn CommandRunner>,
    pub audio: Arc<dyn AudioAdapter>,
    pub application: Arc<dyn ApplicationAdapter>,
    pub spotify: Arc<dyn SpotifyAdapter>,
    pub media: Arc<dyn MediaAdapter>,
    pub wispr: Arc<dyn WisprAdapter>,
    pub notifier: Arc<dyn Notifier>,
    pub meet: Arc<dyn MeetLauncher>,
    pub http: HttpClient,
    pub vasttrafik: crate::services::vasttrafik::Client,
    pub walkingpad: Arc<dyn WalkingPadCommander>,
    /// Path to the alert helper, when installed.
    pub helper_path: Option<std::path::PathBuf>,
}

pub struct Runtime {
    state: RuntimeState,
    services: Services,
    device: Option<Arc<dyn DeckDevice>>,
    renderer: Renderer,
    frames: FrameCache,
    /// The last semantic view rendered per key. Rasterizing a 144x144 canvas is
    /// three orders of magnitude more expensive than building and comparing the
    /// view, so an unchanged view skips composition entirely — before this cache,
    /// 92% of rendered images were hashed and then discarded as identical.
    last_views: HashMap<KeyPosition, streamdeck_core::view::KeyView>,
    metrics: Metrics,
    alert: Option<AlertState>,
    /// Track identities with an artwork download already running, so a 2-second
    /// Spotify poll cannot start the same download twice.
    artwork_in_flight: std::collections::HashSet<String>,
    ambient_brightness: AmbientBrightnessController,
    level: Option<LevelControl>,
    events: mpsc::UnboundedReceiver<RuntimeEvent>,
    sender: mpsc::UnboundedSender<RuntimeEvent>,
    /// Monotonic origin, so every deadline is in milliseconds since start.
    origin: Instant,
    /// Wall clock and monotonic clock at the last loop pass, for sleep detection.
    last_wall_ms: i64,
    last_mono_ms: u64,
    screen_locked: bool,
    screensaver_started_ms: u64,
    screensaver_next_frame_ms: u64,
    screensaver_scene: ScreensaverScene,
    next_screensaver_scene: ScreensaverScene,
    /// Set when the loop should exit.
    stopping: bool,
}

/// How much wall-clock drift beyond monotonic time counts as a system sleep.
const SLEEP_DETECTION_MS: i64 = 5_000;
/// The longest the loop sleeps while a timer is running, so a wake is noticed.
const MAX_TIMED_SLEEP_MS: u64 = 60_000;
const SCREENSAVER_FRAME_MS: u64 = 50;
const SCREENSAVER_FADE_IN_MS: u64 = 1_200;
const AMBIENT_FILTER_NEW_SAMPLE_WEIGHT: f64 = 0.25;
const AMBIENT_BRIGHTNESS_STEP: f64 = 5.0;
const AMBIENT_BRIGHTNESS_HYSTERESIS: f64 = 4.0;
const WALKINGPAD_PERSIST_DELAY_MS: u64 = 30_000;
const WALKINGPAD_FEEDBACK_MS: u64 = 6_000;

#[derive(Debug, Default)]
struct AmbientBrightnessController {
    last_lux: Option<f64>,
    filtered_lux: Option<f64>,
    applied: Option<u8>,
}

impl AmbientBrightnessController {
    fn observe(&mut self, config: &AmbientBrightnessConfig, lux: f64) -> Option<u8> {
        if !lux.is_finite() || lux < 0.0 {
            return None;
        }

        self.last_lux = Some(lux);
        let filtered = match self.filtered_lux {
            Some(previous) => {
                previous * (1.0 - AMBIENT_FILTER_NEW_SAMPLE_WEIGHT)
                    + lux * AMBIENT_FILTER_NEW_SAMPLE_WEIGHT
            }
            None => lux,
        };
        self.filtered_lux = Some(filtered);

        if !config.enabled {
            return None;
        }

        let continuous = config.brightness_for_lux(filtered);
        let target = quantize_brightness(continuous, config);
        let changed = match self.applied {
            Some(applied) => {
                target != applied
                    && (continuous - f64::from(applied)).abs() >= AMBIENT_BRIGHTNESS_HYSTERESIS
            }
            None => true,
        };
        changed.then(|| {
            self.applied = Some(target);
            target
        })
    }

    fn reconfigure(&mut self, config: &AmbientBrightnessConfig) {
        self.applied = if config.enabled {
            self.filtered_lux
                .map(|lux| quantize_brightness(config.brightness_for_lux(lux), config))
        } else {
            None
        };
    }
}

fn quantize_brightness(value: f64, config: &AmbientBrightnessConfig) -> u8 {
    if value <= f64::from(config.minimum) {
        return config.minimum;
    }
    if value >= f64::from(config.maximum) {
        return config.maximum;
    }
    let stepped = (value / AMBIENT_BRIGHTNESS_STEP).round() * AMBIENT_BRIGHTNESS_STEP;
    (stepped as u8).clamp(config.minimum, config.maximum)
}

impl Runtime {
    pub fn new(
        state: RuntimeState,
        services: Services,
        renderer: Renderer,
        events: mpsc::UnboundedReceiver<RuntimeEvent>,
        sender: mpsc::UnboundedSender<RuntimeEvent>,
    ) -> Self {
        let origin = Instant::now();
        Self {
            state,
            services,
            device: None,
            renderer,
            frames: FrameCache::new(),
            last_views: HashMap::new(),
            metrics: Metrics::new(),
            alert: None,
            artwork_in_flight: std::collections::HashSet::new(),
            ambient_brightness: AmbientBrightnessController::default(),
            level: None,
            events,
            sender,
            origin,
            last_wall_ms: Utc::now().timestamp_millis(),
            last_mono_ms: 0,
            screen_locked: false,
            screensaver_started_ms: 0,
            screensaver_next_frame_ms: 0,
            screensaver_scene: ScreensaverScene::Aurora,
            next_screensaver_scene: ScreensaverScene::Aurora,
            stopping: false,
        }
    }

    pub fn with_level_control(mut self, level: LevelControl) -> Self {
        self.level = Some(level);
        self
    }

    pub fn with_screen_locked(mut self, screen_locked: bool) -> Self {
        self.screen_locked = screen_locked;
        self
    }

    pub fn with_ambient_lux(mut self, lux: Option<f64>) -> Self {
        if let Some(lux) = lux {
            self.ambient_brightness
                .observe(&self.state.config.ambient_brightness, lux);
        }
        self
    }

    pub fn attach_device(&mut self, device: Arc<dyn DeckDevice>) {
        self.device = Some(device);
        self.invalidate_render_caches();
        self.state.presses.clear();
    }

    /// Forgets every rendered frame and view. The two caches must stay in
    /// lockstep: clearing only the frame hashes while views survive would make
    /// the next repaint skip composition and leave the deck stale.
    fn invalidate_render_caches(&mut self) {
        self.frames.invalidate();
        self.last_views.clear();
    }

    fn now_ms(&self) -> u64 {
        self.origin.elapsed().as_millis() as u64
    }

    /// Brings the deck up: brightness, a clean press state, and a full repaint.
    pub async fn start(&mut self) -> anyhow::Result<()> {
        let now_ms = self.now_ms();
        self.set_deck_brightness(self.effective_brightness()).await;

        if self
            .state
            .persistent
            .walkingpad
            .rollover(Utc::now(), self.state.timezone)
        {
            self.persist(Durability::Normal);
        }
        self.state.schedule_walkingpad_midnight(Utc::now(), now_ms);

        // A deadline crossed while the daemon was not running still fires once.
        self.reconcile_pomodoro().await;
        self.state.schedule_refresh_deadlines(now_ms);
        self.state.schedule_pomodoro_deadlines(Utc::now(), now_ms);
        self.spawn_due_refreshes(now_ms);
        if self.screen_locked {
            self.begin_screensaver(now_ms);
            self.render_screensaver().await;
        } else {
            self.render().await;
        }
        Ok(())
    }

    pub async fn run(&mut self) -> anyhow::Result<()> {
        while !self.stopping {
            let now_ms = self.now_ms();
            let sleep_for = self.sleep_duration(now_ms);

            let event = match sleep_for {
                Some(duration) => {
                    tokio::select! {
                        event = self.events.recv() => event,
                        _ = tokio::time::sleep(duration) => None,
                    }
                }
                None => self.events.recv().await,
            };

            self.detect_sleep();

            match event {
                Some(event) => self.handle(event).await,
                // Either a deadline came due or the channel closed.
                None => {
                    if self.events.is_closed() && sleep_for.is_none() {
                        break;
                    }
                    self.handle_deadlines().await;
                }
            }
        }
        Ok(())
    }

    /// How long to sleep before the next deadline. `None` sleeps until an event.
    fn sleep_duration(&self, now_ms: u64) -> Option<Duration> {
        let next = self.state.deadlines.next_deadline_ms();
        let cap = self
            .state
            .shows_countdown()
            .then_some(now_ms + MAX_TIMED_SLEEP_MS);

        let target = match (next, cap) {
            (Some(next), Some(cap)) => Some(next.min(cap)),
            (Some(next), None) => Some(next),
            (None, capped) => capped,
        }?;
        Some(Duration::from_millis(target.saturating_sub(now_ms)))
    }

    /// Notices a system sleep by comparing wall-clock and monotonic progress.
    fn detect_sleep(&mut self) {
        let wall = Utc::now().timestamp_millis();
        let mono = self.now_ms();
        let expected = self.last_wall_ms + (mono.saturating_sub(self.last_mono_ms)) as i64;
        let drift = wall - expected;

        self.last_wall_ms = wall;
        self.last_mono_ms = mono;

        if drift > SLEEP_DETECTION_MS {
            tracing::info!(
                component = "runtime",
                drift_ms = drift,
                "wall clock jumped; reconciling after sleep"
            );
            let _ = self.sender.send(RuntimeEvent::SystemWoke);
        }
    }

    async fn handle(&mut self, event: RuntimeEvent) {
        match event {
            RuntimeEvent::Key(_) if self.screen_locked => {}
            RuntimeEvent::Key(KeyEvent::Down(position)) => self.key_down(position).await,
            RuntimeEvent::Key(KeyEvent::Up(position)) => self.key_up(position).await,
            RuntimeEvent::DeviceDisconnected => {
                tracing::warn!(component = "device", "the deck disconnected");
                self.device = None;
                self.state.presses.clear();
                self.invalidate_render_caches();
            }
            RuntimeEvent::DeviceReconnected(connection) => {
                self.attach_device(connection.0);
                self.metrics.device_reconnects += 1;
                self.set_deck_brightness(self.effective_brightness()).await;
                if self.screen_locked {
                    self.render_screensaver().await;
                } else {
                    self.render().await;
                }
            }
            RuntimeEvent::AmbientLight(lux) => {
                if let Some(brightness) = self
                    .ambient_brightness
                    .observe(&self.state.config.ambient_brightness, lux)
                {
                    self.set_deck_brightness(brightness).await;
                }
            }
            RuntimeEvent::WalkingPad(update) => self.walkingpad_update(update).await,
            RuntimeEvent::Refreshed(id, result) => self.apply_refresh(id, result).await,
            RuntimeEvent::ActionFinished(outcome) => self.apply_action_outcome(outcome).await,
            RuntimeEvent::ArtworkFetched { key, result } => {
                self.artwork_in_flight.remove(&key);
                match result {
                    Ok(bytes) => match self.renderer.cache_artwork(&key, &bytes) {
                        Ok(()) => self.render().await,
                        Err(error) => tracing::debug!(
                            component = "spotify",
                            error = %error,
                            "artwork rejected"
                        ),
                    },
                    Err(error) => tracing::debug!(
                        component = "spotify",
                        error = %error,
                        "artwork fetch failed"
                    ),
                }
            }
            RuntimeEvent::ConfigChanged => {
                let _ = self.reload_config().await;
            }
            RuntimeEvent::SystemWoke => self.wake().await,
            RuntimeEvent::ScreenLockChanged(screen_locked) => {
                self.set_screen_locked(screen_locked).await
            }
            RuntimeEvent::HelperOutcome(outcome) => self.helper_outcome(outcome).await,
            RuntimeEvent::Control { request, reply } => self.control(request, reply).await,
            RuntimeEvent::Shutdown => self.stopping = true,
        }
    }

    async fn handle_deadlines(&mut self) {
        let now_ms = self.now_ms();
        let due = self.state.deadlines.take_due(now_ms);
        let mut repaint = false;

        for id in due {
            match id {
                DeadlineId::PomodoroCompletion => {
                    self.reconcile_pomodoro().await;
                    repaint = true;
                }
                DeadlineId::CountdownTick => {
                    // Re-arm from the state, which also picks up a phase change.
                    self.state.schedule_pomodoro_deadlines(Utc::now(), now_ms);
                    if self.alert.is_some() {
                        if let Some(alert) = self.alert.as_mut() {
                            alert.flashing = !alert.flashing;
                            self.state.alert_flashing = alert.flashing;
                        }
                    }
                    repaint = true;
                }
                DeadlineId::ScreensaverFrame => {
                    if self.screen_locked {
                        self.render_screensaver().await;
                    }
                }
                DeadlineId::LongPressArm => {
                    for outcome in self.state.presses.poll_arm(now_ms) {
                        if let PressOutcome::Armed(position) = outcome {
                            self.armed(position).await;
                        }
                    }
                    self.rearm_long_press();
                    repaint = true;
                }
                DeadlineId::PanelDismiss => {
                    if self.state.navigator.poll_panel(now_ms) {
                        self.after_page_change(now_ms);
                        repaint = true;
                    }
                }
                DeadlineId::MeetingLabels => {
                    self.state.deadlines.set(
                        DeadlineId::MeetingLabels,
                        now_ms
                            + crate::services::millis(crate::services::intervals::MEETING_LABELS),
                    );
                    repaint = true;
                }
                DeadlineId::AlertSound => self.repeat_alert_sound(now_ms).await,
                DeadlineId::WeatherDetail => {
                    self.state.weather_detail = None;
                    repaint = true;
                }
                DeadlineId::HomeWeatherBoundary => {
                    self.state
                        .schedule_home_weather_boundary(Utc::now(), now_ms);
                    repaint = true;
                }
                DeadlineId::WalkingPadPersist => {
                    if self.state.walkingpad_persist_dirty {
                        self.persist(Durability::Normal);
                        self.state.walkingpad_persist_dirty = false;
                    }
                }
                DeadlineId::WalkingPadFeedback => {
                    self.state.walkingpad.clear_feedback();
                    repaint = true;
                }
                DeadlineId::WalkingPadMidnight => {
                    if self
                        .state
                        .persistent
                        .walkingpad
                        .rollover(Utc::now(), self.state.timezone)
                    {
                        self.persist(Durability::Critical);
                    }
                    self.state.schedule_walkingpad_midnight(Utc::now(), now_ms);
                    repaint = true;
                }
                DeadlineId::WalkingPadTick => {
                    self.state.schedule_walkingpad_tick(now_ms);
                    repaint = true;
                }
                DeadlineId::Refresh(integration) => {
                    self.spawn_refresh(integration, now_ms);
                }
            }
        }

        // A deadline may also simply have been the timed sleep cap.
        if repaint || self.state.shows_countdown() {
            self.render().await;
        }
    }

    async fn wake(&mut self) {
        self.metrics.wakes += 1;
        let now_ms = self.now_ms();

        if self.state.walkingpad_persist_dirty {
            self.persist(Durability::Normal);
            self.state.walkingpad_persist_dirty = false;
        }
        self.reconcile_pomodoro().await;
        self.state.deadlines.drain_all();
        self.state.presses.clear();
        // A detail window from before the sleep has long since expired.
        self.state.weather_detail = None;
        self.state.schedule_refresh_deadlines(now_ms);
        self.state.schedule_pomodoro_deadlines(Utc::now(), now_ms);
        self.state.schedule_walkingpad_midnight(Utc::now(), now_ms);
        self.state.walkingpad_continuous = false;
        if self.state.navigator.panel_is_open() {
            // A panel that should have closed during sleep closes now.
            if self.state.navigator.poll_panel(now_ms) {
                self.after_page_change(now_ms);
            } else if let Some(at) = self.state.navigator.panel_deadline_ms() {
                self.state.deadlines.set(DeadlineId::PanelDismiss, at);
            }
        }
        if self.alert.is_some() {
            self.schedule_alert_sound(now_ms);
        }

        // Everything on screen may be stale after a long sleep.
        for id in pages::required_integrations(self.state.visible_page()) {
            self.state.feeds.invalidate(id);
        }
        self.spawn_due_refreshes(now_ms);
        if self.screen_locked {
            self.screensaver_next_frame_ms = now_ms;
            self.render_screensaver().await;
        } else {
            self.render().await;
        }
    }

    async fn walkingpad_update(&mut self, update: WalkingPadUpdate) {
        let now_ms = self.now_ms();
        match update {
            WalkingPadUpdate::Connection { state, error } => {
                if state != WalkingPadConnection::Connected {
                    self.state.walkingpad_continuous = false;
                }
                self.state.walkingpad.connection_changed(state, error);
            }
            WalkingPadUpdate::Status {
                telemetry,
                received_at_ms,
            } => {
                let observed_at = chrono::DateTime::<Utc>::from_timestamp_millis(received_at_ms)
                    .unwrap_or_else(Utc::now);
                self.state.persistent.walkingpad.observe(
                    telemetry.counters,
                    observed_at,
                    self.state.timezone,
                    self.state.walkingpad_continuous,
                );
                self.state.walkingpad_continuous = true;
                self.state
                    .walkingpad
                    .apply_status(telemetry, received_at_ms);
                self.state.walkingpad_persist_dirty = true;
                self.state.deadlines.set_if_sooner(
                    DeadlineId::WalkingPadPersist,
                    now_ms + WALKINGPAD_PERSIST_DELAY_MS,
                );
            }
            WalkingPadUpdate::CommandSucceeded(request) => {
                self.state.walkingpad.command_succeeded(request);
            }
            WalkingPadUpdate::CommandFailed { request, error } => {
                self.state.walkingpad.command_failed(request, error);
            }
        }

        if self.state.walkingpad.feedback.is_some() {
            self.state.deadlines.set(
                DeadlineId::WalkingPadFeedback,
                now_ms + WALKINGPAD_FEEDBACK_MS,
            );
        }
        self.state.schedule_walkingpad_tick(now_ms);
        if !self.screen_locked
            && matches!(
                self.state.visible_page(),
                PageId::Dashboard | PageId::WalkingPad | PageId::WalkingPadStats
            )
        {
            self.render().await;
        }
    }

    fn begin_screensaver(&mut self, now_ms: u64) {
        self.screen_locked = true;
        self.screensaver_started_ms = now_ms;
        self.screensaver_next_frame_ms = now_ms;
        self.screensaver_scene = self.next_screensaver_scene;
        self.next_screensaver_scene = self.next_screensaver_scene.next();
        self.state.presses.clear();
        self.invalidate_render_caches();
        self.state
            .deadlines
            .set(DeadlineId::ScreensaverFrame, now_ms);
        tracing::info!(
            component = "screensaver",
            fps = 20,
            scene = self.screensaver_scene.name(),
            "screen locked"
        );
    }

    async fn set_screen_locked(&mut self, screen_locked: bool) {
        if screen_locked == self.screen_locked {
            return;
        }

        if screen_locked {
            self.begin_screensaver(self.now_ms());
            self.render_screensaver().await;
        } else {
            self.screen_locked = false;
            self.state.deadlines.clear(DeadlineId::ScreensaverFrame);
            self.invalidate_render_caches();
            tracing::info!(component = "screensaver", "screen unlocked");
            self.render().await;
        }
    }

    async fn render_screensaver(&mut self) {
        if !self.screen_locked {
            return;
        }

        let elapsed_ms = self.now_ms().saturating_sub(self.screensaver_started_ms);
        let fade = (elapsed_ms as f32 / SCREENSAVER_FADE_IN_MS as f32).clamp(0.0, 1.0);
        let intensity = fade * fade * (3.0 - 2.0 * fade);
        let canvas = ScreensaverCanvas::render(
            self.screensaver_scene,
            elapsed_ms as f32 / 1_000.0,
            intensity,
        );
        self.send(canvas.rendered_keys()).await;

        let now_ms = self.now_ms();
        let candidate = self
            .screensaver_next_frame_ms
            .saturating_add(SCREENSAVER_FRAME_MS);
        let next = if candidate > now_ms {
            candidate
        } else {
            let missed = (now_ms - candidate) / SCREENSAVER_FRAME_MS + 1;
            candidate.saturating_add(missed.saturating_mul(SCREENSAVER_FRAME_MS))
        };
        self.screensaver_next_frame_ms = next;
        self.state.deadlines.set(DeadlineId::ScreensaverFrame, next);
    }

    // --- press handling -----------------------------------------------------

    fn binding(&self, position: KeyPosition) -> Option<KeyBinding> {
        pages::page(self.state.visible_page())
            .binding(position)
            .copied()
    }

    async fn key_down(&mut self, position: KeyPosition) {
        let now_ms = self.now_ms();
        let has_long = self
            .binding(position)
            .is_some_and(|binding| binding.has_long_action());

        self.state.presses.key_down(position, now_ms, has_long);
        self.rearm_long_press();
        // Pressed feedback must reach the deck immediately.
        self.render_key(position).await;
    }

    fn rearm_long_press(&mut self) {
        match self.state.presses.next_arm_deadline_ms() {
            Some(at) => self.state.deadlines.set(DeadlineId::LongPressArm, at),
            None => self.state.deadlines.clear(DeadlineId::LongPressArm),
        }
    }

    async fn armed(&mut self, position: KeyPosition) {
        self.metrics.long_presses += 1;
        // The affordance must be visible before the action changes the page.
        self.render_key(position).await;
        if let Some(action) = self.binding(position).and_then(|binding| binding.long) {
            self.execute(action).await;
        }
    }

    async fn key_up(&mut self, position: KeyPosition) {
        let now_ms = self.now_ms();
        let outcome = self.state.presses.key_up(position, now_ms);
        self.rearm_long_press();

        match outcome {
            PressOutcome::ShortPress(position) => {
                self.metrics.key_presses += 1;
                let action = self
                    .binding(position)
                    .map(|binding| binding.short)
                    .unwrap_or(Action::None);
                self.execute(action).await;
            }
            PressOutcome::LongPressReleased(_) => self.render().await,
            _ => self.render().await,
        }
    }

    /// Runs an action. Pure state changes apply inline; anything that talks to the
    /// system is spawned so input handling never blocks on it.
    async fn execute(&mut self, action: Action) {
        let now_ms = self.now_ms();
        let now = Utc::now();

        // Any interaction acknowledges a pending completion and restarts a panel.
        if self.state.navigator.panel_is_open() {
            self.state.navigator.touch_panel(now_ms);
            if let Some(at) = self.state.navigator.panel_deadline_ms() {
                self.state.deadlines.set(DeadlineId::PanelDismiss, at);
            }
        }
        if !matches!(action, Action::None) {
            self.acknowledge_completion().await;
        }

        let effects = actions::apply(&mut self.state, action, now, now_ms);
        self.after_effects(effects, now, now_ms).await;
    }

    async fn after_effects(&mut self, effects: Effects, now: chrono::DateTime<Utc>, now_ms: u64) {
        if effects.page_changed {
            self.metrics.page_switches += 1;
            self.after_page_change(now_ms);
        }
        if let Some(durability) = effects.persist {
            self.persist(durability);
        }
        if effects.pomodoro_changed {
            self.state.schedule_pomodoro_deadlines(now, now_ms);
        }
        if effects.weather_detail_changed {
            if let Some((_, until)) = self.state.weather_detail {
                self.state.deadlines.set(DeadlineId::WeatherDetail, until);
            }
        }
        for id in &effects.invalidate {
            self.state.feeds.invalidate(*id);
        }
        for task in effects.spawn {
            actions::spawn(task, &self.services, &self.state, self.sender.clone());
        }
        if let Some(request) = effects.walkingpad_request {
            if let Err(error) = self.services.walkingpad.send(request) {
                self.state.walkingpad.command_failed(request, error);
                self.state.deadlines.set(
                    DeadlineId::WalkingPadFeedback,
                    now_ms + WALKINGPAD_FEEDBACK_MS,
                );
            }
        }
        if effects.walkingpad_feedback_changed {
            self.state.deadlines.set(
                DeadlineId::WalkingPadFeedback,
                now_ms + WALKINGPAD_FEEDBACK_MS,
            );
        }
        self.spawn_due_refreshes(now_ms);
        self.render().await;
    }

    fn after_page_change(&mut self, now_ms: u64) {
        self.state.presses.clear();
        self.state.persistent.active_page = self.state.navigator.base_page();
        self.persist(Durability::Normal);
        self.state.schedule_refresh_deadlines(now_ms);
        self.state.schedule_pomodoro_deadlines(Utc::now(), now_ms);
        match self.state.navigator.panel_deadline_ms() {
            Some(at) => self.state.deadlines.set(DeadlineId::PanelDismiss, at),
            None => self.state.deadlines.clear(DeadlineId::PanelDismiss),
        }
        self.spawn_due_refreshes(now_ms);
    }

    // --- pomodoro -----------------------------------------------------------

    async fn reconcile_pomodoro(&mut self) {
        let now = Utc::now();
        let completed = pomodoro::reconcile(
            &mut self.state.persistent.pomodoro,
            now.timestamp_millis(),
            self.state.timezone,
        );
        let Some(phase) = completed else { return };

        // Persist the completion before anything else can fail.
        self.persist(Durability::Critical);
        tracing::info!(
            component = "pomodoro",
            phase = phase.slug(),
            "phase completed"
        );

        let next_phase = self.state.persistent.pomodoro.phase;
        let next_minutes = self.state.persistent.pomodoro.phase_minutes(next_phase);
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let context = AlertContext {
            notifier: Arc::clone(&self.services.notifier),
            config: self.state.config.pomodoro.clone(),
            helper_path: self.services.helper_path.clone(),
        };

        if let Some(mut previous) = self.alert.take() {
            previous.close_helper().await;
        }
        self.alert = Some(alert::begin(&context, phase, next_phase, next_minutes, sender).await);
        self.state.alert_flashing = true;

        let events = self.sender.clone();
        tokio::spawn(async move {
            while let Some(outcome) = receiver.recv().await {
                let _ = events.send(RuntimeEvent::HelperOutcome(outcome));
            }
        });

        let now_ms = self.now_ms();
        self.schedule_alert_sound(now_ms);
        self.state.schedule_pomodoro_deadlines(now, now_ms);
    }

    fn schedule_alert_sound(&mut self, now_ms: u64) {
        match alert::next_sound_deadline_ms(&self.state.config.pomodoro, now_ms) {
            Some(at) => self.state.deadlines.set(DeadlineId::AlertSound, at),
            None => self.state.deadlines.clear(DeadlineId::AlertSound),
        }
    }

    async fn repeat_alert_sound(&mut self, now_ms: u64) {
        if self.alert.is_none() {
            return;
        }
        let notifier = Arc::clone(&self.services.notifier);
        let sound = self.state.config.pomodoro.sound.clone();
        tokio::spawn(async move {
            if let Err(error) = notifier.play_sound(&sound).await {
                tracing::warn!(component = "alert", error = %error, "repeat sound failed");
            }
        });
        self.schedule_alert_sound(now_ms);
    }

    /// Clears a pending completion from whichever surface asked.
    async fn acknowledge_completion(&mut self) {
        let acknowledged = pomodoro::acknowledge(&mut self.state.persistent.pomodoro);
        if let Some(mut alert) = self.alert.take() {
            alert.close_helper().await;
        }
        self.state.alert_flashing = false;
        self.state.deadlines.clear(DeadlineId::AlertSound);

        if acknowledged {
            self.persist(Durability::Critical);
            tracing::info!(component = "pomodoro", "completion acknowledged");
        }
    }

    async fn helper_outcome(&mut self, outcome: HelperOutcome) {
        let next_phase = self.state.persistent.pomodoro.phase;
        self.acknowledge_completion().await;

        if outcome == HelperOutcome::StartNext {
            let now = Utc::now();
            pomodoro::start_phase(
                &mut self.state.persistent.pomodoro,
                next_phase,
                now.timestamp_millis(),
            );
            self.persist(Durability::Critical);
            let now_ms = self.now_ms();
            self.state.schedule_pomodoro_deadlines(now, now_ms);
        }
        self.render().await;
    }

    fn persist(&mut self, durability: Durability) {
        if let Err(error) = self.state.store.save(&self.state.persistent, durability) {
            tracing::error!(component = "state", error = %error, "could not persist state");
        }
    }

    // --- refreshes ----------------------------------------------------------

    fn spawn_due_refreshes(&mut self, now_ms: u64) {
        for id in self.state.due_integrations(now_ms) {
            self.spawn_refresh(id, now_ms);
        }
    }

    fn spawn_refresh(&mut self, id: IntegrationId, now_ms: u64) {
        // Single-flight: never start a second request for the same integration.
        if !self.state.in_flight.insert(id) {
            return;
        }
        if !pages::required_integrations(self.state.visible_page()).contains(&id) {
            // The page changed between scheduling and firing; nothing to do.
            self.state.in_flight.remove(&id);
            return;
        }
        let _ = now_ms;
        actions::spawn_refresh(id, &self.services, &self.state, self.sender.clone());
    }

    async fn apply_refresh(&mut self, id: IntegrationId, result: actions::RefreshResult) {
        self.state.in_flight.remove(&id);
        let now_ms = self.now_ms();
        let succeeded =
            actions::store_refresh(&mut self.state, id, result, now_ms, &mut self.metrics);

        if succeeded {
            self.state.backoff_for(id).reset();
        } else {
            let delay = self.state.backoff_for(id).fail();
            self.state
                .deadlines
                .set(DeadlineId::Refresh(id), now_ms + delay);
        }
        if succeeded {
            if let Some(due) = self.state.feeds.next_due_ms(id) {
                self.state.deadlines.set(DeadlineId::Refresh(id), due);
            }
        }

        // Album artwork is fetched off the coordinator's path and cached by track.
        if id == IntegrationId::Spotify {
            self.maybe_fetch_artwork();
        }
        self.render().await;
    }

    /// Starts an artwork download for the current track when one is needed.
    ///
    /// The fetch runs in its own task and reports back as an event: a slow CDN
    /// must never delay a key press. The previous version awaited the download
    /// inline, which parked the whole coordinator for up to five seconds.
    fn maybe_fetch_artwork(&mut self) {
        let Some(status) = self.state.feeds.spotify.peek() else {
            return;
        };
        let (Some(track_id), Some(url)) = (status.track_id.clone(), status.artwork_url.clone())
        else {
            return;
        };
        if self.renderer.has_artwork(&track_id) || !self.artwork_in_flight.insert(track_id.clone())
        {
            return;
        }

        let http = self.services.http.clone();
        let events = self.sender.clone();
        tokio::spawn(async move {
            let result = actions::fetch_artwork(&http, &url).await;
            let _ = events.send(RuntimeEvent::ArtworkFetched {
                key: track_id,
                result,
            });
        });
    }

    /// Whether artwork for `key` is in the renderer's cache. Test access.
    pub fn artwork_cached(&self, key: &str) -> bool {
        self.renderer.has_artwork(key)
    }

    async fn apply_action_outcome(&mut self, outcome: ActionOutcome) {
        if let Some(error) = &outcome.error {
            tracing::warn!(component = "action", error = %error, "action failed");
        }
        if let Some(volume) = outcome.remembered_input_volume {
            self.state.persistent.input_volume_before_mute = volume;
            self.persist(Durability::Normal);
        }
        if let Some(enabled) = outcome.wispr_hands_free {
            self.state.wispr_hands_free = enabled;
        }
        for id in outcome.invalidate {
            self.state.feeds.invalidate(id);
        }
        let now_ms = self.now_ms();
        self.spawn_due_refreshes(now_ms);
        self.render().await;
    }

    // --- rendering ----------------------------------------------------------

    async fn render(&mut self) {
        if self.screen_locked {
            return;
        }

        let now_ms = self.now_ms();
        let world = self.state.world(Utc::now(), now_ms);
        let context = RenderContext::new(&world)
            .with_audio(&self.state.audio_output, &self.state.audio_input)
            .with_spotify_playlists(&self.state.config.spotify.playlists)
            .with_wispr_microphones(&self.state.config.wispr.microphones);
        let page = self.state.visible_page();

        let mut payloads = HashMap::new();
        for binding in pages::full_page(page, streamdeck_core::model::Grid::MK2) {
            let mut view = render(binding.tile, &context);
            view.pressed = self.state.presses.is_held(binding.position);
            view.armed = self.state.presses.is_armed(binding.position);
            if self.last_views.get(&binding.position) == Some(&view) {
                self.metrics.renders_skipped += 1;
                continue;
            }
            match self.renderer.render(&view) {
                Ok(key) => {
                    self.metrics.renders += 1;
                    payloads.insert(binding.position, key);
                    self.last_views.insert(binding.position, view);
                }
                Err(error) => {
                    tracing::error!(component = "render", error = %error, "could not render a key")
                }
            }
        }
        self.send(payloads).await;
    }

    /// Renders a single key, for immediate press feedback.
    async fn render_key(&mut self, position: KeyPosition) {
        if self.screen_locked {
            return;
        }

        let now_ms = self.now_ms();
        let world = self.state.world(Utc::now(), now_ms);
        let context = RenderContext::new(&world)
            .with_audio(&self.state.audio_output, &self.state.audio_input)
            .with_spotify_playlists(&self.state.config.spotify.playlists)
            .with_wispr_microphones(&self.state.config.wispr.microphones);
        let Some(binding) = self.binding(position).or_else(|| {
            pages::full_page(self.state.visible_page(), streamdeck_core::model::Grid::MK2)
                .into_iter()
                .find(|binding| binding.position == position)
        }) else {
            return;
        };

        let mut view = render(binding.tile, &context);
        view.pressed = self.state.presses.is_held(position);
        view.armed = self.state.presses.is_armed(position);
        if self.last_views.get(&position) == Some(&view) {
            self.metrics.renders_skipped += 1;
            return;
        }
        match self.renderer.render(&view) {
            Ok(key) => {
                self.metrics.renders += 1;
                self.last_views.insert(position, view);
                let mut payloads = HashMap::new();
                payloads.insert(position, key);
                self.send(payloads).await;
            }
            Err(error) => {
                tracing::error!(component = "render", error = %error, "could not render a key")
            }
        }
    }

    async fn send(&mut self, payloads: HashMap<KeyPosition, streamdeck_render::RenderedKey>) {
        let Some(device) = self.device.clone() else {
            return;
        };
        let mut positions: Vec<_> = payloads.keys().copied().collect();
        positions.sort();

        let mut wrote = false;
        for position in positions {
            let key = &payloads[&position];
            if !self.frames.should_send(position, key) {
                self.frames.record_skipped();
                self.metrics.frames_skipped += 1;
                continue;
            }
            match device.set_key(position, key).await {
                Ok(bytes) => {
                    wrote = true;
                    self.frames.record_sent(position, key, bytes);
                    self.metrics.frames_sent += 1;
                    self.metrics.bytes_sent += bytes as u64;
                }
                Err(DeviceError::Disconnected) => {
                    let _ = self.sender.send(RuntimeEvent::DeviceDisconnected);
                    return;
                }
                Err(error) => {
                    tracing::warn!(component = "device", error = %error, "could not send a key");
                    // The failed keys were recorded in the view cache but never
                    // reached the deck; drop both caches so the next repaint
                    // retries them instead of skipping past the stale glass.
                    self.invalidate_render_caches();
                    return;
                }
            }
        }

        // The device layer buffers image writes; nothing reaches the glass until
        // this. Skipped when every frame was unchanged, so an idle repaint still
        // costs zero USB traffic.
        if wrote {
            match device.flush().await {
                Ok(()) => {}
                Err(DeviceError::Disconnected) => {
                    let _ = self.sender.send(RuntimeEvent::DeviceDisconnected);
                }
                Err(error) => {
                    tracing::warn!(component = "device", error = %error, "could not flush");
                    // The batch was recorded as sent but never hit the glass;
                    // drop the caches so the next repaint transmits it again.
                    self.invalidate_render_caches();
                }
            }
        }
    }

    // --- control ------------------------------------------------------------

    /// Handles one control request.
    ///
    /// Most commands answer inline. The two that can take seconds — enumerating
    /// HID devices and running the health checks — are handed to a task along with
    /// the reply channel, so a slow diagnostic never freezes the deck.
    async fn control(&mut self, request: Request, reply: oneshot::Sender<Response>) {
        match request {
            Request::Devices => {
                tokio::spawn(async move {
                    let discovered =
                        tokio::task::spawn_blocking(crate::device::hid::discover).await;
                    let response = match discovered {
                        Ok(Ok(devices)) => Response::data(
                            format!("{} device(s)", devices.len()),
                            serde_json::json!(devices
                                .into_iter()
                                .map(|device| serde_json::json!({
                                    "serial": device.serial,
                                    "kind": device.kind,
                                    "rows": device.rows,
                                    "columns": device.columns,
                                    "available": device.available,
                                }))
                                .collect::<Vec<_>>()),
                        ),
                        Ok(Err(error)) => Response::error(error.to_string()),
                        Err(error) => Response::error(error.to_string()),
                    };
                    let _ = reply.send(response);
                });
            }
            Request::Doctor => {
                let inputs = crate::doctor::Inputs::collect(
                    &self.state,
                    &self.services,
                    self.device.as_ref(),
                );
                tokio::spawn(async move {
                    let _ = reply.send(Response::data("ok", crate::doctor::run(inputs).await));
                });
            }
            other => {
                let response = self.control_inline(other).await;
                let _ = reply.send(response);
            }
        }
    }

    /// Commands that are cheap enough to answer from the coordinator.
    async fn control_inline(&mut self, request: Request) -> Response {
        match request {
            Request::Status => Response::data("ok", self.status_json()),
            // Handled by `control`, which owns the reply channel for these.
            Request::Devices | Request::Doctor => {
                Response::error("this command is handled asynchronously")
            }
            Request::Page { page } => {
                self.execute(Action::Navigate(page)).await;
                Response::ok(format!("switched to {page}"))
            }
            Request::Press { position } => {
                if self.binding(position).is_none() {
                    return Response::error(format!("{position} is blank on this page"));
                }
                self.key_down(position).await;
                self.key_up(position).await;
                Response::ok(format!("pressed {position}"))
            }
            Request::Hold {
                position,
                milliseconds,
            } => {
                if self.binding(position).is_none() {
                    return Response::error(format!("{position} is blank on this page"));
                }
                self.key_down(position).await;
                tokio::time::sleep(Duration::from_millis(milliseconds)).await;
                let now_ms = self.now_ms();
                for outcome in self.state.presses.poll_arm(now_ms) {
                    if let PressOutcome::Armed(position) = outcome {
                        self.armed(position).await;
                    }
                }
                self.key_up(position).await;
                Response::ok(format!("held {position} for {milliseconds}ms"))
            }
            Request::Pomodoro { action } => self.pomodoro_command(action).await,
            Request::Refresh { integration } => {
                self.state.feeds.invalidate(integration);
                let now_ms = self.now_ms();
                self.spawn_refresh(integration, now_ms);
                Response::ok(format!("refreshing {integration}"))
            }
            Request::Reload => match self.reload_config().await {
                Ok(()) => Response::ok("configuration reloaded"),
                Err(error) => Response::error(error),
            },
            Request::RenderPreview { page, output } => self.render_preview(page, &output).await,
            Request::LogLevel { level } => match &self.level {
                Some(control) => match control.set(&level) {
                    Ok(()) => Response::ok(format!("log level is now {level}")),
                    Err(error) => Response::error(error),
                },
                None => Response::error("logging is not initialised"),
            },
            Request::Stop => {
                self.stopping = true;
                Response::ok("stopping")
            }
        }
    }

    async fn pomodoro_command(&mut self, action: PomodoroAction) -> Response {
        let now = Utc::now();
        let now_ms = self.now_ms();

        match action {
            PomodoroAction::Acknowledge => {
                self.acknowledge_completion().await;
                self.render().await;
                Response::ok("acknowledged")
            }
            PomodoroAction::Start { phase } => {
                self.acknowledge_completion().await;
                pomodoro::start_phase(
                    &mut self.state.persistent.pomodoro,
                    phase,
                    now.timestamp_millis(),
                );
                self.persist(Durability::Critical);
                self.state.schedule_pomodoro_deadlines(now, now_ms);
                self.render().await;
                Response::ok(format!("started {}", phase.slug()))
            }
            PomodoroAction::Toggle => {
                self.execute(Action::Pomodoro(pages::PomodoroCommand::Toggle))
                    .await;
                Response::ok("toggled")
            }
            PomodoroAction::Skip => {
                self.execute(Action::Pomodoro(pages::PomodoroCommand::Skip))
                    .await;
                Response::ok("skipped")
            }
            PomodoroAction::Reset => {
                self.execute(Action::Pomodoro(pages::PomodoroCommand::Reset))
                    .await;
                Response::ok("reset")
            }
        }
    }

    async fn reload_config(&mut self) -> Result<(), String> {
        let path = self.state.config_path.clone();
        match streamdeck_core::config::Config::load(&path) {
            Ok(config) => {
                self.state.apply_config(Arc::new(config));
                self.ambient_brightness
                    .reconfigure(&self.state.config.ambient_brightness);
                self.metrics.config_reloads += 1;
                self.metrics.last_config_error = None;
                let now_ms = self.now_ms();
                self.state.schedule_refresh_deadlines(now_ms);
                self.state.schedule_walkingpad_midnight(Utc::now(), now_ms);
                self.set_deck_brightness(self.effective_brightness()).await;
                // Colours or thresholds may have changed, so repaint everything.
                self.invalidate_render_caches();
                self.render().await;
                tracing::info!(component = "config", "configuration reloaded");
                Ok(())
            }
            Err(error) => {
                let message = error.to_string();
                self.metrics.last_config_error = Some(message.clone());
                tracing::error!(
                    component = "config",
                    error = %message,
                    "keeping the previous configuration"
                );
                Err(message)
            }
        }
    }

    async fn render_preview(&mut self, page: PageId, output: &str) -> Response {
        let now_ms = self.now_ms();
        let world = self.state.world(Utc::now(), now_ms);
        let context = RenderContext::new(&world)
            .with_audio(&self.state.audio_output, &self.state.audio_input)
            .with_spotify_playlists(&self.state.config.spotify.playlists)
            .with_wispr_microphones(&self.state.config.wispr.microphones);
        let grid = streamdeck_core::model::Grid::MK2;
        let gutter = 4u32;
        let size = streamdeck_render::KEY_SIZE;
        let mut canvas = image::RgbImage::from_pixel(
            grid.columns as u32 * (size + gutter) + gutter,
            grid.rows as u32 * (size + gutter) + gutter,
            image::Rgb([16, 18, 24]),
        );

        for binding in pages::full_page(page, grid) {
            let view = render(binding.tile, &context);
            match self.renderer.render(&view).and_then(|key| key.to_image()) {
                Ok(image) => image::imageops::replace(
                    &mut canvas,
                    &image,
                    i64::from(gutter + (binding.position.column as u32 - 1) * (size + gutter)),
                    i64::from(gutter + (binding.position.row as u32 - 1) * (size + gutter)),
                ),
                Err(error) => return Response::error(error.to_string()),
            }
        }

        match canvas.save(output) {
            Ok(()) => Response::ok(format!("wrote {output}")),
            Err(error) => Response::error(error.to_string()),
        }
    }

    fn status_json(&self) -> serde_json::Value {
        let (sent, skipped, bytes) = self.frames.totals();
        let pomodoro = pomodoro::snapshot(
            &self.state.persistent.pomodoro,
            Utc::now().timestamp_millis(),
            self.state.timezone,
        );

        serde_json::json!({
            "uptime_seconds": self.metrics.uptime_seconds(),
            "resident_mib": crate::metrics::Metrics::resident_mib(),
            "child_processes": self.services.runner.running(),
            "device": self.device.as_ref().map(|device| {
                let descriptor = device.descriptor();
                serde_json::json!({
                    "serial": descriptor.serial,
                    "kind": descriptor.kind,
                    "rows": descriptor.grid.rows,
                    "columns": descriptor.grid.columns,
                })
            }),
            "brightness": {
                "automatic": self.state.config.ambient_brightness.enabled,
                "percent": self.effective_brightness(),
                "ambient_lux": self.ambient_brightness.last_lux,
            },
            "page": self.state.visible_page().slug(),
            "base_page": self.state.navigator.base_page().slug(),
            "panel_open": self.state.navigator.panel_is_open(),
            "screen_locked": self.screen_locked,
            "screensaver": self.screen_locked.then(|| self.screensaver_scene.name()),
            "renders": self.metrics.renders,
            "renders_skipped": self.metrics.renders_skipped,
            "frames_sent": sent,
            "frames_skipped": skipped,
            "bytes_sent": bytes,
            "key_presses": self.metrics.key_presses,
            "long_presses": self.metrics.long_presses,
            "page_switches": self.metrics.page_switches,
            "device_reconnects": self.metrics.device_reconnects,
            "wakes": self.metrics.wakes,
            "config_reloads": self.metrics.config_reloads,
            "last_config_error": self.metrics.last_config_error,
            "log_level": self.level.as_ref().map(|level| level.current()),
            "pending_deadlines": self.state.deadlines
                .pending()
                .into_iter()
                .map(|(id, at)| serde_json::json!({
                    "deadline": id.describe(),
                    "in_ms": at.saturating_sub(self.now_ms()),
                }))
                .collect::<Vec<_>>(),
            "in_flight": self.state.in_flight
                .iter()
                .map(|id| id.slug())
                .collect::<Vec<_>>(),
            "integrations": self.metrics.integrations(),
            "application": self.state.feeds.application.peek().map(|application| serde_json::json!({
                "name": application.name,
                "bundle_id": application.bundle_id,
                "pid": application.pid,
            })),
            "recent_applications": (0..5)
                .filter_map(|index| self.state.recent_application(index))
                .map(|application| serde_json::json!({
                    "name": application.name,
                    "bundle_id": application.bundle_id,
                    "pid": application.pid,
                }))
                .collect::<Vec<_>>(),
            "pomodoro": {
                "phase": pomodoro.phase.slug(),
                "status": format!("{:?}", pomodoro.status).to_lowercase(),
                "remaining_seconds": pomodoro.remaining_seconds,
                "cycle_focus_sessions": pomodoro.cycle_focus_sessions,
                "today_focus_minutes": pomodoro.today_focus_minutes,
                "total_focus_minutes": pomodoro.total_focus_minutes,
                "pending_completion": pomodoro.pending_completion_phase.map(|phase| phase.slug()),
                "alert_helper_running": self.alert.as_ref().is_some_and(AlertState::helper_running),
            },
            "wispr": {
                "hands_free": self.state.wispr_hands_free,
                "microphones": self.state.config.wispr.microphones
                    .iter()
                    .map(|microphone| microphone.label.as_str())
                    .collect::<Vec<_>>(),
            },
            "walkingpad": {
                "connection": format!("{:?}", self.state.walkingpad.connection).to_lowercase(),
                "status_age_seconds": self.state.walkingpad
                    .status_age_ms(Utc::now().timestamp_millis())
                    .map(|age| age / 1_000),
                "speed_tenths": self.state.walkingpad.telemetry
                    .as_ref()
                    .map(|status| status.speed_tenths),
                "today": {
                    "date": self.state.persistent.walkingpad.date,
                    "distance_hundredths": self.state.persistent.walkingpad.distance_hundredths,
                    "steps": self.state.persistent.walkingpad.steps,
                    "elapsed_seconds": self.state.persistent.walkingpad.elapsed_seconds,
                },
            },
        })
    }

    /// Shuts down: cancel work, reap children, and leave the deck as configured.
    pub async fn shutdown(&mut self) {
        tracing::info!(component = "runtime", "shutting down");
        if let Some(mut alert) = self.alert.take() {
            alert.close_helper().await;
        }
        self.persist(Durability::Critical);

        if let Some(device) = self.device.take() {
            if self.state.config.blank_on_exit {
                let _ = device.clear().await;
            }
            let _ = device.close().await;
        }
    }

    /// Test and diagnostic access to the state.
    pub fn state(&self) -> &RuntimeState {
        &self.state
    }

    pub fn state_mut(&mut self) -> &mut RuntimeState {
        &mut self.state
    }

    pub fn metrics(&self) -> &Metrics {
        &self.metrics
    }

    pub fn screen_locked(&self) -> bool {
        self.screen_locked
    }

    pub fn screensaver_scene(&self) -> Option<&'static str> {
        self.screen_locked.then(|| self.screensaver_scene.name())
    }

    fn effective_brightness(&self) -> u8 {
        if self.state.config.ambient_brightness.enabled {
            self.ambient_brightness
                .applied
                .unwrap_or(self.state.config.brightness)
        } else {
            self.state.config.brightness
        }
    }

    async fn set_deck_brightness(&mut self, brightness: u8) {
        let Some(device) = self.device.clone() else {
            return;
        };
        match device.set_brightness(brightness).await {
            Ok(()) => {}
            Err(DeviceError::Disconnected) => {
                let _ = self.sender.send(RuntimeEvent::DeviceDisconnected);
            }
            Err(error) => {
                tracing::warn!(
                    component = "device",
                    error = %error,
                    brightness,
                    "could not set brightness"
                );
            }
        }
    }
}

#[cfg(test)]
mod ambient_brightness_tests {
    use super::*;

    #[test]
    fn ambient_brightness_is_smoothed_quantized_and_bounded() {
        let config = AmbientBrightnessConfig::default();
        let mut controller = AmbientBrightnessController::default();

        assert_eq!(controller.observe(&config, 0.0), Some(config.minimum));
        let brighter = controller.observe(&config, 1_000.0);
        assert!(brighter.is_some_and(|value| value > config.minimum));
        assert_eq!(brighter.unwrap() % 5, 0);

        for _ in 0..30 {
            controller.observe(&config, 100_000.0);
        }
        assert_eq!(controller.applied, Some(config.maximum));
    }

    #[test]
    fn ambient_brightness_hysteresis_ignores_small_fluctuations() {
        let config = AmbientBrightnessConfig::default();
        let mut controller = AmbientBrightnessController::default();

        let initial = controller.observe(&config, 100.0).expect("initial");
        for lux in [98.0, 102.0, 99.0, 101.0, 97.0, 103.0] {
            assert_eq!(controller.observe(&config, lux), None);
        }
        assert_eq!(controller.applied, Some(initial));
    }

    #[test]
    fn disabled_automatic_brightness_only_tracks_the_sensor() {
        let config = AmbientBrightnessConfig {
            enabled: false,
            ..AmbientBrightnessConfig::default()
        };
        let mut controller = AmbientBrightnessController::default();

        assert_eq!(controller.observe(&config, 500.0), None);
        assert_eq!(controller.last_lux, Some(500.0));
        assert_eq!(controller.applied, None);
    }
}
