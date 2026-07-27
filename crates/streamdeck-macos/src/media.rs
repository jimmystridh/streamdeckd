//! Generic macOS media-session controls.
//!
//! MediaRemote is the service macOS itself uses for the keyboard transport keys
//! and Control Center. It routes commands to the current owner whether that is
//! Spotify, Music, a browser, or another media-session client.

use async_trait::async_trait;
use streamdeck_core::integrations::media::MediaStatus;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    PlayPause,
    Next,
    Previous,
}

#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("the macOS media-session service is unavailable: {0}")]
    Unavailable(String),
    #[error("the macOS media session did not answer")]
    Timeout,
    #[error("the current media application rejected {0}")]
    Rejected(&'static str),
}

#[async_trait]
pub trait MediaAdapter: Send + Sync {
    async fn status(&self) -> Result<MediaStatus, MediaError>;
    async fn control(&self, control: Control) -> Result<(), MediaError>;
}

/// Deterministic adapter for tests and non-interactive previews.
pub struct InactiveMediaAdapter;

#[async_trait]
impl MediaAdapter for InactiveMediaAdapter {
    async fn status(&self) -> Result<MediaStatus, MediaError> {
        Ok(MediaStatus::inactive())
    }

    async fn control(&self, _control: Control) -> Result<(), MediaError> {
        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use std::ffi::{c_void, CStr};
    use std::mem::size_of;
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::Duration;

    use async_trait::async_trait;
    use block2::{Block, RcBlock};
    use core_foundation::base::{CFType, TCFType};
    use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
    use core_foundation::string::{CFString, CFStringRef};
    use dispatch2::{DispatchQoS, DispatchQueue, GlobalQueueIdentifier};
    use objc2_app_kit::NSRunningApplication;
    use streamdeck_core::config::ToolsConfig;
    use tokio::sync::oneshot;

    use super::{Control, MediaAdapter, MediaError, MediaStatus};
    use crate::{timeouts, CommandRunner};

    const MEDIA_REMOTE_PATH: &CStr =
        c"/System/Library/PrivateFrameworks/MediaRemote.framework/MediaRemote";
    const CALLBACK_TIMEOUT: Duration = Duration::from_secs(2);

    type GetPid = unsafe extern "C" fn(&DispatchQueue, &Block<dyn Fn(i32)>);
    type GetInfo = unsafe extern "C" fn(&DispatchQueue, &Block<dyn Fn(*const c_void)>);
    type SendCommand = unsafe extern "C" fn(i32, *const c_void) -> bool;
    type RegisterNotifications = unsafe extern "C" fn(&DispatchQueue);
    type SetCanBeNowPlaying = unsafe extern "C" fn(u8);

    #[derive(Clone, Copy)]
    struct Symbols {
        get_pid: GetPid,
        get_info: GetInfo,
        send_command: SendCommand,
        register_notifications: Option<RegisterNotifications>,
        set_can_be_now_playing: Option<SetCanBeNowPlaying>,
    }

    impl Symbols {
        fn load() -> Result<Self, String> {
            static SYMBOLS: OnceLock<Result<Symbols, String>> = OnceLock::new();
            SYMBOLS
                .get_or_init(|| unsafe {
                    let handle = libc::dlopen(
                        MEDIA_REMOTE_PATH.as_ptr(),
                        libc::RTLD_NOW | libc::RTLD_GLOBAL,
                    );
                    if handle.is_null() {
                        return Err(dl_error());
                    }

                    Ok(Symbols {
                        get_pid: load_symbol(handle, c"MRMediaRemoteGetNowPlayingApplicationPID")?,
                        get_info: load_symbol(handle, c"MRMediaRemoteGetNowPlayingInfo")?,
                        send_command: load_symbol(handle, c"MRMediaRemoteSendCommand")?,
                        register_notifications: load_optional_symbol(
                            handle,
                            c"MRMediaRemoteRegisterForNowPlayingNotifications",
                        ),
                        set_can_be_now_playing: load_optional_symbol(
                            handle,
                            c"MRMediaRemoteSetCanBeNowPlayingApplication",
                        ),
                    })
                })
                .clone()
        }
    }

    unsafe fn load_symbol<T: Copy>(handle: *mut c_void, name: &CStr) -> Result<T, String> {
        let symbol = libc::dlsym(handle, name.as_ptr());
        if symbol.is_null() {
            return Err(format!("missing {}", name.to_string_lossy()));
        }
        Ok(std::mem::transmute_copy(&symbol))
    }

    unsafe fn load_optional_symbol<T: Copy>(handle: *mut c_void, name: &CStr) -> Option<T> {
        let symbol = libc::dlsym(handle, name.as_ptr());
        (!symbol.is_null()).then(|| std::mem::transmute_copy(&symbol))
    }

    fn dl_error() -> String {
        let error = unsafe { libc::dlerror() };
        if error.is_null() {
            "could not load MediaRemote".to_string()
        } else {
            unsafe { CStr::from_ptr(error) }
                .to_string_lossy()
                .into_owned()
        }
    }

    fn callback_queue() -> dispatch2::DispatchRetained<DispatchQueue> {
        DispatchQueue::global_queue(GlobalQueueIdentifier::QualityOfService(
            DispatchQoS::Utility,
        ))
    }

    #[derive(Default)]
    struct NowPlayingInfo {
        title: Option<String>,
        bundle_id: Option<String>,
    }

    pub struct SystemMediaAdapter {
        runner: Arc<dyn CommandRunner>,
        tools: ToolsConfig,
    }

    impl SystemMediaAdapter {
        pub fn new(runner: Arc<dyn CommandRunner>, tools: ToolsConfig) -> Self {
            Self { runner, tools }
        }

        async fn prepare(&self, symbols: Symbols) {
            static REGISTERED: OnceLock<()> = OnceLock::new();
            if REGISTERED.set(()).is_err() {
                return;
            }
            let queue = callback_queue();
            if let Some(register) = symbols.register_notifications {
                unsafe { register(&queue) };
            }
            if let Some(set_can_be_now_playing) = symbols.set_can_be_now_playing {
                unsafe { set_can_be_now_playing(0) };
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }

        async fn current_pid(&self, symbols: Symbols) -> Result<i32, MediaError> {
            let (sender, receiver) = oneshot::channel();
            let sender = Mutex::new(Some(sender));
            {
                let block = RcBlock::new(move |pid: i32| {
                    if let Some(sender) = sender.lock().expect("media PID callback lock").take() {
                        let _ = sender.send(pid);
                    }
                });
                let queue = callback_queue();
                unsafe { (symbols.get_pid)(&queue, &block) };
            }

            tokio::time::timeout(CALLBACK_TIMEOUT, receiver)
                .await
                .map_err(|_| MediaError::Timeout)?
                .map_err(|_| MediaError::Timeout)
        }

        async fn now_playing_info(&self, symbols: Symbols) -> Result<NowPlayingInfo, MediaError> {
            let (sender, receiver) = oneshot::channel();
            let sender = Mutex::new(Some(sender));
            {
                let block = RcBlock::new(move |raw: *const c_void| {
                    let dictionary_ref = raw as CFDictionaryRef;
                    let info = if dictionary_ref.is_null() {
                        NowPlayingInfo::default()
                    } else {
                        let dictionary: CFDictionary<CFString, CFType> =
                            unsafe { TCFType::wrap_under_get_rule(dictionary_ref) };
                        NowPlayingInfo {
                            title: dictionary_string(
                                &dictionary,
                                "kMRMediaRemoteNowPlayingInfoTitle",
                            ),
                            bundle_id: dictionary_string(
                                &dictionary,
                                "kMRMediaRemoteNowPlayingInfoClientBundleIdentifier",
                            ),
                        }
                    };
                    if let Some(sender) = sender.lock().expect("media info callback lock").take() {
                        let _ = sender.send(info);
                    }
                });
                let queue = callback_queue();
                unsafe { (symbols.get_info)(&queue, &block) };
            }

            tokio::time::timeout(CALLBACK_TIMEOUT, receiver)
                .await
                .map_err(|_| MediaError::Timeout)?
                .map_err(|_| MediaError::Timeout)
        }

        async fn browser_source(&self, application: &str, title: Option<&str>) -> Option<String> {
            if application != "Google Chrome" {
                return None;
            }
            let script = if title.is_some() {
                MATCH_CHROME_TAB
            } else {
                LIST_CHROME_MEDIA_TABS
            };
            let output = self
                .runner
                .run(
                    &self.tools.osascript,
                    &["-e", script, title.unwrap_or_default()],
                    timeouts::LOCAL,
                )
                .await
                .ok()?;
            unique_source_from_urls(output.trimmed())
        }
    }

    #[async_trait]
    impl MediaAdapter for SystemMediaAdapter {
        async fn status(&self) -> Result<MediaStatus, MediaError> {
            let symbols = Symbols::load().map_err(MediaError::Unavailable)?;
            self.prepare(symbols).await;
            let (pid, info) =
                tokio::join!(self.current_pid(symbols), self.now_playing_info(symbols));
            let info = info.unwrap_or_default();
            let (mut application, mut bundle_id) = pid
                .ok()
                .filter(|pid| *pid > 0)
                .map_or((None, None), application_for_pid);
            if application.is_none() {
                if let Some(info_bundle_id) = info.bundle_id.as_deref() {
                    application = application_name_for_bundle(info_bundle_id);
                    bundle_id = Some(info_bundle_id.to_string());
                }
            }
            if application.is_none() {
                if let Some(owner) = active_audio_application() {
                    application = Some(owner.name);
                    bundle_id = owner.bundle_id;
                }
            }
            let source = match (application.as_deref(), bundle_id.as_deref()) {
                (_, Some("com.spotify.client")) => Some("Spotify".to_string()),
                (_, Some("com.apple.Music")) => Some("Music".to_string()),
                (Some(application), _) => {
                    self.browser_source(application, info.title.as_deref())
                        .await
                }
                _ => None,
            };

            Ok(MediaStatus {
                application,
                source,
                title: info.title,
            })
        }

        async fn control(&self, control: Control) -> Result<(), MediaError> {
            let symbols = Symbols::load().map_err(MediaError::Unavailable)?;
            let (command, label) = match control {
                Control::PlayPause => (2, "play/pause"),
                Control::Next => (4, "next"),
                Control::Previous => (5, "previous"),
            };
            if unsafe { (symbols.send_command)(command, std::ptr::null()) } {
                Ok(())
            } else {
                Err(MediaError::Rejected(label))
            }
        }
    }

    fn dictionary_string(dictionary: &CFDictionary<CFString, CFType>, key: &str) -> Option<String> {
        dictionary
            .find(CFString::new(key))
            .and_then(|value| value.downcast::<CFString>())
            .map(|value| value.to_string())
            .filter(|value| !value.trim().is_empty())
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct AudioApplication {
        name: String,
        bundle_id: Option<String>,
    }

    fn application_for_pid(pid: i32) -> (Option<String>, Option<String>) {
        let Some(application) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
        else {
            return (None, None);
        };
        let name = application.localizedName().map(|name| name.to_string());
        let bundle_id = application
            .bundleIdentifier()
            .map(|identifier| identifier.to_string());
        normalize_application(name, bundle_id)
    }

    fn normalize_application(
        name: Option<String>,
        bundle_id: Option<String>,
    ) -> (Option<String>, Option<String>) {
        let chrome = bundle_id
            .as_deref()
            .is_some_and(|bundle| bundle.starts_with("com.google.Chrome"))
            || name
                .as_deref()
                .is_some_and(|name| name.starts_with("Google Chrome Helper"));
        if chrome {
            return (
                Some("Google Chrome".to_string()),
                Some("com.google.Chrome".to_string()),
            );
        }
        if bundle_id
            .as_deref()
            .is_some_and(|bundle| bundle.starts_with("com.spotify.client"))
        {
            return (
                Some("Spotify".to_string()),
                Some("com.spotify.client".to_string()),
            );
        }
        (name, bundle_id)
    }

    fn application_name_for_bundle(bundle_id: &str) -> Option<String> {
        let name = match bundle_id {
            bundle if bundle.starts_with("com.spotify.client") => "Spotify",
            "com.apple.Music" => "Music",
            bundle if bundle.starts_with("com.google.Chrome") => "Google Chrome",
            bundle if bundle.starts_with("com.apple.Safari") => "Safari",
            bundle if bundle.starts_with("org.mozilla.firefox") => "Firefox",
            bundle if bundle.starts_with("com.brave.Browser") => "Brave Browser",
            bundle if bundle.starts_with("com.colliderli.iina") => "IINA",
            bundle if bundle.starts_with("org.videolan.vlc") => "VLC",
            _ => return None,
        };
        Some(name.to_string())
    }

    fn active_audio_application() -> Option<AudioApplication> {
        let mut applications: Vec<AudioApplication> = active_audio_processes()
            .into_iter()
            .filter(|process| process.pid != std::process::id() as i32)
            .filter_map(|process| {
                let (mut name, mut bundle_id) = application_for_pid(process.pid);
                if bundle_id.is_none() {
                    bundle_id = process.bundle_id;
                }
                if name.is_none() {
                    name = bundle_id.as_deref().and_then(application_name_for_bundle);
                }
                let (name, bundle_id) = normalize_application(name, bundle_id);
                name.map(|name| AudioApplication { name, bundle_id })
            })
            .collect();
        applications.sort_by(|left, right| {
            (&left.name, &left.bundle_id).cmp(&(&right.name, &right.bundle_id))
        });
        applications.dedup();

        if applications.len() == 1 {
            return applications.pop();
        }
        let mut likely_media: Vec<_> = applications
            .into_iter()
            .filter(|application| {
                application
                    .bundle_id
                    .as_deref()
                    .and_then(application_name_for_bundle)
                    .is_some()
            })
            .collect();
        (likely_media.len() == 1).then(|| likely_media.remove(0))
    }

    type AudioObjectId = u32;
    type OsStatus = i32;

    #[repr(C)]
    struct AudioObjectPropertyAddress {
        selector: u32,
        scope: u32,
        element: u32,
    }

    const AUDIO_OBJECT_SYSTEM: AudioObjectId = 1;
    const SCOPE_GLOBAL: u32 = fourcc(*b"glob");
    const ELEMENT_MAIN: u32 = 0;
    const PROCESS_OBJECT_LIST: u32 = fourcc(*b"prs#");
    const PROCESS_PID: u32 = fourcc(*b"ppid");
    const PROCESS_BUNDLE_ID: u32 = fourcc(*b"pbid");
    const PROCESS_IS_RUNNING_OUTPUT: u32 = fourcc(*b"piro");

    const fn fourcc(bytes: [u8; 4]) -> u32 {
        ((bytes[0] as u32) << 24)
            | ((bytes[1] as u32) << 16)
            | ((bytes[2] as u32) << 8)
            | bytes[3] as u32
    }

    #[link(name = "CoreAudio", kind = "framework")]
    extern "C" {
        fn AudioObjectGetPropertyDataSize(
            object: AudioObjectId,
            address: *const AudioObjectPropertyAddress,
            qualifier_size: u32,
            qualifier: *const c_void,
            size: *mut u32,
        ) -> OsStatus;
        fn AudioObjectGetPropertyData(
            object: AudioObjectId,
            address: *const AudioObjectPropertyAddress,
            qualifier_size: u32,
            qualifier: *const c_void,
            size: *mut u32,
            data: *mut c_void,
        ) -> OsStatus;
    }

    struct AudioProcess {
        pid: i32,
        bundle_id: Option<String>,
    }

    fn active_audio_processes() -> Vec<AudioProcess> {
        let address = property_address(PROCESS_OBJECT_LIST);
        let mut bytes = 0;
        let status = unsafe {
            AudioObjectGetPropertyDataSize(
                AUDIO_OBJECT_SYSTEM,
                &address,
                0,
                std::ptr::null(),
                &mut bytes,
            )
        };
        if status != 0 || bytes == 0 {
            return Vec::new();
        }

        let mut objects = vec![0; bytes as usize / size_of::<AudioObjectId>()];
        let status = unsafe {
            AudioObjectGetPropertyData(
                AUDIO_OBJECT_SYSTEM,
                &address,
                0,
                std::ptr::null(),
                &mut bytes,
                objects.as_mut_ptr().cast(),
            )
        };
        if status != 0 {
            return Vec::new();
        }

        objects
            .into_iter()
            .filter(|object| property_u32(*object, PROCESS_IS_RUNNING_OUTPUT) == Some(1))
            .filter_map(|object| {
                property_u32(object, PROCESS_PID).map(|pid| AudioProcess {
                    pid: pid as i32,
                    bundle_id: property_string(object, PROCESS_BUNDLE_ID),
                })
            })
            .collect()
    }

    fn property_u32(object: AudioObjectId, selector: u32) -> Option<u32> {
        let address = property_address(selector);
        let mut value = 0;
        let mut bytes = size_of::<u32>() as u32;
        let status = unsafe {
            AudioObjectGetPropertyData(
                object,
                &address,
                0,
                std::ptr::null(),
                &mut bytes,
                (&mut value as *mut u32).cast(),
            )
        };
        (status == 0 && bytes as usize == size_of::<u32>()).then_some(value)
    }

    fn property_string(object: AudioObjectId, selector: u32) -> Option<String> {
        let address = property_address(selector);
        let mut value: CFStringRef = std::ptr::null();
        let mut bytes = size_of::<CFStringRef>() as u32;
        let status = unsafe {
            AudioObjectGetPropertyData(
                object,
                &address,
                0,
                std::ptr::null(),
                &mut bytes,
                (&mut value as *mut CFStringRef).cast(),
            )
        };
        if status != 0 || value.is_null() {
            return None;
        }
        Some(unsafe { CFString::wrap_under_create_rule(value) }.to_string())
    }

    const fn property_address(selector: u32) -> AudioObjectPropertyAddress {
        AudioObjectPropertyAddress {
            selector,
            scope: SCOPE_GLOBAL,
            element: ELEMENT_MAIN,
        }
    }

    fn source_from_url(url: &str) -> Option<String> {
        let normalized = url.trim().to_ascii_lowercase();
        let source = if normalized.contains("youtube.com/") || normalized.contains("youtu.be/") {
            "YouTube"
        } else if normalized.contains("netflix.com/") {
            "Netflix"
        } else if normalized.contains("disneyplus.com/") {
            "Disney+"
        } else if normalized.contains("skyshowtime.com/") {
            "SkyShowtime"
        } else if normalized.contains("svtplay.se/") {
            "SVT Play"
        } else {
            return None;
        };
        Some(source.to_string())
    }

    fn unique_source_from_urls(urls: &str) -> Option<String> {
        let mut sources: Vec<String> = urls.lines().filter_map(source_from_url).collect();
        sources.sort();
        sources.dedup();
        (sources.len() == 1).then(|| sources.remove(0))
    }

    const MATCH_CHROME_TAB: &str = r#"
on run argv
  set needle to item 1 of argv
  if application "Google Chrome" is not running then return "not-found"
  tell application "Google Chrome"
    repeat with w in windows
      repeat with t in tabs of w
        try
          set tabTitle to title of t
          if tabTitle contains needle or needle contains tabTitle then return URL of t
        end try
      end repeat
    end repeat
  end tell
  return "not-found"
end run
"#;

    const LIST_CHROME_MEDIA_TABS: &str = r#"
on run argv
  if application "Google Chrome" is not running then return ""
  set mediaUrls to {}
  tell application "Google Chrome"
    repeat with w in windows
      repeat with t in tabs of w
        try
          set tabUrl to URL of t
          if tabUrl contains "youtube.com/" or tabUrl contains "youtu.be/" or tabUrl contains "netflix.com/" or tabUrl contains "disneyplus.com/" or tabUrl contains "skyshowtime.com/" or tabUrl contains "svtplay.se/" then
            set end of mediaUrls to tabUrl
          end if
        end try
      end repeat
    end repeat
  end tell
  set previousDelimiters to AppleScript's text item delimiters
  set AppleScript's text item delimiters to linefeed
  set resultText to mediaUrls as text
  set AppleScript's text item delimiters to previousDelimiters
  return resultText
end run
"#;

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::fake::{FakeCommandRunner, Reply};

        #[test]
        fn browser_urls_are_mapped_to_friendly_service_names() {
            assert_eq!(
                source_from_url("https://www.youtube.com/watch?v=abc"),
                Some("YouTube".to_string())
            );
            assert_eq!(
                source_from_url("https://www.netflix.com/watch/123"),
                Some("Netflix".to_string())
            );
            assert_eq!(source_from_url("https://example.com/video"), None);
            assert_eq!(
                unique_source_from_urls(
                    "https://youtube.com/watch?v=1\nhttps://youtube.com/watch?v=2"
                ),
                Some("YouTube".to_string())
            );
            assert_eq!(
                unique_source_from_urls(
                    "https://youtube.com/watch?v=1\nhttps://netflix.com/watch/2"
                ),
                None
            );
        }

        #[test]
        fn chrome_helpers_are_reported_as_the_chrome_application() {
            assert_eq!(
                normalize_application(
                    Some("Google Chrome Helper (Renderer)".to_string()),
                    Some("com.google.Chrome.helper.renderer".to_string())
                ),
                (
                    Some("Google Chrome".to_string()),
                    Some("com.google.Chrome".to_string())
                )
            );
            assert_eq!(
                normalize_application(None, Some("com.spotify.client.helper".to_string())),
                (
                    Some("Spotify".to_string()),
                    Some("com.spotify.client".to_string())
                )
            );
        }

        #[test]
        fn core_audio_process_enumeration_is_available() {
            let _ = active_audio_processes();
        }

        #[test]
        fn media_remote_symbols_are_available_on_the_running_macos() {
            Symbols::load().expect("MediaRemote symbols");
        }

        #[tokio::test]
        async fn the_live_media_session_can_be_queried() {
            let runner = Arc::new(FakeCommandRunner::new());
            runner.fallback(Reply::ok("not-found"));
            let adapter = SystemMediaAdapter::new(runner, ToolsConfig::default());

            adapter.status().await.expect("media status");
        }
    }
}

#[cfg(target_os = "macos")]
pub use macos::SystemMediaAdapter;
