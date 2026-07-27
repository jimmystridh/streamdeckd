//! The macOS system media session shown on the generic media page.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaStatus {
    /// The application registered with macOS as the current media-key owner.
    pub application: Option<String>,
    /// A friendlier service name when it can be resolved, such as YouTube.
    pub source: Option<String>,
    /// The current item title published through macOS Now Playing.
    pub title: Option<String>,
}

impl MediaStatus {
    pub fn inactive() -> Self {
        Self::default()
    }

    pub fn is_active(&self) -> bool {
        self.application.is_some() || self.source.is_some() || self.title.is_some()
    }

    pub fn display_source(&self) -> Option<&str> {
        self.source
            .as_deref()
            .or(self.application.as_deref())
            .or_else(|| self.title.as_ref().map(|_| "System Media"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_service_name_wins_over_the_host_application() {
        let status = MediaStatus {
            application: Some("Google Chrome".to_string()),
            source: Some("YouTube".to_string()),
            title: Some("A video".to_string()),
        };

        assert!(status.is_active());
        assert_eq!(status.display_source(), Some("YouTube"));
    }

    #[test]
    fn an_empty_status_is_an_inactive_session() {
        let status = MediaStatus::inactive();
        assert!(!status.is_active());
        assert_eq!(status.display_source(), None);
    }

    #[test]
    fn metadata_without_an_owner_is_still_an_active_system_session() {
        let status = MediaStatus {
            application: None,
            source: None,
            title: Some("A video".to_string()),
        };

        assert!(status.is_active());
        assert_eq!(status.display_source(), Some("System Media"));
    }
}
