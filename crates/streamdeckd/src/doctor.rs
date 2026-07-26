//! Health checks.
//!
//! Each check answers one question with a status and a short explanation, so
//! `streamdeckctl doctor` reads like a checklist. Nothing here prints a secret: a
//! credential check reports presence only.
//!
//! Several checks are genuinely slow — HID enumeration, a Keychain lookup that may
//! be waiting on an authorization prompt, spawning `SwitchAudioSource`. The
//! coordinator therefore never runs them itself: it collects [`Inputs`], which is
//! owned and `Send`, and hands them to a task.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use streamdeck_core::config::Config;
use streamdeck_macos::audio::AudioAdapter;
use streamdeck_macos::credentials;

use crate::device::DeckDevice;
use crate::runtime::state::RuntimeState;
use crate::runtime::Services;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Health {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub name: &'static str,
    pub health: Health,
    pub detail: String,
}

impl Check {
    fn ok(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            health: Health::Ok,
            detail: detail.into(),
        }
    }

    fn warn(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            health: Health::Warn,
            detail: detail.into(),
        }
    }

    fn fail(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            health: Health::Fail,
            detail: detail.into(),
        }
    }
}

/// Everything the checks need, owned so they can run off the coordinator.
pub struct Inputs {
    pub config: Arc<Config>,
    pub config_path: PathBuf,
    pub state_path: PathBuf,
    /// `Some` when this daemon already owns the device.
    pub device: Option<String>,
    pub audio: Arc<dyn AudioAdapter>,
    pub child_processes: usize,
    pub socket_path: PathBuf,
}

impl Inputs {
    /// Gathers the inputs. Cheap and non-blocking, so it is safe on the coordinator.
    pub fn collect(
        state: &RuntimeState,
        services: &Services,
        device: Option<&Arc<dyn DeckDevice>>,
    ) -> Self {
        Self {
            config: Arc::clone(&state.config),
            config_path: state.config_path.clone(),
            state_path: state.store.path().to_path_buf(),
            device: device.map(|device| {
                let descriptor = device.descriptor();
                format!("{} ({})", descriptor.serial, descriptor.kind)
            }),
            audio: Arc::clone(&services.audio),
            child_processes: services.runner.running(),
            socket_path: streamdeck_macos::socket_path(),
        }
    }
}

/// Runs every check and returns them as JSON for the CLI to format.
pub async fn run(inputs: Inputs) -> serde_json::Value {
    let mut checks = vec![
        device_check(inputs.device.as_deref()).await,
        config_check(&inputs.config_path),
        state_check(&inputs.state_path, &inputs.config),
        launch_agent_check(),
        child_process_check(inputs.child_processes),
    ];

    checks.extend(credential_checks(&inputs.config).await);
    checks.extend(tool_checks(&inputs.config));
    checks.push(audio_check(inputs.audio.as_ref()).await);
    checks.push(orphan_check(&inputs.socket_path).await);

    let worst = checks
        .iter()
        .map(|check| check.health)
        .max_by_key(|health| match health {
            Health::Ok => 0,
            Health::Warn => 1,
            Health::Fail => 2,
        })
        .unwrap_or(Health::Ok);

    serde_json::json!({ "summary": worst, "checks": checks })
}

async fn device_check(owned: Option<&str>) -> Check {
    if let Some(owned) = owned {
        return Check::ok("device", format!("{owned} owned by this daemon"));
    }

    // Enumeration is blocking and is handed to the dedicated hid thread.
    match tokio::task::spawn_blocking(crate::device::hid::discover).await {
        Ok(Ok(devices)) if devices.is_empty() => {
            Check::fail("device", "no Stream Deck is connected")
        }
        Ok(Ok(devices)) => {
            let owned_elsewhere: Vec<&str> = devices
                .iter()
                .filter(|device| !device.available)
                .map(|device| device.serial.as_str())
                .collect();
            if owned_elsewhere.is_empty() {
                Check::warn(
                    "device",
                    format!("{} device(s) available but not opened", devices.len()),
                )
            } else {
                Check::fail(
                    "device",
                    format!(
                        "another application owns {}; stop it before starting streamdeckd",
                        owned_elsewhere.join(", ")
                    ),
                )
            }
        }
        Ok(Err(error)) => Check::fail("device", error.to_string()),
        Err(error) => Check::fail("device", format!("the check panicked: {error}")),
    }
}

fn config_check(path: &std::path::Path) -> Check {
    if !path.exists() {
        return Check::warn(
            "config",
            format!("{} does not exist; using defaults", path.display()),
        );
    }
    match Config::load(path) {
        Ok(_) => Check::ok("config", format!("{} is valid", path.display())),
        Err(error) => Check::fail("config", error.to_string()),
    }
}

