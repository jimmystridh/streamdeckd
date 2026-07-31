//! Native frontmost-application discovery and safe lifecycle controls.

use std::sync::Arc;

use async_trait::async_trait;
use streamdeck_core::integrations::application::{ApplicationInfo, CustomApplicationAction};

use crate::command::CommandRunner;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplicationControl {
    Activate,
    Hide,
    Quit,
    ForceQuit,
}

impl ApplicationControl {
    const fn verb(self) -> &'static str {
        match self {
            Self::Activate => "activate",
            Self::Hide => "hide",
            Self::Quit => "quit",
            Self::ForceQuit => "force quit",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ApplicationError {
    #[error("macOS did not report a frontmost application")]
    NoFrontmostApplication,
    #[error("application {0} is no longer running")]
    NotRunning(i32),
    #[error("macOS rejected the request to {action} {application}")]
    Rejected {
        action: &'static str,
        application: String,
    },
    #[error(transparent)]
    Command(#[from] crate::command::CommandError),
    #[error("frontmost-application control is only available on macOS")]
    Unsupported,
}

#[async_trait]
pub trait ApplicationAdapter: Send + Sync {
    async fn frontmost(&self) -> Result<ApplicationInfo, ApplicationError>;
    async fn control(
        &self,
        application: &ApplicationInfo,
        control: ApplicationControl,
    ) -> Result<(), ApplicationError>;
    async fn custom(
        &self,
        application: &ApplicationInfo,
        action: CustomApplicationAction,
    ) -> Result<(), ApplicationError>;
}

pub struct SystemApplicationAdapter {
    runner: Arc<dyn CommandRunner>,
    osascript: String,
}

impl SystemApplicationAdapter {
    pub fn new(runner: Arc<dyn CommandRunner>, osascript: impl Into<String>) -> Self {
        Self {
            runner,
            osascript: osascript.into(),
        }
    }
}

#[cfg(target_os = "macos")]
#[async_trait]
impl ApplicationAdapter for SystemApplicationAdapter {
    async fn frontmost(&self) -> Result<ApplicationInfo, ApplicationError> {
        use objc2_app_kit::{NSRunningApplication, NSWorkspace};

        let application = frontmost_window_pid()
            .and_then(NSRunningApplication::runningApplicationWithProcessIdentifier)
            .or_else(|| NSWorkspace::sharedWorkspace().frontmostApplication())
            .ok_or(ApplicationError::NoFrontmostApplication)?;
        let name = application
            .localizedName()
            .map(|name| name.to_string())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| "Unknown Application".to_string());

        Ok(ApplicationInfo {
            name,
            bundle_id: application
                .bundleIdentifier()
                .map(|identifier| identifier.to_string()),
            pid: application.processIdentifier(),
        })
    }

    async fn control(
        &self,
        application: &ApplicationInfo,
        control: ApplicationControl,
    ) -> Result<(), ApplicationError> {
        use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};

        let running =
            NSRunningApplication::runningApplicationWithProcessIdentifier(application.pid)
                .ok_or(ApplicationError::NotRunning(application.pid))?;
        let accepted = match control {
            ApplicationControl::Activate => {
                running.activateWithOptions(NSApplicationActivationOptions::ActivateAllWindows)
            }
            ApplicationControl::Hide => running.hide(),
            ApplicationControl::Quit => running.terminate(),
            ApplicationControl::ForceQuit => running.forceTerminate(),
        };
        if accepted || control_completed(application.pid, control).await {
            Ok(())
        } else {
            Err(ApplicationError::Rejected {
                action: control.verb(),
                application: application.name.clone(),
            })
        }
    }

    async fn custom(
        &self,
        _application: &ApplicationInfo,
        action: CustomApplicationAction,
    ) -> Result<(), ApplicationError> {
        self.runner
            .run(
                &self.osascript,
                &["-e", custom_script(action)],
                crate::timeouts::LOCAL,
            )
            .await?;
        Ok(())
    }
}

#[cfg(target_os = "macos")]
async fn control_completed(pid: i32, control: ApplicationControl) -> bool {
    use objc2_app_kit::NSRunningApplication;

    match control {
        ApplicationControl::Activate | ApplicationControl::Hide => {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            NSRunningApplication::runningApplicationWithProcessIdentifier(pid).is_some_and(
                |application| match control {
                    ApplicationControl::Activate => application.isActive(),
                    ApplicationControl::Hide => application.isHidden(),
                    _ => false,
                },
            )
        }
        ApplicationControl::Quit | ApplicationControl::ForceQuit => false,
    }
}

#[cfg(not(target_os = "macos"))]
#[async_trait]
impl ApplicationAdapter for SystemApplicationAdapter {
    async fn frontmost(&self) -> Result<ApplicationInfo, ApplicationError> {
        Err(ApplicationError::Unsupported)
    }

