//! Pomodoro completion alerts.
//!
//! A completion stays pending until it is acknowledged from the deck, the
//! notification, the alert helper, or the CLI. While it is pending the deck shows
//! an alert state and the configured sound repeats at a non-aggressive interval.
//! The helper process exists only for the duration of the pending completion.

use std::process::Stdio;
use std::sync::Arc;

use streamdeck_core::config::PomodoroConfig;
use streamdeck_core::pomodoro::Phase;
use streamdeck_macos::notify::{completion_message, Notifier};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

/// What the helper reported back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelperOutcome {
    /// The user dismissed the alert.
    Dismissed,
    /// The user asked to start the next phase.
    StartNext,
}

/// Owns everything about the currently pending completion.
pub struct AlertState {
    /// The phase that finished.
    pub phase: Phase,
    /// The phase that will start next, for the helper's primary button.
    pub next_phase: Phase,
    /// Toggled once a second so the deck can flash while the alert is fresh.
    pub flashing: bool,
    helper: Option<Child>,
}

impl AlertState {
    pub fn new(phase: Phase, next_phase: Phase) -> Self {
        Self {
            phase,
            next_phase,
            flashing: true,
            helper: None,
        }
    }

    /// Kills the helper if one is running. Called on acknowledgement and shutdown.
    pub async fn close_helper(&mut self) {
        if let Some(mut helper) = self.helper.take() {
            let _ = helper.kill().await;
            // Reap it so the daemon leaves no zombie behind.
            let _ = helper.wait().await;
        }
    }

    pub fn helper_running(&self) -> bool {
        self.helper.is_some()
    }
}

/// Everything the alert needs from the rest of the daemon.
pub struct AlertContext {
    pub notifier: Arc<dyn Notifier>,
    pub config: PomodoroConfig,
    /// Path to the `streamdeck-alert` binary, when it is installed.
    pub helper_path: Option<std::path::PathBuf>,
}

/// Starts the alert: notification, sound, and optionally the helper window.
///
/// Returns the state to hold while the completion is pending. Failures in any one
/// surface are logged and do not prevent the others.
pub async fn begin(
    context: &AlertContext,
    phase: Phase,
    next_phase: Phase,
    next_minutes: u32,
    outcomes: mpsc::UnboundedSender<HelperOutcome>,
) -> AlertState {
    let mut state = AlertState::new(phase, next_phase);
    let message = completion_message(phase == Phase::Focus, next_minutes, phase_noun(next_phase));

    if let Err(error) = context
        .notifier
        .notify("Pomodoro timer finished", &message, &context.config.sound)
        .await
    {
        tracing::warn!(component = "alert", error = %error, "notification failed");
    }
    if let Err(error) = context.notifier.play_sound(&context.config.sound).await {
        tracing::warn!(component = "alert", error = %error, "alert sound failed");
    }

    if context.config.persistent_alert {
        match spawn_helper(context, next_phase, &message, outcomes) {
            Ok(helper) => state.helper = helper,
            Err(error) => tracing::warn!(
                component = "alert",
                error = %error,
                "could not start the persistent alert helper"
            ),
        }
    }

    state
}

/// The noun the message uses for the phase that is about to start.
pub fn phase_noun(phase: Phase) -> &'static str {
    match phase {
        Phase::Focus => "focus session",
        Phase::ShortBreak => "break",
        Phase::LongBreak => "long break",
    }
}

/// The label for the helper's primary button.
pub fn primary_button(phase: Phase) -> &'static str {
    match phase {
        Phase::Focus => "Start Focus",
        Phase::ShortBreak => "Start Break",
        Phase::LongBreak => "Start Long Break",
    }
}

fn spawn_helper(
    context: &AlertContext,
    next_phase: Phase,
    message: &str,
    outcomes: mpsc::UnboundedSender<HelperOutcome>,
) -> std::io::Result<Option<Child>> {
    let Some(path) = &context.helper_path else {
        return Ok(None);
    };
    if !path.exists() {
        return Ok(None);
    }

    let mut child = Command::new(path)
        .arg("--title")
        .arg("Pomodoro")
        .arg("--message")
        .arg(message)
        .arg("--primary")
        .arg(primary_button(next_phase))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        // If the daemon exits, the helper must not survive it.
        .kill_on_drop(true)
        .spawn()?;

    let stdout = child.stdout.take();
    tokio::spawn(async move {
        use tokio::io::AsyncBufReadExt;
        let Some(stdout) = stdout else { return };
        let mut lines = tokio::io::BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let outcome = match line.trim() {
                "start" => HelperOutcome::StartNext,
                "dismiss" => HelperOutcome::Dismissed,
                _ => continue,
            };
            let _ = outcomes.send(outcome);
            break;
        }
    });

    Ok(Some(child))
}

