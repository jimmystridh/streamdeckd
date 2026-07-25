//! Spotify adapter.
//!
//! One short-lived AppleScript reads the whole player state in a single
//! invocation, so a visible page polls once per tick rather than once per tile.
//! No permanent watcher process is ever started.

use std::sync::Arc;

use async_trait::async_trait;
use streamdeck_core::config::ToolsConfig;
use streamdeck_core::integrations::spotify::{self, RepeatMode, SpotifyStatus};

use crate::command::CommandRunner;
use crate::timeouts;

#[derive(Debug, thiserror::Error)]
pub enum SpotifyError {
    #[error(transparent)]
    Command(#[from] crate::command::CommandError),
    #[error(transparent)]
    Parse(#[from] streamdeck_core::integrations::ParseError),
    #[error("Spotify is not running")]
    NotRunning,
    #[error("macOS denied automation access to Spotify; grant it in Privacy & Security")]
    PermissionDenied,
}

/// Reads the whole player state as one tab-separated line. Field order matches
/// [`streamdeck_core::integrations::spotify::parse_status`].
const READ_STATUS: &str = r#"
if application "Spotify" is running then
  tell application "Spotify"
    set state to player state as text
    try
      set t to name of current track
      set a to artist of current track
      set al to album of current track
      set art to artwork url of current track
      set tid to id of current track
    on error
      set t to ""
      set a to ""
      set al to ""
      set art to ""
      set tid to ""
    end try
    set v to sound volume as text
    set sh to (shuffling as text)
    try
      set rp to (repeating as text)
    on error
      set rp to "false"
    end try
    return state & tab & t & tab & a & tab & al & tab & art & tab & tid & tab & v & tab & sh & tab & rp
  end tell
else
  return "not-running"
end if
"#;

/// The player controls the Spotify page exposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    PlayPause,
    Next,
    Previous,
    ToggleShuffle,
    ToggleRepeat,
    /// Absolute volume, already clamped.
    SetVolume(u8),
}

#[async_trait]
pub trait SpotifyAdapter: Send + Sync {
    async fn status(&self) -> Result<SpotifyStatus, SpotifyError>;
    async fn control(&self, control: Control) -> Result<(), SpotifyError>;
    /// Opens or focuses the application. Works even when it is not running.
    async fn open(&self) -> Result<(), SpotifyError>;
}

pub struct AppleScriptSpotifyAdapter {
    runner: Arc<dyn CommandRunner>,
    tools: ToolsConfig,
}

impl AppleScriptSpotifyAdapter {
    pub fn new(runner: Arc<dyn CommandRunner>, tools: ToolsConfig) -> Self {
        Self { runner, tools }
    }

    async fn osascript(&self, script: &str) -> Result<String, SpotifyError> {
        match self
            .runner
            .run(&self.tools.osascript, &["-e", script], timeouts::LOCAL)
            .await
        {
            Ok(output) => Ok(output.stdout),
            Err(crate::command::CommandError::Failed {
                stderr,
                code,
                program,
            }) if is_permission_error(&stderr) => {
                tracing::warn!(
                    component = "spotify",
                    code,
                    program,
                    "automation permission denied"
                );
                Err(SpotifyError::PermissionDenied)
            }
            Err(error) => Err(error.into()),
        }
    }
}

/// AppleScript reports a denied Automation grant as error -1743.
fn is_permission_error(stderr: &str) -> bool {
    stderr.contains("-1743") || stderr.contains("Not authorized to send Apple events")
}

#[async_trait]
impl SpotifyAdapter for AppleScriptSpotifyAdapter {
    async fn status(&self) -> Result<SpotifyStatus, SpotifyError> {
        let stdout = self.osascript(READ_STATUS).await?;
        Ok(spotify::parse_status(&stdout)?)
    }

    async fn control(&self, control: Control) -> Result<(), SpotifyError> {
        let script = match control {
            Control::PlayPause => "tell application \"Spotify\" to playpause".to_string(),
            Control::Next => "tell application \"Spotify\" to next track".to_string(),
            Control::Previous => "tell application \"Spotify\" to previous track".to_string(),
            Control::ToggleShuffle => {
                "tell application \"Spotify\" to set shuffling to not shuffling".to_string()
            }
            Control::ToggleRepeat => {
                "tell application \"Spotify\" to set repeating to not repeating".to_string()
            }
            Control::SetVolume(volume) => format!(
                "tell application \"Spotify\" to set sound volume to {}",
                volume.min(100)
            ),
        };

        // Guard every control so a press while Spotify is closed is a clear error
        // rather than an AppleScript failure that launches the application.
        let guarded = format!(
            "if application \"Spotify\" is running then\n{script}\nelse\nreturn \"not-running\"\nend if"
        );
        let stdout = self.osascript(&guarded).await?;
        if stdout.trim() == "not-running" {
            return Err(SpotifyError::NotRunning);
        }
        Ok(())
    }

