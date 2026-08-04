//! `streamdeckd` — a headless macOS daemon for a 5x3 Stream Deck.

use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use streamdeck_core::config::Config;
use streamdeck_core::model::Grid;
use streamdeck_core::state::StateStore;
use streamdeck_macos::ambient::AmbientLightSensor;
use streamdeck_macos::application::SystemApplicationAdapter;
#[cfg(target_os = "macos")]
use streamdeck_macos::audio::CoreAudioAdapter;
use streamdeck_macos::audio::{AudioAdapter, CommandAudioAdapter};
use streamdeck_macos::media::SystemMediaAdapter;
use streamdeck_macos::meet::SystemMeetLauncher;
use streamdeck_macos::notify::SystemNotifier;
use streamdeck_macos::spotify::AppleScriptSpotifyAdapter;
use streamdeck_macos::wispr::SystemWisprAdapter;
use streamdeck_macos::SystemCommandRunner;
use streamdeck_render::Renderer;
use tokio::sync::mpsc;

use streamdeckd::device::{self, DeckDevice, DeviceError};
use streamdeckd::runtime::{self, Runtime, RuntimeEvent, Services};
use streamdeckd::{config_path, control, logging, services, state_path};

#[derive(Debug, Parser)]
#[command(
    name = "streamdeckd",
    version,
    about = "Headless Stream Deck daemon for the Command Center layout"
)]
struct Cli {
    /// Configuration file. Defaults to the application-support directory.
    #[arg(long)]
    config: Option<PathBuf>,

    /// Render to a PNG instead of opening the hardware. The default for development.
    #[arg(long, value_name = "PATH")]
    preview: Option<PathBuf>,

    /// Log level: error, warn, info, debug, trace.
    #[arg(long, default_value = "info")]
    log_level: String,

    /// Also log to stderr. Implied when a terminal is attached.
    #[arg(long)]
    foreground: bool,

    /// Validate the configuration and exit.
    #[arg(long)]
    check: bool,
}

/// Why the daemon stopped, which decides whether `launchd` should respawn it.
enum Outcome {
    /// Ran, then shut down cleanly or on request.
    Completed,
    /// Cannot run, and restarting will not change that: another instance owns the
    /// control socket, or another application owns the device.
    ///
    /// Reported as a *successful* exit. The LaunchAgent uses
    /// `KeepAlive { SuccessfulExit: false }`, so a non-zero exit here would make
    /// launchd respawn every 30 seconds for as long as Elgato Stream Deck is open.
    DoNotRetry,
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    if let Some(path) = &cli.config {
        std::env::set_var("STREAMDECKD_CONFIG", path);
    }

    if cli.check {
        let path = config_path();
        return match Config::load(&path) {
            Ok(config) => {
                println!("{} is valid (version {})", path.display(), config.version);
                std::process::ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("{error}");
                std::process::ExitCode::FAILURE
            }
        };
    }

    #[cfg(target_os = "macos")]
    streamdeck_macos::application::run_agent_application(move || run(cli));

    #[cfg(not(target_os = "macos"))]
    run(cli)
}

