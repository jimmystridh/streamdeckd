//! The semantic key-view model.
//!
//! Actions and integrations never draw pixels. They fill a [`KeyView`], and the
//! renderer applies one consistent set of typography, spacing, colours,
//! truncation, and error treatments to every key on every page.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Parses `#rrggbb`. Used only by the theme tables, never by user input.
    pub const fn hex(value: u32) -> Self {
        Self {
            r: ((value >> 16) & 0xff) as u8,
            g: ((value >> 8) & 0xff) as u8,
            b: (value & 0xff) as u8,
        }
    }

    pub fn mix(self, other: Color, amount: f32) -> Color {
        let amount = amount.clamp(0.0, 1.0);
        let blend = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * amount).round() as u8;
        Color::rgb(
            blend(self.r, other.r),
            blend(self.g, other.g),
            blend(self.b, other.b),
        )
    }

    pub fn darken(self, amount: f32) -> Color {
        self.mix(Color::rgb(0, 0, 0), amount)
    }

    pub fn lighten(self, amount: f32) -> Color {
        self.mix(Color::rgb(255, 255, 255), amount)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Background {
    Solid(Color),
    /// Diagonal gradient, matching the weather tiles.
    Diagonal {
        top: Color,
        bottom: Color,
    },
    /// Vertical gradient, matching the water tiles.
    Vertical {
        top: Color,
        bottom: Color,
    },
}

impl Background {
    pub fn representative(self) -> Color {
        match self {
            Background::Solid(color) => color,
            Background::Diagonal { top, bottom } | Background::Vertical { top, bottom } => {
                top.mix(bottom, 0.5)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FontFamily {
    /// Inter, for labels and titles.
    Ui,
    /// JetBrains Mono, for countdowns and other values that must not jitter.
    Mono,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Weight {
    Regular,
    Semibold,
    Bold,
    Black,
}

impl Weight {
    pub const fn axis_value(self) -> f32 {
        match self {
            Weight::Regular => 400.0,
            Weight::Semibold => 600.0,
            Weight::Bold => 700.0,
            Weight::Black => 900.0,
        }
    }
}

/// A single run of text with everything the renderer needs and nothing more.
///
/// `size` is in the renderer's internal 144x144 coordinate space, so a value of
/// 16 becomes roughly 8 device pixels after the downsample to 72x72.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextRun {
    pub text: String,
    pub size: f32,
    pub weight: Weight,
    pub family: FontFamily,
    pub opacity: f32,
}

impl TextRun {
    pub fn new(text: impl Into<String>, size: f32, weight: Weight) -> Self {
        Self {
            text: text.into(),
            size,
            weight,
            family: FontFamily::Ui,
            opacity: 1.0,
        }
    }

    pub fn mono(mut self) -> Self {
        self.family = FontFamily::Mono;
        self
    }

    pub fn opacity(mut self, opacity: f32) -> Self {
        self.opacity = opacity.clamp(0.0, 1.0);
        self
    }
}

/// Project-owned vector glyphs. Drawing these as paths avoids depending on a font
/// happening to contain a play triangle or a check mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Icon {
    Play,
    Pause,
    PlayPause,
    Next,
    Previous,
    Skip,
    Reset,
    Refresh,
    Check,
    Cross,
    Plus,
    Minus,
    Shuffle,
    Repeat,
    RepeatOne,
    Home,
    Speaker,
    SpeakerMuted,
    Microphone,
    MicrophoneMuted,
    Calendar,
    Tomato,
    GitHub,
    Note,
    Sun,
    Moon,
    Cloud,
    Rain,
    Snow,
    Sleet,
    Thunder,
    Fog,
    Water,
    TrendUp,
    TrendDown,
    Warning,
}

/// A ring gauge, used for the Pomodoro countdown, the panel countdown, and usage.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Progress {
    /// Fraction remaining or used, in `0.0..=1.0`.
    pub fraction: f32,
    pub track: Color,
    pub fill: Color,
}

/// A small corner badge, used for meeting ordinals and page hints.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Badge {
    pub text: String,
    pub background: Color,
}

/// The health of the data behind a key. The renderer gives each a distinct
/// treatment so stale never looks like a hard error, and unavailable hardware
/// stays visible instead of disappearing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum KeyStatus {
    Ok,
    /// First fetch in flight.
    Loading,
    /// Showing cached data after a failed refresh.
    Stale,
    /// No usable data.
    Error,
    /// The control exists but cannot act right now.
    Disabled,
    /// A toggle or device selection that is currently active.
    Selected,
    /// A configured device matched more than one candidate.
    Ambiguous,
    /// Demanding attention, such as a pending Pomodoro completion.
    Alert,
}

/// The renderer's composition slots. A tile fills the slots it needs; the layout
/// engine positions them identically on every page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeyView {
    pub background: Background,
    /// Large decorative artwork behind the text, drawn in the reserved art region.
    pub art: Option<Icon>,
    /// A centred glyph, used where a control has no numeric value.
    pub glyph: Option<Icon>,
    pub header: Option<TextRun>,
    pub header_right: Option<TextRun>,
    /// The dominant value, centred in the reserved value region.
    pub value: Option<TextRun>,
    pub subvalue: Option<TextRun>,
    pub footer_left: Option<TextRun>,
    pub footer_right: Option<TextRun>,
    pub footer_center: Option<TextRun>,
    /// Label/value rows for detail panels. Rendered instead of `value`.
    pub rows: Vec<(String, String)>,
    pub badge: Option<Badge>,
    pub progress: Option<Progress>,
    pub status: KeyStatus,
    /// The key is physically held down.
    pub pressed: bool,
    /// The long-press threshold has been reached; draw the unmistakable affordance.
    pub armed: bool,
    /// Optional artwork key. The renderer resolves it against its image cache.
    pub artwork: Option<String>,
}

