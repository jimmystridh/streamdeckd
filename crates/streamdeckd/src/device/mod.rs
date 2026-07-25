//! The device subsystem.
//!
//! A [`DeckDevice`] is the only way the runtime touches hardware. Three
//! implementations exist: the real HID device, a recording device for tests, and a
//! preview device that writes a composed PNG so nearly all development and CI can
//! run without exclusive access to the deck.

pub mod hid;
pub mod preview;
pub mod recording;

use async_trait::async_trait;
use streamdeck_core::model::{Grid, KeyPosition};
use streamdeck_render::RenderedKey;

#[derive(Debug, thiserror::Error)]
pub enum DeviceError {
    #[error("no Stream Deck matching {0} is connected")]
    NotFound(String),
    #[error("the Stream Deck is owned by another application; stop it and retry")]
    Busy,
    #[error("the Stream Deck disconnected")]
    Disconnected,
    #[error("device error: {0}")]
    Other(String),
}

/// What the daemon knows about a connected device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceDescriptor {
    pub serial: String,
    pub kind: String,
    pub grid: Grid,
    pub firmware: String,
}

/// A physical key transition read from the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyEvent {
    Down(KeyPosition),
    Up(KeyPosition),
}

/// The device contract. Everything is fallible because a USB device can vanish
/// between any two calls.
#[async_trait]
pub trait DeckDevice: Send + Sync {
    fn descriptor(&self) -> DeviceDescriptor;

    /// Sends one key image. Callers are expected to have already skipped
    /// unchanged payloads.
    ///
    /// Writes may be buffered by the device layer; nothing is guaranteed to reach
    /// the glass until [`DeckDevice::flush`] is called.
    async fn set_key(&self, position: KeyPosition, key: &RenderedKey)
        -> Result<usize, DeviceError>;

    /// Commits buffered key images to the glass. Called once after each batch of
    /// `set_key` writes; the MK.2 protocol layer caches image reports until then.
    async fn flush(&self) -> Result<(), DeviceError>;

    async fn set_brightness(&self, percent: u8) -> Result<(), DeviceError>;

    async fn clear(&self) -> Result<(), DeviceError>;

    /// Waits for the next key transition. `None` means the device is gone.
    async fn next_event(&self) -> Result<Option<KeyEvent>, DeviceError>;

    /// Releases the device. Called on shutdown and before a reconnect.
    async fn close(&self) -> Result<(), DeviceError>;
}

/// Tracks the last payload sent to each key so an unchanged frame is never
/// written to USB again.
#[derive(Debug, Default)]
pub struct FrameCache {
    hashes: std::collections::HashMap<KeyPosition, u64>,
    sent: u64,
    skipped: u64,
    bytes: u64,
}

impl FrameCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a successful send. Returns `false` when the payload is unchanged and
    /// the caller should not write it.
    pub fn should_send(&self, position: KeyPosition, key: &RenderedKey) -> bool {
        self.hashes.get(&position) != Some(&key.hash)
    }

    pub fn record_sent(&mut self, position: KeyPosition, key: &RenderedKey, bytes: usize) {
        self.hashes.insert(position, key.hash);
        self.sent += 1;
        self.bytes += bytes as u64;
    }

    pub fn record_skipped(&mut self) {
        self.skipped += 1;
    }

    /// Forgets every key, so a reconnect repaints the whole deck.
    pub fn invalidate(&mut self) {
        self.hashes.clear();
    }

    pub fn totals(&self) -> (u64, u64, u64) {
        (self.sent, self.skipped, self.bytes)
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

    const POSITION: KeyPosition = KeyPosition::new(2, 3);

    #[test]
    fn the_first_frame_for_a_key_is_always_sent() {
        let cache = FrameCache::new();
        assert!(cache.should_send(POSITION, &key(0x123456)));
    }

    #[test]
    fn an_unchanged_frame_is_never_sent_twice() {
        let mut cache = FrameCache::new();
        let frame = key(0x123456);

        assert!(cache.should_send(POSITION, &frame));
        cache.record_sent(POSITION, &frame, 900);
        assert!(!cache.should_send(POSITION, &frame));

        cache.record_skipped();
        assert_eq!(cache.totals(), (1, 1, 900));
    }

    #[test]
    fn a_changed_frame_is_sent() {
        let mut cache = FrameCache::new();
        let first = key(0x123456);
        cache.record_sent(POSITION, &first, 900);

        assert!(cache.should_send(POSITION, &key(0x654321)));
    }

    #[test]
    fn each_key_is_tracked_independently() {
        let mut cache = FrameCache::new();
        let frame = key(0x123456);
        cache.record_sent(POSITION, &frame, 900);

        assert!(cache.should_send(KeyPosition::new(1, 1), &frame));
    }

    #[test]
    fn invalidating_forces_a_full_repaint_after_a_reconnect() {
        let mut cache = FrameCache::new();
        let frame = key(0x123456);
        cache.record_sent(POSITION, &frame, 900);

        cache.invalidate();
        assert!(cache.should_send(POSITION, &frame));
        assert_eq!(
            cache.totals(),
            (1, 0, 900),
            "invalidation keeps the counters"
        );
    }
}
