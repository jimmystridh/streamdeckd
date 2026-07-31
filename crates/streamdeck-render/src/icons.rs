//! Project-owned vector glyphs.
//!
//! Every icon is authored here as paths in a unit square and scaled into its
//! region at draw time. Owning the geometry means no font has to happen to contain
//! a play triangle, and no proprietary plugin artwork is redistributed.

use streamdeck_core::view::{Color, Icon};
use tiny_skia::{Path, PathBuilder, Rect};

/// One filled or stroked layer of an icon.
pub struct Layer {
    pub path: Path,
    /// `None` fills the path; `Some(width)` strokes it, in unit-square units.
    pub stroke: Option<f32>,
    /// `None` uses the caller's foreground colour.
    pub tint: Option<Color>,
    pub opacity: f32,
}

impl Layer {
    fn fill(path: Path) -> Self {
        Self {
            path,
            stroke: None,
            tint: None,
            opacity: 1.0,
        }
    }

    fn stroke(path: Path, width: f32) -> Self {
        Self {
            path,
            stroke: Some(width),
            tint: None,
            opacity: 1.0,
        }
    }

    fn tinted(mut self, tint: Color) -> Self {
        self.tint = Some(tint);
        self
    }

    fn faded(mut self, opacity: f32) -> Self {
        self.opacity = opacity;
        self
    }
}

const SUN: Color = Color::hex(0xfde68a);
const MOON: Color = Color::hex(0xfef3c7);
const CLOUD: Color = Color::hex(0xf8fafc);
const RAIN: Color = Color::hex(0x7dd3fc);
const SNOW: Color = Color::hex(0xe0f2fe);
const BOLT: Color = Color::hex(0xfde047);
const MIST: Color = Color::hex(0xe2e8f0);
const WATER: Color = Color::hex(0x38bdf8);
const WATER_LINE: Color = Color::hex(0xe0f2fe);

