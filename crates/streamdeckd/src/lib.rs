//! `streamdeckd` — a headless macOS daemon for a 5x3 Stream Deck.
//!
//! The daemon is a library plus a thin binary so the runtime, the recording
//! device, and the control client can all be driven from integration tests.

pub mod alert;
pub mod control;
pub mod device;
pub mod doctor;
pub mod logging;
pub mod metrics;
pub mod runtime;
pub mod services;

use std::path::PathBuf;

/// Where the daemon looks for its configuration.
pub fn config_path() -> PathBuf {
    std::env::var_os("STREAMDECKD_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| streamdeck_macos::support_dir().join("config.toml"))
}

/// Where the daemon persists its state.
pub fn state_path() -> PathBuf {
    std::env::var_os("STREAMDECKD_STATE")
        .map(PathBuf::from)
        .unwrap_or_else(|| streamdeck_macos::support_dir().join("state.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_config_path_honours_the_environment_override() {
        std::env::set_var("STREAMDECKD_CONFIG", "/tmp/streamdeckd-test.toml");
        assert_eq!(config_path(), PathBuf::from("/tmp/streamdeckd-test.toml"));
        std::env::remove_var("STREAMDECKD_CONFIG");

        std::env::set_var("HOME", "/Users/tester");
        assert_eq!(
            config_path(),
            PathBuf::from("/Users/tester/Library/Application Support/streamdeckd/config.toml")
        );
    }

    #[test]
    fn the_state_path_honours_the_environment_override() {
        std::env::set_var("STREAMDECKD_STATE", "/tmp/streamdeckd-state.json");
        assert_eq!(state_path(), PathBuf::from("/tmp/streamdeckd-state.json"));
        std::env::remove_var("STREAMDECKD_STATE");
    }
}
