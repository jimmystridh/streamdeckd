use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::f32::consts::TAU;
use std::hash::{Hash, Hasher};
use std::path::Path;

use streamdeck_core::model::{Grid, KeyPosition};
use streamdeck_render::{RenderedKey, KEY_SIZE};

use crate::device::{DeckDevice, DeviceError};

pub const GRID: Grid = Grid::MK2;
pub const WIDTH: u32 = GRID.columns as u32 * KEY_SIZE;
pub const HEIGHT: u32 = GRID.rows as u32 * KEY_SIZE;
const PREVIEW_GUTTER: u32 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreensaverScene {
    Aurora,
    Matrix,
    Space,
}

impl ScreensaverScene {
    #[cfg(test)]
    const ALL: [Self; 3] = [Self::Aurora, Self::Matrix, Self::Space];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Aurora => "aurora",
            Self::Matrix => "matrix",
            Self::Space => "space",
        }
    }

    pub const fn next(self) -> Self {
        match self {
            Self::Aurora => Self::Matrix,
            Self::Matrix => Self::Space,
            Self::Space => Self::Aurora,
        }
    }
}

pub struct ScreensaverCanvas {
    rgb: Vec<u8>,
}

impl ScreensaverCanvas {
    pub fn render(scene: ScreensaverScene, time: f32, intensity: f32) -> Self {
        let mut rgb = match scene {
            ScreensaverScene::Aurora => render_aurora(time),
            ScreensaverScene::Matrix => render_matrix(time),
            ScreensaverScene::Space => render_space(time),
        };
        scale(&mut rgb, intensity.clamp(0.0, 1.0));
        Self { rgb }
    }

    pub fn black() -> Self {
        Self {
            rgb: vec![0; (WIDTH * HEIGHT * 3) as usize],
        }
    }

    pub async fn send(&self, device: &dyn DeckDevice) -> Result<usize, DeviceError> {
        let mut bytes = 0;
        for position in GRID.positions() {
            bytes += device.set_key(position, &self.key(position)).await?;
        }
        device.flush().await?;
        Ok(bytes)
    }

    pub fn rendered_keys(&self) -> HashMap<KeyPosition, RenderedKey> {
        GRID.positions()
            .map(|position| (position, self.key(position)))
            .collect()
    }

    pub fn save_preview(&self, path: &Path) -> anyhow::Result<()> {
        let width = WIDTH + (GRID.columns as u32 - 1) * PREVIEW_GUTTER;
        let height = HEIGHT + (GRID.rows as u32 - 1) * PREVIEW_GUTTER;
        let mut preview = image::RgbImage::from_pixel(width, height, image::Rgb([5, 6, 10]));

        for position in GRID.positions() {
            let key = self.key(position).to_image()?;
            let x = (position.column as u32 - 1) * (KEY_SIZE + PREVIEW_GUTTER);
            let y = (position.row as u32 - 1) * (KEY_SIZE + PREVIEW_GUTTER);
            image::imageops::replace(&mut preview, &key, i64::from(x), i64::from(y));
        }

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        preview.save(path)?;
        Ok(())
    }

    fn key(&self, position: KeyPosition) -> RenderedKey {
        let mut rgb = Vec::with_capacity((KEY_SIZE * KEY_SIZE * 3) as usize);
        let origin_x = (position.column as u32 - 1) * KEY_SIZE;
        let origin_y = (position.row as u32 - 1) * KEY_SIZE;

        for y in origin_y..origin_y + KEY_SIZE {
            let start = ((y * WIDTH + origin_x) * 3) as usize;
            let end = start + (KEY_SIZE * 3) as usize;
            rgb.extend_from_slice(&self.rgb[start..end]);
        }

        let mut hasher = DefaultHasher::new();
        rgb.hash(&mut hasher);
        RenderedKey {
            size: KEY_SIZE,
            rgb,
            hash: hasher.finish(),
        }
    }
}

