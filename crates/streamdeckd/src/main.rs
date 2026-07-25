//! `streamdeckd` — a headless macOS daemon for a 5x3 Stream Deck.

use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use streamdeck_core::config::Config;
use streamdeck_core::model::Grid;
use streamdeck_core::state::StateStore;
use streamdeck_macos::audio::CommandAudioAdapter;
use streamdeck_macos::meet::SystemMeetLauncher;
use streamdeck_macos::notify::SystemNotifier;
use streamdeck_macos::spotify::AppleScriptSpotifyAdapter;
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

    let runner = Arc::new(SystemCommandRunner::new());
    let services = Services {
        runner: Arc::clone(&runner) as Arc<dyn streamdeck_macos::CommandRunner>,
        audio: Arc::new(CommandAudioAdapter::new(
            Arc::clone(&runner) as Arc<dyn streamdeck_macos::CommandRunner>,
            config.tools.clone(),
        )),
        spotify: Arc::new(AppleScriptSpotifyAdapter::new(
            Arc::clone(&runner) as Arc<dyn streamdeck_macos::CommandRunner>,
            config.tools.clone(),
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
        http: services::http::HttpClient::new()?,
        helper_path: Some(streamdeck_macos::support_dir().join("bin/streamdeck-alert")),
    };

    let (sender, receiver) = mpsc::unbounded_channel();
    let state = runtime::state::RuntimeState::new(Arc::clone(&config), store, persistent);
    let mut daemon = Runtime::new(state, services, Renderer::new()?, receiver, sender.clone())
        .with_level_control(level);

    // Claim the control socket before touching any hardware. Binding is what
    // enforces one instance per user, so doing it second would mean a duplicate
    // start opened the deck before discovering it had to exit.
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

    // Open the device. Preview mode never touches the hardware.
    let device: Arc<dyn DeckDevice> = match &cli.preview {
        Some(path) => {
            let (preview, events) = device::preview::PreviewDeckDevice::new(path, Grid::MK2);
            // Nothing sends synthetic presses to the preview; keep the channel open.
            std::mem::forget(events);
            tracing::info!(component = "device", path = %path.display(), "preview mode");
            Arc::new(preview)
        }
        None => match device::hid::HidDeckDevice::open(config.device_serial.as_deref()) {
            Ok(device) => {
                let descriptor = device.descriptor();
                tracing::info!(
                    component = "device",
                    serial = %descriptor.serial,
                    kind = %descriptor.kind,
                    "opened the deck"
                );
                Arc::new(device)
            }
            Err(error) => {
                // Report clearly and exit successfully: retrying cannot help until
                // the other controller is stopped, and a non-zero exit would make
                // the LaunchAgent respawn forever.
                tracing::error!(component = "device", error = %error, "could not open the deck");
                eprintln!("streamdeckd: {error}");
                if matches!(error, DeviceError::Busy | DeviceError::NotFound(_)) {
                    eprintln!(
                        "Another application owns the Stream Deck. \
                         Quit Elgato Stream Deck or OpenDeck and try again."
                    );
                }
                return Ok(Outcome::DoNotRetry);
            }
        },
    };
    daemon.attach_device(Arc::clone(&device));

    // Input pump: turns device reports into runtime events.
    let input_device = Arc::clone(&device);
    let input_sender = sender.clone();
    let input = tokio::spawn(async move {
        loop {
            match input_device.next_event().await {
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
                    tracing::warn!(component = "device", error = %error, "input read failed");
                    let _ = input_sender.send(RuntimeEvent::DeviceDisconnected);
                    break;
                }
            }
        }
    });

    // Start serving on the socket claimed above, now that the runtime can answer.
    let control_sender = sender.clone();
    let control = tokio::spawn(socket.serve(control_sender));

    // Configuration watcher: an edit reloads transactionally.
    let watcher = spawn_config_watcher(config_file.clone(), sender.clone());

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
    daemon.shutdown().await;

    // Cancel every helper task and wait briefly so nothing outlives this process.
    input.abort();
    control.abort();
    signals.abort();
    drop(watcher);
    let _ = tokio::time::timeout(std::time::Duration::from_millis(500), async {
        let _ = input.await;
        let _ = control.await;
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