/// The layers for an icon, in a unit square. Later layers draw on top.
pub fn layers(icon: Icon) -> Vec<Layer> {
    match icon {
        Icon::Play => vec![Layer::fill(polygon(&[
            (0.28, 0.16),
            (0.82, 0.5),
            (0.28, 0.84),
        ]))],
        Icon::Pause => vec![
            Layer::fill(rect(0.26, 0.16, 0.16, 0.68)),
            Layer::fill(rect(0.58, 0.16, 0.16, 0.68)),
        ],
        Icon::PlayPause => vec![
            Layer::fill(polygon(&[(0.12, 0.18), (0.52, 0.5), (0.12, 0.82)])),
            Layer::fill(rect(0.62, 0.18, 0.1, 0.64)),
            Layer::fill(rect(0.79, 0.18, 0.1, 0.64)),
        ],
        Icon::Next => vec![
            Layer::fill(polygon(&[(0.18, 0.18), (0.62, 0.5), (0.18, 0.82)])),
            Layer::fill(rect(0.68, 0.18, 0.14, 0.64)),
        ],
        Icon::Previous => vec![
            Layer::fill(polygon(&[(0.82, 0.18), (0.38, 0.5), (0.82, 0.82)])),
            Layer::fill(rect(0.18, 0.18, 0.14, 0.64)),
        ],
        Icon::Skip => vec![
            Layer::fill(polygon(&[(0.16, 0.2), (0.56, 0.5), (0.16, 0.8)])),
            Layer::fill(polygon(&[(0.5, 0.2), (0.9, 0.5), (0.5, 0.8)])),
        ],
        Icon::Reset | Icon::Refresh => {
            let mut builder = PathBuilder::new();
            // Three-quarter circle, open at the top right.
            builder.move_to(0.82, 0.5);
            builder.cubic_to(0.82, 0.72, 0.66, 0.86, 0.5, 0.86);
            builder.cubic_to(0.28, 0.86, 0.16, 0.7, 0.16, 0.5);
            builder.cubic_to(0.16, 0.3, 0.3, 0.16, 0.5, 0.16);
            builder.cubic_to(0.64, 0.16, 0.74, 0.22, 0.8, 0.3);
            vec![
                Layer::stroke(builder.finish().expect("arc"), 0.12),
                // Arrow head closing the loop.
                Layer::fill(polygon(&[(0.66, 0.3), (0.9, 0.24), (0.86, 0.48)])),
            ]
        }
        Icon::Check => vec![Layer::stroke(
            polyline(&[(0.2, 0.54), (0.42, 0.76), (0.82, 0.26)]),
            0.15,
        )],
        Icon::Cross => vec![
            Layer::stroke(polyline(&[(0.24, 0.24), (0.76, 0.76)]), 0.14),
            Layer::stroke(polyline(&[(0.76, 0.24), (0.24, 0.76)]), 0.14),
        ],
        Icon::Plus => vec![
            Layer::fill(rect(0.44, 0.16, 0.12, 0.68)),
            Layer::fill(rect(0.16, 0.44, 0.68, 0.12)),
        ],
        Icon::Minus => vec![Layer::fill(rect(0.16, 0.44, 0.68, 0.12))],
        Icon::Shuffle => vec![
            Layer::stroke(
                polyline(&[(0.14, 0.28), (0.4, 0.28), (0.62, 0.72), (0.86, 0.72)]),
                0.1,
            ),
            Layer::stroke(polyline(&[(0.14, 0.72), (0.36, 0.72), (0.46, 0.52)]), 0.1),
            Layer::fill(polygon(&[(0.74, 0.58), (0.94, 0.72), (0.74, 0.86)])),
            Layer::fill(polygon(&[(0.74, 0.14), (0.94, 0.28), (0.74, 0.42)])),
            Layer::stroke(polyline(&[(0.62, 0.28), (0.86, 0.28)]), 0.1),
        ],
        Icon::Repeat => vec![
            Layer::stroke(polyline(&[(0.28, 0.24), (0.78, 0.24), (0.78, 0.5)]), 0.1),
            Layer::stroke(polyline(&[(0.72, 0.76), (0.22, 0.76), (0.22, 0.5)]), 0.1),
            Layer::fill(polygon(&[(0.62, 0.62), (0.84, 0.76), (0.62, 0.9)])),
            Layer::fill(polygon(&[(0.38, 0.1), (0.16, 0.24), (0.38, 0.38)])),
        ],
        Icon::RepeatOne => {
            let mut layers = layers(Icon::Repeat);
            layers.push(Layer::fill(rect(0.46, 0.4, 0.08, 0.2)));
            layers
        }
        Icon::Home => vec![
            Layer::fill(polygon(&[
                (0.5, 0.14),
                (0.9, 0.48),
                (0.78, 0.48),
                (0.78, 0.84),
                (0.22, 0.84),
                (0.22, 0.48),
                (0.1, 0.48),
            ])),
            Layer::fill(rect(0.42, 0.58, 0.16, 0.26)).faded(0.35),
        ],
        Icon::Dashboard => vec![
            Layer::fill(rounded_rect(0.12, 0.12, 0.32, 0.32, 0.06)),
            Layer::fill(rounded_rect(0.56, 0.12, 0.32, 0.32, 0.06)),
            Layer::fill(rounded_rect(0.12, 0.56, 0.32, 0.32, 0.06)),
            Layer::fill(rounded_rect(0.56, 0.56, 0.32, 0.32, 0.06)),
        ],
        Icon::Capture => vec![
            Layer::stroke(rounded_rect(0.16, 0.1, 0.62, 0.76, 0.06), 0.07),
            Layer::stroke(polyline(&[(0.28, 0.3), (0.62, 0.3)]), 0.06).faded(0.7),
            Layer::stroke(polyline(&[(0.28, 0.46), (0.54, 0.46)]), 0.06).faded(0.7),
            Layer::stroke(polyline(&[(0.48, 0.76), (0.84, 0.4)]), 0.12),
            Layer::fill(polygon(&[(0.43, 0.82), (0.5, 0.68), (0.57, 0.75)])),
        ],
        Icon::Application => vec![
            Layer::fill(rounded_rect(0.1, 0.16, 0.8, 0.68, 0.09)),
            Layer::fill(rect(0.1, 0.3, 0.8, 0.08))
                .tinted(Color::hex(0x0f172a))
                .faded(0.38),
            Layer::fill(circle(0.22, 0.23, 0.035))
                .tinted(Color::hex(0x0f172a))
                .faded(0.55),
            Layer::fill(circle(0.32, 0.23, 0.035))
                .tinted(Color::hex(0x0f172a))
                .faded(0.55),
        ],
        Icon::Search => vec![
            Layer::stroke(circle(0.43, 0.42, 0.25), 0.1),
            Layer::stroke(polyline(&[(0.61, 0.61), (0.84, 0.84)]), 0.12),
        ],
        Icon::Message => vec![
            Layer::fill(rounded_rect(0.12, 0.16, 0.76, 0.58, 0.1)),
            Layer::fill(polygon(&[(0.28, 0.68), (0.2, 0.9), (0.48, 0.72)])),
            Layer::fill(circle(0.32, 0.44, 0.04))
                .tinted(Color::hex(0x0f172a))
                .faded(0.5),
            Layer::fill(circle(0.5, 0.44, 0.04))
                .tinted(Color::hex(0x0f172a))
                .faded(0.5),
            Layer::fill(circle(0.68, 0.44, 0.04))
                .tinted(Color::hex(0x0f172a))
                .faded(0.5),
        ],
        Icon::Thread => vec![
            Layer::fill(rounded_rect(0.08, 0.12, 0.6, 0.42, 0.08)),
            Layer::fill(polygon(&[(0.2, 0.5), (0.16, 0.68), (0.38, 0.52)])),
            Layer::fill(rounded_rect(0.34, 0.48, 0.58, 0.38, 0.08)).faded(0.78),
            Layer::fill(polygon(&[(0.7, 0.82), (0.82, 0.94), (0.82, 0.78)])).faded(0.78),
        ],
        Icon::Activity => vec![
            Layer::fill(polygon(&[
                (0.24, 0.7),
                (0.3, 0.62),
                (0.3, 0.4),
                (0.36, 0.24),
                (0.5, 0.16),
                (0.64, 0.24),
                (0.7, 0.4),
                (0.7, 0.62),
                (0.76, 0.7),
            ])),
            Layer::fill(rect(0.2, 0.7, 0.6, 0.1)),
            Layer::fill(circle(0.5, 0.85, 0.08)),
        ],
        Icon::Network => vec![
            Layer::stroke(polyline(&[(0.22, 0.72), (0.5, 0.32), (0.78, 0.72)]), 0.09),
            Layer::fill(circle(0.5, 0.24, 0.13)),
            Layer::fill(circle(0.18, 0.78, 0.13)),
            Layer::fill(circle(0.82, 0.78, 0.13)),
        ],
        Icon::Bus => vec![
            Layer::fill(rounded_rect(0.16, 0.1, 0.68, 0.72, 0.1)),
            Layer::fill(rounded_rect(0.25, 0.2, 0.5, 0.28, 0.04))
                .tinted(Color::hex(0x0f172a))
                .faded(0.45),
            Layer::fill(circle(0.31, 0.82, 0.1)),
            Layer::fill(circle(0.69, 0.82, 0.1)),
        ],
        Icon::Speaker => vec![
            Layer::fill(polygon(&[
                (0.12, 0.36),
                (0.3, 0.36),
                (0.52, 0.16),
                (0.52, 0.84),
                (0.3, 0.64),
                (0.12, 0.64),
            ])),
            Layer::stroke(arc_right(0.62), 0.08),
            Layer::stroke(arc_right(0.78), 0.08).faded(0.7),
        ],
        Icon::SpeakerMuted => vec![
            Layer::fill(polygon(&[
                (0.12, 0.36),
                (0.3, 0.36),
                (0.52, 0.16),
                (0.52, 0.84),
                (0.3, 0.64),
                (0.12, 0.64),
            ])),
            Layer::stroke(polyline(&[(0.62, 0.36), (0.9, 0.64)]), 0.1),
            Layer::stroke(polyline(&[(0.9, 0.36), (0.62, 0.64)]), 0.1),
        ],
        Icon::Microphone => vec![
            Layer::fill(rounded_rect(0.36, 0.1, 0.28, 0.46, 0.14)),
            Layer::stroke(arc_bottom(), 0.09),
            Layer::fill(rect(0.47, 0.76, 0.06, 0.14)),
        ],
        Icon::MicrophoneMuted => {
            let mut layers = layers(Icon::Microphone);
            layers.push(Layer::stroke(polyline(&[(0.16, 0.86), (0.84, 0.14)]), 0.1));
            layers
        }
        Icon::Calendar => vec![
            Layer::fill(rounded_rect(0.14, 0.2, 0.72, 0.66, 0.1)),
            Layer::fill(rect(0.14, 0.34, 0.72, 0.1))
                .tinted(Color::hex(0x0f172a))
                .faded(0.35),
            Layer::stroke(polyline(&[(0.32, 0.1), (0.32, 0.26)]), 0.09),
            Layer::stroke(polyline(&[(0.68, 0.1), (0.68, 0.26)]), 0.09),
        ],
        Icon::Tomato => vec![
            Layer::fill(circle(0.5, 0.58, 0.34)),
            Layer::fill(polygon(&[(0.5, 0.1), (0.66, 0.26), (0.34, 0.26)]))
                .tinted(Color::hex(0x15803d)),
        ],
        // A pull-request branch: two nodes on a trunk, one node on a branch.
        Icon::GitHub => vec![
            Layer::stroke(polyline(&[(0.28, 0.22), (0.28, 0.78)]), 0.1),
            Layer::stroke(polyline(&[(0.72, 0.34), (0.72, 0.5), (0.28, 0.5)]), 0.1),
            Layer::fill(circle(0.28, 0.18, 0.14)),
            Layer::fill(circle(0.28, 0.82, 0.14)),
            Layer::fill(circle(0.72, 0.24, 0.14)),
        ],
        Icon::Note => vec![
            Layer::fill(circle(0.36, 0.74, 0.16)),
            Layer::fill(rect(0.48, 0.16, 0.08, 0.6)),
            Layer::fill(polygon(&[
                (0.48, 0.16),
                (0.86, 0.24),
                (0.86, 0.4),
                (0.48, 0.32),
            ])),
        ],
        Icon::Sun => vec![
            Layer::fill(circle(0.5, 0.5, 0.26)).tinted(SUN),
            Layer::stroke(rays(), 0.07).tinted(SUN),
        ],
        Icon::Moon => vec![Layer::fill(crescent()).tinted(MOON)],
        Icon::Cloud => vec![Layer::fill(cloud(0.0)).tinted(CLOUD)],
        Icon::Rain => vec![
            Layer::fill(cloud(-0.1)).tinted(CLOUD),
            Layer::stroke(polyline(&[(0.24, 0.7), (0.18, 0.9)]), 0.07).tinted(RAIN),
            Layer::stroke(polyline(&[(0.5, 0.7), (0.44, 0.9)]), 0.07).tinted(RAIN),
            Layer::stroke(polyline(&[(0.76, 0.7), (0.7, 0.9)]), 0.07).tinted(RAIN),
        ],
        Icon::Snow => vec![
            Layer::fill(cloud(-0.1)).tinted(CLOUD),
            Layer::fill(circle(0.26, 0.78, 0.06)).tinted(SNOW),
            Layer::fill(circle(0.5, 0.88, 0.06)).tinted(SNOW),
            Layer::fill(circle(0.74, 0.78, 0.06)).tinted(SNOW),
        ],
        Icon::Sleet => vec![
            Layer::fill(cloud(-0.1)).tinted(CLOUD),
            Layer::stroke(polyline(&[(0.28, 0.7), (0.22, 0.9)]), 0.07).tinted(RAIN),
            Layer::fill(circle(0.56, 0.82, 0.06)).tinted(SNOW),
            Layer::stroke(polyline(&[(0.8, 0.7), (0.74, 0.9)]), 0.07).tinted(RAIN),
        ],
        Icon::Thunder => vec![
            Layer::fill(cloud(-0.12)).tinted(CLOUD),
            Layer::fill(polygon(&[
                (0.52, 0.6),
                (0.36, 0.86),
                (0.5, 0.86),
                (0.42, 1.0),
                (0.7, 0.76),
                (0.54, 0.76),
                (0.62, 0.6),
            ]))
            .tinted(BOLT),
        ],
        Icon::Fog => vec![
            Layer::fill(cloud(-0.16)).tinted(CLOUD),
            Layer::stroke(polyline(&[(0.1, 0.72), (0.9, 0.72)]), 0.06)
                .tinted(MIST)
                .faded(0.8),
            Layer::stroke(polyline(&[(0.18, 0.84), (0.82, 0.84)]), 0.06)
                .tinted(MIST)
                .faded(0.8),
            Layer::stroke(polyline(&[(0.1, 0.96), (0.72, 0.96)]), 0.06)
                .tinted(MIST)
                .faded(0.8),
        ],
        Icon::Water => vec![
            Layer::fill(wave(0.52, 0.48)).tinted(WATER).faded(0.45),
            Layer::stroke(wave_line(0.62), 0.06)
                .tinted(WATER_LINE)
                .faded(0.8),
            Layer::stroke(wave_line(0.82), 0.06)
                .tinted(WATER_LINE)
                .faded(0.5),
        ],
        Icon::TrendUp => vec![Layer::stroke(
            polyline(&[(0.12, 0.82), (0.4, 0.5), (0.56, 0.66), (0.88, 0.2)]),
            0.12,
        )],
        Icon::TrendDown => vec![Layer::stroke(
            polyline(&[(0.12, 0.2), (0.4, 0.52), (0.56, 0.36), (0.88, 0.82)]),
            0.12,
        )],
        Icon::Warning => vec![
            Layer::fill(polygon(&[(0.5, 0.1), (0.94, 0.88), (0.06, 0.88)])),
            Layer::fill(rect(0.46, 0.34, 0.08, 0.3)).tinted(Color::hex(0x7f1d1d)),
            Layer::fill(circle(0.5, 0.76, 0.06)).tinted(Color::hex(0x7f1d1d)),
        ],
    }
}