    async fn open(&self) -> Result<(), SpotifyError> {
        self.runner
            .run(&self.tools.open, &["-a", "Spotify"], timeouts::LOCAL)
            .await?;
        Ok(())
    }
}

/// The next repeat mode a toggle produces, for the optimistic tile update.
pub fn next_repeat(current: RepeatMode) -> RepeatMode {
    match current {
        RepeatMode::Off => RepeatMode::All,
        _ => RepeatMode::Off,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{FakeCommandRunner, Reply};
    use streamdeck_core::integrations::spotify::PlayerState;

    const PLAYING: &str = "playing\tTruth\tKamasi Washington\tThe Epic\thttps://i.scdn.co/image/abc\tspotify:track:1\t72\ttrue\tall\n";

    fn adapter(runner: Arc<FakeCommandRunner>) -> AppleScriptSpotifyAdapter {
        AppleScriptSpotifyAdapter::new(runner, ToolsConfig::default())
    }

    #[tokio::test]
    async fn one_invocation_reads_the_whole_player_state() {
        let runner = Arc::new(FakeCommandRunner::new());
        runner.on("player state", Reply::ok(PLAYING));

        let status = adapter(Arc::clone(&runner)).status().await.expect("status");

        assert_eq!(status.state, PlayerState::Playing);
        assert_eq!(status.track, "Truth");
        assert_eq!(status.volume, 72);
        assert_eq!(
            runner.call_count(),
            1,
            "a tick must not spawn one process per tile"
        );
    }

    #[tokio::test]
    async fn a_closed_application_is_reported_as_not_running() {
        let runner = Arc::new(FakeCommandRunner::new());
        runner.on("player state", Reply::ok("not-running\n"));

        let status = adapter(runner).status().await.expect("status");
        assert_eq!(status.state, PlayerState::NotRunning);
        assert!(!status.is_available());
    }

    #[tokio::test]
    async fn every_control_sends_exactly_one_guarded_script() {
        let cases = [
            (Control::PlayPause, "playpause"),
            (Control::Next, "next track"),
            (Control::Previous, "previous track"),
            (Control::ToggleShuffle, "set shuffling to not shuffling"),
            (Control::ToggleRepeat, "set repeating to not repeating"),
            (Control::SetVolume(65), "set sound volume to 65"),
        ];

        for (control, expected) in cases {
            let runner = Arc::new(FakeCommandRunner::new());
            runner.fallback(Reply::ok(""));
            adapter(Arc::clone(&runner))
                .control(control)
                .await
                .expect("control");

            assert_eq!(runner.call_count(), 1, "{control:?}");
            assert!(
                runner.called_with(expected),
                "{control:?} sent no {expected}"
            );
            assert!(
                runner.called_with("if application \"Spotify\" is running then"),
                "{control:?} was not guarded"
            );
        }
    }

    #[tokio::test]
    async fn volumes_are_clamped_before_they_reach_applescript() {
        let runner = Arc::new(FakeCommandRunner::new());
        runner.fallback(Reply::ok(""));
        adapter(Arc::clone(&runner))
            .control(Control::SetVolume(250))
            .await
            .expect("control");
        assert!(runner.called_with("set sound volume to 100"));
    }

    #[tokio::test]
    async fn a_control_press_while_spotify_is_closed_is_a_clear_error() {
        let runner = Arc::new(FakeCommandRunner::new());
        runner.fallback(Reply::ok("not-running\n"));

        let error = adapter(runner)
            .control(Control::PlayPause)
            .await
            .expect_err("not running");
        assert!(matches!(error, SpotifyError::NotRunning), "{error}");
    }

    #[tokio::test]
    async fn a_denied_automation_grant_is_its_own_diagnostic() {
        let runner = Arc::new(FakeCommandRunner::new());
        runner.fallback(Reply::fails(
            1,
            "execution error: Not authorized to send Apple events to Spotify. (-1743)",
        ));

        let error = adapter(runner).status().await.expect_err("denied");
        assert!(matches!(error, SpotifyError::PermissionDenied), "{error}");
        assert!(error.to_string().contains("Privacy & Security"), "{error}");
    }

    #[tokio::test]
    async fn opening_the_application_uses_open_and_works_when_it_is_closed() {
        let runner = Arc::new(FakeCommandRunner::new());
        runner.fallback(Reply::ok(""));
        adapter(Arc::clone(&runner)).open().await.expect("opened");
        assert!(runner.called_with("/usr/bin/open -a Spotify"));
    }

    #[tokio::test]
    async fn a_malformed_status_line_is_a_parse_error() {
        let runner = Arc::new(FakeCommandRunner::new());
        runner.on("player state", Reply::ok("playing\tonly-two\n"));

        let error = adapter(runner).status().await.expect_err("malformed");
        assert!(matches!(error, SpotifyError::Parse(_)), "{error}");
    }

    #[test]
    fn repeat_toggles_between_off_and_all() {
        assert_eq!(next_repeat(RepeatMode::Off), RepeatMode::All);
        assert_eq!(next_repeat(RepeatMode::All), RepeatMode::Off);
        assert_eq!(next_repeat(RepeatMode::One), RepeatMode::Off);
    }
}
