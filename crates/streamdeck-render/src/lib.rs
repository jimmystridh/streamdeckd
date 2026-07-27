//! Native key renderer.
//!
//! The pipeline follows the plan exactly: resolve the semantic view, draw at an
//! internal 144x144 resolution, downsample to the device's 72x72 key size,
//! hash the result, and let the caller skip an unchanged payload.

pub mod font;
pub mod icons;

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use streamdeck_core::view::{Background, Color, Icon, KeyStatus, KeyView, TextRun, Weight};
use tiny_skia::{
    FillRule, LinearGradient, Paint, PathBuilder, Pixmap, Point, Rect, Shader, Stroke, Transform,
};

use font::{FontError, Fonts};

/// Internal drawing resolution. Twice the device key size, so curves and small
/// text survive the downsample.
pub const INTERNAL_SIZE: u32 = 144;
/// The Stream Deck MK.2 key image size.
pub const KEY_SIZE: u32 = 72;

const CORNER_RADIUS: f32 = 22.0;

/// A rendered key: device-sized RGB pixels plus a hash of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedKey {
    pub size: u32,
    /// Row-major RGB, `size * size * 3` bytes.
    pub rgb: Vec<u8>,
    /// Content hash, so an unchanged key is never written to the device again.
    pub hash: u64,
}

impl RenderedKey {
    /// Encodes the key as a PNG, for previews, goldens, and the CLI.
    pub fn to_png(&self) -> Result<Vec<u8>, RenderError> {
        let mut png = Vec::new();
        let image =
            image::RgbImage::from_raw(self.size, self.size, self.rgb.clone()).ok_or_else(|| {
                RenderError::Encode("rendered key has the wrong number of bytes".to_string())
            })?;
        image::DynamicImage::ImageRgb8(image)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .map_err(|error| RenderError::Encode(error.to_string()))?;
        Ok(png)
    }

