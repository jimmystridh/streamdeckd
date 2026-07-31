//! The single colour table for every tile.
//!
//! Keeping the palette here means a status change — selected, stale, disabled,
//! alert — reads the same on every page, and a colour tweak is one edit.

use crate::integrations::claude::UsageSeverity;
use crate::integrations::meetings::MeetingUrgency;
use crate::integrations::weather::{SymbolFamily, WeatherSymbol};
use crate::pomodoro::Phase;
use crate::view::Color;

pub const SURFACE: Color = Color::hex(0x1e293b);
pub const SURFACE_SUNKEN: Color = Color::hex(0x0b1120);
pub const SURFACE_RAISED: Color = Color::hex(0x334155);
pub const NAVIGATION: Color = Color::hex(0x475569);
pub const SELECTED: Color = Color::hex(0x15803d);
pub const LIVE: Color = Color::hex(0x166534);
pub const MUTED: Color = Color::hex(0xb91c1c);
pub const ERROR: Color = Color::hex(0x7f1d1d);
pub const DISABLED: Color = Color::hex(0x1e293b);
pub const WARNING: Color = Color::hex(0xb45309);
pub const CRITICAL: Color = Color::hex(0x9f1239);
pub const ALERT: Color = Color::hex(0xf59e0b);
pub const ARMED: Color = Color::hex(0x7c3aed);

pub const MIXER: Color = Color::hex(0x0e7490);
pub const GITHUB: Color = Color::hex(0x4c1d95);
pub const GITHUB_ACTIVE: Color = Color::hex(0x7c3aed);
pub const GITHUB_ITEM: Color = Color::hex(0x5b21b6);
pub const SPOTIFY: Color = Color::hex(0x1db954);
pub const MEDIA: Color = Color::hex(0x4338ca);
pub const APPLICATION: Color = Color::hex(0x0369a1);
pub const WISPR: Color = Color::hex(0x6d28d9);
pub const CLAUDE: Color = Color::hex(0xb45309);
pub const CODEX: Color = Color::hex(0x0f172a);
pub const QUICK_CAPTURE: Color = Color::hex(0x7c3aed);
pub const MAC_HEALTH: Color = Color::hex(0x0f766e);
pub const NETWORK: Color = Color::hex(0x0369a1);
pub const VASTTRAFIK: Color = Color::hex(0x005a9c);

pub const FOCUS: Color = Color::hex(0xbe123c);
pub const SHORT_BREAK: Color = Color::hex(0x0f766e);
pub const LONG_BREAK: Color = Color::hex(0x1d4ed8);

pub const MEETING_NEXT: Color = Color::hex(0x2563eb);
pub const MEETING_FOLLOWING: Color = Color::hex(0x0f766e);
pub const MEETING_NOW: Color = Color::hex(0x15803d);
pub const MEETING_IMMINENT: Color = Color::hex(0xb91c1c);
pub const MEETING_SOON: Color = Color::hex(0xb45309);

pub const TRACK: Color = Color::hex(0x64748b);
pub const FILL: Color = Color::hex(0xffffff);

pub const fn phase(phase: Phase) -> Color {
    match phase {
        Phase::Focus => FOCUS,
        Phase::ShortBreak => SHORT_BREAK,
        Phase::LongBreak => LONG_BREAK,
    }
}

/// Water temperature bands, matching the previous tiles.
pub fn water(celsius: f64) -> Color {
    if celsius < 10.0 {
        Color::hex(0x1d4ed8)
    } else if celsius < 15.0 {
        Color::hex(0x0369a1)
    } else if celsius < 20.0 {
        Color::hex(0x0e7490)
    } else if celsius < 24.0 {
        Color::hex(0x0f766e)
    } else {
        Color::hex(0xc2410c)
    }
}

