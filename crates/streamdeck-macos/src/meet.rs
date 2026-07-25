//! Google Meet window focus and PWA launch.
//!
//! Pressing a meeting tile first tries to raise an existing Chrome window or tab,
//! and only opens the configured PWA when there is nothing to raise. Meeting URLs
//! are re-validated here even though the parser already did it, because this is
//! the point at which one is handed to the system.

use std::sync::Arc;

use async_trait::async_trait;
use streamdeck_core::config::ToolsConfig;
use streamdeck_core::integrations::meetings::normalize_meet_url;

use crate::command::CommandRunner;
use crate::{expand_home, timeouts};

#[derive(Debug, thiserror::Error)]
pub enum MeetError {
    #[error(transparent)]
    Command(#[from] crate::command::CommandError),
    #[error("`{0}` is not a Google Meet URL")]
    NotAMeetUrl(String),
    #[error("macOS denied Accessibility access; grant it in Privacy & Security > Accessibility")]
    AccessibilityDenied,
}

/// What happened when a meeting tile was pressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opened {
    /// An existing Chrome window or tab was raised.
    Focused,
    /// The Meet PWA was launched with the meeting URL.
    Launched,
}

#[async_trait]
pub trait MeetLauncher: Send + Sync {
    async fn focus_or_open(&self, meet_url: &str) -> Result<Opened, MeetError>;
}

/// Raises an existing Meet window, then an existing Meet tab, and reports
/// `not-found` if neither exists.
const FOCUS_EXISTING: &str = r#"
if application "Google Chrome" is running then
  tell application "Google Chrome"
    repeat with w in windows
      repeat with t in tabs of w
        try
          if URL of t contains "meet.google.com/" then
            set active tab index of w to (index of t)
            set index of w to 1
            activate
            return "focused"
          end if
        end try
      end repeat
    end repeat
  end tell
end if
return "not-found"
"#;

pub struct SystemMeetLauncher {
    runner: Arc<dyn CommandRunner>,
    tools: ToolsConfig,
    meet_app: String,
}

impl SystemMeetLauncher {
    pub fn new(runner: Arc<dyn CommandRunner>, tools: ToolsConfig, meet_app: &str) -> Self {
        Self {
            runner,
            tools,
            meet_app: expand_home(meet_app),
        }
    }
}

#[async_trait]
impl MeetLauncher for SystemMeetLauncher {
    async fn focus_or_open(&self, meet_url: &str) -> Result<Opened, MeetError> {
        let url = normalize_meet_url(meet_url)
            .ok_or_else(|| MeetError::NotAMeetUrl(meet_url.to_string()))?;

        match self
            .runner
            .run(
                &self.tools.osascript,
                &["-e", FOCUS_EXISTING],
                timeouts::LOCAL,
            )
            .await
        {
            Ok(output) if output.trimmed() == "focused" => return Ok(Opened::Focused),
            Ok(_) => {}
            Err(crate::command::CommandError::Failed { stderr, .. })
                if is_accessibility_error(&stderr) =>
            {
                // Report the specific permission problem, but still open the PWA:
                // the user asked to join a meeting, not to fix permissions.
                tracing::warn!(component = "meet", "accessibility permission denied");
            }
            Err(error) => {
                tracing::debug!(component = "meet", error = %error, "could not raise a window");
            }
        }

        self.runner
            .run(
                &self.tools.open,
                &["-a", &self.meet_app, &url],
                timeouts::LOCAL,
            )
            .await?;
        Ok(Opened::Launched)
    }
}