impl Default for KeyView {
    fn default() -> Self {
        Self {
            background: Background::Solid(Color::hex(0x1e293b)),
            art: None,
            glyph: None,
            header: None,
            header_right: None,
            value: None,
            subvalue: None,
            footer_left: None,
            footer_right: None,
            footer_center: None,
            rows: Vec::new(),
            badge: None,
            progress: None,
            status: KeyStatus::Ok,
            pressed: false,
            armed: false,
            artwork: None,
        }
    }
}

impl KeyView {
    pub fn solid(color: Color) -> Self {
        Self {
            background: Background::Solid(color),
            ..Default::default()
        }
    }

    pub fn header(mut self, text: impl Into<String>) -> Self {
        self.header = Some(TextRun::new(text, 16.0, Weight::Bold).opacity(0.92));
        self
    }

    pub fn header_right(mut self, text: impl Into<String>) -> Self {
        self.header_right = Some(TextRun::new(text, 14.0, Weight::Bold).opacity(0.72));
        self
    }

    pub fn value(mut self, text: impl Into<String>, size: f32) -> Self {
        self.value = Some(TextRun::new(text, size, Weight::Black));
        self
    }

    pub fn mono_value(mut self, text: impl Into<String>, size: f32) -> Self {
        self.value = Some(TextRun::new(text, size, Weight::Bold).mono());
        self
    }

    pub fn subvalue(mut self, text: impl Into<String>) -> Self {
        self.subvalue = Some(TextRun::new(text, 15.0, Weight::Bold).opacity(0.8));
        self
    }

    pub fn footer(mut self, text: impl Into<String>) -> Self {
        self.footer_center = Some(TextRun::new(text, 14.0, Weight::Semibold).opacity(0.82));
        self
    }

    pub fn footers(mut self, left: impl Into<String>, right: impl Into<String>) -> Self {
        self.footer_left = Some(TextRun::new(left, 16.0, Weight::Bold).opacity(0.9));
        self.footer_right = Some(TextRun::new(right, 16.0, Weight::Bold).opacity(0.9));
        self
    }

    pub fn glyph(mut self, icon: Icon) -> Self {
        self.glyph = Some(icon);
        self
    }

    pub fn art(mut self, icon: Icon) -> Self {
        self.art = Some(icon);
        self
    }

    pub fn status(mut self, status: KeyStatus) -> Self {
        self.status = status;
        self
    }

    pub fn badge(mut self, text: impl Into<String>, background: Color) -> Self {
        self.badge = Some(Badge {
            text: text.into(),
            background,
        });
        self
    }

    pub fn progress(mut self, fraction: f32, track: Color, fill: Color) -> Self {
        self.progress = Some(Progress {
            fraction: fraction.clamp(0.0, 1.0),
            track,
            fill,
        });
        self
    }

