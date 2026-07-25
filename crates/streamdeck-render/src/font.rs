//! Deterministic text rendering from two embedded variable fonts.
//!
//! Glyph outlines are converted straight into `tiny-skia` paths, so text is
//! anti-aliased by the same rasteriser as everything else and no system font is
//! ever consulted. Both faces are variable, so a weight is a variation setting
//! rather than another embedded file.

use std::collections::HashMap;

use streamdeck_core::view::{FontFamily, TextRun, Weight};
use tiny_skia::{Path, PathBuilder, Transform};
use ttf_parser::{Face, GlyphId, OutlineBuilder, Tag};

/// Inter, SIL Open Font License 1.1. See `assets/fonts/Inter-OFL.txt`.
const INTER: &[u8] = include_bytes!("../../../assets/fonts/Inter.ttf");
/// JetBrains Mono, SIL Open Font License 1.1. See `assets/fonts/JetBrainsMono-OFL.txt`.
const JETBRAINS_MONO: &[u8] = include_bytes!("../../../assets/fonts/JetBrainsMono.ttf");

const WEIGHT_AXIS: Tag = Tag::from_bytes(b"wght");

#[derive(Debug, thiserror::Error)]
pub enum FontError {
    #[error("embedded font could not be parsed: {0}")]
    Parse(#[from] ttf_parser::FaceParsingError),
}

/// One weight instance of one family.
struct Instance {
    face: Face<'static>,
    units_per_em: f32,
}

impl Instance {
    fn new(data: &'static [u8], weight: Weight) -> Result<Self, FontError> {
        let mut face = Face::parse(data, 0)?;
        // A variable face without the axis simply keeps its default weight.
        face.set_variation(WEIGHT_AXIS, weight.axis_value());
        let units_per_em = f32::from(face.units_per_em());
        Ok(Self { face, units_per_em })
    }

    fn scale(&self, size: f32) -> f32 {
        size / self.units_per_em
    }

    fn advance(&self, glyph: GlyphId) -> f32 {
        f32::from(self.face.glyph_hor_advance(glyph).unwrap_or(0))
    }

    /// Resolves a character, treating `.notdef` as absent so a tofu box never
    /// reaches the deck.
    fn glyph(&self, character: char) -> Option<GlyphId> {
        self.face
            .glyph_index(character)
            .filter(|glyph| glyph.0 != 0)
    }
}

/// The renderer's font set: one UI family and one monospaced family, each in the
/// four weights the theme uses.
pub struct Fonts {
    instances: HashMap<(FontFamily, Weight), Instance>,
}

impl Fonts {
    pub fn load() -> Result<Self, FontError> {
        let mut instances = HashMap::new();
        for weight in [
            Weight::Regular,
            Weight::Semibold,
            Weight::Bold,
            Weight::Black,
        ] {
            instances.insert((FontFamily::Ui, weight), Instance::new(INTER, weight)?);
            instances.insert(
                (FontFamily::Mono, weight),
                Instance::new(JETBRAINS_MONO, weight)?,
            );
        }
        Ok(Self { instances })
    }

    fn instance(&self, family: FontFamily, weight: Weight) -> &Instance {
        self.instances
            .get(&(family, weight))
            .or_else(|| self.instances.get(&(family, Weight::Regular)))
            .expect("every family is loaded in at least one weight")
    }

    /// Advance width of a run at its own size, in the internal 144x144 space.
    pub fn measure(&self, run: &TextRun) -> f32 {
        let instance = self.instance(run.family, run.weight);
        let scale = instance.scale(run.size);
        run.text
            .chars()
            .filter_map(|character| instance.glyph(character))
            .map(|glyph| instance.advance(glyph))
            .sum::<f32>()
            * scale
    }

    /// Distance from the baseline to the top of a capital letter, used to centre
    /// text vertically without depending on a particular string's ascenders.
    pub fn cap_height(&self, run: &TextRun) -> f32 {
        let instance = self.instance(run.family, run.weight);
        let cap = instance
            .face
            .capital_height()
            .map(f32::from)
            .unwrap_or_else(|| f32::from(instance.face.ascender()) * 0.72);
        cap * instance.scale(run.size)
    }

