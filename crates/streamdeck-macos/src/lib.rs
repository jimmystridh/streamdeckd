//! macOS adapters.
//!
//! Each adapter sits behind a narrow trait so the daemon can be tested with fakes
//! and so a command-based implementation can later be replaced by a native one
//! without any caller noticing.

pub mod audio;
pub mod command;
pub mod credentials;
pub mod fake;
pub mod meet;
pub mod notify;
pub mod spotify;

pub use command::{CommandError, CommandRunner, Output, SystemCommandRunner};

use std::time::Duration;

/// Timeouts for the external tools, chosen from the plan's guidance.
pub mod timeouts {
    use std::time::Duration;

    /// Local commands: audio switching, AppleScript, `open`.
    pub const LOCAL: Duration = Duration::from_secs(10);
    /// A `gh search` call.
    pub const GITHUB: Duration = Duration::from_secs(30);
    /// A `gog calendar events` call.
    pub const CALENDAR: Duration = Duration::from_secs(25);
    /// Playing an alert sound.
    pub const SOUND: Duration = Duration::from_secs(5);
}

/// Expands a leading `~` against the current home directory.
pub fn expand_home(path: &str) -> String {
    match path.strip_prefix("~/") {
        Some(rest) => match std::env::var_os("HOME") {
            Some(home) => format!("{}/{rest}", home.to_string_lossy()),
            None => path.to_string(),
        },
        None => path.to_string(),
    }
}

/// The application-support directory the daemon owns.
pub fn support_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(expand_home("~/Library/Application Support/streamdeckd"))
}

pub fn log_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(expand_home("~/Library/Logs/streamdeckd"))
}

/// Default control-socket path. Inside the user's application-support directory so
/// it inherits that directory's ownership.
pub fn socket_path() -> std::path::PathBuf {
    support_dir().join("streamdeckd.sock")
}

/// Wraps a duration in a human label for logs.
pub fn describe(duration: Duration) -> String {
    if duration.as_secs() >= 60 {
        format!("{}m{}s", duration.as_secs() / 60, duration.as_secs() % 60)
    } else if duration.as_secs() >= 1 {
        format!("{}s", duration.as_secs())
    } else {
        format!("{}ms", duration.as_millis())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_expansion_only_touches_a_leading_tilde() {
        std::env::set_var("HOME", "/Users/tester");
        assert_eq!(
            expand_home("~/Applications/x.app"),
            "/Users/tester/Applications/x.app"
        );
        assert_eq!(expand_home("/usr/bin/open"), "/usr/bin/open");
        assert_eq!(expand_home("relative/~/path"), "relative/~/path");
        assert_eq!(expand_home("~"), "~", "a bare tilde is not a path");
    }

    #[test]
    fn the_owned_directories_live_under_the_users_library() {
        std::env::set_var("HOME", "/Users/tester");
        assert_eq!(
            support_dir().to_string_lossy(),
            "/Users/tester/Library/Application Support/streamdeckd"
        );
        assert!(socket_path().starts_with(support_dir()));
        assert!(log_dir().to_string_lossy().ends_with("Logs/streamdeckd"));
    }

    #[test]
    fn durations_describe_themselves_compactly() {
        assert_eq!(describe(Duration::from_millis(250)), "250ms");
        assert_eq!(describe(Duration::from_secs(9)), "9s");
        assert_eq!(describe(Duration::from_secs(95)), "1m35s");
    }
}
