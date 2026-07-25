//! `streamdeckctl` — local control and diagnostics for a running `streamdeckd`.
//!
//! Every command is one closed protocol request over the user-only Unix socket.
//! Nothing here can pass a command string to the daemon.

mod client;
mod format;

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use streamdeck_core::control::{PomodoroAction, Request};
use streamdeck_core::model::{IntegrationId, KeyPosition, PageId};
use streamdeck_core::pomodoro::Phase;

#[derive(Debug, Parser)]
#[command(
    name = "streamdeckctl",
    version,
    about = "Control and inspect a running streamdeckd"
)]
struct Cli {
    /// Control socket. Defaults to the application-support directory.
    #[arg(long, global = true)]
    socket: Option<PathBuf>,

    /// Print the raw JSON payload instead of a formatted summary.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Uptime, memory, current page, counters, and integration health.
    Status,
    /// Connected Stream Decks and whether each can be opened.
    Devices,
    /// Switch to a page.
    Page {
        /// home, mixer, github, spotify, stensjon, or pomodoro.
        page: String,
    },
    /// Synthesise a short press at `row,column`.
    Press { position: String },
    /// Synthesise a press held for a duration.
    ///
    /// The daemon holds the key for the whole duration, which briefly blocks its
    /// event loop, so this is a diagnostic rather than something to script.
    Hold {
        position: String,
        #[arg(long, default_value_t = 700)]
        milliseconds: u64,
    },
    /// Pomodoro control.
    #[command(subcommand)]
    Pomodoro(PomodoroCommand),
    /// Force one integration to refresh now.
    Refresh { integration: String },
    /// Re-read and validate the configuration.
    Reload,
    /// Render a page to a PNG.
    RenderPreview {
        #[arg(long)]
        page: String,
        #[arg(long)]
        output: PathBuf,
    },
    /// Run every health check.
    Doctor,
    /// Change the log level until the daemon restarts.
    LogLevel { level: String },
    /// Ask the daemon to shut down cleanly.
    Stop,
}

#[derive(Debug, Subcommand)]
enum PomodoroCommand {
    /// Clear a pending completion.
    Acknowledge,
    /// Start a phase: focus, short-break, or long-break.
    Start { phase: String },
    /// Start, pause, or resume.
    Toggle,
    /// Queue the next phase without crediting statistics.
    Skip,
    /// Return to a ready focus phase.
    Reset,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let socket = cli
        .socket
        .clone()
        .unwrap_or_else(streamdeck_macos::socket_path);

    let request = match build_request(&cli.command) {
        Ok(request) => request,
        Err(message) => {
            eprintln!("streamdeckctl: {message}");
            std::process::exit(2);
        }
    };

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let response = runtime.block_on(client::send(&socket, &request));

    match response {
        Ok(response) => {
            let exit = format::print(&cli.command, &response, cli.json);
            std::process::exit(exit);
        }
        Err(message) => {
            eprintln!("streamdeckctl: {message}");
            std::process::exit(1);
        }
    }
}