fn render_aurora(time: f32) -> Vec<u8> {
    let mut rgb = vec![0; (WIDTH * HEIGHT * 3) as usize];

    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let nx = x as f32 / WIDTH as f32;
            let ny = y as f32 / HEIGHT as f32;
            let vignette =
                (1.0 - ((nx - 0.5).powi(2) + (ny - 0.5).powi(2)) * 1.35).clamp(0.25, 1.0);
            let mut color = [2.0, 5.0, 17.0];

            add_aurora(&mut color, nx, ny, time, 0.0, [20.0, 255.0, 185.0]);
            add_aurora(&mut color, nx, ny, -time * 0.77, 1.9, [55.0, 145.0, 255.0]);
            add_aurora(&mut color, nx, ny, time * 0.61, 3.7, [235.0, 50.0, 255.0]);

            for trail in 0..5 {
                let age = trail as f32 * 0.11;
                let orbit_time = time - age;
                let ox = 0.5 + 0.39 * (orbit_time * 0.73).cos();
                let oy = 0.5 + 0.31 * (orbit_time * 1.07).sin();
                let distance = ((nx - ox).powi(2) + (ny - oy).powi(2)).sqrt();
                let glow = (-distance * distance * 210.0).exp() * (1.0 - trail as f32 / 6.0);
                color[0] += 120.0 * glow;
                color[1] += 185.0 * glow;
                color[2] += 255.0 * glow;
            }

            let star = star_brightness(x, y, time);
            color[0] += star * 180.0;
            color[1] += star * 215.0;
            color[2] += star * 255.0;

            put(
                &mut rgb,
                x,
                y,
                [
                    color[0] * vignette,
                    color[1] * vignette,
                    color[2] * vignette,
                ],
            );
        }
    }

    rgb
}

fn render_matrix(time: f32) -> Vec<u8> {
    const CELL_WIDTH: u32 = 8;
    const CELL_HEIGHT: u32 = 12;

    let mut rgb = vec![0; (WIDTH * HEIGHT * 3) as usize];
    let epoch = (time * 5.0) as u32;

    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let column = x / CELL_WIDTH;
            let row = y / CELL_HEIGHT;
            let seed = mix32(column.wrapping_mul(0x9e37_79b9));
            let speed = 34.0 + (seed % 47) as f32;
            let tail = 54.0 + (seed.rotate_left(11) % 118) as f32;
            let cycle = HEIGHT as f32 + tail;
            let head = (time * speed + (seed % cycle as u32) as f32) % cycle;
            let distance = (head - y as f32).rem_euclid(cycle);

            let glyph_x = x % CELL_WIDTH;
            let glyph_y = y % CELL_HEIGHT;
            let glyph = glyph_pixel(column, row, epoch, glyph_x, glyph_y);
            let background_glyph = mix32(column ^ row.wrapping_mul(0x85eb_ca6b)) % 37 == 0 && glyph;
            let mut color = if background_glyph {
                [0.0, 9.0, 4.0]
            } else {
                [0.0, 1.0, 1.0]
            };

            if distance <= tail && glyph {
                let trail = (1.0 - distance / tail).powf(1.65);
                let flicker = 0.72 + (mix32(seed ^ row ^ epoch) & 0xff) as f32 / 255.0 * 0.28;
                color = [5.0 * trail, 215.0 * trail * flicker, 69.0 * trail];

                if distance < CELL_HEIGHT as f32 * 1.25 {
                    let head_glow = 1.0 - distance / (CELL_HEIGHT as f32 * 1.25);
                    color[0] += 185.0 * head_glow;
                    color[1] += 90.0 * head_glow;
                    color[2] += 190.0 * head_glow;
                }
            }

            let scanline = if y % 3 == 0 { 0.84 } else { 1.0 };
            put(
                &mut rgb,
                x,
                y,
                [
                    color[0] * scanline,
                    color[1] * scanline,
                    color[2] * scanline,
                ],
            );
        }
    }

    rgb
}

fn glyph_pixel(column: u32, row: u32, epoch: u32, x: u32, y: u32) -> bool {
    if x == 0 || x >= 7 || y == 0 || y >= 10 {
        return false;
    }

    let symbol_epoch = epoch.wrapping_add(row / 3);
    let seed = mix32(
        column.wrapping_mul(0x9e37_79b9)
            ^ row.wrapping_mul(0x85eb_ca6b)
            ^ symbol_epoch.wrapping_mul(0xc2b2_ae35),
    );
    let line = mix32(seed ^ y.wrapping_mul(0x27d4_eb2d));
    let random_stroke = line & (1 << (x - 1)) != 0;
    let spine = x == 3 && (seed.rotate_left(y) & 3 != 0);
    let cap = (y == 2 || y == 8) && x > 1 && x < 6 && seed & (1 << x) != 0;
    random_stroke ^ spine ^ cap
}