fn run(cli: Cli) -> std::process::ExitCode {
    // A small pool: this daemon is I/O bound and must stay under 80 MiB.
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(4)
        .enable_all()
        .thread_name("streamdeckd")
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("streamdeckd: could not start the async runtime: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };

    let result = runtime.block_on(serve(cli));

    // Dropping the runtime would wait for blocking tasks to finish, and one of
    // them can legitimately be stuck: a Keychain read waits indefinitely while
    // macOS shows an authorization prompt. Shutting down with a deadline means an
    // unanswered prompt can never stop the daemon from exiting.
    runtime.shutdown_timeout(SHUTDOWN_GRACE);

    match result {
        Ok(Outcome::Completed) | Ok(Outcome::DoNotRetry) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("streamdeckd: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

/// How long shutdown waits for tasks before abandoning them and exiting.
const SHUTDOWN_GRACE: std::time::Duration = std::time::Duration::from_millis(500);

async fn serve(cli: Cli) -> anyhow::Result<Outcome> {
    let (level, _writer) = logging::init(
        &streamdeck_macos::log_dir(),
        &cli.log_level,
        cli.foreground || cli.preview.is_some(),
    )?;

    let config_file = config_path();
    let config = match Config::load(&config_file) {
        Ok(config) => config,
        Err(error) => {
            // Starting with defaults is better than not starting at all; the
            // error is visible in the log and in `streamdeckctl status`.
            tracing::warn!(
                component = "config",
                error = %error,
                "using built-in defaults"
            );
            Config::default()
        }
    };
    let config = Arc::new(config);

    let store = StateStore::new(state_path());
    let persistent = match store.load(config.pomodoro_defaults()) {
        Ok(state) => state,
        Err(error) => {
            tracing::error!(component = "state", error = %error, "starting from defaults");
            streamdeck_core::state::PersistentState {
                pomodoro: config.pomodoro_defaults(),
                ..Default::default()
            }
        }
    };

    let socket = match control::ControlSocket::bind(streamdeck_macos::socket_path()).await {
        Ok(socket) => socket,
        Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
            eprintln!("streamdeckd: {error}");
            eprintln!("Stop it with `streamdeckctl stop` before starting another.");
            return Ok(Outcome::DoNotRetry);
        }
        Err(error) => return Err(error.into()),
    };
    tracing::info!(component = "control", path = %socket.path().display(), "listening");

    let (sender, receiver) = mpsc::unbounded_channel();
    let (walkingpad, walkingpad_service): (
        Arc<dyn services::walkingpad::WalkingPadCommander>,
        Option<services::walkingpad::WalkingPadService>,
    ) = if cli.preview.is_some() {
        let reason = "WalkingPad is disabled in preview mode";
        let _ = sender.send(RuntimeEvent::WalkingPad(
            streamdeck_core::integrations::walkingpad::WalkingPadUpdate::Connection {
                state:
                    streamdeck_core::integrations::walkingpad::WalkingPadConnection::Disconnected,
                error: Some(reason.to_string()),
            },
        ));
        (
            Arc::new(services::walkingpad::UnavailableWalkingPadController::new(
                reason,
            )),
            None,
        )
    } else {
        match services::walkingpad::WalkingPadService::spawn(sender.clone()) {
            Ok(service) => (service.controller(), Some(service)),
            Err(error) => {
                let _ = sender.send(RuntimeEvent::WalkingPad(
                    streamdeck_core::integrations::walkingpad::WalkingPadUpdate::Connection {
                        state: streamdeck_core::integrations::walkingpad::WalkingPadConnection::Disconnected,
                        error: Some(error.clone()),
                    },
                ));
                (
                    Arc::new(services::walkingpad::UnavailableWalkingPadController::new(
                        error,
                    )),
                    None,
                )
            }
        }
    };

    let runner = Arc::new(SystemCommandRunner::new());
    let audio = audio_adapter(&runner, &config);
    let http = services::http::HttpClient::new()?;
    let services = Services {
        runner: Arc::clone(&runner) as Arc<dyn streamdeck_macos::CommandRunner>,
        audio,
        application: Arc::new(SystemApplicationAdapter::new(
            Arc::clone(&runner) as Arc<dyn streamdeck_macos::CommandRunner>,
            config.tools.osascript.clone(),
        )),
        spotify: Arc::new(AppleScriptSpotifyAdapter::new(
            Arc::clone(&runner) as Arc<dyn streamdeck_macos::CommandRunner>,
            config.tools.clone(),
        )),
        media: Arc::new(SystemMediaAdapter::new(
            Arc::clone(&runner) as Arc<dyn streamdeck_macos::CommandRunner>,
            config.tools.clone(),
        )),
        wispr: Arc::new(SystemWisprAdapter::new(
            Arc::clone(&runner) as Arc<dyn streamdeck_macos::CommandRunner>,
            config.tools.open.clone(),
        )),
        notifier: Arc::new(SystemNotifier::new(
            Arc::clone(&runner) as Arc<dyn streamdeck_macos::CommandRunner>,
            config.tools.clone(),
        )),
        meet: Arc::new(SystemMeetLauncher::new(
            Arc::clone(&runner) as Arc<dyn streamdeck_macos::CommandRunner>,
            config.tools.clone(),
            &config.meetings.meet_app,
        )),
        http: http.clone(),
        vasttrafik: services::vasttrafik::Client::new(http),
        walkingpad,
        helper_path: Some(streamdeck_macos::support_dir().join("bin/streamdeck-alert")),
    };

    let state =
        runtime::state::RuntimeState::new(Arc::clone(&config), &config_file, store, persistent);
    let screen_locked = streamdeck_macos::session::screen_is_locked();
    let initial_ambient_lux = read_ambient_light();
    let mut daemon = Runtime::new(state, services, Renderer::new()?, receiver, sender.clone())
        .with_level_control(level)
        .with_screen_locked(screen_locked)
        .with_ambient_lux(initial_ambient_lux);

    // Open the device. Preview mode never touches the hardware.
    let input = match &cli.preview {
        Some(path) => {
            let (preview, events) = device::preview::PreviewDeckDevice::new(path, Grid::MK2);
            // Nothing sends synthetic presses to the preview; keep the channel open.
            std::mem::forget(events);
            tracing::info!(component = "device", path = %path.display(), "preview mode");
            let device = Arc::new(preview) as Arc<dyn DeckDevice>;
            daemon.attach_device(Arc::clone(&device));

            let input_sender = sender.clone();
            tokio::spawn(async move {
                loop {
                    match device.next_event().await {
                        Ok(Some(event)) => {
                            if input_sender.send(RuntimeEvent::Key(event)).is_err() {
                                break;
                            }
                        }
                        Ok(None) => {
                            let _ = input_sender.send(RuntimeEvent::DeviceDisconnected);
                            break;
                        }
                        Err(error) => {
                            tracing::warn!(
                                component = "device",
                                error = %error,
                                "preview input failed"
                            );
                            let _ = input_sender.send(RuntimeEvent::DeviceDisconnected);
                            break;
                        }
                    }
                }
            })
        }
        None => {
            let initial = match device::hid::HidDeckDevice::open(config.device_serial.as_deref()) {
                Ok(device) => {
                    let device = Arc::new(device) as Arc<dyn DeckDevice>;
                    let descriptor = device.descriptor();
                    tracing::info!(
                        component = "device",
                        serial = %descriptor.serial,
                        kind = %descriptor.kind,
                        "opened the deck"
                    );
                    daemon.attach_device(Arc::clone(&device));
                    Some(device)
                }
                Err(DeviceError::NotFound(wanted)) => {
                    tracing::warn!(
                        component = "device",
                        device = %wanted,
                        "the deck is not connected; waiting for it"
                    );
                    None
                }
                Err(DeviceError::Busy) => {
                    tracing::error!(component = "device", "another application owns the deck");
                    eprintln!("streamdeckd: {}", DeviceError::Busy);
                    eprintln!(
                        "Another application owns the Stream Deck. \
                         Quit Elgato Stream Deck or OpenDeck and try again."
                    );
                    return Ok(Outcome::DoNotRetry);
                }
                Err(error) => return Err(error.into()),
            };

            let serial = config.device_serial.clone();
            let input_sender = sender.clone();
            tokio::spawn(device::supervise(
                initial,
                input_sender,
                device::RECONNECT_RETRY_INTERVAL,
                move || {
                    let serial = serial.clone();
                    async move {
                        match tokio::task::spawn_blocking(move || {
                            device::hid::HidDeckDevice::open(serial.as_deref())
                                .map(|device| Arc::new(device) as Arc<dyn DeckDevice>)
                        })
                        .await
                        {
                            Ok(result) => result,
                            Err(error) => Err(DeviceError::Other(format!(
                                "the reconnect task failed: {error}"
                            ))),
                        }
                    }
                },
            ))
        }
    };

    // Start serving on the socket claimed above, now that the runtime can answer.
    let control_sender = sender.clone();
    let control = tokio::spawn(socket.serve(control_sender));

    // Configuration watcher: an edit reloads transactionally.
    let watcher = spawn_config_watcher(config_file.clone(), sender.clone());
    let screen_lock_monitor = spawn_screen_lock_monitor(screen_locked, sender.clone());
    let ambient_light_monitor = spawn_ambient_light_monitor(sender.clone());

    // Signals.
    let signal_sender = sender.clone();
    let signals = tokio::spawn(async move {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("SIGTERM handler");
        let mut interrupt =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
                .expect("SIGINT handler");
        tokio::select! {
            _ = terminate.recv() => {}
            _ = interrupt.recv() => {}
        }
        let _ = signal_sender.send(RuntimeEvent::Shutdown);
    });

    daemon.start().await?;
    let result = daemon.run().await;
    if let Some(monitor) = ambient_light_monitor {
        monitor.stop();
    }
    if let Some(service) = walkingpad_service {
        service.shutdown().await;
    }
    daemon.shutdown().await;

    // Cancel every helper task and wait briefly so nothing outlives this process.
    input.abort();
    control.abort();
    screen_lock_monitor.abort();
    signals.abort();
    drop(watcher);
    let _ = tokio::time::timeout(std::time::Duration::from_millis(500), async {
        let _ = input.await;
        let _ = control.await;
        let _ = screen_lock_monitor.await;
    })
    .await;

    let stragglers = streamdeck_macos::CommandRunner::running(runner.as_ref());
    if stragglers > 0 {
        tracing::warn!(
            component = "runtime",
            children = stragglers,
            "children were still running at shutdown"
        );
    }
    tracing::info!(component = "runtime", "stopped");
    result.map(|()| Outcome::Completed)
}

fn audio_adapter(runner: &Arc<SystemCommandRunner>, config: &Config) -> Arc<dyn AudioAdapter> {
    if config.audio.native {
        #[cfg(target_os = "macos")]
        {
            return Arc::new(CoreAudioAdapter::new());
        }
    }
    Arc::new(CommandAudioAdapter::new(
        Arc::clone(runner) as Arc<dyn streamdeck_macos::CommandRunner>,
        config.tools.clone(),
    ))
}

fn spawn_screen_lock_monitor(
    initial_state: bool,
    events: mpsc::UnboundedSender<RuntimeEvent>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut previous = initial_state;
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            let current = streamdeck_macos::session::screen_is_locked();
            if current != previous {
                previous = current;
                if events
                    .send(RuntimeEvent::ScreenLockChanged(current))
                    .is_err()
                {
                    break;
                }
            }
        }
    })
}

