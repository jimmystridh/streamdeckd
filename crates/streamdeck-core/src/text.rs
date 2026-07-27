//! Deterministic label shortening and value formatting.
//!
//! These are layout decisions, not drawing: keeping them here means the same
//! rules apply to golden tests, the preview device, and the physical deck.

/// Truncates on a character boundary and appends an ellipsis. Never returns more
/// than `max_chars` characters.
pub fn ellipsize(value: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let count = value.chars().count();
    if count <= max_chars {
        return value.to_string();
    }
    if max_chars == 1 {
        return "…".to_string();
    }
    let kept: String = value.chars().take(max_chars - 1).collect();
    format!("{}…", kept.trim_end())
}

/// Collapses the long macOS device names into the short forms used on tiles.
pub fn compact_device_label(value: &str, max_chars: usize) -> String {
    let lower = value.to_lowercase();
    let alias =
        if lower.contains("macbook pro speakers") || lower.contains("macbook pro microphone") {
            "MacBook"
        } else if lower.contains("bose nc 700") {
            "Bose NC 700"
        } else if lower.contains("røde") || lower.contains("rode") {
            "RØDE"
        } else {
            value
        };
    ellipsize(alias, max_chars)
}

/// The device family shown on the Mixer summary tile.
pub fn device_family(value: &str) -> &'static str {
    let lower = value.to_lowercase();
    if lower.contains("macbook") {
        "MAC"
    } else if lower.contains("airpods") {
        "AIRPODS"
    } else if lower.contains("bose") {
        "BOSE"
    } else if lower.contains("røde") || lower.contains("rode") {
        "RØDE"
    } else if lower.contains("usb") {
        "USB"
    } else {
        "OTHER"
    }
}

/// `M:SS` countdown used on Pomodoro tiles.
pub fn format_timer(seconds: u32) -> String {
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// Compact duration used for meeting countdowns: `42M`, `2H`, `1H 5M`.
pub fn format_duration_minutes(minutes: u32) -> String {
    if minutes < 60 {
        return format!("{minutes}M");
    }
    let hours = minutes / 60;
    let remainder = minutes % 60;
    if remainder == 0 {
        format!("{hours}H")
    } else {
        format!("{hours}H {remainder}M")
    }
}

/// Compact duration that stays readable out to a week, for usage reset labels.
///
/// `format_duration_minutes` is right for a meeting countdown, which never exceeds
/// a day, but a seven-day usage window resets days away and `92H 57M` is hard to
/// read at a glance.
pub fn format_long_duration_minutes(minutes: u32) -> String {
    if minutes < 60 {
        return format!("{minutes}M");
    }
    let hours = minutes / 60;
    if hours < 48 {
        let remainder = minutes % 60;
        return if remainder == 0 {
            format!("{hours}H")
        } else {
            format!("{hours}H {remainder}M")
        };
    }
    let days = hours / 24;
    let remainder = hours % 24;
    if remainder == 0 {
        format!("{days}D")
    } else {
        format!("{days}D {remainder}H")
    }
}

/// Total focus time as shown on the all-time statistics tile.
pub fn format_focus_time(minutes: u32) -> String {
    if minutes < 60 {
        return format!("{minutes} FOCUS MIN");
    }
    let hours = minutes / 60;
    let remainder = minutes % 60;
    if remainder == 0 {
        format!("{hours} FOCUS HOURS")
    } else {
        format!("{hours}H {remainder}M FOCUS")
    }
}

/// Rounded whole degrees with a degree sign, including negatives.
pub fn format_temperature(celsius: f64) -> String {
    format!("{}°", celsius.round() as i64)
}

/// One decimal place, with a trailing `.0` dropped, as the lake tiles show it.
pub fn format_lake_temperature(celsius: f64) -> String {
    let rounded = (celsius * 10.0).round() / 10.0;
    if (rounded.fract()).abs() < f64::EPSILON {
        format!("{}°", rounded as i64)
    } else {
        format!("{rounded:.1}°")
    }
}

/// Precipitation as shown on forecast tiles.
pub fn format_precipitation(millimetres: f64) -> String {
    if millimetres < 0.05 {
        "DRY".to_string()
    } else {
        format!("{millimetres:.1} mm")
    }
}

/// Eight-point compass label for a wind bearing in degrees.
pub fn compass_direction(degrees: f64) -> &'static str {
    const DIRECTIONS: [&str; 8] = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];
    let index = ((degrees / 45.0).round() as i64).rem_euclid(8) as usize;
    DIRECTIONS[index]
}

/// Uppercases using Swedish casing rules, then truncates to `max_chars`.
pub fn upper_short(value: &str, max_chars: usize) -> String {
    ellipsize(&value.to_uppercase(), max_chars)
}

/// Applies the configured repository prefix aliases and shortens the result.
pub fn short_repository(name: &str, aliases: &[(String, String)], max_chars: usize) -> String {
    for (prefix, replacement) in aliases {
        if let Some(rest) = name.strip_prefix(prefix.as_str()) {
            return ellipsize(&format!("{replacement}{rest}"), max_chars);
        }
    }
    ellipsize(name, max_chars)
}

