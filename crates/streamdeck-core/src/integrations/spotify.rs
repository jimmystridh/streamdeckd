//! Spotify player state parsing.
//!
//! The adapter runs one short-lived AppleScript that prints tab-separated fields.
//! Parsing lives here so the daemon can later swap the adapter for direct Apple
//! Events without changing any behaviour the tiles depend on.

use serde::{Deserialize, Serialize};

use super::ParseError;
use crate::text::sanitize_single_line;

const INTEGRATION: &str = "spotify";

/// Field order the adapter's AppleScript emits, tab separated.
pub const FIELD_COUNT: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlayerState {
    Playing,
    Paused,
    Stopped,
    /// Spotify is not running. Controls stay visible but disabled.
    NotRunning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RepeatMode {
    Off,
    All,
    One,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpotifyStatus {
    pub state: PlayerState,
    pub track: String,
    pub artist: String,
    pub album: String,
    pub artwork_url: Option<String>,
    /// Stable identity for the artwork cache.
    pub track_id: Option<String>,
    pub volume: u8,
    pub shuffling: bool,
    pub repeat: RepeatMode,
}

impl SpotifyStatus {
    pub fn not_running() -> Self {
        Self {
            state: PlayerState::NotRunning,
            track: String::new(),
            artist: String::new(),
            album: String::new(),
            artwork_url: None,
            track_id: None,
            volume: 0,
            shuffling: false,
            repeat: RepeatMode::Off,
        }
    }

    pub fn is_playing(&self) -> bool {
        self.state == PlayerState::Playing
    }

    pub fn is_available(&self) -> bool {
        self.state != PlayerState::NotRunning
    }

    /// The label the play/pause and glance tiles show.
    pub fn glance_label(&self, max_chars: usize) -> String {
        match self.state {
            PlayerState::NotRunning => "SPOTIFY".to_string(),
            _ if self.track.is_empty() => "SPOTIFY".to_string(),
            _ => crate::text::ellipsize(&self.track, max_chars),
        }
    }
}

/// Parses the adapter's tab-separated line.
///
/// `not-running<TAB>...` is a normal outcome, not an error: Spotify simply is not
/// open. Anything else malformed is an error so a broken adapter is visible.
pub fn parse_status(stdout: &str) -> Result<SpotifyStatus, ParseError> {
    let line = stdout.trim();
    if line.is_empty() {
        return Err(ParseError::shape(INTEGRATION, "adapter produced no output"));
    }
    if line.starts_with("not-running") {
        return Ok(SpotifyStatus::not_running());
    }

    let fields: Vec<&str> = line.split('\t').collect();
    if fields.len() < FIELD_COUNT {
        return Err(ParseError::shape(
            INTEGRATION,
            format!(
                "expected {FIELD_COUNT} tab-separated fields, got {}",
                fields.len()
            ),
        ));
    }

    let state = match fields[0].trim() {
        "playing" => PlayerState::Playing,
        "paused" => PlayerState::Paused,
        "stopped" => PlayerState::Stopped,
        other => {
            return Err(ParseError::shape(
                INTEGRATION,
                format!("unknown player state `{other}`"),
            ))
        }
    };

    Ok(SpotifyStatus {
        state,
        track: sanitize_single_line(fields[1]),
        artist: sanitize_single_line(fields[2]),
        album: sanitize_single_line(fields[3]),
        artwork_url: normalize_artwork_url(fields[4]),
        track_id: (!fields[5].trim().is_empty()).then(|| fields[5].trim().to_string()),
        volume: fields[6].trim().parse::<i32>().unwrap_or(0).clamp(0, 100) as u8,
        shuffling: fields[7].trim() == "true",
        repeat: match fields.get(8).map(|value| value.trim()) {
            Some("one") | Some("track") => RepeatMode::One,
            Some("all") | Some("true") | Some("context") => RepeatMode::All,
            _ => RepeatMode::Off,
        },
    })
}

/// Only Spotify's own image CDN is accepted, so a spoofed track cannot make the
/// daemon fetch an arbitrary URL.
pub fn normalize_artwork_url(candidate: &str) -> Option<String> {
    let candidate = candidate.trim();
    let rest = candidate.strip_prefix("https://")?;
    let host = rest.split('/').next()?;
    let allowed = host == "i.scdn.co"
        || host.ends_with(".scdn.co")
        || host == "mosaic.scdn.co"
        || host.ends_with(".spotifycdn.com");
    (allowed && rest.len() > host.len() + 1).then(|| candidate.to_string())
}

/// Clamps a relative Spotify volume change.
pub fn next_volume(current: u8, delta: i32) -> u8 {
    (i32::from(current) + delta).clamp(0, 100) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAYING: &str = "playing\tTruth\tKamasi Washington\tThe Epic\thttps://i.scdn.co/image/ab67616d0000b273abc\tspotify:track:4uLU6hMCjMI75M1A2tKUQC\t72\ttrue\tall";

    #[test]
    fn a_playing_track_parses_every_field() {
        let status = parse_status(PLAYING).expect("parsed");

        assert_eq!(status.state, PlayerState::Playing);
        assert!(status.is_playing());
        assert_eq!(status.track, "Truth");
        assert_eq!(status.artist, "Kamasi Washington");
        assert_eq!(status.album, "The Epic");
        assert_eq!(
            status.artwork_url.as_deref(),
            Some("https://i.scdn.co/image/ab67616d0000b273abc")
        );
        assert_eq!(
            status.track_id.as_deref(),
            Some("spotify:track:4uLU6hMCjMI75M1A2tKUQC")
        );
        assert_eq!(status.volume, 72);
        assert!(status.shuffling);
        assert_eq!(status.repeat, RepeatMode::All);
    }

    #[test]
    fn a_missing_application_is_a_normal_state_not_an_error() {
        let status = parse_status("not-running").expect("parsed");
        assert_eq!(status.state, PlayerState::NotRunning);
        assert!(!status.is_available());
        assert_eq!(status.glance_label(13), "SPOTIFY");
    }

    #[test]
    fn paused_and_stopped_states_parse() {
        let paused = PLAYING.replacen("playing", "paused", 1);
        assert_eq!(
            parse_status(&paused).expect("parsed").state,
            PlayerState::Paused
        );

        let stopped = PLAYING.replacen("playing", "stopped", 1);
        assert_eq!(
            parse_status(&stopped).expect("parsed").state,
            PlayerState::Stopped
        );
    }

    #[test]
    fn repeat_modes_cover_the_applescript_vocabulary() {
        for (raw, expected) in [
            ("off", RepeatMode::Off),
            ("all", RepeatMode::All),
            ("true", RepeatMode::All),
            ("context", RepeatMode::All),
            ("one", RepeatMode::One),
            ("track", RepeatMode::One),
            ("nonsense", RepeatMode::Off),
        ] {
            let line = format!("{}\t{raw}", PLAYING.rsplit_once('\t').expect("split").0);
            assert_eq!(
                parse_status(&line).expect("parsed").repeat,
                expected,
                "{raw}"
            );
        }
    }

    #[test]
    fn a_truncated_or_unknown_line_is_an_error() {
        assert!(parse_status("").is_err());
        assert!(parse_status("playing\tTruth").is_err());
        assert!(parse_status(&PLAYING.replacen("playing", "buffering", 1)).is_err());
    }

    #[test]
    fn track_names_are_sanitized_for_a_single_line_tile() {
        let line = PLAYING.replacen("Truth", "Truth  (Live\u{0007})", 1);
        assert_eq!(parse_status(&line).expect("parsed").track, "Truth (Live)");
    }

    #[test]
    fn only_spotify_image_hosts_are_accepted() {
        assert_eq!(
            normalize_artwork_url("https://i.scdn.co/image/abc"),
            Some("https://i.scdn.co/image/abc".to_string())
        );
        assert_eq!(
            normalize_artwork_url("https://mosaic.scdn.co/640/abc"),
            Some("https://mosaic.scdn.co/640/abc".to_string())
        );
        assert_eq!(
            normalize_artwork_url("https://evil.example/image/abc"),
            None
        );
        assert_eq!(normalize_artwork_url("http://i.scdn.co/image/abc"), None);
        assert_eq!(normalize_artwork_url("https://i.scdn.co"), None);
        assert_eq!(normalize_artwork_url(""), None);
    }

    #[test]
    fn a_rejected_artwork_url_still_leaves_a_usable_status() {
        let line = PLAYING.replacen(
            "https://i.scdn.co/image/ab67616d0000b273abc",
            "https://evil.example/x",
            1,
        );
        let status = parse_status(&line).expect("parsed");
        assert_eq!(status.artwork_url, None);
        assert_eq!(status.track, "Truth");
    }

    #[test]
    fn volumes_are_clamped_and_bad_values_become_zero() {
        let line = PLAYING.replacen("\t72\t", "\t150\t", 1);
        assert_eq!(parse_status(&line).expect("parsed").volume, 100);

        let line = PLAYING.replacen("\t72\t", "\tmissing\t", 1);
        assert_eq!(parse_status(&line).expect("parsed").volume, 0);
    }

    #[test]
    fn glance_labels_truncate_long_track_names() {
        let line = PLAYING.replacen("Truth", "A Very Long Track Title Indeed", 1);
        let status = parse_status(&line).expect("parsed");
        assert_eq!(status.glance_label(13), "A Very Long…");
    }

    #[test]
    fn volume_changes_saturate() {
        assert_eq!(next_volume(72, 5), 77);
        assert_eq!(next_volume(98, 5), 100);
        assert_eq!(next_volume(3, -5), 0);
    }
}
