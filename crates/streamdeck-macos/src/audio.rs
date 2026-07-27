//! Audio adapter.
//!
//! Stage one of the plan: parity through short-lived `SwitchAudioSource` and
//! `osascript` invocations. The trait is the whole contract, so a native
//! CoreAudio implementation can replace it without touching a caller.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use streamdeck_core::config::ToolsConfig;
use streamdeck_core::integrations::audio::{
    self, AudioInventory, AudioSnapshot, AudioStatus, AudioTarget, Resolution,
};
use streamdeck_core::model::AudioKind;

use crate::command::{CommandRunner, Output};
use crate::timeouts;

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error(transparent)]
    Command(#[from] crate::command::CommandError),
    #[error(transparent)]
    Parse(#[from] streamdeck_core::integrations::ParseError),
    #[error("`{label}` is not available right now")]
    Unavailable { label: String },
    #[error("`{label}` matches {count} devices; make the pattern more specific")]
    Ambiguous { label: String, count: usize },
    #[error("no audio target is configured at position {index}")]
    NoSuchTarget { index: usize },
    #[error("CoreAudio failed while trying to {operation} (OSStatus {status})")]
    CoreAudio {
        operation: &'static str,
        status: i32,
    },
    #[error("CoreAudio cannot control {property} on `{device}`")]
    Unsupported {
        device: String,
        property: &'static str,
    },
}

/// What every audio-capable adapter must provide.
#[async_trait]
pub trait AudioAdapter: Send + Sync {
    async fn status(&self) -> Result<AudioStatus, AudioError>;
    async fn inventory(&self) -> Result<AudioInventory, AudioError>;
    async fn select(&self, kind: AudioKind, device: &str) -> Result<(), AudioError>;
    async fn set_volume(&self, kind: AudioKind, volume: u8) -> Result<(), AudioError>;
    async fn set_output_muted(&self, muted: bool) -> Result<(), AudioError>;

    async fn adjust_volume_relative(&self, kind: AudioKind, delta: i32) -> Result<u8, AudioError> {
        let status = self.status().await?;
        let next = audio::next_volume(status.volume(kind), delta);
        self.set_volume(kind, next).await?;
        Ok(next)
    }

    async fn toggle_mute_state(
        &self,
        kind: AudioKind,
        restore_volume: u8,
    ) -> Result<(bool, Option<u8>), AudioError> {
        let status = self.status().await?;
        match kind {
            AudioKind::Output => {
                let muted = !status.output_muted;
                self.set_output_muted(muted).await?;
                Ok((muted, None))
            }
            AudioKind::Input if status.input_volume == 0 => {
                self.set_volume(AudioKind::Input, restore_volume.clamp(1, 100))
                    .await?;
                Ok((false, None))
            }
            AudioKind::Input => {
                self.set_volume(AudioKind::Input, 0).await?;
                Ok((true, Some(status.input_volume)))
            }
        }
    }

    async fn snapshot(&self) -> Result<AudioSnapshot, AudioError> {
        // Status and inventory are independent, so fetch them together.
        let (status, inventory) = tokio::join!(self.status(), self.inventory());
        Ok(AudioSnapshot {
            status: Some(status?),
            inventory: inventory?,
        })
    }
}

/// The `SwitchAudioSource` + AppleScript parity adapter.
pub struct CommandAudioAdapter {
    runner: Arc<dyn CommandRunner>,
    tools: ToolsConfig,
    timeout: Duration,
}

impl CommandAudioAdapter {
    pub fn new(runner: Arc<dyn CommandRunner>, tools: ToolsConfig) -> Self {
        Self {
            runner,
            tools,
            timeout: timeouts::LOCAL,
        }
    }

    async fn switch_audio(&self, args: &[&str]) -> Result<Output, AudioError> {
        Ok(self
            .runner
            .run(&self.tools.switch_audio_source, args, self.timeout)
            .await?)
    }

    async fn osascript(&self, script: &str) -> Result<Output, AudioError> {
        Ok(self
            .runner
            .run(&self.tools.osascript, &["-e", script], self.timeout)
            .await?)
    }
}

#[async_trait]
impl AudioAdapter for CommandAudioAdapter {
    async fn status(&self) -> Result<AudioStatus, AudioError> {
        let (output, input, volumes) = tokio::join!(
            self.switch_audio(&["-c", "-t", "output"]),
            self.switch_audio(&["-c", "-t", "input"]),
            self.osascript("get volume settings")
        );
        Ok(audio::parse_status(
            output?.trimmed(),
            input?.trimmed(),
            volumes?.trimmed(),
        )?)
    }

    async fn inventory(&self) -> Result<AudioInventory, AudioError> {
        let (outputs, inputs) = tokio::join!(
            self.switch_audio(&["-a", "-t", "output"]),
            self.switch_audio(&["-a", "-t", "input"])
        );
        Ok(AudioInventory {
            outputs: audio::parse_device_list(&outputs?.stdout),
            inputs: audio::parse_device_list(&inputs?.stdout),
        })
    }

    async fn select(&self, kind: AudioKind, device: &str) -> Result<(), AudioError> {
        self.switch_audio(&["-s", device, "-t", kind.switch_audio_flag()])
            .await?;
        Ok(())
    }

    async fn set_volume(&self, kind: AudioKind, volume: u8) -> Result<(), AudioError> {
        let property = match kind {
            AudioKind::Output => "output volume",
            AudioKind::Input => "input volume",
        };
        self.osascript(&format!("set volume {property} {}", volume.min(100)))
            .await?;
        Ok(())
    }

    async fn set_output_muted(&self, muted: bool) -> Result<(), AudioError> {
        self.osascript(&format!("set volume output muted {muted}"))
            .await?;
        Ok(())
    }
}

/// Selects a configured target, refusing to guess when the pattern is ambiguous.
pub async fn select_target(
    adapter: &dyn AudioAdapter,
    kind: AudioKind,
    targets: &[AudioTarget],
    index: usize,
) -> Result<String, AudioError> {
    let target = targets
        .get(index)
        .ok_or(AudioError::NoSuchTarget { index })?;
    // Always re-read the inventory: a device may have appeared or vanished since
    // the tile was last drawn.
    let inventory = adapter.inventory().await?;

    match target.resolve(inventory.devices(kind)) {
        Resolution::Available(device) => {
            adapter.select(kind, &device).await?;
            Ok(device)
        }
        Resolution::Ambiguous(matches) => Err(AudioError::Ambiguous {
            label: target.label.clone(),
            count: matches.len(),
        }),
        Resolution::Unavailable => Err(AudioError::Unavailable {
            label: target.label.clone(),
        }),
    }
}

/// Applies a relative volume change and returns the new level.
pub async fn adjust_volume(
    adapter: &dyn AudioAdapter,
    kind: AudioKind,
    delta: i32,
) -> Result<u8, AudioError> {
    adapter.adjust_volume_relative(kind, delta).await
}

/// Toggles mute. A microphone has no mute switch, so zero gain stands in and the
/// previous level is restored on unmute.
pub async fn toggle_mute(
    adapter: &dyn AudioAdapter,
    kind: AudioKind,
    restore_volume: u8,
) -> Result<(bool, Option<u8>), AudioError> {
    adapter.toggle_mute_state(kind, restore_volume).await
}

#[cfg(target_os = "macos")]
mod native;
#[cfg(target_os = "macos")]
pub use native::CoreAudioAdapter;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{FakeCommandRunner, Reply};
    use streamdeck_core::config::AudioTargetConfig;

    fn tools() -> ToolsConfig {
        ToolsConfig::default()
    }

    fn adapter(runner: Arc<FakeCommandRunner>) -> CommandAudioAdapter {
        CommandAudioAdapter::new(runner, tools())
    }

    fn healthy() -> Arc<FakeCommandRunner> {
        let runner = Arc::new(FakeCommandRunner::new());
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
                Reply::ok(
                    "output volume:42, input volume:75, alert volume:100, output muted:false",
                ),
            )
            .on("set volume", Reply::ok(""))
            .on("-s ", Reply::ok(""));
        runner
    }

    fn targets(specs: &[(&str, Option<&str>, Option<&str>)]) -> Vec<AudioTarget> {
        specs
            .iter()
            .map(|(label, exact, pattern)| {
                AudioTarget::from_config(&AudioTargetConfig {
                    label: label.to_string(),
                    exact: exact.map(str::to_string),
                    pattern: pattern.map(str::to_string),
                })
                .expect("valid target")
            })
            .collect()
    }

    #[tokio::test]
    async fn status_reads_both_devices_and_the_volume_settings() {
        let runner = healthy();
        let status = adapter(Arc::clone(&runner)).status().await.expect("status");

        assert_eq!(status.current_output, "Bose NC 700 Headphones");
        assert_eq!(status.current_input, "MacBook Pro Microphone");
        assert_eq!(status.output_volume, 42);
        assert_eq!(status.input_volume, 75);
        assert!(!status.output_muted);
    }

    #[tokio::test]
    async fn a_snapshot_reads_status_and_inventory_together() {
        let runner = healthy();
        let snapshot = adapter(Arc::clone(&runner))
            .snapshot()
            .await
            .expect("snapshot");

        assert_eq!(snapshot.inventory.outputs.len(), 2);
        assert_eq!(snapshot.inventory.inputs.len(), 1);
        assert!(snapshot.status.is_some());
    }

    #[tokio::test]
    async fn a_failing_tool_surfaces_rather_than_producing_empty_state() {
        let runner = Arc::new(FakeCommandRunner::new());
        runner.fallback(Reply::fails(127, "SwitchAudioSource: command not found"));

        let error = adapter(runner).status().await.expect_err("fails");
        assert!(error.to_string().contains("not found"), "{error}");
    }

    #[tokio::test]
    async fn unparseable_volume_output_is_a_parse_error() {
        let runner = healthy();
        runner.on("get volume settings", Reply::ok("nothing useful here"));

        let error = adapter(runner).status().await.expect_err("fails");
        assert!(matches!(error, AudioError::Parse(_)), "{error}");
    }

    #[tokio::test]
    async fn selecting_an_exact_device_passes_its_name_as_an_argument() {
        let runner = healthy();
        let adapter = adapter(Arc::clone(&runner));
        let targets = targets(&[("Bose", Some("Bose NC 700 Headphones"), None)]);

        let device = select_target(&adapter, AudioKind::Output, &targets, 0)
            .await
            .expect("selected");

        assert_eq!(device, "Bose NC 700 Headphones");
        assert!(runner.called_with("-s Bose NC 700 Headphones -t output"));
    }

    #[tokio::test]
    async fn selecting_an_absent_device_reports_it_unavailable_and_sends_nothing() {
        let runner = healthy();
        let adapter = adapter(Arc::clone(&runner));
        let targets = targets(&[("RØDE Mic", None, Some("røde|rode"))]);

        let error = select_target(&adapter, AudioKind::Input, &targets, 0)
            .await
            .expect_err("unavailable");

        assert!(matches!(error, AudioError::Unavailable { .. }), "{error}");
        assert!(!runner.called_with("-s "), "no device should be selected");
    }

    #[tokio::test]
    async fn an_ambiguous_pattern_refuses_to_select_anything() {
        let runner = healthy();
        runner.on(
            "-a -t output",
            Reply::ok("Scarlett 2i2 USB\nRØDE NT-USB Mini\n"),
        );
        let adapter = adapter(Arc::clone(&runner));
        let targets = targets(&[("USB Home", None, Some("usb"))]);

        let error = select_target(&adapter, AudioKind::Output, &targets, 0)
            .await
            .expect_err("ambiguous");

        match error {
            AudioError::Ambiguous { count, .. } => assert_eq!(count, 2),
            other => panic!("expected ambiguity, got {other}"),
        }
        assert!(!runner.called_with("-s "));
    }

    #[tokio::test]
    async fn an_unconfigured_tile_index_reports_a_clear_error() {
        let runner = healthy();
        let adapter = adapter(runner);
        let error = select_target(&adapter, AudioKind::Output, &[], 2)
            .await
            .expect_err("no target");
        assert!(
            matches!(error, AudioError::NoSuchTarget { index: 2 }),
            "{error}"
        );
    }

    #[tokio::test]
    async fn volume_changes_are_relative_and_clamped() {
        let runner = healthy();
        let adapter = adapter(Arc::clone(&runner));

        let next = adjust_volume(&adapter, AudioKind::Output, 10)
            .await
            .expect("adjusted");
        assert_eq!(next, 52);
        assert!(runner.called_with("set volume output volume 52"));

        runner.reset();
        let next = adjust_volume(&adapter, AudioKind::Input, 40)
            .await
            .expect("adjusted");
        assert_eq!(next, 100, "75 + 40 clamps at 100");
        assert!(runner.called_with("set volume input volume 100"));
    }

    #[tokio::test]
    async fn output_mute_toggles_both_ways() {
        let runner = healthy();
        let adapter = adapter(Arc::clone(&runner));

        let (muted, remembered) = toggle_mute(&adapter, AudioKind::Output, 50)
            .await
            .expect("toggled");
        assert!(muted);
        assert_eq!(remembered, None);
        assert!(runner.called_with("set volume output muted true"));

        runner.reset();
        runner.on(
            "get volume settings",
            Reply::ok("output volume:42, input volume:75, output muted:true"),
        );
        let (muted, _) = toggle_mute(&adapter, AudioKind::Output, 50)
            .await
            .expect("toggled");
        assert!(!muted);
        assert!(runner.called_with("set volume output muted false"));
    }

    #[tokio::test]
    async fn muting_the_microphone_remembers_the_previous_gain() {
        let runner = healthy();
        let adapter = adapter(Arc::clone(&runner));

        let (muted, remembered) = toggle_mute(&adapter, AudioKind::Input, 50)
            .await
            .expect("toggled");
        assert!(muted);
        assert_eq!(
            remembered,
            Some(75),
            "the previous gain is reported for state"
        );
        assert!(runner.called_with("set volume input volume 0"));
    }

    #[tokio::test]
    async fn unmuting_the_microphone_restores_the_remembered_gain() {
        let runner = healthy();
        runner.on(
            "get volume settings",
            Reply::ok("output volume:42, input volume:0, output muted:false"),
        );
        let adapter = adapter(Arc::clone(&runner));

        let (muted, _) = toggle_mute(&adapter, AudioKind::Input, 75)
            .await
            .expect("toggled");
        assert!(!muted);
        assert!(runner.called_with("set volume input volume 75"));
    }

    #[tokio::test]
    async fn unmuting_never_restores_to_silence() {
        let runner = healthy();
        runner.on(
            "get volume settings",
            Reply::ok("output volume:42, input volume:0, output muted:false"),
        );
        let adapter = adapter(Arc::clone(&runner));

        toggle_mute(&adapter, AudioKind::Input, 0)
            .await
            .expect("toggled");
        assert!(
            runner.called_with("set volume input volume 1"),
            "a remembered level of zero must not leave the mic muted"
        );
    }
}