fn rect(x: f32, y: f32, width: f32, height: f32) -> Path {
    let mut builder = PathBuilder::new();
    builder.push_rect(Rect::from_xywh(x, y, width, height).expect("positive rect"));
    builder.finish().expect("rect path")
}

fn rounded_rect(x: f32, y: f32, width: f32, height: f32, radius: f32) -> Path {
    let radius = radius.min(width / 2.0).min(height / 2.0);
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

fn circle(cx: f32, cy: f32, radius: f32) -> Path {
    let mut builder = PathBuilder::new();
    builder.push_circle(cx, cy, radius);
    builder.finish().expect("circle path")
}

fn polygon(points: &[(f32, f32)]) -> Path {
    let mut builder = PathBuilder::new();
    let mut points = points.iter();
    let (x, y) = *points.next().expect("a polygon needs a first point");
    builder.move_to(x, y);
    for (x, y) in points {
        builder.line_to(*x, *y);
    }
    builder.close();
    builder.finish().expect("polygon path")
}

fn polyline(points: &[(f32, f32)]) -> Path {
    let mut builder = PathBuilder::new();
    let mut points = points.iter();
    let (x, y) = *points.next().expect("a polyline needs a first point");
    builder.move_to(x, y);
    for (x, y) in points {
        builder.line_to(*x, *y);
    }
    builder.finish().expect("polyline path")
}

fn rays() -> Path {
    let mut builder = PathBuilder::new();
    let spokes = [
        ((0.5, 0.02), (0.5, 0.14)),
        ((0.5, 0.86), (0.5, 0.98)),
        ((0.02, 0.5), (0.14, 0.5)),
        ((0.86, 0.5), (0.98, 0.5)),
        ((0.16, 0.16), (0.25, 0.25)),
        ((0.75, 0.75), (0.84, 0.84)),
        ((0.84, 0.16), (0.75, 0.25)),
        ((0.25, 0.75), (0.16, 0.84)),
    ];
    for ((x1, y1), (x2, y2)) in spokes {
        builder.move_to(x1, y1);
        builder.line_to(x2, y2);
    }
    builder.finish().expect("rays path")
}

fn crescent() -> Path {
    let mut builder = PathBuilder::new();
    builder.move_to(0.62, 0.08);
    builder.cubic_to(0.3, 0.16, 0.12, 0.44, 0.24, 0.7);
    builder.cubic_to(0.36, 0.94, 0.68, 0.98, 0.86, 0.8);
    builder.cubic_to(0.6, 0.78, 0.44, 0.56, 0.5, 0.34);
    builder.cubic_to(0.53, 0.22, 0.57, 0.13, 0.62, 0.08);
    builder.close();
    builder.finish().expect("crescent path")
}

fn cloud(offset: f32) -> Path {
    let mut builder = PathBuilder::new();
    builder.move_to(0.08, 0.58 + offset);
    builder.cubic_to(0.04, 0.36 + offset, 0.3, 0.24 + offset, 0.4, 0.4 + offset);
    builder.cubic_to(0.52, 0.2 + offset, 0.86, 0.3 + offset, 0.82, 0.5 + offset);
    builder.cubic_to(
        0.98,
        0.52 + offset,
        0.98,
        0.74 + offset,
        0.84,
        0.76 + offset,
    );
    builder.line_to(0.16, 0.76 + offset);
    builder.cubic_to(0.0, 0.74 + offset, 0.0, 0.6 + offset, 0.08, 0.58 + offset);
    builder.close();
    builder.finish().expect("cloud path")
}

fn wave(top: f32, height: f32) -> Path {
    let mut builder = PathBuilder::new();
    builder.move_to(0.0, top);
    builder.cubic_to(0.25, top - 0.12, 0.4, top + 0.12, 0.5, top);
    builder.cubic_to(0.6, top - 0.12, 0.75, top + 0.12, 1.0, top);
    builder.line_to(1.0, top + height);
    builder.line_to(0.0, top + height);
    builder.close();
    builder.finish().expect("wave path")
}

fn wave_line(y: f32) -> Path {
    let mut builder = PathBuilder::new();
    builder.move_to(0.0, y);
    builder.cubic_to(0.25, y - 0.1, 0.4, y + 0.1, 0.5, y);
    builder.cubic_to(0.6, y - 0.1, 0.75, y + 0.1, 1.0, y);
    builder.finish().expect("wave line path")
}

/// A speaker's sound arc at the given radius from the cone.
fn arc_right(radius: f32) -> Path {
    let mut builder = PathBuilder::new();
    builder.move_to(radius, 0.3);
    builder.quad_to(radius + 0.16, 0.5, radius, 0.7);
    builder.finish().expect("arc path")
}

/// The microphone's stand cradle.
fn arc_bottom() -> Path {
    let mut builder = PathBuilder::new();
    builder.move_to(0.24, 0.5);
    builder.cubic_to(0.24, 0.78, 0.76, 0.78, 0.76, 0.5);
    builder.finish().expect("arc path")
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Icon; 45] = [
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
        Icon::Dashboard,
        Icon::Capture,
        Icon::Application,
        Icon::Search,
        Icon::Message,
        Icon::Thread,
        Icon::Activity,
        Icon::Network,
        Icon::Bus,
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

    #[test]
    fn every_icon_produces_at_least_one_layer() {
        for icon in ALL {
            let layers = layers(icon);
            assert!(!layers.is_empty(), "{icon:?} has no layers");
        }
    }

    #[test]
    fn every_icon_stays_inside_its_unit_square_with_room_for_its_stroke() {
        for icon in ALL {
            for layer in layers(icon) {
                let bounds = layer.path.bounds();
                let margin = layer.stroke.unwrap_or(0.0);
                assert!(
                    bounds.left() >= -margin - 0.01
                        && bounds.top() >= -margin - 0.01
                        && bounds.right() <= 1.0 + margin + 0.01
                        && bounds.bottom() <= 1.0 + margin + 0.01,
                    "{icon:?} escapes its box: {bounds:?}"
                );
            }
        }
    }

    #[test]
    fn stroke_widths_are_visible_but_not_overwhelming() {
        for icon in ALL {
            for layer in layers(icon) {
                if let Some(width) = layer.stroke {
                    assert!(
                        (0.04..=0.2).contains(&width),
                        "{icon:?} stroke width {width}"
                    );
                }
            }
        }
    }

    #[test]
    fn opacities_stay_in_range() {
        for icon in ALL {
            for layer in layers(icon) {
                assert!(
                    (0.0..=1.0).contains(&layer.opacity),
                    "{icon:?} opacity {}",
                    layer.opacity
                );
            }
        }
    }

    #[test]
    fn weather_icons_carry_their_own_tints_so_they_read_on_any_sky() {
        for icon in [
            Icon::Sun,
            Icon::Moon,
            Icon::Cloud,
            Icon::Rain,
            Icon::Snow,
            Icon::Sleet,
            Icon::Thunder,
            Icon::Fog,
        ] {
            assert!(
                layers(icon).iter().all(|layer| layer.tint.is_some()),
                "{icon:?} has an untinted layer"
            );
        }
    }

    #[test]
    fn repeat_one_adds_a_marker_to_the_repeat_icon() {
        assert_eq!(
            layers(Icon::RepeatOne).len(),
            layers(Icon::Repeat).len() + 1
        );
    }

    #[test]
    fn muted_variants_add_a_slash_to_their_base_icon() {
        assert_eq!(
            layers(Icon::MicrophoneMuted).len(),
            layers(Icon::Microphone).len() + 1
        );
        assert!(layers(Icon::SpeakerMuted)
            .iter()
            .any(|layer| layer.stroke.is_some()));
    }
}