/// The next instant at which the alert sound should repeat, if repetition is on.
pub fn next_sound_deadline_ms(config: &PomodoroConfig, now_ms: u64) -> Option<u64> {
    (config.repeat_sound_seconds > 0).then(|| now_ms + config.repeat_sound_seconds * 1_000)
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::Mutex;
    use streamdeck_macos::notify::NotifyError;

    #[derive(Default)]
    struct RecordingNotifier {
        notifications: Mutex<Vec<(String, String, String)>>,
        sounds: Mutex<Vec<String>>,
        fail: bool,
    }

    #[async_trait]
    impl Notifier for RecordingNotifier {
        async fn notify(&self, title: &str, message: &str, sound: &str) -> Result<(), NotifyError> {
            if self.fail {
                return Err(NotifyError::UnsafeText);
            }
            self.notifications.lock().expect("lock").push((
                title.to_string(),
                message.to_string(),
                sound.to_string(),
            ));
            Ok(())
        }

        async fn play_sound(&self, sound: &str) -> Result<(), NotifyError> {
            if self.fail {
                return Err(NotifyError::UnsafeText);
            }
            self.sounds.lock().expect("lock").push(sound.to_string());
            Ok(())
        }
    }

    fn context(notifier: Arc<RecordingNotifier>, persistent: bool) -> AlertContext {
        AlertContext {
            notifier,
            config: PomodoroConfig {
                persistent_alert: persistent,
                ..Default::default()
            },
            helper_path: None,
        }
    }

    #[tokio::test]
    async fn beginning_an_alert_notifies_and_plays_the_configured_sound() {
        let notifier = Arc::new(RecordingNotifier::default());
        let (sender, _receiver) = mpsc::unbounded_channel();

        let state = begin(
            &context(Arc::clone(&notifier), false),
            Phase::Focus,
            Phase::ShortBreak,
            5,
            sender,
        )
        .await;

        let notifications = notifier.notifications.lock().expect("lock").clone();
        assert_eq!(notifications.len(), 1);
        assert_eq!(notifications[0].0, "Pomodoro timer finished");
        assert_eq!(
            notifications[0].1,
            "Focus complete. Your 5-minute break is ready."
        );
        assert_eq!(notifications[0].2, "Glass");
        assert_eq!(*notifier.sounds.lock().expect("lock"), vec!["Glass"]);

        assert_eq!(state.phase, Phase::Focus);
        assert_eq!(state.next_phase, Phase::ShortBreak);
        assert!(state.flashing);
        assert!(!state.helper_running());
    }

    #[tokio::test]
    async fn a_break_completion_says_so() {
        let notifier = Arc::new(RecordingNotifier::default());
        let (sender, _receiver) = mpsc::unbounded_channel();

        begin(
            &context(Arc::clone(&notifier), false),
            Phase::ShortBreak,
            Phase::Focus,
            25,
            sender,
        )
        .await;

        let notifications = notifier.notifications.lock().expect("lock").clone();
        assert_eq!(
            notifications[0].1,
            "Break complete. Your 25-minute focus session is ready."
        );
    }

    #[tokio::test]
    async fn a_failing_notification_does_not_prevent_the_alert_state() {
        let notifier = Arc::new(RecordingNotifier {
            fail: true,
            ..Default::default()
        });
        let (sender, _receiver) = mpsc::unbounded_channel();

        let state = begin(
            &context(notifier, false),
            Phase::Focus,
            Phase::ShortBreak,
            5,
            sender,
        )
        .await;
        assert_eq!(state.phase, Phase::Focus);
    }

    #[tokio::test]
    async fn a_missing_helper_binary_is_not_an_error() {
        let notifier = Arc::new(RecordingNotifier::default());
        let (sender, _receiver) = mpsc::unbounded_channel();
        let mut context = context(notifier, true);
        context.helper_path = Some(std::path::PathBuf::from("/nonexistent/streamdeck-alert"));

        let state = begin(&context, Phase::Focus, Phase::ShortBreak, 5, sender).await;
        assert!(!state.helper_running());
    }

    #[tokio::test]
    async fn closing_the_helper_is_safe_when_none_was_started() {
        let mut state = AlertState::new(Phase::Focus, Phase::ShortBreak);
        state.close_helper().await;
        assert!(!state.helper_running());
    }

    #[test]
    fn the_sound_repeat_deadline_honours_the_configuration() {
        let mut config = PomodoroConfig::default();
        assert_eq!(next_sound_deadline_ms(&config, 1_000), Some(31_000));

        config.repeat_sound_seconds = 0;
        assert_eq!(next_sound_deadline_ms(&config, 1_000), None);
    }

    #[test]
    fn phase_nouns_and_buttons_read_naturally() {
        assert_eq!(phase_noun(Phase::Focus), "focus session");
        assert_eq!(phase_noun(Phase::ShortBreak), "break");
        assert_eq!(phase_noun(Phase::LongBreak), "long break");

        assert_eq!(primary_button(Phase::Focus), "Start Focus");
        assert_eq!(primary_button(Phase::LongBreak), "Start Long Break");
    }
}