fn build_request(command: &Command) -> Result<Request, String> {
    let page = |value: &str| -> Result<PageId, String> {
        value
            .parse::<PageId>()
            .map_err(|error| format!("{error} (got `{value}`)"))
    };
    let position = |value: &str| -> Result<KeyPosition, String> {
        value
            .parse::<KeyPosition>()
            .map_err(|error| format!("{error} (got `{value}`)"))
    };

    Ok(match command {
        Command::Status => Request::Status,
        Command::Devices => Request::Devices,
        Command::Page { page: value } => Request::Page { page: page(value)? },
        Command::Press { position: value } => Request::Press {
            position: position(value)?,
        },
        Command::Hold {
            position: value,
            milliseconds,
        } => {
            if !(50..=3_000).contains(milliseconds) {
                return Err("--milliseconds must be between 50 and 3000".to_string());
            }
            Request::Hold {
                position: position(value)?,
                milliseconds: *milliseconds,
            }
        }
        Command::Pomodoro(action) => Request::Pomodoro {
            action: match action {
                PomodoroCommand::Acknowledge => PomodoroAction::Acknowledge,
                PomodoroCommand::Start { phase } => PomodoroAction::Start {
                    phase: Phase::parse(phase).ok_or_else(|| {
                        format!(
                            "unknown phase `{phase}`; expected focus, short-break, or long-break"
                        )
                    })?,
                },
                PomodoroCommand::Toggle => PomodoroAction::Toggle,
                PomodoroCommand::Skip => PomodoroAction::Skip,
                PomodoroCommand::Reset => PomodoroAction::Reset,
            },
        },
        Command::Refresh { integration } => Request::Refresh {
            integration: integration
                .parse::<IntegrationId>()
                .map_err(|error| format!("{error} (got `{integration}`)"))?,
        },
        Command::Reload => Request::Reload,
        Command::RenderPreview {
            page: value,
            output,
        } => Request::RenderPreview {
            page: page(value)?,
            output: output.to_string_lossy().to_string(),
        },
        Command::Doctor => Request::Doctor,
        Command::LogLevel { level } => Request::LogLevel {
            level: level.clone(),
        },
        Command::Stop => Request::Stop,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_documented_command_builds_a_request() {
        let cases: Vec<(Vec<&str>, Request)> = vec![
            (vec!["streamdeckctl", "status"], Request::Status),
            (vec!["streamdeckctl", "devices"], Request::Devices),
            (
                vec!["streamdeckctl", "page", "home"],
                Request::Page { page: PageId::Home },
            ),
            (
                vec!["streamdeckctl", "press", "2,3"],
                Request::Press {
                    position: KeyPosition::new(2, 3),
                },
            ),
            (
                vec!["streamdeckctl", "hold", "2,3", "--milliseconds", "700"],
                Request::Hold {
                    position: KeyPosition::new(2, 3),
                    milliseconds: 700,
                },
            ),
            (
                vec!["streamdeckctl", "pomodoro", "acknowledge"],
                Request::Pomodoro {
                    action: PomodoroAction::Acknowledge,
                },
            ),
            (
                vec!["streamdeckctl", "pomodoro", "start", "focus"],
                Request::Pomodoro {
                    action: PomodoroAction::Start {
                        phase: Phase::Focus,
                    },
                },
            ),
            (
                vec!["streamdeckctl", "refresh", "github"],
                Request::Refresh {
                    integration: IntegrationId::GitHub,
                },
            ),
            (vec!["streamdeckctl", "reload"], Request::Reload),
            (
                vec![
                    "streamdeckctl",
                    "render-preview",
                    "--page",
                    "home",
                    "--output",
                    "/tmp/home.png",
                ],
                Request::RenderPreview {
                    page: PageId::Home,
                    output: "/tmp/home.png".to_string(),
                },
            ),
            (vec!["streamdeckctl", "doctor"], Request::Doctor),
            (
                vec!["streamdeckctl", "log-level", "debug"],
                Request::LogLevel {
                    level: "debug".to_string(),
                },
            ),
            (vec!["streamdeckctl", "stop"], Request::Stop),
        ];

        for (argv, expected) in cases {
            let cli =
                Cli::try_parse_from(&argv).unwrap_or_else(|error| panic!("{argv:?}: {error}"));
            assert_eq!(
                build_request(&cli.command).expect("built"),
                expected,
                "{argv:?}"
            );
        }
    }

    #[test]
    fn a_bad_page_or_coordinate_is_refused_before_connecting() {
        let cli = Cli::try_parse_from(["streamdeckctl", "page", "nope"]).expect("parsed");
        let error = build_request(&cli.command).expect_err("refused");
        assert!(error.contains("nope"), "{error}");

        let cli = Cli::try_parse_from(["streamdeckctl", "press", "9"]).expect("parsed");
        assert!(build_request(&cli.command).is_err());

        let cli = Cli::try_parse_from(["streamdeckctl", "press", "0,0"]).expect("parsed");
        assert!(build_request(&cli.command).is_err());
    }

    #[test]
    fn an_unreasonable_hold_duration_is_refused() {
        for milliseconds in ["10", "5000"] {
            let cli = Cli::try_parse_from([
                "streamdeckctl",
                "hold",
                "2,3",
                "--milliseconds",
                milliseconds,
            ])
            .expect("parsed");
            assert!(build_request(&cli.command).is_err(), "{milliseconds}");
        }
    }

    #[test]
    fn an_unknown_phase_or_integration_is_refused() {
        let cli =
            Cli::try_parse_from(["streamdeckctl", "pomodoro", "start", "siesta"]).expect("parsed");
        assert!(build_request(&cli.command).is_err());

        let cli = Cli::try_parse_from(["streamdeckctl", "refresh", "nothing"]).expect("parsed");
        assert!(build_request(&cli.command).is_err());
    }

    #[test]
    fn the_hold_duration_defaults_to_the_documented_value() {
        let cli = Cli::try_parse_from(["streamdeckctl", "hold", "2,3"]).expect("parsed");
        assert_eq!(
            build_request(&cli.command).expect("built"),
            Request::Hold {
                position: KeyPosition::new(2, 3),
                milliseconds: 700
            }
        );
    }

    #[test]
    fn the_socket_and_json_flags_are_global() {
        let cli = Cli::try_parse_from([
            "streamdeckctl",
            "--socket",
            "/tmp/x.sock",
            "--json",
            "status",
        ])
        .expect("parsed");
        assert_eq!(cli.socket, Some(PathBuf::from("/tmp/x.sock")));
        assert!(cli.json);
    }

    #[test]
    fn page_aliases_are_accepted() {
        for alias in ["stensjon", "Stensjön", "lake"] {
            let cli = Cli::try_parse_from(["streamdeckctl", "page", alias]).expect("parsed");
            assert_eq!(
                build_request(&cli.command).expect("built"),
                Request::Page {
                    page: PageId::Stensjon
                },
                "{alias}"
            );
        }
    }
}