fn is_accessibility_error(stderr: &str) -> bool {
    stderr.contains("-1728")
        || stderr.contains("-25211")
        || stderr.contains("not allowed assistive access")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{FakeCommandRunner, Reply};

    const URL: &str = "https://meet.google.com/abc-defg-hij";
    /// Identifies the focus script, which no `open` invocation can contain.
    const FOCUS_NEEDLE: &str = "repeat with w in windows";

    fn launcher(runner: Arc<FakeCommandRunner>) -> SystemMeetLauncher {
        SystemMeetLauncher::new(
            runner,
            ToolsConfig::default(),
            "~/Applications/Chrome Apps.localized/Google Meet.app",
        )
    }

    #[tokio::test]
    async fn an_existing_meet_tab_is_raised_without_opening_anything() {
        let runner = Arc::new(FakeCommandRunner::new());
        runner.on(FOCUS_NEEDLE, Reply::ok("focused\n"));

        let opened = launcher(Arc::clone(&runner))
            .focus_or_open(URL)
            .await
            .expect("focused");

        assert_eq!(opened, Opened::Focused);
        assert!(
            !runner.called_with("/usr/bin/open"),
            "nothing should be launched"
        );
    }

    #[tokio::test]
    async fn with_no_window_to_raise_the_pwa_is_opened_with_the_meeting_url() {
        std::env::set_var("HOME", "/Users/tester");
        let runner = Arc::new(FakeCommandRunner::new());
        runner
            .on(FOCUS_NEEDLE, Reply::ok("not-found\n"))
            .on("/usr/bin/open", Reply::ok(""));

        let opened = launcher(Arc::clone(&runner))
            .focus_or_open(URL)
            .await
            .expect("launched");

        assert_eq!(opened, Opened::Launched);
        assert!(runner.called_with(
            "/usr/bin/open -a /Users/tester/Applications/Chrome Apps.localized/Google Meet.app https://meet.google.com/abc-defg-hij"
        ), "{:?}", runner.calls());
    }

    #[tokio::test]
    async fn a_url_that_is_not_on_meet_google_com_is_never_opened() {
        let runner = Arc::new(FakeCommandRunner::new());
        runner.fallback(Reply::ok(""));

        for candidate in [
            "https://evil.example/meet",
            "https://meet.google.com.evil.example/abc",
            "http://meet.google.com/abc",
            "file:///etc/passwd",
        ] {
            let error = launcher(Arc::clone(&runner))
                .focus_or_open(candidate)
                .await
                .expect_err("refused");
            assert!(matches!(error, MeetError::NotAMeetUrl(_)), "{candidate}");
        }
        assert_eq!(runner.call_count(), 0, "nothing should have been run");
    }

    #[tokio::test]
    async fn query_parameters_are_stripped_before_the_url_is_handed_to_the_system() {
        std::env::set_var("HOME", "/Users/tester");
        let runner = Arc::new(FakeCommandRunner::new());
        runner
            .on(FOCUS_NEEDLE, Reply::ok("not-found\n"))
            .on("/usr/bin/open", Reply::ok(""));

        launcher(Arc::clone(&runner))
            .focus_or_open("https://meet.google.com/abc-defg-hij?authuser=2&hs=1")
            .await
            .expect("launched");

        assert!(runner.called_with("https://meet.google.com/abc-defg-hij"));
        assert!(!runner.called_with("authuser"));
    }

    #[tokio::test]
    async fn a_denied_accessibility_grant_still_falls_back_to_opening_the_pwa() {
        let runner = Arc::new(FakeCommandRunner::new());
        runner
            .on(
                FOCUS_NEEDLE,
                Reply::fails(1, "osascript is not allowed assistive access. (-1728)"),
            )
            .on("/usr/bin/open", Reply::ok(""));

        let opened = launcher(Arc::clone(&runner))
            .focus_or_open(URL)
            .await
            .expect("launched");
        assert_eq!(opened, Opened::Launched);
    }

    #[tokio::test]
    async fn a_failure_to_open_the_pwa_is_reported() {
        let runner = Arc::new(FakeCommandRunner::new());
        runner
            .on(FOCUS_NEEDLE, Reply::ok("not-found\n"))
            .on("/usr/bin/open", Reply::fails(1, "app not found"));

        let error = launcher(runner)
            .focus_or_open(URL)
            .await
            .expect_err("fails");
        assert!(matches!(error, MeetError::Command(_)), "{error}");
    }

    #[test]
    fn accessibility_errors_are_recognised_by_code_and_by_message() {
        assert!(is_accessibility_error("error -1728"));
        assert!(is_accessibility_error("not allowed assistive access"));
        assert!(!is_accessibility_error("some other failure"));
    }
}