    /// Shrinks a run's size until it fits `max_width`, so a long Swedish label
    /// narrows rather than spilling out of its region.
    pub fn fitted(&self, run: &TextRun, max_width: f32) -> TextRun {
        const FLOOR: f32 = 6.0;
        let mut fitted = run.clone();
        while self.measure(&fitted) > max_width && fitted.size > FLOOR {
            fitted.size = (fitted.size * 0.92).max(FLOOR);
        }
        fitted
    }

    /// Builds the filled outline of a run with its left edge on `x` and its
    /// baseline on `y`. Returns `None` when the run has no drawable glyphs.
    pub fn outline(&self, run: &TextRun, x: f32, y: f32) -> Option<Path> {
        let instance = self.instance(run.family, run.weight);
        let scale = instance.scale(run.size);
        let mut builder = PathBuilder::new();
        let mut pen = 0.0;
        let mut drew = false;

        for character in run.text.chars() {
            let Some(glyph) = instance.glyph(character) else {
                continue;
            };
            let mut sink = GlyphSink {
                builder: &mut builder,
                // Font units are y-up; the canvas is y-down.
                transform: Transform::from_row(scale, 0.0, 0.0, -scale, x + pen * scale, y),
            };
            if instance.face.outline_glyph(glyph, &mut sink).is_some() {
                drew = true;
            }
            pen += instance.advance(glyph);
        }

        drew.then(|| builder.finish()).flatten()
    }

    /// True when every character in `text` has a glyph in the chosen family.
    pub fn covers(&self, family: FontFamily, weight: Weight, text: &str) -> bool {
        let instance = self.instance(family, weight);
        text.chars()
            .filter(|character| !character.is_whitespace())
            .all(|character| instance.glyph(character).is_some())
    }
}

/// Feeds `ttf-parser` outlines into a `tiny-skia` path with a transform applied.
struct GlyphSink<'a> {
    builder: &'a mut PathBuilder,
    transform: Transform,
}

impl GlyphSink<'_> {
    fn map(&self, x: f32, y: f32) -> (f32, f32) {
        let mut point = [tiny_skia::Point::from_xy(x, y)];
        self.transform.map_points(&mut point);
        (point[0].x, point[0].y)
    }
}