fn render_space(time: f32) -> Vec<u8> {
    let mut rgb = vec![0; (WIDTH * HEIGHT * 3) as usize];
    let center_x = WIDTH as f32 * (0.5 + 0.035 * (time * 0.19).sin());
    let center_y = HEIGHT as f32 * (0.5 + 0.045 * (time * 0.23).cos());

    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let dx = (x as f32 - center_x) / WIDTH as f32;
            let dy = (y as f32 - center_y) / HEIGHT as f32;
            let radius = (dx * dx + dy * dy).sqrt();
            let core = (-radius * radius * 90.0).exp();
            let nebula =
                ((dx * 13.0 + dy * 7.0 + time * 0.11).sin() * 0.5 + 0.5) * (-radius * 2.2).exp();
            put(
                &mut rgb,
                x,
                y,
                [
                    1.0 + core * 6.0 + nebula * 2.0,
                    2.0 + core * 9.0 + nebula * 3.0,
                    8.0 + core * 19.0 + nebula * 9.0,
                ],
            );
        }
    }

    for index in 0u32..520 {
        let seed_x = mix32(index.wrapping_mul(0x9e37_79b9) ^ 0xa341_316c);
        let seed_y = mix32(index.wrapping_mul(0x85eb_ca6b) ^ 0xc801_3ea4);
        let seed_z = mix32(index.wrapping_mul(0xc2b2_ae35) ^ 0xad90_777d);
        let world_x = unit(seed_x) * 0.9 - 0.45;
        let world_y = unit(seed_y) * 0.68 - 0.34;
        let speed = 0.13 + unit(seed_z.rotate_left(9)) * 0.18;
        let depth = (unit(seed_z) - time * speed).rem_euclid(1.0);
        let z = 0.055 + depth * 0.945;
        let tail_z = (z + 0.038 + (1.0 - z) * 0.055).min(1.15);
        let focal = 0.54;

        let head_x = center_x + world_x * WIDTH as f32 * focal / z;
        let head_y = center_y + world_y * HEIGHT as f32 * focal / z;
        let tail_x = center_x + world_x * WIDTH as f32 * focal / tail_z;
        let tail_y = center_y + world_y * HEIGHT as f32 * focal / tail_z;

        if head_x < -16.0
            || head_x >= WIDTH as f32 + 16.0
            || head_y < -16.0
            || head_y >= HEIGHT as f32 + 16.0
        {
            continue;
        }

        let proximity = (1.0 - z).powf(0.85) * 0.92;
        let tint = unit(seed_x.rotate_left(7));
        let color = [145.0 + tint * 110.0, 180.0 + tint * 75.0, 255.0];
        draw_streak(&mut rgb, tail_x, tail_y, head_x, head_y, color, proximity);
    }

    rgb
}

fn draw_streak(
    rgb: &mut [u8],
    start_x: f32,
    start_y: f32,
    end_x: f32,
    end_y: f32,
    color: [f32; 3],
    brightness: f32,
) {
    let dx = end_x - start_x;
    let dy = end_y - start_y;
    let steps = dx.abs().max(dy.abs()).ceil().clamp(1.0, 36.0) as u32;

    for step in 0..=steps {
        let progress = step as f32 / steps as f32;
        let x = start_x + dx * progress;
        let y = start_y + dy * progress;
        let alpha = brightness * (0.18 + 0.82 * progress.powi(2));
        add_pixel(rgb, x.round() as i32, y.round() as i32, color, alpha);

        if brightness > 0.45 {
            add_pixel(
                rgb,
                x.round() as i32 + 1,
                y.round() as i32,
                color,
                alpha * 0.22,
            );
            add_pixel(
                rgb,
                x.round() as i32,
                y.round() as i32 + 1,
                color,
                alpha * 0.22,
            );
        }
    }
}

fn add_pixel(rgb: &mut [u8], x: i32, y: i32, color: [f32; 3], alpha: f32) {
    if x < 0 || y < 0 || x >= WIDTH as i32 || y >= HEIGHT as i32 {
        return;
    }
    let index = ((y as u32 * WIDTH + x as u32) * 3) as usize;
    for channel in 0..3 {
        rgb[index + channel] =
            (rgb[index + channel] as f32 + color[channel] * alpha).clamp(0.0, 255.0) as u8;
    }
}

fn put(rgb: &mut [u8], x: u32, y: u32, color: [f32; 3]) {
    let index = ((y * WIDTH + x) * 3) as usize;
    for channel in 0..3 {
        rgb[index + channel] = color[channel].clamp(0.0, 255.0) as u8;
    }
}

fn scale(rgb: &mut [u8], intensity: f32) {
    if intensity >= 1.0 {
        return;
    }
    for channel in rgb {
        *channel = (*channel as f32 * intensity) as u8;
    }
}

fn mix32(mut value: u32) -> u32 {
    value ^= value >> 16;
    value = value.wrapping_mul(0x7feb_352d);
    value ^= value >> 15;
    value = value.wrapping_mul(0x846c_a68b);
    value ^ (value >> 16)
}

fn unit(value: u32) -> f32 {
    value as f32 / u32::MAX as f32
}