    async fn control(
        &self,
        _application: &ApplicationInfo,
        _control: ApplicationControl,
    ) -> Result<(), ApplicationError> {
        Err(ApplicationError::Unsupported)
    }

    async fn custom(
        &self,
        _application: &ApplicationInfo,
        _action: CustomApplicationAction,
    ) -> Result<(), ApplicationError> {
        Err(ApplicationError::Unsupported)
    }
}

#[cfg(target_os = "macos")]
fn frontmost_window_pid() -> Option<i32> {
    use core::ffi::c_void;
    use core_foundation::array::{CFArray, CFArrayRef};
    use core_foundation::base::TCFType;
    use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
    use core_foundation::string::CFStringRef;

    const ON_SCREEN_ONLY: u32 = 1;
    const EXCLUDE_DESKTOP_ELEMENTS: u32 = 16;

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGWindowListCopyWindowInfo(option: u32, relative_to_window: u32) -> CFArrayRef;
        static kCGWindowLayer: CFStringRef;
        static kCGWindowOwnerPID: CFStringRef;
    }

    let raw = unsafe { CGWindowListCopyWindowInfo(ON_SCREEN_ONLY | EXCLUDE_DESKTOP_ELEMENTS, 0) };
    if raw.is_null() {
        return None;
    }
    let windows = unsafe { CFArray::<*const c_void>::wrap_under_create_rule(raw) };
    for raw_window in windows.get_all_values() {
        let window = unsafe {
            CFDictionary::<*const c_void, *const c_void>::wrap_under_get_rule(
                raw_window as CFDictionaryRef,
            )
        };
        let Some(layer) = cf_number(&window, unsafe { kCGWindowLayer }) else {
            continue;
        };
        if layer != 0 {
            continue;
        }
        let Some(pid) = cf_number(&window, unsafe { kCGWindowOwnerPID }) else {
            continue;
        };
        if pid > 0 && pid != std::process::id() as i32 {
            return Some(pid);
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn cf_number(
    dictionary: &core_foundation::dictionary::CFDictionary<
        *const core::ffi::c_void,
        *const core::ffi::c_void,
    >,
    key: core_foundation::string::CFStringRef,
) -> Option<i32> {
    use core_foundation::base::TCFType;
    use core_foundation::number::{CFNumber, CFNumberRef};

    let raw = *dictionary.find(key as *const core::ffi::c_void)?;
    let number = unsafe { CFNumber::wrap_under_get_rule(raw as CFNumberRef) };
    number.to_i32()
}

fn custom_script(action: CustomApplicationAction) -> &'static str {
    use CustomApplicationAction::*;

    match action {
        GhosttyNewWindow => "tell application \"Ghostty\" to new window",
        GhosttyNewTab => "tell application \"Ghostty\" to new tab in front window",
        GhosttySplitRight => {
            "tell application \"Ghostty\" to perform action \"new_split:right\" on focused terminal of selected tab of front window"
        }
        GhosttySplitDown => {
            "tell application \"Ghostty\" to perform action \"new_split:down\" on focused terminal of selected tab of front window"
        }
        GhosttyToggleSplitZoom => {
            "tell application \"Ghostty\" to perform action \"toggle_split_zoom\" on focused terminal of selected tab of front window"
        }
        ChromeNewTab => {
            "tell application \"Google Chrome\" to tell front window\nmake new tab at end of tabs\nset active tab index to count of tabs\nend tell"
        }
        ChromeIncognito => {
            "tell application \"Google Chrome\" to make new window with properties {mode:\"incognito\"}"
        }
        ChromeDownloads => chrome_page_script("chrome://downloads/"),
        ChromeHistory => chrome_page_script("chrome://history/"),
        ChromeExtensions => chrome_page_script("chrome://extensions/"),
        FinderNewWindow => "tell application \"Finder\" to make new Finder window",
        FinderHome => "tell application \"Finder\" to open home",
        FinderDownloads => "tell application \"Finder\" to open folder \"Downloads\" of home",
        FinderApplications => "tell application \"Finder\" to open applications folder",
        FinderAirDrop => "open location \"airdrop:\"",
        SlackNewMessage => {
            "tell application \"Slack\" to activate\ntell application \"System Events\" to tell process \"Slack\" to click menu item \"New Message\" of menu 1 of menu bar item \"File\" of menu bar 1"
        }
        SlackSearch => {
            "tell application \"Slack\" to activate\ntell application \"System Events\" to tell process \"Slack\" to click menu item \"Search\" of menu 1 of menu bar item \"Edit\" of menu bar 1"
        }
        SlackActivity => {
            "tell application \"Slack\" to activate\ntell application \"System Events\"\ntell process \"Slack\" to set frontmost to true\nkeystroke \"m\" using {command down, shift down}\nend tell"
        }
        SlackThreads => {
            "tell application \"Slack\" to activate\ntell application \"System Events\"\ntell process \"Slack\" to set frontmost to true\nkeystroke \"t\" using {command down, shift down}\nend tell"
        }
        SlackDirectMessages => {
            "tell application \"Slack\" to activate\ntell application \"System Events\"\ntell process \"Slack\" to set frontmost to true\nkeystroke \"k\" using {command down, shift down}\nend tell"
        }
    }
}

fn chrome_page_script(url: &str) -> &'static str {
    match url {
        "chrome://downloads/" => {
            "tell application \"Google Chrome\" to tell front window\nmake new tab at end of tabs with properties {URL:\"chrome://downloads/\"}\nset active tab index to count of tabs\nend tell"
        }
        "chrome://history/" => {
            "tell application \"Google Chrome\" to tell front window\nmake new tab at end of tabs with properties {URL:\"chrome://history/\"}\nset active tab index to count of tabs\nend tell"
        }
        "chrome://extensions/" => {
            "tell application \"Google Chrome\" to tell front window\nmake new tab at end of tabs with properties {URL:\"chrome://extensions/\"}\nset active tab index to count of tabs\nend tell"
        }
        _ => unreachable!("only static Chrome pages are accepted"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{FakeCommandRunner, Reply};

    fn adapter(runner: Arc<FakeCommandRunner>) -> SystemApplicationAdapter {
        SystemApplicationAdapter::new(runner, "/usr/bin/osascript")
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn the_live_frontmost_application_has_a_name_and_pid() {
        let application = adapter(Arc::new(FakeCommandRunner::new()))
            .frontmost()
            .await
            .expect("frontmost application");
        assert!(!application.name.trim().is_empty());
        assert!(application.pid > 0);
    }

    #[tokio::test]
    async fn custom_actions_are_fixed_scripts_not_interpolated_application_data() {
        let runner = Arc::new(FakeCommandRunner::new());
        runner.fallback(Reply::ok(""));
        let suspicious = ApplicationInfo {
            name: "Ghostty\" & do shell script \"nope".to_string(),
            bundle_id: Some("com.mitchellh.ghostty".to_string()),
            pid: 42,
        };

        adapter(Arc::clone(&runner))
            .custom(&suspicious, CustomApplicationAction::GhosttySplitRight)
            .await
            .expect("custom action");

        assert!(runner.called_with("perform action \"new_split:right\""));
        assert!(!runner.called_with("do shell script"));
    }

    #[test]
    fn every_custom_action_has_an_app_specific_script() {
        use CustomApplicationAction::*;

        let actions = [
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
        ];
        for action in actions {
            assert!(!custom_script(action).is_empty(), "{action:?}");
        }
    }
}