impl OutlineBuilder for GlyphSink<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.map(x, y);
        self.builder.move_to(x, y);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.map(x, y);
        self.builder.line_to(x, y);
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (x1, y1) = self.map(x1, y1);
        let (x, y) = self.map(x, y);
        self.builder.quad_to(x1, y1, x, y);
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (x1, y1) = self.map(x1, y1);
        let (x2, y2) = self.map(x2, y2);
        let (x, y) = self.map(x, y);
        self.builder.cubic_to(x1, y1, x2, y2, x, y);
    }

    fn close(&mut self) {
        self.builder.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fonts() -> Fonts {
        Fonts::load().expect("embedded fonts load")
    }

    fn run(text: &str, size: f32) -> TextRun {
        TextRun::new(text, size, Weight::Bold)
    }

    #[test]
    fn both_embedded_families_load_in_every_weight() {
        let fonts = fonts();
        for family in [FontFamily::Ui, FontFamily::Mono] {
            for weight in [
                Weight::Regular,
                Weight::Semibold,
                Weight::Bold,
                Weight::Black,
            ] {
                assert!(fonts.instances.contains_key(&(family, weight)));
            }
        }
    }

    #[test]
    fn measurement_grows_with_text_length_and_size() {
        let fonts = fonts();
        let short = fonts.measure(&run("25", 28.0));
        let long = fonts.measure(&run("25:00", 28.0));
        let large = fonts.measure(&run("25:00", 56.0));

        assert!(long > short, "{long} should exceed {short}");
        assert!((large - long * 2.0).abs() < 0.5, "size scales linearly");
        assert_eq!(fonts.measure(&run("", 28.0)), 0.0);
    }

    #[test]
    fn a_heavier_weight_is_at_least_as_wide() {
        let fonts = fonts();
        let regular = fonts.measure(&TextRun::new("STENSJÖN", 16.0, Weight::Regular));
        let black = fonts.measure(&TextRun::new("STENSJÖN", 16.0, Weight::Black));
        assert!(black >= regular, "{black} < {regular}");
    }

    #[test]
    fn the_monospaced_family_gives_every_digit_the_same_advance() {
        let fonts = fonts();
        let widths: Vec<f32> = "0123456789"
            .chars()
            .map(|digit| fonts.measure(&TextRun::new(digit.to_string(), 28.0, Weight::Bold).mono()))
            .collect();
        let first = widths[0];
        for width in &widths {
            assert!(
                (width - first).abs() < 0.01,
                "digit advances differ: {width} vs {first}"
            );
        }
    }

    #[test]
    fn a_countdown_never_changes_width_as_digits_change() {
        let fonts = fonts();
        let a = fonts.measure(&TextRun::new("25:00", 28.0, Weight::Bold).mono());
        let b = fonts.measure(&TextRun::new("11:11", 28.0, Weight::Bold).mono());
        assert!((a - b).abs() < 0.01, "{a} vs {b}");
    }

    #[test]
    fn every_character_the_tiles_use_has_a_glyph() {
        let fonts = fonts();
        let alphabet = concat!(
            "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789",
            " .,:;/-+%#°…·ÅÄÖåäöØøéÉ!?()'\"",
        );
        assert!(
            fonts.covers(FontFamily::Ui, Weight::Bold, alphabet),
            "the UI font is missing a character the tiles use"
        );
        assert!(
            fonts.covers(FontFamily::Mono, Weight::Bold, "0123456789:.-"),
            "the monospaced font is missing a countdown character"
        );
    }

    #[test]
    fn swedish_text_measures_and_outlines() {
        let fonts = fonts();
        let run = run("Stensjön Årsmöte", 16.0);
        assert!(fonts.measure(&run) > 0.0);
        assert!(fonts.outline(&run, 12.0, 21.0).is_some());
    }

    #[test]
    fn outlines_land_where_they_are_placed() {
        let fonts = fonts();
        let run = run("H", 40.0);
        let path = fonts.outline(&run, 20.0, 100.0).expect("outline");
        let bounds = path.bounds();

        assert!(bounds.left() >= 19.0, "left {}", bounds.left());
        assert!(bounds.right() <= 20.0 + 40.0, "right {}", bounds.right());
        // The baseline is at y = 100, so the glyph must sit above it.
        assert!(bounds.bottom() <= 101.0, "bottom {}", bounds.bottom());
        assert!(bounds.top() > 100.0 - 40.0, "top {}", bounds.top());
    }

    #[test]
    fn text_with_no_drawable_glyphs_produces_no_path() {
        let fonts = fonts();
        assert!(fonts.outline(&run("", 20.0), 0.0, 0.0).is_none());
        assert!(fonts.outline(&run("   ", 20.0), 0.0, 0.0).is_none());
    }

    #[test]
    fn cap_height_is_a_sane_fraction_of_the_size() {
        let fonts = fonts();
        let cap = fonts.cap_height(&run("X", 40.0));
        assert!((20.0..40.0).contains(&cap), "cap height {cap}");
    }

    #[test]
    fn fitting_shrinks_only_what_does_not_fit() {
        let fonts = fonts();
        let short = run("21°", 34.0);
        assert_eq!(fonts.fitted(&short, 120.0).size, 34.0);

        let long = run("Architecture review", 34.0);
        let fitted = fonts.fitted(&long, 120.0);
        assert!(fitted.size < 34.0, "fitted size {}", fitted.size);
        assert!(fonts.measure(&fitted) <= 120.0 + 0.5);
    }

    #[test]
    fn fitting_never_shrinks_below_a_readable_floor() {
        let fonts = fonts();
        let impossible = run("a very long string that cannot possibly fit", 14.0);
        let fitted = fonts.fitted(&impossible, 4.0);
        assert!(fitted.size >= 6.0, "fitted size {}", fitted.size);
    }

    #[test]
    fn an_unknown_character_is_skipped_rather_than_panicking() {
        let fonts = fonts();
        // U+0378 is unassigned in Unicode, so no family can have a glyph for it.
        let run = run("A\u{0378}B", 20.0);
        assert!(fonts.outline(&run, 0.0, 20.0).is_some());
        assert!(!fonts.covers(FontFamily::Ui, Weight::Bold, "\u{0378}"));
    }
}
