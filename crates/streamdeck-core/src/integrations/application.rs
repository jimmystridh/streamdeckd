//! The frontmost macOS application and the actions available for its context page.

use serde::{Deserialize, Serialize};

use crate::model::PageId;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplicationInfo {
    pub name: String,
    pub bundle_id: Option<String>,
    pub pid: i32,
}

impl ApplicationInfo {
    pub fn kind(&self) -> ApplicationKind {
        let bundle = self.bundle_id.as_deref().unwrap_or_default();
        let name = self.name.to_ascii_lowercase();

        if bundle.starts_with("com.spotify.client") || name == "spotify" {
            ApplicationKind::Spotify
        } else if bundle.starts_with("com.electron.wispr-flow") || name.contains("wispr flow") {
            ApplicationKind::Wispr
        } else if name.contains("google meet") {
            ApplicationKind::Meet
        } else if bundle == "com.mitchellh.ghostty" || name == "ghostty" {
            ApplicationKind::Ghostty
        } else if bundle == "com.google.Chrome" || name == "google chrome" {
            ApplicationKind::Chrome
        } else if bundle == "com.apple.finder" || name == "finder" {
            ApplicationKind::Finder
        } else if bundle == "com.tinyspeck.slackmacgap" || name == "slack" {
            ApplicationKind::Slack
        } else if is_browser(bundle, &name) {
            ApplicationKind::Browser
        } else {
            ApplicationKind::Other
        }
    }

    pub fn context_action(&self, slot: usize) -> ContextAction {
        use ContextAction::*;

        match self.kind() {
            ApplicationKind::Spotify => match slot {
                0 => SpotifyPrevious,
                1 => SpotifyPlayPause,
                2 => SpotifyNext,
                3 => SpotifySeek(-15),
                4 => SpotifySeek(15),
                _ => None,
            },
            ApplicationKind::Wispr => match slot {
                0 => WisprToggle,
                1..=3 => WisprMicrophone(slot - 1),
                4 => Navigate(PageId::Wispr),
                _ => None,
            },
            ApplicationKind::Meet => match slot {
                0 | 1 => OpenMeeting(slot),
                2 => WisprToggle,
                3 => Navigate(PageId::Mixer),
                4 => Navigate(PageId::Wispr),
                _ => None,
            },
            ApplicationKind::Ghostty => match slot {
                0 => Custom(CustomApplicationAction::GhosttyNewWindow),
                1 => Custom(CustomApplicationAction::GhosttyNewTab),
                2 => Custom(CustomApplicationAction::GhosttySplitRight),
                3 => Custom(CustomApplicationAction::GhosttySplitDown),
                4 => Custom(CustomApplicationAction::GhosttyToggleSplitZoom),
                _ => None,
            },
            ApplicationKind::Chrome => match slot {
                0 => Custom(CustomApplicationAction::ChromeNewTab),
                1 => Custom(CustomApplicationAction::ChromeIncognito),
                2 => Custom(CustomApplicationAction::ChromeDownloads),
                3 => Custom(CustomApplicationAction::ChromeHistory),
                4 => Custom(CustomApplicationAction::ChromeExtensions),
                _ => None,
            },
            ApplicationKind::Finder => match slot {
                0 => Custom(CustomApplicationAction::FinderNewWindow),
                1 => Custom(CustomApplicationAction::FinderHome),
                2 => Custom(CustomApplicationAction::FinderDownloads),
                3 => Custom(CustomApplicationAction::FinderApplications),
                4 => Custom(CustomApplicationAction::FinderAirDrop),
                _ => None,
            },
            ApplicationKind::Slack => match slot {
                0 => Custom(CustomApplicationAction::SlackNewMessage),
                1 => Custom(CustomApplicationAction::SlackSearch),
                2 => Custom(CustomApplicationAction::SlackActivity),
                3 => Custom(CustomApplicationAction::SlackThreads),
                4 => Custom(CustomApplicationAction::SlackDirectMessages),
                _ => None,
            },
            ApplicationKind::Browser | ApplicationKind::Other => None,
        }
    }

