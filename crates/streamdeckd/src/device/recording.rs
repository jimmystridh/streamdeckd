//! A device that records what it was sent and replays scripted key events.
//!
//! Lets the whole runtime — navigation, press handling, rendering, shutdown — be
//! tested without hardware.

use std::sync::Mutex;

use async_trait::async_trait;
use streamdeck_core::model::{Grid, KeyPosition};
use streamdeck_render::RenderedKey;
use tokio::sync::mpsc;

use super::{DeckDevice, DeviceDescriptor, DeviceError, KeyEvent};

/// One thing the runtime asked the device to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sent {
    Key { position: KeyPosition, hash: u64 },
    Brightness(u8),
    Cleared,
    Closed,
}

pub struct RecordingDeckDevice {
    descriptor: DeviceDescriptor,
    sent: Mutex<Vec<Sent>>,
    /// Counted separately from `sent` so tests can assert exact write sequences
    /// without flushes interleaved.
    flushes: Mutex<u64>,
    events: tokio::sync::Mutex<mpsc::UnboundedReceiver<KeyEvent>>,
    /// Set to simulate a device that has gone away.
    failing: Mutex<Option<DeviceError>>,
}

impl RecordingDeckDevice {
    /// Returns the device and a sender for scripted key events.
    pub fn new() -> (Self, mpsc::UnboundedSender<KeyEvent>) {
        let (sender, receiver) = mpsc::unbounded_channel();
        (
            Self {
                descriptor: DeviceDescriptor {
                    serial: "RECORDING0001".to_string(),
                    kind: "Recording".to_string(),
                    grid: Grid::MK2,
                    firmware: "test".to_string(),
                },
                sent: Mutex::new(Vec::new()),
                flushes: Mutex::new(0),
                events: tokio::sync::Mutex::new(receiver),
                failing: Mutex::new(None),
            },
            sender,
        )
    }

    pub fn sent(&self) -> Vec<Sent> {
        self.sent.lock().expect("recording lock").clone()
    }

    pub fn keys_sent(&self) -> Vec<KeyPosition> {
        self.sent()
            .into_iter()
            .filter_map(|entry| match entry {
                Sent::Key { position, .. } => Some(position),
                _ => None,
            })
            .collect()
    }

    pub fn reset(&self) {
        self.sent.lock().expect("recording lock").clear();
        *self.flushes.lock().expect("flush lock") = 0;
    }

    pub fn flushes(&self) -> u64 {
        *self.flushes.lock().expect("flush lock")
    }

    /// Makes every subsequent operation fail, as a disconnect would.
    pub fn start_failing(&self, error: DeviceError) {
        *self.failing.lock().expect("failing lock") = Some(error);
    }

    fn check(&self) -> Result<(), DeviceError> {
        match &*self.failing.lock().expect("failing lock") {
            Some(DeviceError::Busy) => Err(DeviceError::Busy),
            Some(DeviceError::Disconnected) => Err(DeviceError::Disconnected),
            Some(DeviceError::NotFound(what)) => Err(DeviceError::NotFound(what.clone())),
            Some(DeviceError::Other(message)) => Err(DeviceError::Other(message.clone())),
            None => Ok(()),
        }
    }

    fn record(&self, entry: Sent) {
        self.sent.lock().expect("recording lock").push(entry);
    }
}

#[async_trait]
impl DeckDevice for RecordingDeckDevice {
    fn descriptor(&self) -> DeviceDescriptor {
        self.descriptor.clone()
    }

    async fn set_key(
        &self,
        position: KeyPosition,
        key: &RenderedKey,
    ) -> Result<usize, DeviceError> {
        self.check()?;
        self.record(Sent::Key {
            position,
            hash: key.hash,
        });
        Ok(key.rgb.len())
    }

    async fn flush(&self) -> Result<(), DeviceError> {
        self.check()?;
        *self.flushes.lock().expect("flush lock") += 1;
        Ok(())
    }

    async fn set_brightness(&self, percent: u8) -> Result<(), DeviceError> {
        self.check()?;
        self.record(Sent::Brightness(percent));
        Ok(())
    }

    async fn clear(&self) -> Result<(), DeviceError> {
        self.check()?;
        self.record(Sent::Cleared);
        Ok(())
    }

    async fn next_event(&self) -> Result<Option<KeyEvent>, DeviceError> {
        self.check()?;
        let mut events = self.events.lock().await;
        Ok(events.recv().await)
    }

    async fn close(&self) -> Result<(), DeviceError> {
        self.record(Sent::Closed);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use streamdeck_core::view::{Color, KeyView};
    use streamdeck_render::Renderer;

    fn key() -> RenderedKey {
        Renderer::new()
            .expect("renderer")
            .render(&KeyView::solid(Color::hex(0x123456)))
            .expect("rendered")
    }

    #[tokio::test]
    async fn everything_sent_is_recorded_in_order() {
        let (device, _events) = RecordingDeckDevice::new();
        device.set_brightness(60).await.expect("brightness");
        device
            .set_key(KeyPosition::new(1, 1), &key())
            .await
            .expect("key");
        device.clear().await.expect("clear");
        device.close().await.expect("close");

        let sent = device.sent();
        assert_eq!(sent.len(), 4);
        assert_eq!(sent[0], Sent::Brightness(60));
        assert!(matches!(sent[1], Sent::Key { .. }));
        assert_eq!(sent[2], Sent::Cleared);
        assert_eq!(sent[3], Sent::Closed);
    }

    #[tokio::test]
    async fn scripted_key_events_are_delivered_in_order() {
        let (device, events) = RecordingDeckDevice::new();
        events
            .send(KeyEvent::Down(KeyPosition::new(2, 3)))
            .expect("sent");
        events
            .send(KeyEvent::Up(KeyPosition::new(2, 3)))
            .expect("sent");

        assert_eq!(
            device.next_event().await.expect("event"),
            Some(KeyEvent::Down(KeyPosition::new(2, 3)))
        );
        assert_eq!(
            device.next_event().await.expect("event"),
            Some(KeyEvent::Up(KeyPosition::new(2, 3)))
        );
    }

    #[tokio::test]
    async fn closing_the_event_channel_reports_a_gone_device() {
        let (device, events) = RecordingDeckDevice::new();
        drop(events);
        assert_eq!(device.next_event().await.expect("event"), None);
    }

    #[tokio::test]
    async fn a_failing_device_reports_its_error_on_every_operation() {
        let (device, _events) = RecordingDeckDevice::new();
        device.start_failing(DeviceError::Disconnected);

        assert!(device.set_brightness(50).await.is_err());
        assert!(device
            .set_key(KeyPosition::new(1, 1), &key())
            .await
            .is_err());
        assert!(device.clear().await.is_err());
        assert!(
            device.close().await.is_ok(),
            "closing must always succeed so shutdown cannot hang"
        );
    }

    #[tokio::test]
    async fn flushes_are_counted_without_polluting_the_write_sequence() {
        let (device, _events) = RecordingDeckDevice::new();
        device
            .set_key(KeyPosition::new(1, 1), &key())
            .await
            .expect("key");
        device.flush().await.expect("flush");
        device.flush().await.expect("flush");

        assert_eq!(device.flushes(), 2);
        assert_eq!(device.sent().len(), 1, "flushes stay out of the sent list");
    }

    #[tokio::test]
    async fn resetting_clears_the_recording() {
        let (device, _events) = RecordingDeckDevice::new();
        device.set_brightness(10).await.expect("brightness");
        device.reset();
        assert!(device.sent().is_empty());
    }
}