/// Strips control characters and collapses whitespace out of untrusted text such
/// as calendar summaries and track names before it reaches a tile or a log.
pub fn sanitize_single_line(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .filter_map(|character| match character {
            '\t' | '\n' | '\r' => Some(' '),
            // Other control characters are removed rather than replaced, so a
            // stray byte inside a word does not split it in two.
            character if character.is_control() => None,
            character => Some(character),
        })
        .collect();
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ellipsize_respects_character_boundaries_and_the_limit() {
        assert_eq!(ellipsize("short", 13), "short");
        assert_eq!(ellipsize("exactlythirteen", 15), "exactlythirteen");
        assert_eq!(ellipsize("Stensjön vattentemperatur", 10), "Stensjön…");
        assert_eq!(ellipsize("åäöåäöåäöåäö", 5).chars().count(), 5);
        assert_eq!(ellipsize("anything", 1), "…");
        assert_eq!(ellipsize("anything", 0), "");
    }

    #[test]
    fn ellipsize_never_leaves_a_dangling_space_before_the_ellipsis() {
        assert_eq!(ellipsize("Weekly team sync", 12), "Weekly team…");
    }

    #[test]
    fn device_names_collapse_to_their_short_forms() {
        assert_eq!(compact_device_label("MacBook Pro Speakers", 13), "MacBook");
        assert_eq!(
            compact_device_label("MacBook Pro Microphone", 13),
            "MacBook"
        );
        assert_eq!(
            compact_device_label("Bose NC 700 Headphones", 13),
            "Bose NC 700"
        );
        assert_eq!(compact_device_label("RØDE NT-USB Mini", 13), "RØDE");
        assert_eq!(compact_device_label("Rode PodMic", 13), "RØDE");
        assert_eq!(
            compact_device_label("Some Very Long Interface Name", 13),
            "Some Very Lo…"
        );
    }

    #[test]
    fn device_families_cover_the_configured_hardware() {
        assert_eq!(device_family("MacBook Pro Speakers"), "MAC");
        assert_eq!(device_family("Bose NC 700 Headphones"), "BOSE");
        assert_eq!(device_family("RØDE NT-USB"), "RØDE");
        assert_eq!(device_family("Generic USB Audio"), "USB");
        assert_eq!(device_family("Jimmy’s AirPods - Find My"), "AIRPODS");
        assert_eq!(device_family("Studio Display Speakers"), "OTHER");
    }

    #[test]
    fn timers_and_durations_format_as_the_tiles_expect() {
        assert_eq!(format_timer(1_500), "25:00");
        assert_eq!(format_timer(59), "0:59");
        assert_eq!(format_timer(0), "0:00");
        assert_eq!(format_timer(3_601), "60:01");

        assert_eq!(format_duration_minutes(42), "42M");
        assert_eq!(format_duration_minutes(120), "2H");
        assert_eq!(format_duration_minutes(65), "1H 5M");
        assert_eq!(format_duration_minutes(0), "0M");
    }

    #[test]
    fn long_durations_switch_to_days_rather_than_piling_up_hours() {
        assert_eq!(format_long_duration_minutes(42), "42M");
        assert_eq!(format_long_duration_minutes(95), "1H 35M");
        assert_eq!(format_long_duration_minutes(2_820), "47H");
        assert_eq!(format_long_duration_minutes(5_577), "3D 20H");
        assert_eq!(format_long_duration_minutes(10_080), "7D");
    }

    #[test]
    fn focus_totals_read_naturally_at_every_magnitude() {
        assert_eq!(format_focus_time(45), "45 FOCUS MIN");
        assert_eq!(format_focus_time(120), "2 FOCUS HOURS");
        assert_eq!(format_focus_time(125), "2H 5M FOCUS");
    }

    #[test]
    fn temperatures_round_and_keep_their_sign() {
        assert_eq!(format_temperature(21.4), "21°");
        assert_eq!(format_temperature(21.5), "22°");
        assert_eq!(format_temperature(-3.2), "-3°");
        assert_eq!(format_temperature(-0.4), "0°");

        assert_eq!(format_lake_temperature(18.0), "18°");
        assert_eq!(format_lake_temperature(18.04), "18°");
        assert_eq!(format_lake_temperature(18.25), "18.3°");
        assert_eq!(format_lake_temperature(-1.5), "-1.5°");
    }

    #[test]
    fn precipitation_reports_dry_below_the_threshold() {
        assert_eq!(format_precipitation(0.0), "DRY");
        assert_eq!(format_precipitation(0.04), "DRY");
        assert_eq!(format_precipitation(0.05), "0.1 mm");
        assert_eq!(format_precipitation(12.34), "12.3 mm");
    }

    #[test]
    fn compass_directions_wrap_around_north() {
        assert_eq!(compass_direction(0.0), "N");
        assert_eq!(compass_direction(22.0), "N");
        assert_eq!(compass_direction(23.0), "NE");
        assert_eq!(compass_direction(359.0), "N");
        assert_eq!(compass_direction(-45.0), "NW");
        assert_eq!(compass_direction(720.0), "N");
    }

    #[test]
    fn repository_aliases_shorten_long_monorepo_names() {
        let aliases = vec![("visma.administration.".to_string(), "admin.".to_string())];
        assert_eq!(
            short_repository("visma.administration.web", &aliases, 13),
            "admin.web"
        );
        assert_eq!(short_repository("streamdeckd", &aliases, 13), "streamdeckd");
        assert_eq!(
            short_repository("visma.administration.averylongservice", &aliases, 13),
            "admin.averyl…"
        );
    }

    #[test]
    fn untrusted_text_is_reduced_to_a_single_clean_line() {
        assert_eq!(
            sanitize_single_line("  Weekly\n\tteam   sync \u{0007}"),
            "Weekly team sync"
        );
        assert_eq!(sanitize_single_line(""), "");
        assert_eq!(sanitize_single_line("Årsmöte"), "Årsmöte");
    }

    #[test]
    fn swedish_uppercasing_is_preserved() {
        assert_eq!(upper_short("Stensjön", 13), "STENSJÖN");
        assert_eq!(upper_short("måndag", 3), "MÅ…");
    }
}