    pub fn same_application(&self, other: &Self) -> bool {
        match (&self.bundle_id, &other.bundle_id) {
            (Some(left), Some(right)) => left == right,
            _ => self.pid == other.pid || self.name == other.name,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplicationKind {
    Spotify,
    Wispr,
    Meet,
    Ghostty,
    Chrome,
    Finder,
    Slack,
    Browser,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CustomApplicationAction {
    GhosttyNewWindow,
    GhosttyNewTab,
    GhosttySplitRight,
    GhosttySplitDown,
    GhosttyToggleSplitZoom,
    ChromeNewTab,
    ChromeIncognito,
    ChromeDownloads,
    ChromeHistory,
    ChromeExtensions,
    FinderNewWindow,
    FinderHome,
    FinderDownloads,
    FinderApplications,
    FinderAirDrop,
    SlackNewMessage,
    SlackSearch,
    SlackActivity,
    SlackThreads,
    SlackDirectMessages,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextAction {
    None,
    SpotifyPrevious,
    SpotifyPlayPause,
    SpotifyNext,
    SpotifySeek(i32),
    WisprToggle,
    WisprMicrophone(usize),
    OpenMeeting(usize),
    Navigate(PageId),
    Custom(CustomApplicationAction),
}

fn is_browser(bundle: &str, name: &str) -> bool {
    bundle.starts_with("com.google.Chrome")
        || bundle.starts_with("com.apple.Safari")
        || bundle.starts_with("org.mozilla.firefox")
        || bundle.starts_with("com.brave.Browser")
        || bundle.starts_with("company.thebrowser.Browser")
        || bundle.starts_with("com.microsoft.edgemac")
        || ["chrome", "safari", "firefox", "brave", "arc", "edge"]
            .iter()
            .any(|browser| name.contains(browser))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(name: &str, bundle_id: &str) -> ApplicationInfo {
        ApplicationInfo {
            name: name.to_string(),
            bundle_id: Some(bundle_id.to_string()),
            pid: 42,
        }
    }

    #[test]
    fn known_applications_get_their_custom_action_sets() {
        let spotify = app("Spotify", "com.spotify.client");
        assert_eq!(spotify.kind(), ApplicationKind::Spotify);
        assert_eq!(spotify.context_action(3), ContextAction::SpotifySeek(-15));

        let wispr = app("Wispr Flow", "com.electron.wispr-flow");
        assert_eq!(wispr.kind(), ApplicationKind::Wispr);
        assert_eq!(wispr.context_action(2), ContextAction::WisprMicrophone(1));

        let meet = app("Google Meet", "com.google.Chrome.app.meet");
        assert_eq!(meet.kind(), ApplicationKind::Meet);
        assert_eq!(meet.context_action(0), ContextAction::OpenMeeting(0));

        let ghostty = app("Ghostty", "com.mitchellh.ghostty");
        assert_eq!(ghostty.kind(), ApplicationKind::Ghostty);
        assert_eq!(
            ghostty.context_action(3),
            ContextAction::Custom(CustomApplicationAction::GhosttySplitDown)
        );

        let chrome = app("Google Chrome", "com.google.Chrome");
        assert_eq!(chrome.kind(), ApplicationKind::Chrome);
        assert_eq!(
            chrome.context_action(2),
            ContextAction::Custom(CustomApplicationAction::ChromeDownloads)
        );

        let finder = app("Finder", "com.apple.finder");
        assert_eq!(finder.kind(), ApplicationKind::Finder);
        assert_eq!(
            finder.context_action(4),
            ContextAction::Custom(CustomApplicationAction::FinderAirDrop)
        );

        let slack = app("Slack", "com.tinyspeck.slackmacgap");
        assert_eq!(slack.kind(), ApplicationKind::Slack);
        assert_eq!(
            slack.context_action(2),
            ContextAction::Custom(CustomApplicationAction::SlackActivity)
        );
    }

    #[test]
    fn unknown_browsers_and_other_apps_do_not_pretend_media_controls_are_contextual() {
        let browser = app("Safari", "com.apple.Safari");
        assert_eq!(browser.kind(), ApplicationKind::Browser);
        assert_eq!(browser.context_action(1), ContextAction::None);

        let notes = app("Notes", "com.apple.Notes");
        assert_eq!(notes.kind(), ApplicationKind::Other);
        assert_eq!(notes.context_action(4), ContextAction::None);
        assert_eq!(notes.context_action(99), ContextAction::None);
    }

    #[test]
    fn application_identity_prefers_the_stable_bundle_identifier() {
        let old = app("Ghostty", "com.mitchellh.ghostty");
        let mut relaunched = old.clone();
        relaunched.pid = 99;
        assert!(old.same_application(&relaunched));

        let chrome = app("Google Chrome", "com.google.Chrome");
        assert!(!old.same_application(&chrome));
    }
}