fn state_check(path: &std::path::Path, config: &Config) -> Check {
    if !path.exists() {
        return Check::warn("state", format!("{} does not exist yet", path.display()));
    }
    match streamdeck_core::state::StateStore::new(path).load(config.pomodoro_defaults()) {
        Ok(_) => Check::ok("state", format!("{} is valid", path.display())),
        Err(error) => Check::fail("state", error.to_string()),
    }
}

fn launch_agent_check() -> Check {
    let path = PathBuf::from(streamdeck_macos::expand_home(
        "~/Library/LaunchAgents/io.github.jimmystridh.streamdeckd.plist",
    ));
    if path.exists() {
        Check::ok("launch agent", format!("{} is installed", path.display()))
    } else {
        Check::warn(
            "launch agent",
            "not installed; the daemon will not start at login",
        )
    }
}

fn child_process_check(running: usize) -> Check {
    if running == 0 {
        Check::ok("child processes", "none running")
    } else {
        Check::ok("child processes", format!("{running} running"))
    }
}

/// Reports credential presence only.
///
/// Presence is checked without asking for the secret, so it does not normally
/// prompt — but the Keychain is still a system service that can stall, so the probe
/// runs off the runtime and under a timeout. A health check must never hang.
async fn credential_checks(config: &Config) -> Vec<Check> {
    let codex_path = config.usage.codex_auth_path.clone();
    let probe = tokio::time::timeout(
        Duration::from_secs(3),
        tokio::task::spawn_blocking(move || {
            (
                credentials::claude_credential_present(),
                credentials::codex_credential_present(codex_path.as_deref()),
            )
        }),
    )
    .await;

    match probe {
        Ok(Ok((claude, codex))) => vec![
            credential_check("claude credential", claude),
            credential_check("codex credential", codex),
        ],
        _ => vec![Check::warn(
            "credentials",
            "the Keychain did not answer within 3s; it may be waiting on an \
             authorization prompt",
        )],
    }
}

fn credential_check(name: &'static str, present: bool) -> Check {
    if present {
        Check::ok(name, "present")
    } else {
        Check::warn(name, "not found; the usage tile will show an error")
    }
}

