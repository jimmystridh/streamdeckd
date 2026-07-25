//! macOS notifications and alert sounds.

use std::sync::Arc;

use async_trait::async_trait;
use streamdeck_core::config::ToolsConfig;

use crate::command::CommandRunner;
use crate::timeouts;

#[derive(Debug, thiserror::Error)]
pub enum NotifyError {
    #[error(transparent)]
    Command(#[from] crate::command::CommandError),
    #[error("notification text contained a character that cannot be sent safely")]
    UnsafeText,
}

#[async_trait]
pub trait Notifier: Send + Sync {
    async fn notify(&self, title: &str, message: &str, sound: &str) -> Result<(), NotifyError>;
    async fn play_sound(&self, sound: &str) -> Result<(), NotifyError>;
}

pub struct SystemNotifier {
    runner: Arc<dyn CommandRunner>,
    tools: ToolsConfig,
}

impl SystemNotifier {
    pub fn new(runner: Arc<dyn CommandRunner>, tools: ToolsConfig) -> Self {
        Self { runner, tools }
    }
}

#[async_trait]
impl Notifier for SystemNotifier {
    async fn notify(&self, title: &str, message: &str, sound: &str) -> Result<(), NotifyError> {
        let script = format!(
            "display notification \"{}\" with title \"{}\" sound name \"{}\"",
            applescript_string(message)?,
            applescript_string(title)?,
            sound_name(sound)?
        );
        self.runner
            .run(&self.tools.osascript, &["-e", &script], timeouts::LOCAL)
            .await?;
        Ok(())
    }

    async fn play_sound(&self, sound: &str) -> Result<(), NotifyError> {
        let path = sound_path(sound)?;
        self.runner
            .run(&self.tools.afplay, &[&path], timeouts::SOUND)
            .await?;
        Ok(())
    }
}

/// Escapes a string for an AppleScript literal. Refuses anything that could break
/// out of the quotes even after escaping.
pub fn applescript_string(value: &str) -> Result<String, NotifyError> {
    if value
        .chars()
        .any(|character| character.is_control() && character != '\t')
    {
        return Err(NotifyError::UnsafeText);
    }
    Ok(value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// macOS system sounds are referenced by bare alphanumeric name.
fn sound_name(sound: &str) -> Result<&str, NotifyError> {
    if sound.is_empty()
        || sound.len() > 32
        || !sound
            .chars()
            .all(|character| character.is_ascii_alphanumeric())
    {
        return Err(NotifyError::UnsafeText);
    }
    Ok(sound)
}

/// The on-disk path for a system sound.
pub fn sound_path(sound: &str) -> Result<String, NotifyError> {
    Ok(format!(
        "/System/Library/Sounds/{}.aiff",
        sound_name(sound)?
    ))
}

/// The completion messages, kept here so the notification, the alert helper, and
/// the deck all say the same thing.
pub fn completion_message(finished_focus: bool, next_minutes: u32, next_label: &str) -> String {
    if finished_focus {
        format!("Focus complete. Your {next_minutes}-minute {next_label} is ready.")
    } else {
        format!("Break complete. Your {next_minutes}-minute {next_label} is ready.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{FakeCommandRunner, Reply};

    fn notifier(runner: Arc<FakeCommandRunner>) -> SystemNotifier {
        SystemNotifier::new(runner, ToolsConfig::default())
    }

    #[tokio::test]
    async fn a_notification_is_sent_as_one_applescript_argument() {
        let runner = Arc::new(FakeCommandRunner::new());
        runner.fallback(Reply::ok(""));

        notifier(Arc::clone(&runner))
            .notify("Pomodoro timer finished", "Focus complete.", "Glass")
            .await
            .expect("notified");

        assert_eq!(runner.call_count(), 1);
        assert!(runner.called_with("display notification \"Focus complete.\""));
        assert!(runner.called_with("with title \"Pomodoro timer finished\""));
        assert!(runner.called_with("sound name \"Glass\""));
    }

    #[tokio::test]
    async fn quotes_and_backslashes_in_a_message_are_escaped() {
        let runner = Arc::new(FakeCommandRunner::new());
        runner.fallback(Reply::ok(""));

        notifier(Arc::clone(&runner))
            .notify("Title", r#"He said "go" \ now"#, "Glass")
            .await
            .expect("notified");

        let call = runner.calls().into_iter().next().expect("one call");
        let script = call.args.last().expect("script");
        assert!(script.contains(r#"\"go\""#), "{script}");
        assert!(script.contains(r"\\"), "{script}");
    }

    #[tokio::test]
    async fn a_message_that_could_break_out_of_the_script_is_refused() {
        let runner = Arc::new(FakeCommandRunner::new());
        runner.fallback(Reply::ok(""));

        let error = notifier(Arc::clone(&runner))
            .notify(
                "Title",
                "line one\nend tell\ndo shell script \"rm -rf ~\"",
                "Glass",
            )
            .await
            .expect_err("refused");

        assert!(matches!(error, NotifyError::UnsafeText), "{error}");
        assert_eq!(runner.call_count(), 0, "nothing should have been run");
    }

    #[tokio::test]
    async fn a_sound_name_that_is_not_a_bare_identifier_is_refused() {
        let runner = Arc::new(FakeCommandRunner::new());
        runner.fallback(Reply::ok(""));

        for sound in [
            "",
            "../../etc/passwd",
            "Glass\" evil",
            "a".repeat(40).as_str(),
        ] {
            let error = notifier(Arc::clone(&runner))
                .notify("Title", "Message", sound)
                .await
                .expect_err("refused");
            assert!(matches!(error, NotifyError::UnsafeText), "{sound}");
        }
    }

    #[tokio::test]
    async fn playing_a_sound_uses_the_system_sounds_directory() {
        let runner = Arc::new(FakeCommandRunner::new());
        runner.fallback(Reply::ok(""));

        notifier(Arc::clone(&runner))
            .play_sound("Glass")
            .await
            .expect("played");
        assert!(runner.called_with("/usr/bin/afplay /System/Library/Sounds/Glass.aiff"));
    }

    #[test]
    fn sound_paths_are_built_only_from_safe_names() {
        assert_eq!(
            sound_path("Glass").expect("safe"),
            "/System/Library/Sounds/Glass.aiff"
        );
        assert!(sound_path("../evil").is_err());
    }

    #[test]
    fn completion_messages_name_the_next_phase_and_its_length() {
        assert_eq!(
            completion_message(true, 5, "break"),
            "Focus complete. Your 5-minute break is ready."
        );
        assert_eq!(
            completion_message(false, 25, "focus session"),
            "Break complete. Your 25-minute focus session is ready."
        );
    }

    #[test]
    fn tabs_survive_escaping_because_they_cannot_end_a_statement() {
        assert_eq!(applescript_string("a\tb").expect("safe"), "a\tb");
    }
}
