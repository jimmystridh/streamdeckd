use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Physical key coordinate. One-based, `row,column`, matching the plan's tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct KeyPosition {
    pub row: u8,
    pub column: u8,
}

impl KeyPosition {
    pub const fn new(row: u8, column: u8) -> Self {
        Self { row, column }
    }
}

impl fmt::Display for KeyPosition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{},{}", self.row, self.column)
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("expected a `row,column` coordinate such as `2,3`")]
pub struct KeyPositionParseError;

impl FromStr for KeyPosition {
    type Err = KeyPositionParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (row, column) = value.split_once(',').ok_or(KeyPositionParseError)?;
        let row = row.trim().parse().map_err(|_| KeyPositionParseError)?;
        let column = column.trim().parse().map_err(|_| KeyPositionParseError)?;
        if row == 0 || column == 0 {
            return Err(KeyPositionParseError);
        }
        Ok(Self { row, column })
    }
}

/// Key grid geometry. The only supported layout in this version is 5x3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grid {
    pub rows: u8,
    pub columns: u8,
}

impl Grid {
    pub const MK2: Self = Self {
        rows: 3,
        columns: 5,
    };

    pub const fn key_count(&self) -> usize {
        self.rows as usize * self.columns as usize
    }

    /// Zero-based, row-major device key index for a one-based coordinate.
    pub fn index_of(&self, position: KeyPosition) -> Option<usize> {
        if position.row == 0
            || position.column == 0
            || position.row > self.rows
            || position.column > self.columns
        {
            return None;
        }
        Some((position.row as usize - 1) * self.columns as usize + (position.column as usize - 1))
    }

    /// One-based coordinate for a zero-based, row-major device key index.
    pub fn position_of(&self, index: usize) -> Option<KeyPosition> {
        if index >= self.key_count() {
            return None;
        }
        Some(KeyPosition::new(
            (index / self.columns as usize) as u8 + 1,
            (index % self.columns as usize) as u8 + 1,
        ))
    }

    pub fn positions(&self) -> impl Iterator<Item = KeyPosition> + '_ {
        (0..self.key_count()).filter_map(move |index| self.position_of(index))
    }
}

/// The pages of the Command Center layout.
///
/// The numeric order matches the historical Elgato profile page order so state
/// imported from the old profile keeps pointing at the same page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PageId {
    Home,
    Mixer,
    GitHub,
    Spotify,
    Stensjon,
    Pomodoro,
    Weather,
    Media,
    Wispr,
}

impl PageId {
    pub const ALL: [PageId; 9] = [
        PageId::Home,
        PageId::Mixer,
        PageId::GitHub,
        PageId::Spotify,
        PageId::Stensjon,
        PageId::Pomodoro,
        PageId::Weather,
        PageId::Media,
        PageId::Wispr,
    ];

    pub const fn slug(self) -> &'static str {
        match self {
            PageId::Home => "home",
            PageId::Mixer => "mixer",
            PageId::GitHub => "github",
            PageId::Spotify => "spotify",
            PageId::Stensjon => "stensjon",
            PageId::Pomodoro => "pomodoro",
            PageId::Weather => "weather",
            PageId::Media => "media",
            PageId::Wispr => "wispr",
        }
    }

    pub const fn title(self) -> &'static str {
        match self {
            PageId::Home => "Home",
            PageId::Mixer => "Mixer",
            PageId::GitHub => "GitHub",
            PageId::Spotify => "Spotify",
            PageId::Stensjon => "Stensjön",
            PageId::Pomodoro => "Pomodoro",
            PageId::Weather => "Weather",
            PageId::Media => "Media",
            PageId::Wispr => "Wispr Flow",
        }
    }

    pub const fn profile_index(self) -> u8 {
        match self {
            PageId::Home => 0,
            PageId::Mixer => 1,
            PageId::GitHub => 2,
            PageId::Spotify => 3,
            PageId::Stensjon => 4,
            PageId::Pomodoro => 5,
            PageId::Weather => 6,
            PageId::Media => 7,
            PageId::Wispr => 8,
        }
    }
}

impl fmt::Display for PageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.slug())
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error(
    "unknown page; expected one of home, mixer, github, spotify, stensjon, pomodoro, weather, media, wispr"
)]
pub struct PageIdParseError;

impl FromStr for PageId {
    type Err = PageIdParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "home" => Ok(PageId::Home),
            "mixer" | "audio" => Ok(PageId::Mixer),
            "github" | "gh" => Ok(PageId::GitHub),
            "spotify" => Ok(PageId::Spotify),
            "stensjon" | "stensjön" | "lake" => Ok(PageId::Stensjon),
            "pomodoro" => Ok(PageId::Pomodoro),
            "weather" => Ok(PageId::Weather),
            "media" => Ok(PageId::Media),
            "wispr" | "microphone" | "mic" => Ok(PageId::Wispr),
            _ => Err(PageIdParseError),
        }
    }
}

