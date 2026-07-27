//! Typed configuration with validation.
//!
//! Loading is transactional: a candidate is parsed and fully validated before the
//! runtime is allowed to swap it in, so an invalid edit leaves the last good
//! configuration active and surfaces an error through the CLI.

use std::path::{Path, PathBuf};

use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

use crate::model::PageId;

pub const CURRENT_VERSION: u32 = 1;

/// The versioned configuration template shipped in the repository.
pub const TEMPLATE: &str = include_str!("../../../config/command-center.toml");

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("could not read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not parse {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("{0}")]
    Invalid(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub device_serial: Option<String>,
    #[serde(default = "default_startup_page")]
    pub startup_page: PageId,
    #[serde(default = "default_brightness")]
    pub brightness: u8,
    #[serde(default = "default_long_press_ms")]
    pub long_press_ms: u64,
    #[serde(default = "default_panel_seconds")]
    pub temporary_panel_seconds: u64,
    /// Whether to blank the deck on a clean shutdown. `false` preserves the last frame.
    #[serde(default)]
    pub blank_on_exit: bool,
    #[serde(default)]
    pub location: LocationConfig,
    #[serde(default)]
    pub lake: LakeConfig,
    #[serde(default)]
    pub pomodoro: PomodoroConfig,
    #[serde(default)]
    pub meetings: MeetingsConfig,
    #[serde(default)]
    pub github: GitHubConfig,
    #[serde(default)]
    pub usage: UsageConfig,
    #[serde(default)]
    pub wispr: WisprConfig,
    #[serde(default)]
    pub spotify: SpotifyConfig,
    #[serde(default)]
    pub audio: AudioConfig,
    #[serde(default)]
    pub tools: ToolsConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: CURRENT_VERSION,
            device_serial: None,
            startup_page: default_startup_page(),
            brightness: default_brightness(),
            long_press_ms: default_long_press_ms(),
            temporary_panel_seconds: default_panel_seconds(),
            blank_on_exit: false,
            location: LocationConfig::default(),
            lake: LakeConfig::default(),
            pomodoro: PomodoroConfig::default(),
            meetings: MeetingsConfig::default(),
            github: GitHubConfig::default(),
            usage: UsageConfig::default(),
            wispr: WisprConfig::default(),
            spotify: SpotifyConfig::default(),
            audio: AudioConfig::default(),
            tools: ToolsConfig::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocationConfig {
    pub name: String,
    pub latitude: f64,
    pub longitude: f64,
    pub timezone: String,
}

impl Default for LocationConfig {
    fn default() -> Self {
        Self {
            name: "Stensjön".to_string(),
            latitude: 57.6627,
            longitude: 12.0341,
            timezone: "Europe/Stockholm".to_string(),
        }
    }
}

impl LocationConfig {
    pub fn timezone(&self) -> Tz {
        self.timezone
            .parse()
            .unwrap_or(chrono_tz::Europe::Stockholm)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LakeConfig {
    pub id: String,
}

impl Default for LakeConfig {
    fn default() -> Self {
        Self {
            id: "A84041BDC1864B41".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PomodoroConfig {
    pub focus_minutes: u32,
    pub short_break_minutes: u32,
    pub long_break_minutes: u32,
    pub long_break_every: u32,
    pub sound: String,
    pub repeat_sound_seconds: u64,
    /// Show a native always-on-top alert window while a completion is pending.
    pub persistent_alert: bool,
}

impl Default for PomodoroConfig {
    fn default() -> Self {
        Self {
            focus_minutes: 25,
            short_break_minutes: 5,
            long_break_minutes: 15,
            long_break_every: 4,
            sound: "Glass".to_string(),
            repeat_sound_seconds: 30,
            persistent_alert: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeetingsConfig {
    pub accounts: Vec<String>,
    pub horizon_days: u32,
    pub max_events: u32,
    /// Chrome PWA bundle used when no existing Meet window can be raised.
    pub meet_app: String,
}

impl Default for MeetingsConfig {
    fn default() -> Self {
        Self {
            accounts: Vec::new(),
            horizon_days: 14,
            max_events: 100,
            meet_app: "~/Applications/Chrome Apps.localized/Google Meet.app".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitHubConfig {
    pub updated_within_days: u32,
    pub item_limit: u32,
    /// Repository-name prefixes collapsed into a short alias on item tiles.
    #[serde(default)]
    pub repository_aliases: Vec<RepositoryAlias>,
}

impl Default for GitHubConfig {
    fn default() -> Self {
        Self {
            updated_within_days: 30,
            item_limit: 100,
            repository_aliases: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryAlias {
    pub prefix: String,
    pub replacement: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageConfig {
    pub warning_percent: u8,
    pub critical_percent: u8,
    /// Optional override for the Codex credential file. Empty means `~/.codex/auth.json`.
    #[serde(default)]
    pub codex_auth_path: Option<String>,
}

impl Default for UsageConfig {
    fn default() -> Self {
        Self {
            warning_percent: 50,
            critical_percent: 80,
            codex_auth_path: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WisprConfig {
    pub microphones: Vec<WisprMicrophoneConfig>,
}

impl Default for WisprConfig {
    fn default() -> Self {
        Self {
            microphones: vec![
                WisprMicrophoneConfig {
                    label: "MacBook".to_string(),
                    name: "Built-in mic".to_string(),
                },
                WisprMicrophoneConfig {
                    label: "Bose".to_string(),
                    name: "Bose NC 700 Headphones".to_string(),
                },
                WisprMicrophoneConfig {
                    label: "RØDE".to_string(),
                    name: "RODE NT-USB".to_string(),
                },
            ],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WisprMicrophoneConfig {
    pub label: String,
    /// Prefix of the device name exposed by Wispr Flow.
    pub name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpotifyConfig {
    #[serde(default)]
    pub playlists: Vec<SpotifyPlaylistConfig>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpotifyPlaylistConfig {
    pub label: String,
    pub uri: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AudioConfig {
    #[serde(default)]
    pub output: Vec<AudioTargetConfig>,
    #[serde(default)]
    pub input: Vec<AudioTargetConfig>,
    /// Use the native CoreAudio adapter instead of the `SwitchAudioSource` parity adapter.
    #[serde(default)]
    pub native: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AudioTargetConfig {
    pub label: String,
    #[serde(default)]
    pub exact: Option<String>,
    #[serde(default)]
    pub pattern: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolsConfig {
    pub gh: String,
    pub gog: String,
    pub switch_audio_source: String,
    pub osascript: String,
    pub afplay: String,
    pub open: String,
    /// Retained for compatibility with existing configuration files. Claude
    /// credential reads no longer launch this potentially interactive tool.
    #[serde(default = "default_security")]
    pub security: String,
}

impl Default for ToolsConfig {
    fn default() -> Self {
        Self {
            gh: "/opt/homebrew/bin/gh".to_string(),
            gog: "/opt/homebrew/bin/gog".to_string(),
            switch_audio_source: "/opt/homebrew/bin/SwitchAudioSource".to_string(),
            osascript: "/usr/bin/osascript".to_string(),
            afplay: "/usr/bin/afplay".to_string(),
            open: "/usr/bin/open".to_string(),
            security: default_security(),
        }
    }
}

impl Config {
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let config: Config = toml::from_str(text).map_err(|source| ConfigError::Parse {
            path: PathBuf::from("<memory>"),
            source,
        })?;
        config.validate()?;
        Ok(config)
    }

    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        let config: Config = toml::from_str(&text).map_err(|source| ConfigError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
        config.validate()?;
        Ok(config)
    }

    /// Rejects every candidate the runtime could not honour. Called before a swap
    /// so a bad reload never partially applies.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let invalid = |message: String| ConfigError::Invalid(message);

        if self.version != CURRENT_VERSION {
            return Err(invalid(format!(
                "unsupported config version {}; this build understands {CURRENT_VERSION}",
                self.version
            )));
        }
        if !(10..=100).contains(&self.brightness) {
            return Err(invalid(format!(
                "brightness must be between 10 and 100, got {}",
                self.brightness
            )));
        }
        if !(150..=3_000).contains(&self.long_press_ms) {
            return Err(invalid(format!(
                "long_press_ms must be between 150 and 3000, got {}",
                self.long_press_ms
            )));
        }
        if !(2..=120).contains(&self.temporary_panel_seconds) {
            return Err(invalid(format!(
                "temporary_panel_seconds must be between 2 and 120, got {}",
                self.temporary_panel_seconds
            )));
        }
        if !(-90.0..=90.0).contains(&self.location.latitude)
            || !(-180.0..=180.0).contains(&self.location.longitude)
        {
            return Err(invalid("location coordinates are out of range".to_string()));
        }
        if self.location.timezone.parse::<Tz>().is_err() {
            return Err(invalid(format!(
                "unknown timezone `{}`",
                self.location.timezone
            )));
        }
        if self.lake.id.trim().is_empty() {
            return Err(invalid("lake.id must not be empty".to_string()));
        }
        if !(1..=90).contains(&self.pomodoro.focus_minutes)
            || !(1..=60).contains(&self.pomodoro.short_break_minutes)
            || !(1..=90).contains(&self.pomodoro.long_break_minutes)
        {
            return Err(invalid("pomodoro durations are out of range".to_string()));
        }
        if !(1..=12).contains(&self.pomodoro.long_break_every) {
            return Err(invalid(
                "pomodoro.long_break_every must be between 1 and 12".to_string(),
            ));
        }
        if self.pomodoro.repeat_sound_seconds != 0
            && !(5..=600).contains(&self.pomodoro.repeat_sound_seconds)
        {
            return Err(invalid(
                "pomodoro.repeat_sound_seconds must be 0 or between 5 and 600".to_string(),
            ));
        }
        if !is_safe_sound_name(&self.pomodoro.sound) {
            return Err(invalid(format!(
                "pomodoro.sound `{}` must be a bare macOS system sound name",
                self.pomodoro.sound
            )));
        }
        if self.meetings.horizon_days == 0 || self.meetings.horizon_days > 60 {
            return Err(invalid(
                "meetings.horizon_days must be between 1 and 60".to_string(),
            ));
        }
        if self.meetings.max_events == 0 || self.meetings.max_events > 2_500 {
            return Err(invalid(
                "meetings.max_events must be between 1 and 2500".to_string(),
            ));
        }
        for account in &self.meetings.accounts {
            if !account.contains('@') || account.contains(char::is_whitespace) {
                return Err(invalid(format!(
                    "meetings.accounts entry `{account}` is not an email address"
                )));
            }
        }
        if self.github.updated_within_days == 0 || self.github.updated_within_days > 365 {
            return Err(invalid(
                "github.updated_within_days must be between 1 and 365".to_string(),
            ));
        }
        if !(1..=100).contains(&self.github.item_limit) {
            return Err(invalid(
                "github.item_limit must be between 1 and 100".to_string(),
            ));
        }
        if self.usage.warning_percent >= self.usage.critical_percent
            || self.usage.critical_percent > 100
        {
            return Err(invalid(
                "usage.warning_percent must be below usage.critical_percent, which must be at most 100"
                    .to_string(),
            ));
        }
        if self.wispr.microphones.is_empty() || self.wispr.microphones.len() > 3 {
            return Err(invalid(
                "wispr.microphones must contain between 1 and 3 entries".to_string(),
            ));
        }
        let mut wispr_names = std::collections::HashSet::new();
        for microphone in &self.wispr.microphones {
            if microphone.label.trim().is_empty() || microphone.label.chars().count() > 20 {
                return Err(invalid(
                    "wispr microphone labels must contain between 1 and 20 characters".to_string(),
                ));
            }
            if microphone.name.trim().is_empty()
                || microphone.name.chars().count() > 100
                || microphone.name.chars().any(char::is_control)
            {
                return Err(invalid(
                    "wispr microphone names must contain between 1 and 100 printable characters"
                        .to_string(),
                ));
            }
            if !wispr_names.insert(microphone.name.trim().to_lowercase()) {
                return Err(invalid(
                    "wispr microphone names must not contain duplicates".to_string(),
                ));
            }
        }
        if self.spotify.playlists.len() > 5 {
            return Err(invalid(
                "spotify.playlists supports at most 5 entries".to_string(),
            ));
        }
        for playlist in &self.spotify.playlists {
            if playlist.label.trim().is_empty() || playlist.label.chars().count() > 24 {
                return Err(invalid(
                    "spotify playlist labels must contain between 1 and 24 characters".to_string(),
                ));
            }
            if !is_spotify_playlist_uri(&playlist.uri) {
                return Err(invalid(format!(
                    "spotify playlist `{}` has an invalid URI",
                    playlist.label
                )));
            }
        }
        for (kind, targets) in [("output", &self.audio.output), ("input", &self.audio.input)] {
            for target in targets {
                if target.label.trim().is_empty() {
                    return Err(invalid(format!("audio.{kind} entry needs a label")));
                }
                if target.exact.is_none() && target.pattern.is_none() {
                    return Err(invalid(format!(
                        "audio.{kind} `{}` needs either `exact` or `pattern`",
                        target.label
                    )));
                }
                if let Some(pattern) = &target.pattern {
                    regex::RegexBuilder::new(pattern)
                        .case_insensitive(true)
                        .size_limit(64 * 1024)
                        .build()
                        .map_err(|error| {
                            invalid(format!(
                                "audio.{kind} `{}` has an invalid pattern: {error}",
                                target.label
                            ))
                        })?;
                }
            }
        }
        for tool in [
            &self.tools.gh,
            &self.tools.gog,
            &self.tools.switch_audio_source,
            &self.tools.osascript,
            &self.tools.afplay,
            &self.tools.open,
            &self.tools.security,
        ] {
            if !Path::new(tool).is_absolute() {
                return Err(invalid(format!(
                    "tool path `{tool}` must be absolute so the daemon never resolves it through PATH"
                )));
            }
        }
        Ok(())
    }

    /// Pomodoro durations a fresh state file should start from.
    pub fn pomodoro_defaults(&self) -> crate::pomodoro::PomodoroState {
        let mut state = crate::pomodoro::PomodoroState {
            focus_minutes: self.pomodoro.focus_minutes,
            short_break_minutes: self.pomodoro.short_break_minutes,
            long_break_minutes: self.pomodoro.long_break_minutes,
            ..Default::default()
        };
        state.remaining_seconds = state.focus_minutes * 60;
        state.normalize();
        state
    }
}

/// macOS system sounds are referenced by bare name; anything else could escape
/// the `display notification ... sound name` AppleScript string.
fn is_safe_sound_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric())
}

pub fn is_spotify_playlist_uri(value: &str) -> bool {
    value.strip_prefix("spotify:playlist:").is_some_and(|id| {
        id.len() == 22
            && id
                .chars()
                .all(|character| character.is_ascii_alphanumeric())
    })
}

fn default_version() -> u32 {
    CURRENT_VERSION
}
fn default_startup_page() -> PageId {
    PageId::Home
}
fn default_brightness() -> u8 {
    60
}
fn default_long_press_ms() -> u64 {
    600
}
fn default_panel_seconds() -> u64 {
    10
}
fn default_security() -> String {
    "/usr/bin/security".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_template_is_valid() {
        let config = Config::parse(TEMPLATE).expect("template parses");
        assert_eq!(config.version, CURRENT_VERSION);
        assert_eq!(config.startup_page, PageId::Home);
        assert_eq!(config.audio.output.len(), 4);
        assert_eq!(config.audio.input.len(), 3);
        assert!(config.audio.native);
        assert_eq!(config.wispr.microphones.len(), 3);
        assert_eq!(config.wispr.microphones[2].name, "RODE NT-USB");
        assert_eq!(config.spotify.playlists.len(), 5);
        assert_eq!(config.location.timezone(), chrono_tz::Europe::Stockholm);
    }

    #[test]
    fn defaults_are_valid() {
        Config::default().validate().expect("defaults are valid");
    }

    #[test]
    fn wispr_microphones_are_bounded_and_require_unique_printable_names() {
        let mut config = Config::default();
        config.wispr.microphones.clear();
        assert!(config.validate().is_err());

        config.wispr = WisprConfig::default();
        config.wispr.microphones[1].name = "built-IN MIC".to_string();
        assert!(config.validate().is_err());

        config.wispr = WisprConfig::default();
        config.wispr.microphones[0].name = "bad\nname".to_string();
        assert!(config.validate().is_err());
    }

    #[test]
    fn spotify_playlists_are_bounded_and_require_safe_uris() {
        let mut config = Config::default();
        config.spotify.playlists.push(SpotifyPlaylistConfig {
            label: "Recent".to_string(),
            uri: "spotify:playlist:1jnizfcJFNGVeJgmp7ngK9".to_string(),
        });
        config.validate().expect("valid playlist");

        config.spotify.playlists[0].uri =
            "spotify:playlist:bad\" & do shell script \"oops".to_string();
        assert!(config.validate().is_err());

        config.spotify.playlists = (0..6)
            .map(|index| SpotifyPlaylistConfig {
                label: format!("Recent {index}"),
                uri: "spotify:playlist:1jnizfcJFNGVeJgmp7ngK9".to_string(),
            })
            .collect();
        assert!(config.validate().is_err());
    }

    #[test]
    fn unknown_keys_are_rejected_so_typos_do_not_pass_silently() {
        let error = Config::parse("version = 1\nbrighness = 50\n").expect_err("typo rejected");
        assert!(matches!(error, ConfigError::Parse { .. }), "{error}");
    }

    #[test]
    fn out_of_range_values_are_rejected() {
        for text in [
            "version = 1\nbrightness = 5\n",
            "version = 1\nlong_press_ms = 40\n",
            "version = 1\ntemporary_panel_seconds = 400\n",
            "version = 2\n",
            "version = 1\n[location]\nname='x'\nlatitude=99.0\nlongitude=0.0\ntimezone='Europe/Stockholm'\n",
            "version = 1\n[location]\nname='x'\nlatitude=0.0\nlongitude=0.0\ntimezone='Mars/Olympus'\n",
            "version = 1\n[usage]\nwarning_percent=90\ncritical_percent=50\n",
            "version = 1\n[pomodoro]\nfocus_minutes=25\nshort_break_minutes=5\nlong_break_minutes=15\nlong_break_every=4\nsound='Glass\"; do bad'\nrepeat_sound_seconds=30\npersistent_alert=true\n",
        ] {
            let error = Config::parse(text).expect_err("should be rejected");
            assert!(matches!(error, ConfigError::Invalid(_)), "{text} -> {error}");
        }
    }

    #[test]
    fn an_audio_target_needs_a_matcher_and_a_valid_pattern() {
        let error = Config::parse("version = 1\n[[audio.output]]\nlabel = 'Broken'\n")
            .expect_err("needs a matcher");
        assert!(matches!(error, ConfigError::Invalid(_)), "{error}");

        let error = Config::parse(
            "version = 1\n[[audio.output]]\nlabel = 'Broken'\npattern = '(unclosed'\n",
        )
        .expect_err("needs a valid pattern");
        assert!(matches!(error, ConfigError::Invalid(_)), "{error}");
    }

    #[test]
    fn a_configuration_without_the_security_tool_still_parses() {
        let config = Config::parse(
            "version = 1\n[tools]\ngh = '/opt/homebrew/bin/gh'\n\
             gog = '/opt/homebrew/bin/gog'\n\
             switch_audio_source = '/opt/homebrew/bin/SwitchAudioSource'\n\
             osascript = '/usr/bin/osascript'\nafplay = '/usr/bin/afplay'\n\
             open = '/usr/bin/open'\n",
        )
        .expect("older configurations keep working");
        assert_eq!(config.tools.security, "/usr/bin/security");
    }

    #[test]
    fn tool_paths_must_be_absolute() {
        let error = Config::parse("version = 1\n[tools]\ngh = 'gh'\ngog = '/opt/homebrew/bin/gog'\nswitch_audio_source = '/opt/homebrew/bin/SwitchAudioSource'\nosascript = '/usr/bin/osascript'\nafplay = '/usr/bin/afplay'\nopen = '/usr/bin/open'\n")
            .expect_err("relative tool rejected");
        assert!(matches!(error, ConfigError::Invalid(_)), "{error}");
    }

    #[test]
    fn meeting_accounts_must_look_like_addresses() {
        let error = Config::parse("version = 1\n[meetings]\naccounts = ['not an email']\nhorizon_days = 14\nmax_events = 100\nmeet_app = 'x'\n")
            .expect_err("bad account rejected");
        assert!(matches!(error, ConfigError::Invalid(_)), "{error}");
    }
}