const AMBIENT_LIGHT_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(3);
const AMBIENT_LIGHT_RETRY_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

struct AmbientLightMonitor {
    stop: std::sync::mpsc::SyncSender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl AmbientLightMonitor {
    fn stop(mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            if thread.join().is_err() {
                tracing::warn!(
                    component = "ambient-light",
                    "ambient-light monitor panicked"
                );
            }
        }
    }
}

fn read_ambient_light() -> Option<f64> {
    let sensor = match AmbientLightSensor::open() {
        Ok(sensor) => sensor,
        Err(error) => {
            tracing::info!(
                component = "ambient-light",
                error = %error,
                "automatic brightness is waiting for a sensor"
            );
            return None;
        }
    };
    match sensor.lux() {
        Ok(lux) => Some(lux),
        Err(error) => {
            tracing::info!(
                component = "ambient-light",
                error = %error,
                "automatic brightness is waiting for a reading"
            );
            None
        }
    }
}

fn spawn_ambient_light_monitor(
    events: mpsc::UnboundedSender<RuntimeEvent>,
) -> Option<AmbientLightMonitor> {
    let (stop, stop_receiver) = std::sync::mpsc::sync_channel(1);
    let thread = std::thread::Builder::new()
        .name("streamdeckd-ambient".to_string())
        .stack_size(256 * 1024)
        .spawn(move || {
            let mut sensor = None;
            let mut last_error = None;

            loop {
                if sensor.is_none() {
                    match AmbientLightSensor::open() {
                        Ok(opened) => {
                            sensor = Some(opened);
                            last_error = None;
                        }
                        Err(error) => {
                            log_ambient_error_once(&mut last_error, error.to_string());
                        }
                    }
                }

                if let Some(opened) = sensor.as_ref() {
                    match opened.lux() {
                        Ok(lux) => {
                            last_error = None;
                            if events.send(RuntimeEvent::AmbientLight(lux)).is_err() {
                                break;
                            }
                        }
                        Err(error) => {
                            log_ambient_error_once(&mut last_error, error.to_string());
                            sensor = None;
                        }
                    }
                }

                let interval = if sensor.is_some() {
                    AMBIENT_LIGHT_POLL_INTERVAL
                } else {
                    AMBIENT_LIGHT_RETRY_INTERVAL
                };
                match stop_receiver.recv_timeout(interval) {
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        })
        .map_err(|error| {
            tracing::warn!(
                component = "ambient-light",
                error = %error,
                "could not start ambient-light monitor"
            );
        })
        .ok()?;

    Some(AmbientLightMonitor {
        stop,
        thread: Some(thread),
    })
}

fn log_ambient_error_once(last_error: &mut Option<String>, error: String) {
    if last_error.as_deref() != Some(&error) {
        tracing::warn!(
            component = "ambient-light",
            error,
            "ambient-light reading unavailable; retrying"
        );
        *last_error = Some(error);
    }
}

/// Watches the configuration file and asks the runtime to reload on change.
fn spawn_config_watcher(
    path: PathBuf,
    events: mpsc::UnboundedSender<RuntimeEvent>,
) -> Option<notify::RecommendedWatcher> {
    use notify::Watcher;

    let directory = path.parent()?.to_path_buf();
    let mut watcher = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
        let Ok(event) = result else { return };
        if event.paths.iter().any(|changed| changed == &path) {
            let _ = events.send(RuntimeEvent::ConfigChanged);
        }
    })
    .ok()?;
    watcher
        .watch(&directory, notify::RecursiveMode::NonRecursive)
        .ok()?;
    Some(watcher)
}
