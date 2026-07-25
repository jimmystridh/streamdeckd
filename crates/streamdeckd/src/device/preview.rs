//! A device that writes the composed deck to a PNG.
//!
//! This is the default development target: the whole runtime runs, every key
//! renders, and the result is a file that can be opened next to the physical deck.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use async_trait::async_trait;
use streamdeck_core::model::{Grid, KeyPosition};
use streamdeck_render::{RenderedKey, KEY_SIZE};
use tokio::sync::mpsc;

use super::{DeckDevice, DeviceDescriptor, DeviceError, KeyEvent};

/// Gutter between keys in the composed image, so the layout reads clearly.
const GUTTER: u32 = 4;

pub struct PreviewDeckDevice {
    descriptor: DeviceDescriptor,
    output: PathBuf,
    canvas: Mutex<image::RgbImage>,
    events: tokio::sync::Mutex<mpsc::UnboundedReceiver<KeyEvent>>,
}

impl PreviewDeckDevice {
    pub fn new(output: impl Into<PathBuf>, grid: Grid) -> (Self, mpsc::UnboundedSender<KeyEvent>) {
        let (sender, receiver) = mpsc::unbounded_channel();
        let width = grid.columns as u32 * (KEY_SIZE + GUTTER) + GUTTER;
        let height = grid.rows as u32 * (KEY_SIZE + GUTTER) + GUTTER;
        (
            Self {
                descriptor: DeviceDescriptor {
                    serial: "PREVIEW00001".to_string(),
                    kind: "Preview".to_string(),
                    grid,
                    firmware: env!("CARGO_PKG_VERSION").to_string(),
                },
                output: output.into(),
                canvas: Mutex::new(image::RgbImage::from_pixel(
                    width,
                    height,
                    image::Rgb([16, 18, 24]),
                )),
                events: tokio::sync::Mutex::new(receiver),
            },
            sender,
        )
    }

    pub fn output(&self) -> &Path {
        &self.output
    }

    /// Writes the composed deck. Called after every key so the file always shows
    /// the current frame.
    fn flush(&self) -> Result<(), DeviceError> {
        let canvas = self.canvas.lock().expect("canvas lock");
        if let Some(parent) = self.output.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| DeviceError::Other(error.to_string()))?;
        }
        canvas
            .save(&self.output)
            .map_err(|error| DeviceError::Other(error.to_string()))
    }
}

#[async_trait]
impl DeckDevice for PreviewDeckDevice {
    fn descriptor(&self) -> DeviceDescriptor {
        self.descriptor.clone()
    }

    async fn set_key(
        &self,
        position: KeyPosition,
        key: &RenderedKey,
    ) -> Result<usize, DeviceError> {
        if self.descriptor.grid.index_of(position).is_none() {
            return Err(DeviceError::Other(format!(
                "{position} is outside the grid"
            )));
        }
        let image = key
            .to_image()
            .map_err(|error| DeviceError::Other(error.to_string()))?;
        {
            let mut canvas = self.canvas.lock().expect("canvas lock");
            let x = GUTTER + (position.column as u32 - 1) * (KEY_SIZE + GUTTER);
            let y = GUTTER + (position.row as u32 - 1) * (KEY_SIZE + GUTTER);
            image::imageops::replace(&mut *canvas, &image, i64::from(x), i64::from(y));
        }
        self.flush()?;
        Ok(key.rgb.len())
    }

    async fn flush(&self) -> Result<(), DeviceError> {
        // Every set_key already saved the composed PNG; nothing is buffered.
        Ok(())
    }

    async fn set_brightness(&self, _percent: u8) -> Result<(), DeviceError> {
        Ok(())
    }

    async fn clear(&self) -> Result<(), DeviceError> {
        {
            let mut canvas = self.canvas.lock().expect("canvas lock");
            for pixel in canvas.pixels_mut() {
                *pixel = image::Rgb([16, 18, 24]);
            }
        }
        self.flush()
    }

    async fn next_event(&self) -> Result<Option<KeyEvent>, DeviceError> {
        let mut events = self.events.lock().await;
        Ok(events.recv().await)
    }

    async fn close(&self) -> Result<(), DeviceError> {
        self.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use streamdeck_core::view::{Color, KeyView};
    use streamdeck_render::Renderer;

    fn key(color: u32) -> RenderedKey {
        Renderer::new()
            .expect("renderer")
            .render(&KeyView::solid(Color::hex(color)))
            .expect("rendered")
    }

    #[tokio::test]
    async fn keys_are_composed_into_one_image_at_their_coordinates() {
        let directory = tempfile::tempdir().expect("temp dir");
        let output = directory.path().join("deck.png");
        let (device, _events) = PreviewDeckDevice::new(&output, Grid::MK2);

        device
            .set_key(KeyPosition::new(1, 1), &key(0xff0000))
            .await
            .expect("key");
        device
            .set_key(KeyPosition::new(3, 5), &key(0x00ff00))
            .await
            .expect("key");

        let image = image::open(&output).expect("written").to_rgb8();
        assert_eq!(image.width(), 5 * (KEY_SIZE + GUTTER) + GUTTER);
        assert_eq!(image.height(), 3 * (KEY_SIZE + GUTTER) + GUTTER);

        // The centre of the first key is red, and of the last key is green.
        let first = image.get_pixel(GUTTER + KEY_SIZE / 2, GUTTER + KEY_SIZE / 2);
        assert!(first[0] > 180 && first[1] < 60, "{first:?}");
        let last = image.get_pixel(
            GUTTER + 4 * (KEY_SIZE + GUTTER) + KEY_SIZE / 2,
            GUTTER + 2 * (KEY_SIZE + GUTTER) + KEY_SIZE / 2,
        );
        assert!(last[1] > 180 && last[0] < 60, "{last:?}");
    }

    #[tokio::test]
    async fn a_coordinate_outside_the_grid_is_refused() {
        let directory = tempfile::tempdir().expect("temp dir");
        let (device, _events) =
            PreviewDeckDevice::new(directory.path().join("deck.png"), Grid::MK2);

        assert!(device
            .set_key(KeyPosition::new(4, 1), &key(0xffffff))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn clearing_resets_the_canvas() {
        let directory = tempfile::tempdir().expect("temp dir");
        let output = directory.path().join("deck.png");
        let (device, _events) = PreviewDeckDevice::new(&output, Grid::MK2);

        device
            .set_key(KeyPosition::new(1, 1), &key(0xffffff))
            .await
            .expect("key");
        device.clear().await.expect("cleared");

        let image = image::open(&output).expect("written").to_rgb8();
        let pixel = image.get_pixel(GUTTER + KEY_SIZE / 2, GUTTER + KEY_SIZE / 2);
        assert_eq!(pixel, &image::Rgb([16, 18, 24]));
    }

    #[tokio::test]
    async fn the_output_directory_is_created_on_demand() {
        let directory = tempfile::tempdir().expect("temp dir");
        let output = directory.path().join("nested/deeper/deck.png");
        let (device, _events) = PreviewDeckDevice::new(&output, Grid::MK2);

        device
            .set_key(KeyPosition::new(2, 2), &key(0x00aaff))
            .await
            .expect("key");
        assert!(output.exists());
    }

    #[tokio::test]
    async fn the_preview_reports_the_configured_grid() {
        let directory = tempfile::tempdir().expect("temp dir");
        let (device, _events) =
            PreviewDeckDevice::new(directory.path().join("deck.png"), Grid::MK2);
        assert_eq!(device.descriptor().grid, Grid::MK2);
        assert_eq!(device.output(), directory.path().join("deck.png"));
    }
}