/// Identifies an integration for refresh scheduling, metrics, and CLI commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IntegrationId {
    AudioStatus,
    AudioInventory,
    Meetings,
    LakeCurrent,
    LakeHistory,
    Weather,
    GitHub,
    ClaudeUsage,
    CodexUsage,
    Spotify,
    MediaSession,
}

impl IntegrationId {
    pub const ALL: [IntegrationId; 11] = [
        IntegrationId::AudioStatus,
        IntegrationId::AudioInventory,
        IntegrationId::Meetings,
        IntegrationId::LakeCurrent,
        IntegrationId::LakeHistory,
        IntegrationId::Weather,
        IntegrationId::GitHub,
        IntegrationId::ClaudeUsage,
        IntegrationId::CodexUsage,
        IntegrationId::Spotify,
        IntegrationId::MediaSession,
    ];

    pub const fn slug(self) -> &'static str {
        match self {
            IntegrationId::AudioStatus => "audio-status",
            IntegrationId::AudioInventory => "audio-inventory",
            IntegrationId::Meetings => "meetings",
            IntegrationId::LakeCurrent => "lake-current",
            IntegrationId::LakeHistory => "lake-history",
            IntegrationId::Weather => "weather",
            IntegrationId::GitHub => "github",
            IntegrationId::ClaudeUsage => "claude-usage",
            IntegrationId::CodexUsage => "codex-usage",
            IntegrationId::Spotify => "spotify",
            IntegrationId::MediaSession => "media-session",
        }
    }
}

impl fmt::Display for IntegrationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.slug())
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("unknown integration")]
pub struct IntegrationIdParseError;

impl FromStr for IntegrationId {
    type Err = IntegrationIdParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let normalized = value.trim().to_ascii_lowercase().replace('_', "-");
        let alias = match normalized.as_str() {
            "audio" => Some(IntegrationId::AudioStatus),
            "lake" => Some(IntegrationId::LakeCurrent),
            "claude" => Some(IntegrationId::ClaudeUsage),
            "codex" => Some(IntegrationId::CodexUsage),
            "media" => Some(IntegrationId::MediaSession),
            _ => None,
        };
        IntegrationId::ALL
            .into_iter()
            .find(|candidate| candidate.slug() == normalized)
            .or(alias)
            .ok_or(IntegrationIdParseError)
    }
}

/// The two Home weather tiles, for the temporary detail view a press opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WeatherTile {
    Current,
    Forecast,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioKind {
    Output,
    Input,
}

impl AudioKind {
    pub const fn switch_audio_flag(self) -> &'static str {
        match self {
            AudioKind::Output => "output",
            AudioKind::Input => "input",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_round_trips_every_position() {
        let grid = Grid::MK2;
        assert_eq!(grid.key_count(), 15);
        for index in 0..grid.key_count() {
            let position = grid.position_of(index).expect("position");
            assert_eq!(grid.index_of(position), Some(index));
        }
    }

    #[test]
    fn grid_maps_documented_coordinates() {
        let grid = Grid::MK2;
        assert_eq!(grid.index_of(KeyPosition::new(1, 1)), Some(0));
        assert_eq!(grid.index_of(KeyPosition::new(1, 5)), Some(4));
        assert_eq!(grid.index_of(KeyPosition::new(2, 3)), Some(7));
        assert_eq!(grid.index_of(KeyPosition::new(3, 5)), Some(14));
        assert_eq!(grid.index_of(KeyPosition::new(4, 1)), None);
        assert_eq!(grid.index_of(KeyPosition::new(1, 6)), None);
        assert_eq!(grid.index_of(KeyPosition::new(0, 1)), None);
    }

    #[test]
    fn key_position_parses_and_rejects_bad_input() {
        assert_eq!("2,3".parse::<KeyPosition>(), Ok(KeyPosition::new(2, 3)));
        assert_eq!(" 3 , 1 ".parse::<KeyPosition>(), Ok(KeyPosition::new(3, 1)));
        assert!("2".parse::<KeyPosition>().is_err());
        assert!("0,1".parse::<KeyPosition>().is_err());
        assert!("a,b".parse::<KeyPosition>().is_err());
    }

    #[test]
    fn page_ids_match_historical_profile_order() {
        for page in PageId::ALL {
            assert_eq!(page.slug().parse::<PageId>(), Ok(page));
        }
        assert_eq!(PageId::Home.profile_index(), 0);
        assert_eq!(PageId::Pomodoro.profile_index(), 5);
        assert_eq!("Stensjön".parse::<PageId>(), Ok(PageId::Stensjon));
    }

    #[test]
    fn integration_ids_parse_from_cli_aliases() {
        assert_eq!("github".parse::<IntegrationId>(), Ok(IntegrationId::GitHub));
        assert_eq!(
            "claude".parse::<IntegrationId>(),
            Ok(IntegrationId::ClaudeUsage)
        );
        assert_eq!(
            "lake_history".parse::<IntegrationId>(),
            Ok(IntegrationId::LakeHistory)
        );
        assert_eq!(
            "media".parse::<IntegrationId>(),
            Ok(IntegrationId::MediaSession)
        );
        assert!("nope".parse::<IntegrationId>().is_err());
    }
}
