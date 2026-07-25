//! Audio device inventory, status parsing, and target resolution.

use serde::{Deserialize, Serialize};

use super::ParseError;
use crate::config::AudioTargetConfig;
use crate::model::AudioKind;

const INTEGRATION: &str = "audio";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioInventory {
    pub outputs: Vec<String>,
    pub inputs: Vec<String>,
}

impl AudioInventory {
    pub fn devices(&self, kind: AudioKind) -> &[String] {
        match kind {
            AudioKind::Output => &self.outputs,
            AudioKind::Input => &self.inputs,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioStatus {
    pub current_output: String,
    pub current_input: String,
    pub output_volume: u8,
    pub input_volume: u8,
    pub output_muted: bool,
}

impl AudioStatus {
    pub fn current(&self, kind: AudioKind) -> &str {
        match kind {
            AudioKind::Output => &self.current_output,
            AudioKind::Input => &self.current_input,
        }
    }

    pub fn volume(&self, kind: AudioKind) -> u8 {
        match kind {
            AudioKind::Output => self.output_volume,
            AudioKind::Input => self.input_volume,
        }
    }

    /// A microphone has no separate mute switch on macOS; zero gain is muted.
    pub fn is_muted(&self, kind: AudioKind) -> bool {
        match kind {
            AudioKind::Output => self.output_muted,
            AudioKind::Input => self.input_volume == 0,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioSnapshot {
    pub status: Option<AudioStatus>,
    pub inventory: AudioInventory,
}

/// What a configured tile can do with its device right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// Exactly one candidate; selecting it is safe.
    Available(String),
    /// The device is not currently connected. The tile stays visible but disabled.
    Unavailable,
    /// The pattern matched several devices; selecting would be a guess, so refuse.
    Ambiguous(Vec<String>),
}

impl Resolution {
    pub fn device(&self) -> Option<&str> {
        match self {
            Resolution::Available(name) => Some(name.as_str()),
            _ => None,
        }
    }
}

/// A resolved audio target from configuration.
#[derive(Debug, Clone)]
pub struct AudioTarget {
    pub label: String,
    pub exact: Option<String>,
    pub pattern: Option<regex::Regex>,
}

impl AudioTarget {
    pub fn from_config(config: &AudioTargetConfig) -> Result<Self, ParseError> {
        let pattern = match &config.pattern {
            Some(pattern) => Some(
                regex::RegexBuilder::new(pattern)
                    .case_insensitive(true)
                    .size_limit(64 * 1024)
                    .build()
                    .map_err(|error| {
                        ParseError::shape(INTEGRATION, format!("invalid audio pattern: {error}"))
                    })?,
            ),
            None => None,
        };
        Ok(Self {
            label: config.label.clone(),
            exact: config.exact.clone(),
            pattern,
        })
    }

    /// Resolves against the current device inventory. An exact name always wins so
    /// a broad pattern on another tile cannot steal it.
    pub fn resolve(&self, devices: &[String]) -> Resolution {
        if let Some(exact) = &self.exact {
            if devices.iter().any(|device| device == exact) {
                return Resolution::Available(exact.clone());
            }
        }

        let Some(pattern) = &self.pattern else {
            return Resolution::Unavailable;
        };
        let matches: Vec<String> = devices
            .iter()
            .filter(|device| pattern.is_match(device))
            .cloned()
            .collect();

        match matches.len() {
            0 => Resolution::Unavailable,
            1 => Resolution::Available(matches.into_iter().next().expect("one match")),
            _ => Resolution::Ambiguous(matches),
        }
    }
}

/// `SwitchAudioSource -a -t <kind>` prints one device name per line.
pub fn parse_device_list(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// `osascript -e 'get volume settings'` prints a single comma-separated line.
pub fn parse_volume_settings(stdout: &str) -> Result<(u8, u8, bool), ParseError> {
    let field = |name: &str| -> Option<&str> {
        stdout.split(',').find_map(|part| {
            let (key, value) = part.split_once(':')?;
            (key.trim() == name).then(|| value.trim())
        })
    };
    let number = |name: &str| -> Option<u8> {
        field(name)
            .and_then(|value| value.parse::<i32>().ok())
            .map(|value| value.clamp(0, 100) as u8)
    };

    let output_volume = number("output volume").ok_or_else(|| {
        ParseError::shape(INTEGRATION, "missing `output volume` in volume settings")
    })?;
    let input_volume = number("input volume").ok_or_else(|| {
        ParseError::shape(INTEGRATION, "missing `input volume` in volume settings")
    })?;
    let output_muted = match field("output muted") {
        Some("true") => true,
        Some("false") => false,
        _ => {
            return Err(ParseError::shape(
                INTEGRATION,
                "missing `output muted` in volume settings",
            ))
        }
    };

    Ok((output_volume, input_volume, output_muted))
}

pub fn parse_status(
    current_output: &str,
    current_input: &str,
    volume_settings: &str,
) -> Result<AudioStatus, ParseError> {
    let (output_volume, input_volume, output_muted) = parse_volume_settings(volume_settings)?;
    Ok(AudioStatus {
        current_output: current_output.trim().to_string(),
        current_input: current_input.trim().to_string(),
        output_volume,
        input_volume,
        output_muted,
    })
}

/// Clamps a relative volume change into the device's `0..=100` range.
pub fn next_volume(current: u8, delta: i32) -> u8 {
    (i32::from(current) + delta).clamp(0, 100) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(label: &str, exact: Option<&str>, pattern: Option<&str>) -> AudioTarget {
        AudioTarget::from_config(&AudioTargetConfig {
            label: label.to_string(),
            exact: exact.map(str::to_string),
            pattern: pattern.map(str::to_string),
        })
        .expect("valid target")
    }

    fn devices(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn device_lists_ignore_blank_and_padded_lines() {
        let parsed = parse_device_list(
            "MacBook Pro Speakers\r\n  Bose NC 700 Headphones  \n\n\tExternal USB Audio\n",
        );
        assert_eq!(
            parsed,
            devices(&[
                "MacBook Pro Speakers",
                "Bose NC 700 Headphones",
                "External USB Audio"
            ])
        );
        assert!(parse_device_list("   \n\n").is_empty());
    }

    #[test]
    fn volume_settings_parse_the_applescript_line() {
        let (output, input, muted) = parse_volume_settings(
            "output volume:42, input volume:75, alert volume:100, output muted:false",
        )
        .expect("parsed");
        assert_eq!((output, input, muted), (42, 75, false));
    }

    #[test]
    fn a_muted_output_and_a_missing_volume_are_both_handled() {
        let (_, _, muted) =
            parse_volume_settings("output volume:0, input volume:0, output muted:true")
                .expect("parsed");
        assert!(muted);

        let error = parse_volume_settings("output volume:10, output muted:false")
            .expect_err("missing input volume");
        assert!(error.to_string().contains("input volume"), "{error}");
    }

    #[test]
    fn applescript_reporting_minus_one_is_clamped_rather_than_rejected() {
        // macOS reports -1 for devices with no software-controllable volume.
        let (output, input, _) =
            parse_volume_settings("output volume:-1, input volume:-1, output muted:false")
                .expect("parsed");
        assert_eq!((output, input), (0, 0));
    }

    #[test]
    fn an_exact_name_resolves_when_the_device_is_present() {
        let macbook = target("MacBook", Some("MacBook Pro Speakers"), None);
        assert_eq!(
            macbook.resolve(&devices(&[
                "MacBook Pro Speakers",
                "Bose NC 700 Headphones"
            ])),
            Resolution::Available("MacBook Pro Speakers".to_string())
        );
    }

    #[test]
    fn a_missing_device_is_unavailable_rather_than_absent() {
        let bose = target("Bose", Some("Bose NC 700 Headphones"), None);
        assert_eq!(
            bose.resolve(&devices(&["MacBook Pro Speakers"])),
            Resolution::Unavailable
        );
    }

    #[test]
    fn a_pattern_resolves_a_single_case_insensitive_match() {
        let usb = target("USB Home", None, Some("usb"));
        assert_eq!(
            usb.resolve(&devices(&["MacBook Pro Speakers", "Scarlett 2i2 USB"])),
            Resolution::Available("Scarlett 2i2 USB".to_string())
        );
    }

    #[test]
    fn an_ambiguous_pattern_refuses_to_guess() {
        let usb = target("USB Home", None, Some("usb"));
        let resolution = usb.resolve(&devices(&["Scarlett USB", "RØDE NT-USB Mini"]));

        match resolution {
            Resolution::Ambiguous(matches) => assert_eq!(matches.len(), 2),
            other => panic!("expected an ambiguous resolution, got {other:?}"),
        }
        assert_eq!(
            usb.resolve(&devices(&["Scarlett USB", "RØDE NT-USB Mini"]))
                .device(),
            None
        );
    }

    #[test]
    fn the_rode_pattern_matches_both_spellings() {
        let rode = target("RØDE Mic", None, Some("røde|rode"));
        assert_eq!(
            rode.resolve(&devices(&["RØDE NT-USB Mini"])),
            Resolution::Available("RØDE NT-USB Mini".to_string())
        );
        assert_eq!(
            rode.resolve(&devices(&["Rode PodMic USB"])),
            Resolution::Available("Rode PodMic USB".to_string())
        );
    }

    #[test]
    fn an_exact_match_wins_over_a_pattern_that_would_be_ambiguous() {
        let both = target("Bose", Some("Bose NC 700 Headphones"), Some("bose"));
        assert_eq!(
            both.resolve(&devices(&["Bose NC 700 Headphones", "Bose Companion"])),
            Resolution::Available("Bose NC 700 Headphones".to_string())
        );
    }

    #[test]
    fn a_target_with_no_matcher_at_all_is_unavailable() {
        let empty = target("Nothing", None, None);
        assert_eq!(
            empty.resolve(&devices(&["Anything"])),
            Resolution::Unavailable
        );
    }

    #[test]
    fn status_reports_mute_per_kind() {
        let status = parse_status(
            "Bose NC 700 Headphones",
            "MacBook Pro Microphone",
            "output volume:30, input volume:0, output muted:false",
        )
        .expect("parsed");

        assert_eq!(status.current(AudioKind::Output), "Bose NC 700 Headphones");
        assert_eq!(status.volume(AudioKind::Input), 0);
        assert!(status.is_muted(AudioKind::Input), "zero gain is muted");
        assert!(!status.is_muted(AudioKind::Output));
    }

    #[test]
    fn volume_changes_saturate_at_both_ends() {
        assert_eq!(next_volume(45, 10), 55);
        assert_eq!(next_volume(95, 10), 100);
        assert_eq!(next_volume(5, -10), 0);
        assert_eq!(next_volume(0, -10), 0);
        assert_eq!(next_volume(100, 10), 100);
    }
}