/// Verifies each configured tool exists where the configuration says it does.
fn tool_checks(config: &Config) -> Vec<Check> {
    let tools: [(&'static str, &str); 4] = [
        ("gh", &config.tools.gh),
        ("gog", &config.tools.gog),
        ("SwitchAudioSource", &config.tools.switch_audio_source),
        ("osascript", &config.tools.osascript),
    ];

    tools
        .into_iter()
        .map(|(name, path)| {
            if std::path::Path::new(path).exists() {
                Check::ok(name, format!("{path} is installed"))
            } else {
                Check::fail(name, format!("{path} does not exist"))
            }
        })
        .collect()
}

/// Confirms the audio adapter can actually resolve the configured devices.
async fn audio_check(audio: &dyn AudioAdapter) -> Check {
    match tokio::time::timeout(Duration::from_secs(8), audio.snapshot()).await {
        Ok(Ok(snapshot)) => Check::ok(
            "audio",
            format!(
                "{} output(s) and {} input(s) visible",
                snapshot.inventory.outputs.len(),
                snapshot.inventory.inputs.len()
            ),
        ),
        Ok(Err(error)) => Check::fail("audio", error.to_string()),
        Err(_) => Check::fail("audio", "the audio adapter did not answer within 8s"),
    }
}

/// Looks for a socket left behind by a daemon that exited uncleanly.
async fn orphan_check(socket: &std::path::Path) -> Check {
    if !socket.exists() {
        return Check::ok("orphans", "no socket file present");
    }
    match tokio::time::timeout(
        Duration::from_secs(2),
        tokio::net::UnixStream::connect(socket),
    )
    .await
    {
        Ok(Ok(_)) => Check::ok("orphans", "the socket belongs to a live daemon"),
        _ => Check::warn(
            "orphans",
            format!(
                "{} exists but nothing is listening; a previous daemon exited uncleanly",
                socket.display()
            ),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_carry_a_name_a_health_and_an_explanation() {
        let check = Check::ok("device", "connected");
        assert_eq!(check.name, "device");
        assert_eq!(check.health, Health::Ok);
        assert_eq!(check.detail, "connected");

        assert_eq!(Check::warn("x", "y").health, Health::Warn);
        assert_eq!(Check::fail("x", "y").health, Health::Fail);
    }

    #[test]
    fn a_credential_check_reports_presence_and_never_a_value() {
        let present = credential_check("claude credential", true);
        assert_eq!(present.health, Health::Ok);
        assert_eq!(present.detail, "present");

        let missing = credential_check("claude credential", false);
        assert_eq!(missing.health, Health::Warn);
        assert!(missing.detail.contains("not found"));
    }

    #[test]
    fn a_missing_launch_agent_is_a_warning_rather_than_a_failure() {
        let _guard = crate::env_lock();
        std::env::set_var("HOME", "/nonexistent-home");
        assert_eq!(launch_agent_check().health, Health::Warn);
    }

    #[test]
    fn health_serializes_in_lower_case_for_the_cli() {
        assert_eq!(
            serde_json::to_string(&Health::Warn).expect("json"),
            "\"warn\""
        );
    }

    #[tokio::test]
    async fn an_owned_device_needs_no_enumeration() {
        let check = device_check(Some("A00SA5432IDMMF (Mk2)")).await;
        assert_eq!(check.health, Health::Ok);
        assert!(check.detail.contains("owned by this daemon"));
    }

    #[test]
    fn a_missing_configuration_is_a_warning_and_an_invalid_one_a_failure() {
        let directory = tempfile::tempdir().expect("temp dir");

        let missing = directory.path().join("absent.toml");
        assert_eq!(config_check(&missing).health, Health::Warn);

        let invalid = directory.path().join("invalid.toml");
        std::fs::write(&invalid, "version = 1\nbrightness = 500\n").expect("write");
        assert_eq!(config_check(&invalid).health, Health::Fail);

        let valid = directory.path().join("valid.toml");
        std::fs::write(&valid, streamdeck_core::config::TEMPLATE).expect("write");
        assert_eq!(config_check(&valid).health, Health::Ok);
    }

    #[test]
    fn a_missing_state_file_is_a_warning_and_a_corrupt_one_a_failure() {
        let directory = tempfile::tempdir().expect("temp dir");
        let config = Config::default();

        let missing = directory.path().join("absent.json");
        assert_eq!(state_check(&missing, &config).health, Health::Warn);

        let corrupt = directory.path().join("corrupt.json");
        std::fs::write(&corrupt, "{not json").expect("write");
        assert_eq!(state_check(&corrupt, &config).health, Health::Fail);
    }

    #[test]
    fn a_tool_that_does_not_exist_is_a_failure() {
        let mut config = Config::default();
        config.tools.gh = "/nonexistent/gh".to_string();

        let checks = tool_checks(&config);
        let gh = checks.iter().find(|check| check.name == "gh").expect("gh");
        assert_eq!(gh.health, Health::Fail);

        let osascript = checks
            .iter()
            .find(|check| check.name == "osascript")
            .expect("osascript");
        assert_eq!(osascript.health, Health::Ok, "osascript always exists");
    }

    #[tokio::test]
    async fn a_stale_socket_is_reported_as_an_orphan() {
        let directory = tempfile::tempdir().expect("temp dir");

        let absent = directory.path().join("absent.sock");
        assert_eq!(orphan_check(&absent).await.health, Health::Ok);

        let stale = directory.path().join("stale.sock");
        std::fs::write(&stale, b"").expect("write");
        let check = orphan_check(&stale).await;
        assert_eq!(check.health, Health::Warn);
        assert!(check.detail.contains("exited uncleanly"));

        let live = directory.path().join("live.sock");
        let _listener = tokio::net::UnixListener::bind(&live).expect("bound");
        assert_eq!(orphan_check(&live).await.health, Health::Ok);
    }

    #[tokio::test]
    async fn an_audio_adapter_that_never_answers_is_a_failure_rather_than_a_hang() {
        use async_trait::async_trait;
        use streamdeck_core::integrations::audio::{AudioInventory, AudioSnapshot, AudioStatus};
        use streamdeck_core::model::AudioKind;
        use streamdeck_macos::audio::AudioError;

        struct Silent;

        #[async_trait]
        impl AudioAdapter for Silent {
            async fn status(&self) -> Result<AudioStatus, AudioError> {
                std::future::pending().await
            }
            async fn inventory(&self) -> Result<AudioInventory, AudioError> {
                std::future::pending().await
            }
            async fn select(&self, _: AudioKind, _: &str) -> Result<(), AudioError> {
                Ok(())
            }
            async fn set_volume(&self, _: AudioKind, _: u8) -> Result<(), AudioError> {
                Ok(())
            }
            async fn set_output_muted(&self, _: bool) -> Result<(), AudioError> {
                Ok(())
            }
            async fn snapshot(&self) -> Result<AudioSnapshot, AudioError> {
                std::future::pending().await
            }
        }

        let started = std::time::Instant::now();
        let check = audio_check(&Silent).await;
        assert_eq!(check.health, Health::Fail);
        assert!(started.elapsed() < Duration::from_secs(12));
    }
}