/// Sky gradient for a weather symbol family.
pub fn sky(symbol: WeatherSymbol) -> (Color, Color) {
    match symbol.family {
        SymbolFamily::Thunder => (Color::hex(0x111827), Color::hex(0x312e81)),
        SymbolFamily::Snow | SymbolFamily::Sleet => (Color::hex(0x64748b), Color::hex(0x0369a1)),
        SymbolFamily::Rain => (Color::hex(0x1e3a8a), Color::hex(0x075985)),
        SymbolFamily::Fog => (Color::hex(0x64748b), Color::hex(0x475569)),
        _ if symbol.night => (Color::hex(0x020617), Color::hex(0x172554)),
        SymbolFamily::Clear => (Color::hex(0x2563eb), Color::hex(0x0284c7)),
        SymbolFamily::PartlyCloudy => (Color::hex(0x1d4ed8), Color::hex(0x0369a1)),
        SymbolFamily::Cloudy => (Color::hex(0x0f766e), Color::hex(0x334155)),
    }
}

pub fn meeting(urgency: MeetingUrgency, is_next: bool) -> Color {
    match urgency {
        MeetingUrgency::Now => MEETING_NOW,
        MeetingUrgency::Imminent => MEETING_IMMINENT,
        MeetingUrgency::Soon => MEETING_SOON,
        _ if is_next => MEETING_NEXT,
        _ => MEETING_FOLLOWING,
    }
}

pub fn usage(severity: UsageSeverity, base: Color) -> Color {
    match severity {
        UsageSeverity::Normal => base,
        UsageSeverity::Warning => WARNING,
        UsageSeverity::Critical => CRITICAL,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_colours_are_distinct() {
        let colours = [
            phase(Phase::Focus),
            phase(Phase::ShortBreak),
            phase(Phase::LongBreak),
        ];
        assert_eq!(
            colours
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            3
        );
    }

    #[test]
    fn water_bands_step_at_the_documented_temperatures() {
        assert_eq!(water(4.0), Color::hex(0x1d4ed8));
        assert_eq!(water(9.9), Color::hex(0x1d4ed8));
        assert_eq!(water(10.0), Color::hex(0x0369a1));
        assert_eq!(water(19.9), Color::hex(0x0e7490));
        assert_eq!(water(20.0), Color::hex(0x0f766e));
        assert_eq!(water(24.0), Color::hex(0xc2410c));
    }

    #[test]
    fn every_symbol_family_has_a_sky_and_night_differs_for_clear_skies() {
        let day = WeatherSymbol {
            family: SymbolFamily::Clear,
            night: false,
        };
        let night = WeatherSymbol {
            family: SymbolFamily::Clear,
            night: true,
        };
        assert_ne!(sky(day), sky(night));

        // Overcast families keep their own sky regardless of the hour so heavy
        // weather never reads as a clear night.
        let rain_day = WeatherSymbol {
            family: SymbolFamily::Rain,
            night: false,
        };
        let rain_night = WeatherSymbol {
            family: SymbolFamily::Rain,
            night: true,
        };
        assert_eq!(sky(rain_day), sky(rain_night));
    }

    #[test]
    fn meeting_urgency_beats_tile_position() {
        assert_eq!(meeting(MeetingUrgency::Now, true), MEETING_NOW);
        assert_eq!(meeting(MeetingUrgency::Now, false), MEETING_NOW);
        assert_eq!(meeting(MeetingUrgency::Later, true), MEETING_NEXT);
        assert_eq!(meeting(MeetingUrgency::Later, false), MEETING_FOLLOWING);
        assert_eq!(meeting(MeetingUrgency::Today, false), MEETING_FOLLOWING);
    }

    #[test]
    fn usage_severity_overrides_the_service_colour() {
        assert_eq!(usage(UsageSeverity::Normal, CLAUDE), CLAUDE);
        assert_eq!(usage(UsageSeverity::Warning, CLAUDE), WARNING);
        assert_eq!(usage(UsageSeverity::Critical, CODEX), CRITICAL);
    }
}