    pub fn rows(mut self, rows: Vec<(String, String)>) -> Self {
        self.rows = rows;
        self
    }

    pub fn artwork(mut self, key: impl Into<String>) -> Self {
        self.artwork = Some(key.into());
        self
    }

    /// The blank key used by intentionally empty positions. It is a real, rendered
    /// key so the layout keeps its shape instead of showing whatever was there before.
    pub fn blank() -> Self {
        Self::solid(Color::hex(0x0b1120))
    }

    /// A key whose data has not arrived yet.
    pub fn loading(label: &str) -> Self {
        Self::solid(Color::hex(0x1e293b))
            .header(label)
            .value("…", 30.0)
            .status(KeyStatus::Loading)
    }

    /// A key whose data could not be fetched at all.
    pub fn error(label: &str, detail: &str) -> Self {
        Self::solid(Color::hex(0x7f1d1d))
            .header(label)
            .glyph(Icon::Warning)
            .footer(detail)
            .status(KeyStatus::Error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_colours_decode_channelwise() {
        assert_eq!(Color::hex(0x1db954), Color::rgb(0x1d, 0xb9, 0x54));
        assert_eq!(Color::hex(0x000000), Color::rgb(0, 0, 0));
        assert_eq!(Color::hex(0xffffff), Color::rgb(255, 255, 255));
    }

    #[test]
    fn mixing_is_endpoint_exact_and_monotonic() {
        let black = Color::rgb(0, 0, 0);
        let white = Color::rgb(255, 255, 255);

        assert_eq!(black.mix(white, 0.0), black);
        assert_eq!(black.mix(white, 1.0), white);
        assert_eq!(black.mix(white, 0.5), Color::rgb(128, 128, 128));
        assert_eq!(black.mix(white, 2.0), white, "amounts are clamped");
        assert_eq!(white.darken(1.0), black);
        assert_eq!(black.lighten(1.0), white);
    }

    #[test]
    fn a_gradient_reports_a_representative_colour_for_derived_treatments() {
        let background = Background::Diagonal {
            top: Color::rgb(0, 0, 0),
            bottom: Color::rgb(200, 100, 0),
        };
        assert_eq!(background.representative(), Color::rgb(100, 50, 0));
    }

    #[test]
    fn progress_and_opacity_are_clamped_into_range() {
        let view = KeyView::solid(Color::hex(0x123456)).progress(
            5.0,
            Color::hex(0x000000),
            Color::hex(0xffffff),
        );
        assert_eq!(view.progress.expect("progress").fraction, 1.0);

        let run = TextRun::new("x", 10.0, Weight::Bold).opacity(-1.0);
        assert_eq!(run.opacity, 0.0);
    }

    #[test]
    fn the_builders_compose_into_the_expected_slots() {
        let view = KeyView::solid(Color::hex(0x1db954))
            .header("SPOTIFY")
            .header_right("LIVE")
            .value("42", 30.0)
            .subvalue("RUNNING")
            .footers("H 21°", "L 12°")
            .glyph(Icon::Play)
            .art(Icon::Note)
            .badge("2", Color::hex(0x000000))
            .status(KeyStatus::Selected);

        assert_eq!(view.header.expect("header").text, "SPOTIFY");
        assert_eq!(view.value.expect("value").size, 30.0);
        assert_eq!(view.footer_left.expect("footer").text, "H 21°");
        assert_eq!(view.glyph, Some(Icon::Play));
        assert_eq!(view.art, Some(Icon::Note));
        assert_eq!(view.badge.expect("badge").text, "2");
        assert_eq!(view.status, KeyStatus::Selected);
    }

    #[test]
    fn placeholder_views_carry_their_status() {
        assert_eq!(KeyView::loading("GITHUB").status, KeyStatus::Loading);
        assert_eq!(KeyView::error("GITHUB", "offline").status, KeyStatus::Error);
        assert_eq!(KeyView::blank().status, KeyStatus::Ok);
        assert!(KeyView::blank().value.is_none());
    }

    #[test]
    fn mono_runs_pick_the_monospaced_family() {
        let view = KeyView::solid(Color::hex(0)).mono_value("25:00", 28.0);
        let value = view.value.expect("value");
        assert_eq!(value.family, FontFamily::Mono);
        assert_eq!(value.text, "25:00");
    }
}