fn add_aurora(color: &mut [f32; 3], x: f32, y: f32, time: f32, phase: f32, tint: [f32; 3]) {
    let center = 0.5
        + 0.18 * (x * TAU * 1.35 + time * 0.95 + phase).sin()
        + 0.055 * (x * TAU * 4.4 - time * 1.7 + phase).cos();
    let width = 0.025 + 0.018 * (x * TAU * 2.2 + time + phase).sin().abs();
    let distance = (y - center).abs();
    let core = (-(distance * distance) / (2.0 * width * width)).exp();
    let haze = (-(distance * distance) / (2.0 * 0.15_f32.powi(2))).exp() * 0.18;
    let curtain = 0.62
        + 0.38
            * (x * TAU * 9.0 + y * TAU * 2.0 - time * 2.3 + phase)
                .sin()
                .powi(2);
    let intensity = (core * curtain + haze) * 0.72;

    for channel in 0..3 {
        color[channel] += tint[channel] * intensity;
    }
}

fn star_brightness(x: u32, y: u32, time: f32) -> f32 {
    let hash = x.wrapping_mul(0x9e37_79b9).rotate_left(13) ^ y.wrapping_mul(0x85eb_ca6b);
    if hash % 997 > 7 {
        return 0.0;
    }
    let twinkle = (time * (1.2 + (hash % 17) as f32 * 0.07) + (hash % 31) as f32)
        .sin()
        .mul_add(0.35, 0.65);
    twinkle * ((hash % 100) as f32 / 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_scene_fills_every_device_key() {
        for scene in ScreensaverScene::ALL {
            let canvas = ScreensaverCanvas::render(scene, 2.0, 1.0);
            for position in GRID.positions() {
                let key = canvas.key(position);
                assert_eq!(key.size, KEY_SIZE);
                assert_eq!(key.rgb.len(), (KEY_SIZE * KEY_SIZE * 3) as usize);
            }
        }
    }

    #[test]
    fn slicing_and_reassembly_preserve_the_virtual_canvas() {
        let canvas = ScreensaverCanvas::render(ScreensaverScene::Space, 3.0, 1.0);
        for position in GRID.positions() {
            let key = canvas.key(position);
            let origin_x = (position.column as u32 - 1) * KEY_SIZE;
            let origin_y = (position.row as u32 - 1) * KEY_SIZE;
            for y in 0..KEY_SIZE {
                let canvas_start = (((origin_y + y) * WIDTH + origin_x) * 3) as usize;
                let key_start = (y * KEY_SIZE * 3) as usize;
                assert_eq!(
                    &canvas.rgb[canvas_start..canvas_start + (KEY_SIZE * 3) as usize],
                    &key.rgb[key_start..key_start + (KEY_SIZE * 3) as usize]
                );
            }
        }
    }

    #[test]
    fn scene_order_wraps_after_space() {
        assert_eq!(ScreensaverScene::Aurora.next(), ScreensaverScene::Matrix);
        assert_eq!(ScreensaverScene::Matrix.next(), ScreensaverScene::Space);
        assert_eq!(ScreensaverScene::Space.next(), ScreensaverScene::Aurora);
    }

    #[test]
    fn scenes_are_visually_distinct() {
        let aurora = ScreensaverCanvas::render(ScreensaverScene::Aurora, 4.0, 1.0);
        let matrix = ScreensaverCanvas::render(ScreensaverScene::Matrix, 4.0, 1.0);
        let space = ScreensaverCanvas::render(ScreensaverScene::Space, 4.0, 1.0);
        assert_ne!(aurora.rgb, matrix.rgb);
        assert_ne!(matrix.rgb, space.rgb);
        assert_ne!(space.rgb, aurora.rgb);
    }

    #[test]
    fn matrix_is_green_dominant_and_space_contains_bright_stars() {
        let matrix = ScreensaverCanvas::render(ScreensaverScene::Matrix, 7.0, 1.0);
        let red: u64 = matrix
            .rgb
            .iter()
            .step_by(3)
            .map(|value| u64::from(*value))
            .sum();
        let green: u64 = matrix
            .rgb
            .iter()
            .skip(1)
            .step_by(3)
            .map(|value| u64::from(*value))
            .sum();
        assert!(green > red * 2);

        let space = ScreensaverCanvas::render(ScreensaverScene::Space, 7.0, 1.0);
        assert!(space.rgb.iter().any(|channel| *channel > 220));
    }

    #[test]
    fn zero_intensity_is_fully_black() {
        for scene in ScreensaverScene::ALL {
            assert!(ScreensaverCanvas::render(scene, 12.0, 0.0)
                .rgb
                .iter()
                .all(|channel| *channel == 0));
        }
    }
}