    pub fn to_image(&self) -> Result<image::RgbImage, RenderError> {
        image::RgbImage::from_raw(self.size, self.size, self.rgb.clone()).ok_or_else(|| {
            RenderError::Encode("rendered key has the wrong number of bytes".to_string())
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error(transparent)]
    Font(#[from] FontError),
    #[error("could not allocate a {0}x{0} canvas")]
    Canvas(u32),
    #[error("could not encode the rendered key: {0}")]
    Encode(String),
}

/// Decoded album artwork, kept in a small least-recently-used cache.
#[derive(Debug, Clone)]
struct Artwork {
    pixmap: Pixmap,
    last_used: u64,
}

/// Renders semantic key views. Owns the fonts and the bounded artwork cache.
pub struct Renderer {
    fonts: Fonts,
    key_size: u32,
    artwork: HashMap<String, Artwork>,
    artwork_limit: usize,
    clock: u64,
}

impl Renderer {
    pub fn new() -> Result<Self, RenderError> {
        Ok(Self {
            fonts: Fonts::load()?,
            key_size: KEY_SIZE,
            artwork: HashMap::new(),
            artwork_limit: 8,
            clock: 0,
        })
    }

    pub fn with_key_size(mut self, key_size: u32) -> Self {
        self.key_size = key_size.max(1);
        self
    }

    pub fn fonts(&self) -> &Fonts {
        &self.fonts
    }

    /// Decodes and stores artwork under a track identity, evicting the least
    /// recently used entry once the cache is full.
    pub fn cache_artwork(&mut self, key: &str, encoded: &[u8]) -> Result<(), RenderError> {
        let decoded = image::load_from_memory(encoded)
            .map_err(|error| RenderError::Encode(error.to_string()))?
            .thumbnail_exact(INTERNAL_SIZE, INTERNAL_SIZE)
            .to_rgba8();
        let mut pixmap =
            Pixmap::new(INTERNAL_SIZE, INTERNAL_SIZE).ok_or(RenderError::Canvas(INTERNAL_SIZE))?;
        pixmap
            .data_mut()
            .copy_from_slice(premultiply(decoded.as_raw()).as_slice());

        self.clock += 1;
        self.artwork.insert(
            key.to_string(),
            Artwork {
                pixmap,
                last_used: self.clock,
            },
        );
        while self.artwork.len() > self.artwork_limit {
            let oldest = self
                .artwork
                .iter()
                .min_by_key(|(_, artwork)| artwork.last_used)
                .map(|(key, _)| key.clone());
            match oldest {
                Some(key) => {
                    self.artwork.remove(&key);
                }
                None => break,
            }
        }
        Ok(())
    }

    pub fn has_artwork(&self, key: &str) -> bool {
        self.artwork.contains_key(key)
    }

    pub fn artwork_count(&self) -> usize {
        self.artwork.len()
    }

    pub fn render(&mut self, view: &KeyView) -> Result<RenderedKey, RenderError> {
        let canvas = self.draw(view)?;
        Ok(downsample(&canvas, self.key_size))
    }

    /// Renders at the internal resolution, for previews that want the detail.
    pub fn render_internal(&mut self, view: &KeyView) -> Result<RenderedKey, RenderError> {
        let canvas = self.draw(view)?;
        Ok(downsample(&canvas, INTERNAL_SIZE))
    }

    fn draw(&mut self, view: &KeyView) -> Result<Pixmap, RenderError> {
        let mut canvas =
            Pixmap::new(INTERNAL_SIZE, INTERNAL_SIZE).ok_or(RenderError::Canvas(INTERNAL_SIZE))?;
        let card = rounded_rect(0.0, 0.0, 144.0, 144.0, CORNER_RADIUS);

        self.draw_background(&mut canvas, view, &card);
        self.draw_artwork(&mut canvas, view, &card);
        self.draw_progress(&mut canvas, view);

        let art_present = view.art.is_some();
        if let Some(art) = view.art {
            self.draw_icon(
                &mut canvas,
                art,
                Rect::from_xywh(4.0, 32.0, 58.0, 70.0).expect("rect"),
                0.95,
            );
        }

        // With art on the left, the value keeps its own reserved column on the
        // right so a two-digit temperature can never overlap a weather symbol.
        let value_center_x = if art_present { 101.0 } else { 72.0 };
        let value_width = if art_present { 64.0 } else { 128.0 };

        if let Some(glyph) = view.glyph {
            let region = if view.value.is_some() {
                Rect::from_xywh(52.0, 32.0, 40.0, 40.0).expect("rect")
            } else if !view.rows.is_empty() {
                Rect::from_xywh(108.0, 12.0, 26.0, 26.0).expect("rect")
            } else {
                Rect::from_xywh(44.0, 40.0, 56.0, 56.0).expect("rect")
            };
            self.draw_icon(&mut canvas, glyph, region, 1.0);
        }

        self.draw_text_slots(&mut canvas, view, value_center_x, value_width);
        self.draw_rows(&mut canvas, view);
        self.draw_badge(&mut canvas, view);
        self.draw_status(&mut canvas, view, &card);

        Ok(canvas)
    }

    fn draw_background(&self, canvas: &mut Pixmap, view: &KeyView, card: &tiny_skia::Path) {
        let mut paint = Paint {
            anti_alias: true,
            ..Default::default()
        };
        match view.background {
            Background::Solid(color) => paint.set_color(rgba(color, 1.0)),
            Background::Diagonal { top, bottom } => {
                paint.shader = LinearGradient::new(
                    Point::from_xy(0.0, 0.0),
                    Point::from_xy(144.0, 144.0),
                    vec![
                        tiny_skia::GradientStop::new(0.0, rgba(top, 1.0)),
                        tiny_skia::GradientStop::new(1.0, rgba(bottom, 1.0)),
                    ],
                    tiny_skia::SpreadMode::Pad,
                    Transform::identity(),
                )
                .unwrap_or_else(|| Shader::SolidColor(rgba(top, 1.0)));
            }
            Background::Vertical { top, bottom } => {
                paint.shader = LinearGradient::new(
                    Point::from_xy(0.0, 0.0),
                    Point::from_xy(0.0, 144.0),
                    vec![
                        tiny_skia::GradientStop::new(0.0, rgba(top, 1.0)),
                        tiny_skia::GradientStop::new(1.0, rgba(bottom, 1.0)),
                    ],
                    tiny_skia::SpreadMode::Pad,
                    Transform::identity(),
                )
                .unwrap_or_else(|| Shader::SolidColor(rgba(top, 1.0)));
            }
        }
        canvas.fill_path(card, &paint, FillRule::Winding, Transform::identity(), None);

        // A hairline inner edge gives every key the same physical feel.
        let inner = rounded_rect(3.0, 3.0, 138.0, 138.0, CORNER_RADIUS - 3.0);
        let mut edge = Paint {
            anti_alias: true,
            ..Default::default()
        };
        edge.set_color(rgba(Color::rgb(255, 255, 255), 0.10));
        canvas.stroke_path(
            &inner,
            &edge,
            &Stroke {
                width: 3.0,
                ..Default::default()
            },
            Transform::identity(),
            None,
        );
    }

    fn draw_artwork(&mut self, canvas: &mut Pixmap, view: &KeyView, card: &tiny_skia::Path) {
        let Some(key) = &view.artwork else { return };
        self.clock += 1;
        let clock = self.clock;
        let Some(artwork) = self.artwork.get_mut(key) else {
            return;
        };
        artwork.last_used = clock;

        let mut paint = Paint {
            anti_alias: true,
            ..Default::default()
        };
        paint.shader = tiny_skia::Pattern::new(
            artwork.pixmap.as_ref(),
            tiny_skia::SpreadMode::Pad,
            tiny_skia::FilterQuality::Bilinear,
            1.0,
            Transform::identity(),
        );
        canvas.fill_path(card, &paint, FillRule::Winding, Transform::identity(), None);

        // The cover is the button, as the Elgato integration drew it; a light
        // scrim keeps the white text and transport glyph readable over any art.
        let mut scrim = Paint {
            anti_alias: true,
            ..Default::default()
        };
        scrim.set_color(rgba(Color::rgb(2, 6, 23), 0.34));
        canvas.fill_path(card, &scrim, FillRule::Winding, Transform::identity(), None);
    }

    fn draw_progress(&self, canvas: &mut Pixmap, view: &KeyView) {
        let Some(progress) = view.progress else {
            return;
        };
        let center = (72.0, if view.rows.is_empty() { 76.0 } else { 90.0 });
        let radius = 44.0;

        let mut track = Paint {
            anti_alias: true,
            ..Default::default()
        };
        track.set_color(rgba(progress.track, 0.55));
        let mut circle = PathBuilder::new();
        circle.push_circle(center.0, center.1, radius);
        if let Some(path) = circle.finish() {
            canvas.stroke_path(
                &path,
                &track,
                &Stroke {
                    width: 8.0,
                    ..Default::default()
                },
                Transform::identity(),
                None,
            );
        }

        if progress.fraction <= 0.0 {
            return;
        }
        let mut fill = Paint {
            anti_alias: true,
            ..Default::default()
        };
        fill.set_color(rgba(progress.fill, 0.95));
        if let Some(arc) = arc_path(center.0, center.1, radius, progress.fraction) {
            canvas.stroke_path(
                &arc,
                &fill,
                &Stroke {
                    width: 8.0,
                    line_cap: tiny_skia::LineCap::Round,
                    ..Default::default()
                },
                Transform::identity(),
                None,
            );
        }
    }

    fn draw_icon(&self, canvas: &mut Pixmap, icon: Icon, region: Rect, opacity: f32) {
        let scale = region.width().min(region.height());
        let offset_x = region.x() + (region.width() - scale) / 2.0;
        let offset_y = region.y() + (region.height() - scale) / 2.0;
        let transform = Transform::from_row(scale, 0.0, 0.0, scale, offset_x, offset_y);

        for layer in icons::layers(icon) {
            let mut paint = Paint {
                anti_alias: true,
                ..Default::default()
            };
            let color = layer.tint.unwrap_or(Color::rgb(255, 255, 255));
            paint.set_color(rgba(color, layer.opacity * opacity));
            match layer.stroke {
                Some(width) => canvas.stroke_path(
                    &layer.path,
                    &paint,
                    &Stroke {
                        width,
                        line_cap: tiny_skia::LineCap::Round,
                        line_join: tiny_skia::LineJoin::Round,
                        ..Default::default()
                    },
                    transform,
                    None,
                ),
                None => canvas.fill_path(&layer.path, &paint, FillRule::Winding, transform, None),
            };
        }
    }

    fn draw_text_slots(
        &self,
        canvas: &mut Pixmap,
        view: &KeyView,
        value_center_x: f32,
        value_width: f32,
    ) {
        if let Some(header) = &view.header {
            let reserved = if view.header_right.is_some() {
                72.0
            } else {
                120.0
            };
            self.draw_left(canvas, header, 12.0, 24.0, reserved);
        }
        if let Some(right) = &view.header_right {
            self.draw_right(canvas, right, 132.0, 24.0, 44.0);
        }

        let value_baseline = if view.glyph.is_some() { 118.0 } else { 88.0 };
        if let Some(value) = &view.value {
            self.draw_center(canvas, value, value_center_x, value_baseline, value_width);
        }
        if let Some(subvalue) = &view.subvalue {
            let baseline = if view.value.is_some() {
                value_baseline + 18.0
            } else {
                100.0
            };
            self.draw_center(canvas, subvalue, value_center_x, baseline, value_width);
        }

        if let Some(footer) = &view.footer_center {
            self.draw_center(canvas, footer, 72.0, 136.0, 132.0);
        }
        if let Some(left) = &view.footer_left {
            self.draw_left(canvas, left, 12.0, 134.0, 60.0);
        }
        if let Some(right) = &view.footer_right {
            self.draw_right(canvas, right, 132.0, 134.0, 60.0);
        }
    }

    fn draw_rows(&self, canvas: &mut Pixmap, view: &KeyView) {
        if view.rows.is_empty() {
            return;
        }
        let count = view.rows.len().min(4);
        let (start, step) = match count {
            1 => (84.0, 30.0),
            2 => (68.0, 30.0),
            3 => (56.0, 30.0),
            // Four rows is the weather detail card; tighter, above the footer.
            _ => (44.0, 25.0),
        };
        for (index, (label, value)) in view.rows.iter().take(count).enumerate() {
            let baseline = start + index as f32 * step;
            let label_run = TextRun::new(label.as_str(), 15.0, Weight::Bold).opacity(0.68);
            let value_run = TextRun::new(value.as_str(), 19.0, Weight::Black);
            self.draw_left(canvas, &label_run, 12.0, baseline, 68.0);
            self.draw_right(canvas, &value_run, 132.0, baseline, 60.0);
        }
    }

    fn draw_badge(&self, canvas: &mut Pixmap, view: &KeyView) {
        let Some(badge) = &view.badge else { return };
        let (cx, cy, radius) = (116.0, 32.0, 19.0);

        let mut circle = PathBuilder::new();
        circle.push_circle(cx, cy, radius);
        let Some(path) = circle.finish() else { return };

        let mut fill = Paint {
            anti_alias: true,
            ..Default::default()
        };
        fill.set_color(rgba(badge.background, 0.92));
        canvas.fill_path(&path, &fill, FillRule::Winding, Transform::identity(), None);

        let mut edge = Paint {
            anti_alias: true,
            ..Default::default()
        };
        edge.set_color(rgba(Color::rgb(255, 255, 255), 0.9));
        canvas.stroke_path(
            &path,
            &edge,
            &Stroke {
                width: 4.0,
                ..Default::default()
            },
            Transform::identity(),
            None,
        );

        let run = TextRun::new(badge.text.as_str(), 21.0, Weight::Black);
        let cap = self.fonts.cap_height(&run);
        self.draw_center(canvas, &run, cx, cy + cap / 2.0, radius * 1.6);
    }

    /// Status, press, and armed treatments. Drawn last so they are never hidden.
    fn draw_status(&self, canvas: &mut Pixmap, view: &KeyView, card: &tiny_skia::Path) {
        let scrim = match view.status {
            KeyStatus::Disabled => Some(0.5),
            KeyStatus::Loading => Some(0.25),
            _ => None,
        };
        if let Some(alpha) = scrim {
            let mut paint = Paint {
                anti_alias: true,
                ..Default::default()
            };
            paint.set_color(rgba(Color::rgb(2, 6, 23), alpha));
            canvas.fill_path(card, &paint, FillRule::Winding, Transform::identity(), None);
        }

        if view.pressed {
            let mut paint = Paint {
                anti_alias: true,
                ..Default::default()
            };
            paint.set_color(rgba(Color::rgb(255, 255, 255), 0.16));
            canvas.fill_path(card, &paint, FillRule::Winding, Transform::identity(), None);
        }

        let border = match view.status {
            KeyStatus::Selected => Some((Color::hex(0xffffff), 5.0, 0.85)),
            KeyStatus::Ambiguous => Some((Color::hex(0xfbbf24), 5.0, 0.95)),
            KeyStatus::Alert => Some((Color::hex(0xffffff), 7.0, 0.95)),
            KeyStatus::Stale => Some((Color::hex(0xfcd34d), 4.0, 0.9)),
            KeyStatus::Error => Some((Color::hex(0xf87171), 5.0, 0.95)),
            _ => None,
        };
        if let Some((color, width, alpha)) = border {
            let inset = width / 2.0 + 1.0;
            let path = rounded_rect(
                inset,
                inset,
                144.0 - inset * 2.0,
                144.0 - inset * 2.0,
                CORNER_RADIUS - inset,
            );
            let mut paint = Paint {
                anti_alias: true,
                ..Default::default()
            };
            paint.set_color(rgba(color, alpha));
            canvas.stroke_path(
                &path,
                &paint,
                &Stroke {
                    width,
                    ..Default::default()
                },
                Transform::identity(),
                None,
            );
        }

        if view.status == KeyStatus::Stale {
            let run = TextRun::new("STALE", 13.0, Weight::Black);
            self.draw_right(canvas, &run, 130.0, 44.0, 60.0);
        }

        if view.armed {
            self.draw_armed(canvas);
        }
    }

    /// The long-press affordance: an unmistakable border plus a `HOLD` pill.
    fn draw_armed(&self, canvas: &mut Pixmap) {
        let path = rounded_rect(5.0, 5.0, 134.0, 134.0, CORNER_RADIUS - 5.0);
        let mut paint = Paint {
            anti_alias: true,
            ..Default::default()
        };
        paint.set_color(rgba(Color::hex(0x7c3aed), 1.0));
        canvas.stroke_path(
            &path,
            &paint,
            &Stroke {
                width: 10.0,
                ..Default::default()
            },
            Transform::identity(),
            None,
        );

        let pill = rounded_rect(30.0, 100.0, 84.0, 30.0, 15.0);
        let mut fill = Paint {
            anti_alias: true,
            ..Default::default()
        };
        fill.set_color(rgba(Color::hex(0x7c3aed), 0.96));
        canvas.fill_path(&pill, &fill, FillRule::Winding, Transform::identity(), None);

        let run = TextRun::new("HOLD", 18.0, Weight::Black);
        self.draw_center(canvas, &run, 62.0, 122.0, 50.0);
        self.draw_icon(
            canvas,
            Icon::Check,
            Rect::from_xywh(88.0, 106.0, 20.0, 20.0).expect("rect"),
            1.0,
        );
    }

    fn draw_left(&self, canvas: &mut Pixmap, run: &TextRun, x: f32, baseline: f32, max_width: f32) {
        let fitted = self.fonts.fitted(run, max_width);
        self.fill_text(canvas, &fitted, x, baseline);
    }

    fn draw_right(
        &self,
        canvas: &mut Pixmap,
        run: &TextRun,
        x: f32,
        baseline: f32,
        max_width: f32,
    ) {
        let fitted = self.fonts.fitted(run, max_width);
        let width = self.fonts.measure(&fitted);
        self.fill_text(canvas, &fitted, x - width, baseline);
    }

    fn draw_center(
        &self,
        canvas: &mut Pixmap,
        run: &TextRun,
        center_x: f32,
        baseline: f32,
        max_width: f32,
    ) {
        let fitted = self.fonts.fitted(run, max_width);
        let width = self.fonts.measure(&fitted);
        self.fill_text(canvas, &fitted, center_x - width / 2.0, baseline);
    }

    fn fill_text(&self, canvas: &mut Pixmap, run: &TextRun, x: f32, baseline: f32) {
        let Some(path) = self.fonts.outline(run, x, baseline) else {
            return;
        };
        // A soft shadow keeps light text readable over artwork and bright skies.
        let mut shadow = Paint {
            anti_alias: true,
            ..Default::default()
        };
        shadow.set_color(rgba(Color::rgb(2, 6, 23), 0.35 * run.opacity));
        canvas.fill_path(
            &path,
            &shadow,
            FillRule::Winding,
            Transform::from_translate(0.0, 1.5),
            None,
        );

        let mut paint = Paint {
            anti_alias: true,
            ..Default::default()
        };
        paint.set_color(rgba(Color::rgb(255, 255, 255), run.opacity));
        canvas.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

fn rgba(color: Color, alpha: f32) -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8(
        color.r,
        color.g,
        color.b,
        (alpha.clamp(0.0, 1.0) * 255.0).round() as u8,
    )
}

fn rounded_rect(x: f32, y: f32, width: f32, height: f32, radius: f32) -> tiny_skia::Path {
    let radius = radius.clamp(0.0, width.min(height) / 2.0);
    let mut builder = PathBuilder::new();
    builder.move_to(x + radius, y);
    builder.line_to(x + width - radius, y);
    builder.quad_to(x + width, y, x + width, y + radius);
    builder.line_to(x + width, y + height - radius);
    builder.quad_to(x + width, y + height, x + width - radius, y + height);
    builder.line_to(x + radius, y + height);
    builder.quad_to(x, y + height, x, y + height - radius);
    builder.line_to(x, y + radius);
    builder.quad_to(x, y, x + radius, y);
    builder.close();
    builder.finish().expect("rounded rect path")
}

/// A clockwise arc from twelve o'clock covering `fraction` of the circle.
fn arc_path(cx: f32, cy: f32, radius: f32, fraction: f32) -> Option<tiny_skia::Path> {
    let fraction = fraction.clamp(0.0, 1.0);
    if fraction <= 0.0 {
        return None;
    }
    let steps = (128.0 * fraction).ceil().max(2.0) as usize;
    let mut builder = PathBuilder::new();
    for step in 0..=steps {
        let angle = -std::f32::consts::FRAC_PI_2
            + std::f32::consts::TAU * fraction * (step as f32 / steps as f32);
        let (x, y) = (cx + radius * angle.cos(), cy + radius * angle.sin());
        if step == 0 {
            builder.move_to(x, y);
        } else {
            builder.line_to(x, y);
        }
    }
    builder.finish()
}

/// Converts straight RGBA into the premultiplied layout `tiny-skia` expects.
fn premultiply(rgba: &[u8]) -> Vec<u8> {
    rgba.chunks_exact(4)
        .flat_map(|pixel| {
            let alpha = u32::from(pixel[3]);
            let scale = |channel: u8| ((u32::from(channel) * alpha + 127) / 255) as u8;
            [scale(pixel[0]), scale(pixel[1]), scale(pixel[2]), pixel[3]]
        })
        .collect()
}

/// Box-filters the internal canvas down to the device key size and hashes it.
fn downsample(canvas: &Pixmap, target: u32) -> RenderedKey {
    let source = canvas.width();
    let mut rgb = Vec::with_capacity((target * target * 3) as usize);

    if target == source {
        for pixel in canvas.pixels() {
            let color = pixel.demultiply();
            rgb.extend_from_slice(&[color.red(), color.green(), color.blue()]);
        }
    } else {
        let factor = source / target;
        let pixels = canvas.pixels();
        for y in 0..target {
            for x in 0..target {
                let (mut r, mut g, mut b) = (0u32, 0u32, 0u32);
                for dy in 0..factor {
                    for dx in 0..factor {
                        let index = ((y * factor + dy) * source + (x * factor + dx)) as usize;
                        let color = pixels[index].demultiply();
                        r += u32::from(color.red());
                        g += u32::from(color.green());
                        b += u32::from(color.blue());
                    }
                }
                let samples = factor * factor;
                rgb.extend_from_slice(&[
                    (r / samples) as u8,
                    (g / samples) as u8,
                    (b / samples) as u8,
                ]);
            }
        }
    }

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    rgb.hash(&mut hasher);
    RenderedKey {
        size: target,
        rgb,
        hash: hasher.finish(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use streamdeck_core::view::{Badge, KeyStatus, Progress};

    fn renderer() -> Renderer {
        Renderer::new().expect("renderer")
    }

    fn sample() -> KeyView {
        KeyView::solid(Color::hex(0xbe123c))
            .header("FOCUS")
            .mono_value("25:00", 28.0)
            .subvalue("RUNNING")
            .footer("TAP PAUSE")
            .progress(0.8, Color::hex(0x7f1d1d), Color::hex(0xffffff))
    }

    /// Average luminance, used to assert that a treatment actually changed the key.
    fn brightness(key: &RenderedKey) -> f64 {
        let total: u64 = key.rgb.iter().map(|channel| u64::from(*channel)).sum();
        total as f64 / key.rgb.len() as f64
    }

    #[test]
    fn a_rendered_key_is_device_sized_rgb() {
        let mut renderer = renderer();
        let key = renderer.render(&sample()).expect("rendered");

        assert_eq!(key.size, KEY_SIZE);
        assert_eq!(key.rgb.len(), (KEY_SIZE * KEY_SIZE * 3) as usize);
    }

    #[test]
    fn rendering_is_deterministic_so_unchanged_keys_are_never_resent() {
        let mut renderer = renderer();
        let first = renderer.render(&sample()).expect("rendered");
        let second = renderer.render(&sample()).expect("rendered");

        assert_eq!(first.hash, second.hash);
        assert_eq!(first.rgb, second.rgb);
    }

    #[test]
    fn a_changed_countdown_changes_the_hash() {
        let mut renderer = renderer();
        let first = renderer.render(&sample()).expect("rendered");
        let mut changed = sample();
        changed.value = Some(TextRun::new("24:59", 28.0, Weight::Bold).mono());
        let second = renderer.render(&changed).expect("rendered");

        assert_ne!(first.hash, second.hash);
    }

    #[test]
    fn a_blank_key_is_uniform_and_dark() {
        let mut renderer = renderer();
        let key = renderer.render(&KeyView::blank()).expect("rendered");

        assert!(brightness(&key) < 40.0, "blank key is too bright");
        // The centre of a blank key must be flat: no stray text or glyph.
        let center = ((KEY_SIZE / 2) * KEY_SIZE + KEY_SIZE / 2) as usize * 3;
        assert!(key.rgb[center] < 40 && key.rgb[center + 1] < 40);
    }

    #[test]
    fn corners_are_rounded_so_the_key_matches_the_physical_cap() {
        let mut renderer = renderer();
        let key = renderer
            .render(&KeyView::solid(Color::hex(0xffffff)))
            .expect("rendered");

        let corner = 0usize;
        let center = ((KEY_SIZE / 2) * KEY_SIZE + KEY_SIZE / 2) as usize * 3;
        assert!(
            key.rgb[corner] < key.rgb[center],
            "the corner should be darker than the centre"
        );
    }

    #[test]
    fn every_status_treatment_produces_a_distinct_image() {
        let mut renderer = renderer();
        let mut hashes = std::collections::HashSet::new();
        for status in [
            KeyStatus::Ok,
            KeyStatus::Loading,
            KeyStatus::Stale,
            KeyStatus::Error,
            KeyStatus::Disabled,
            KeyStatus::Selected,
            KeyStatus::Ambiguous,
            KeyStatus::Alert,
        ] {
            let view = sample().status(status);
            let key = renderer.render(&view).expect("rendered");
            assert!(
                hashes.insert(key.hash),
                "{status:?} looks like another status"
            );
        }
    }

    #[test]
    fn stale_is_visually_distinct_from_error_and_from_healthy() {
        let mut renderer = renderer();
        let healthy = renderer.render(&sample()).expect("rendered").hash;
        let stale = renderer
            .render(&sample().status(KeyStatus::Stale))
            .expect("rendered")
            .hash;
        let failed = renderer
            .render(&sample().status(KeyStatus::Error))
            .expect("rendered")
            .hash;

        assert_ne!(stale, healthy);
        assert_ne!(stale, failed);
    }

    #[test]
    fn a_disabled_key_stays_visible_but_dimmer() {
        let mut renderer = renderer();
        let normal = renderer.render(&sample()).expect("rendered");
        let disabled = renderer
            .render(&sample().status(KeyStatus::Disabled))
            .expect("rendered");

        assert!(
            brightness(&disabled) < brightness(&normal),
            "disabled should be dimmer"
        );
        assert!(
            brightness(&disabled) > 5.0,
            "disabled must not go fully black"
        );
    }

    #[test]
    fn a_pressed_key_is_brighter_than_the_same_key_at_rest() {
        let mut renderer = renderer();
        let rest = renderer.render(&sample()).expect("rendered");
        let mut pressed = sample();
        pressed.pressed = true;
        let pressed = renderer.render(&pressed).expect("rendered");

        assert!(brightness(&pressed) > brightness(&rest));
    }

    #[test]
    fn the_armed_affordance_is_unmistakable() {
        let mut renderer = renderer();
        let rest = renderer.render(&sample()).expect("rendered");
        let mut armed_view = sample();
        armed_view.armed = true;
        let armed = renderer.render(&armed_view).expect("rendered");

        let changed = rest
            .rgb
            .iter()
            .zip(&armed.rgb)
            .filter(|(before, after)| before.abs_diff(**after) > 24)
            .count();
        let fraction = changed as f64 / rest.rgb.len() as f64;
        assert!(
            fraction > 0.15,
            "only {:.1}% of the key changed when armed",
            fraction * 100.0
        );
    }

    #[test]
    fn progress_rings_of_different_lengths_differ() {
        let mut renderer = renderer();
        let mut hashes = std::collections::HashSet::new();
        for fraction in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let view = KeyView::solid(Color::hex(0x1d4ed8)).progress(
                fraction,
                Color::hex(0x0f172a),
                Color::hex(0xffffff),
            );
            let key = renderer.render(&view).expect("rendered");
            assert!(
                hashes.insert(key.hash),
                "fraction {fraction} looks the same"
            );
        }
    }

    #[test]
    fn long_swedish_text_is_shrunk_rather_than_spilling_over_the_edge() {
        let mut renderer = renderer();
        let view = KeyView::solid(Color::hex(0x0e7490))
            .header("Stensjöns vattentemperatur idag")
            .value("21.3°", 36.0);
        let key = renderer.render(&view).expect("rendered");

        // The left and right edge columns must stay background-coloured.
        for row in 4..(KEY_SIZE - 4) {
            for column in [1u32, KEY_SIZE - 2] {
                let index = ((row * KEY_SIZE + column) * 3) as usize;
                assert!(
                    key.rgb[index] < 120,
                    "text reached the edge at {row},{column}"
                );
            }
        }
    }

    #[test]
    fn negative_and_two_digit_temperatures_both_fit() {
        let mut renderer = renderer();
        for value in ["-15°", "-5°", "0°", "9°", "23°", "-23°/-15°"] {
            let view = KeyView::solid(Color::hex(0x0369a1))
                .header("STENSJÖN")
                .value(value, 34.0);
            let key = renderer.render(&view).expect("rendered");
            assert_eq!(key.rgb.len(), (KEY_SIZE * KEY_SIZE * 3) as usize, "{value}");
        }
    }

    #[test]
    fn a_badge_is_drawn_in_the_corner_without_covering_the_value() {
        let mut renderer = renderer();
        let mut view = sample();
        view.badge = Some(Badge {
            text: "2".to_string(),
            background: Color::hex(0x0f172a),
        });
        let with_badge = renderer.render(&view).expect("rendered");
        let without = renderer.render(&sample()).expect("rendered");

        assert_ne!(with_badge.hash, without.hash);
        // The badge sits top-right; the bottom-left quadrant must be untouched.
        let quadrant_unchanged = (KEY_SIZE / 2..KEY_SIZE)
            .flat_map(|row| (0..KEY_SIZE / 2).map(move |column| (row, column)))
            .all(|(row, column)| {
                let index = ((row * KEY_SIZE + column) * 3) as usize;
                with_badge.rgb[index] == without.rgb[index]
            });
        assert!(quadrant_unchanged, "the badge leaked into the value region");
    }

    #[test]
    fn rows_render_instead_of_a_single_value() {
        let mut renderer = renderer();
        let view = KeyView::solid(Color::hex(0x0e7490))
            .header("MIXER")
            .rows(vec![
                ("BOSE".to_string(), "42%".to_string()),
                ("MIC MAC".to_string(), "75%".to_string()),
            ]);
        let key = renderer.render(&view).expect("rendered");
        assert_eq!(key.rgb.len(), (KEY_SIZE * KEY_SIZE * 3) as usize);
        assert!(brightness(&key) > 30.0, "rows produced an empty key");
    }

    #[test]
    fn every_icon_renders_into_a_key_without_panicking() {
        let mut renderer = renderer();
        let icons = [
            Icon::Play,
            Icon::Pause,
            Icon::PlayPause,
            Icon::Next,
            Icon::Previous,
            Icon::Skip,
            Icon::Reset,
            Icon::Refresh,
            Icon::Check,
            Icon::Cross,
            Icon::Plus,
            Icon::Minus,
            Icon::Shuffle,
            Icon::Repeat,
            Icon::RepeatOne,
            Icon::Home,
            Icon::Speaker,
            Icon::SpeakerMuted,
            Icon::Microphone,
            Icon::MicrophoneMuted,
            Icon::Calendar,
            Icon::Tomato,
            Icon::GitHub,
            Icon::Note,
            Icon::Sun,
            Icon::Moon,
            Icon::Cloud,
            Icon::Rain,
            Icon::Snow,
            Icon::Sleet,
            Icon::Thunder,
            Icon::Fog,
            Icon::Water,
            Icon::TrendUp,
            Icon::TrendDown,
            Icon::Warning,
        ];
        for icon in icons {
            let view = KeyView::solid(Color::hex(0x334155)).glyph(icon);
            let key = renderer.render(&view).expect("rendered");
            assert!(brightness(&key) > 20.0, "{icon:?} rendered nothing visible");
        }
    }

    #[test]
    fn gradients_shade_across_the_key() {
        let mut renderer = renderer();
        let view = KeyView {
            background: Background::Vertical {
                top: Color::hex(0x0e7490),
                bottom: Color::hex(0x082f49),
            },
            ..Default::default()
        };
        let key = renderer.render(&view).expect("rendered");

        let top = ((6 * KEY_SIZE + KEY_SIZE / 2) * 3) as usize;
        let bottom = (((KEY_SIZE - 6) * KEY_SIZE + KEY_SIZE / 2) * 3) as usize;
        assert!(
            key.rgb[top + 2] > key.rgb[bottom + 2],
            "the vertical gradient did not shade"
        );
    }

    #[test]
    fn artwork_is_cached_bounded_and_composited() {
        let mut renderer = renderer();
        let mut encoded = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            64,
            64,
            image::Rgb([200, 40, 40]),
        ))
        .write_to(
            &mut std::io::Cursor::new(&mut encoded),
            image::ImageFormat::Png,
        )
        .expect("encoded");

        renderer
            .cache_artwork("spotify:track:1", &encoded)
            .expect("cached");
        assert!(renderer.has_artwork("spotify:track:1"));

        let plain = KeyView::solid(Color::hex(0x1db954)).header("TRUTH");
        let with_art = plain.clone().artwork("spotify:track:1");
        let without = renderer.render(&plain).expect("rendered");
        let with = renderer.render(&with_art).expect("rendered");
        assert_ne!(without.hash, with.hash, "artwork was not composited");

        for index in 0..20 {
            renderer
                .cache_artwork(&format!("spotify:track:{index}"), &encoded)
                .expect("cached");
        }
        assert!(
            renderer.artwork_count() <= 8,
            "artwork cache grew to {}",
            renderer.artwork_count()
        );
    }

    #[test]
    fn a_missing_artwork_key_falls_back_to_the_plain_tile() {
        let mut renderer = renderer();
        let view = KeyView::solid(Color::hex(0x1db954)).artwork("spotify:track:absent");
        let with_key = renderer.render(&view).expect("rendered");
        let plain = renderer
            .render(&KeyView::solid(Color::hex(0x1db954)))
            .expect("rendered");
        assert_eq!(with_key.hash, plain.hash);
    }

    #[test]
    fn corrupt_artwork_is_rejected_without_breaking_the_renderer() {
        let mut renderer = renderer();
        assert!(renderer.cache_artwork("bad", b"not an image").is_err());
        assert!(!renderer.has_artwork("bad"));
        assert!(renderer.render(&sample()).is_ok());
    }

    #[test]
    fn high_resolution_artwork_is_downsampled_into_the_cache() {
        let mut renderer = renderer();
        let mut encoded = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            640,
            640,
            image::Rgb([10, 200, 90]),
        ))
        .write_to(
            &mut std::io::Cursor::new(&mut encoded),
            image::ImageFormat::Png,
        )
        .expect("encoded");

        renderer.cache_artwork("big", &encoded).expect("cached");
        assert!(renderer.has_artwork("big"));
        assert!(renderer
            .render(&KeyView::solid(Color::hex(0)).artwork("big"))
            .is_ok());
    }

    #[test]
    fn keys_encode_to_png_for_previews_and_goldens() {
        let mut renderer = renderer();
        let png = renderer
            .render(&sample())
            .expect("rendered")
            .to_png()
            .expect("png");
        assert_eq!(&png[1..4], b"PNG");

        let decoded = image::load_from_memory(&png).expect("decoded");
        assert_eq!(decoded.width(), KEY_SIZE);
        assert_eq!(decoded.height(), KEY_SIZE);
    }

    #[test]
    fn the_internal_render_keeps_the_full_detail_for_previews() {
        let mut renderer = renderer();
        let key = renderer.render_internal(&sample()).expect("rendered");
        assert_eq!(key.size, INTERNAL_SIZE);
        assert_eq!(key.rgb.len(), (INTERNAL_SIZE * INTERNAL_SIZE * 3) as usize);
    }

    #[test]
    fn arc_paths_only_exist_for_a_positive_fraction() {
        assert!(arc_path(10.0, 10.0, 5.0, 0.0).is_none());
        assert!(arc_path(10.0, 10.0, 5.0, 0.5).is_some());
        assert!(
            arc_path(10.0, 10.0, 5.0, 2.0).is_some(),
            "clamped to a full ring"
        );
    }

    #[test]
    fn premultiplication_is_endpoint_exact() {
        assert_eq!(premultiply(&[255, 128, 0, 255]), vec![255, 128, 0, 255]);
        assert_eq!(premultiply(&[255, 128, 0, 0]), vec![0, 0, 0, 0]);
    }

    #[test]
    fn a_progress_view_with_rows_moves_the_ring_out_of_the_row_region() {
        let mut renderer = renderer();
        let view = KeyView::solid(Color::hex(0x1d4ed8))
            .rows(vec![("A".to_string(), "1".to_string())])
            .progress(0.5, Color::hex(0x0f172a), Color::hex(0xffffff));
        assert!(renderer.render(&view).is_ok());
        assert_eq!(view.progress.expect("progress").fraction, 0.5);
        assert_eq!(
            view.progress.expect("progress"),
            Progress {
                fraction: 0.5,
                track: Color::hex(0x0f172a),
                fill: Color::hex(0xffffff)
            }
        );
    }
}
