//! The real Stream Deck over HID.
//!
//! The daemon opens the device only when explicitly started, never kills another
//! controller, and reports a clear diagnostic when the device is already owned.

use std::sync::{Arc, Mutex, OnceLock};

use super::{DeckDevice, DeviceDescriptor, DeviceError, KeyEvent};
use async_trait::async_trait;
use elgato_streamdeck::asynchronous::AsyncStreamDeck;
use elgato_streamdeck::info::Kind;
use elgato_streamdeck::{DeviceStateUpdate, StreamDeckError};
use streamdeck_core::model::{Grid, KeyPosition};
use streamdeck_render::RenderedKey;

/// How often the input reader checks for key reports, in polls per second.
///
/// The crate's `poll_rate` is a frequency in hertz — its reader sleeps
/// `1.0 / poll_rate` between non-blocking reads — not a timeout in seconds.
/// Passing 0.25 here once meant one check every *four seconds*, which read as
/// multi-second latency on every physical press. At 50 Hz a press is noticed
/// within 20 ms — well inside the 50 ms feedback budget once rendering and the
/// flush are added — and the whole reader costs about 0.1% CPU when idle. The
/// sleep happens outside the device lock, so writes are never delayed by it.
const INPUT_POLL_HZ: f32 = 50.0;

/// All `hidapi` work happens on one dedicated thread.
///
/// On macOS the library may only be initialised once per process — a second
/// instance that is then dropped calls `hid_exit()` and invalidates the surviving
/// instance's handles — and opening or enumerating devices from several threads
/// crashes it outright. Rather than trying to serialise callers with a lock, which
/// still leaves them on different threads, every call is funnelled to a single
/// long-lived worker that owns the `HidApi`.
type Job = Box<dyn FnOnce(&mut hidapi::HidApi) + Send + 'static>;

static WORKER: OnceLock<Mutex<Option<std::sync::mpsc::Sender<Job>>>> = OnceLock::new();

/// Starts the worker on first use. Returns `None` when `hidapi` is unavailable.
fn worker() -> Result<std::sync::mpsc::Sender<Job>, DeviceError> {
    let slot = WORKER.get_or_init(|| Mutex::new(None));
    let mut slot = slot
        .lock()
        .map_err(|_| DeviceError::Other("the hidapi worker lock was poisoned".to_string()))?;

    if let Some(sender) = slot.as_ref() {
        return Ok(sender.clone());
    }

    let (jobs, receiver) = std::sync::mpsc::channel::<Job>();
    let (ready, started) = std::sync::mpsc::channel::<Result<(), String>>();

    std::thread::Builder::new()
        .name("streamdeckd-hid".to_string())
        .spawn(move || {
            let mut api = match elgato_streamdeck::new_hidapi() {
                Ok(api) => {
                    let _ = ready.send(Ok(()));
                    api
                }
                Err(error) => {
                    let _ = ready.send(Err(error.to_string()));
                    return;
                }
            };
            // The `HidApi` lives for the process lifetime, so `hid_exit()` is
            // never called while a device handle is open.
            while let Ok(job) = receiver.recv() {
                job(&mut api);
            }
        })
        .map_err(|error| DeviceError::Other(format!("could not start the hid thread: {error}")))?;

    match started.recv() {
        Ok(Ok(())) => {
            *slot = Some(jobs.clone());
            Ok(jobs)
        }
        Ok(Err(error)) => Err(DeviceError::Other(format!(
            "could not open hidapi: {error}"
        ))),
        Err(_) => Err(DeviceError::Other(
            "the hid thread stopped before it started".to_string(),
        )),
    }
}

/// Runs `action` on the hid thread and waits for its result.
fn with_hidapi<T>(
    action: impl FnOnce(&mut hidapi::HidApi) -> Result<T, DeviceError> + Send + 'static,
) -> Result<T, DeviceError>
where
    T: Send + 'static,
{
    let jobs = worker()?;
    let (result, answer) = std::sync::mpsc::channel();

    jobs.send(Box::new(move |api| {
        // Rescan so a deck plugged in after startup is seen.
        let outcome = match elgato_streamdeck::refresh_device_list(api) {
            Ok(()) => action(api),
            Err(error) => Err(DeviceError::Other(format!(
                "could not scan for devices: {error}"
            ))),
        };
        let _ = result.send(outcome);
    }))
    .map_err(|_| DeviceError::Other("the hid thread has stopped".to_string()))?;

    answer
        .recv()
        .map_err(|_| DeviceError::Other("the hid thread stopped mid-request".to_string()))?
}

/// One connected device, plus its input reader.
pub struct HidDeckDevice {
    deck: AsyncStreamDeck,
    reader: Arc<elgato_streamdeck::asynchronous::AsyncDeviceStateReader>,
    descriptor: DeviceDescriptor,
    /// Serialises the encode-and-write path; HID writes are not reentrant.
    write_lock: tokio::sync::Mutex<()>,
}

/// What discovery found, for `streamdeckctl devices`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovered {
    pub serial: String,
    pub kind: String,
    pub rows: u8,
    pub columns: u8,
    /// `false` when another application already owns the device.
    pub available: bool,
}

/// Lists every connected Stream Deck and whether it can be opened. Opening is
/// attempted and immediately released, which is the only reliable way macOS
/// reports exclusive ownership.
pub fn discover() -> Result<Vec<Discovered>, DeviceError> {
    with_hidapi(|api| {
        Ok(elgato_streamdeck::list_devices(api)
            .into_iter()
            .map(|(kind, serial)| {
                // Opening and immediately dropping the handle is the only reliable
                // way macOS reports exclusive ownership by another application.
                let available = elgato_streamdeck::StreamDeck::connect(api, kind, &serial).is_ok();
                Discovered {
                    serial,
                    kind: format!("{kind:?}"),
                    rows: kind.row_count(),
                    columns: kind.column_count(),
                    available,
                }
            })
            .collect())
    })
}

impl HidDeckDevice {
    /// Opens the configured serial number, or the first 5x3 deck when none is
    /// configured.
    pub fn open(serial: Option<&str>) -> Result<Self, DeviceError> {
        let serial = serial.map(str::to_string);
        with_hidapi(move |api| {
            let wanted = serial
                .clone()
                .unwrap_or_else(|| "the first 5x3 Stream Deck".to_string());
            let (kind, found_serial) = elgato_streamdeck::list_devices(api)
                .into_iter()
                .find(|(kind, candidate)| match &serial {
                    Some(serial) => candidate == serial,
                    None => kind.row_count() == 3 && kind.column_count() == 5,
                })
                .ok_or(DeviceError::NotFound(wanted))?;

            let deck = AsyncStreamDeck::connect(api, kind, &found_serial).map_err(map_error)?;
            let reader = deck.get_reader();

            Ok(Self {
                descriptor: DeviceDescriptor {
                    serial: found_serial,
                    kind: format!("{kind:?}"),
                    grid: grid_of(kind),
                    firmware: String::new(),
                },
                deck,
                reader,
                write_lock: tokio::sync::Mutex::new(()),
            })
        })
    }
}

fn grid_of(kind: Kind) -> Grid {
    Grid {
        rows: kind.row_count(),
        columns: kind.column_count(),
    }
}

fn map_error(error: StreamDeckError) -> DeviceError {
    let text = error.to_string();
    // hidapi surfaces an exclusive-access failure as a generic open error.
    if text.contains("Permission")
        || text.contains("exclusive")
        || text.contains("could not open")
        || text.contains("Failed to open")
    {
        DeviceError::Busy
    } else if text.contains("No such device") || text.contains("disconnect") {
        DeviceError::Disconnected
    } else {
        DeviceError::Other(text)
    }
}

#[async_trait]
impl DeckDevice for HidDeckDevice {
    fn descriptor(&self) -> DeviceDescriptor {
        self.descriptor.clone()
    }

    async fn set_key(
        &self,
        position: KeyPosition,
        key: &RenderedKey,
    ) -> Result<usize, DeviceError> {
        let index = self
            .descriptor
            .grid
            .index_of(position)
            .ok_or_else(|| DeviceError::Other(format!("{position} is outside the grid")))?;
        let image = key
            .to_image()
            .map_err(|error| DeviceError::Other(error.to_string()))?;
        let dynamic = image::DynamicImage::ImageRgb8(image);

        let _guard = self.write_lock.lock().await;
        let payload = elgato_streamdeck::images::convert_image_async(self.deck.kind(), dynamic)
            .map_err(map_error)?;
        let bytes = payload.len();
        self.deck
            .write_image(index as u8, &payload)
            .await
            .map_err(map_error)?;
        Ok(bytes)
    }

    async fn flush(&self) -> Result<(), DeviceError> {
        // The crate's write_image only fills an in-memory cache; this is the call
        // that actually transmits the image reports.
        let _guard = self.write_lock.lock().await;
        self.deck.flush().await.map_err(map_error)
    }

    async fn set_brightness(&self, percent: u8) -> Result<(), DeviceError> {
        self.deck
            .set_brightness(percent.clamp(0, 100))
            .await
            .map_err(map_error)
    }

    async fn clear(&self) -> Result<(), DeviceError> {
        self.deck
            .clear_all_button_images()
            .await
            .map_err(map_error)?;
        self.deck.flush().await.map_err(map_error)
    }

    async fn next_event(&self) -> Result<Option<KeyEvent>, DeviceError> {
        loop {
            let updates = match self.reader.read(INPUT_POLL_HZ).await {
                Ok(updates) => updates,
                // A timeout with no input is normal; keep waiting.
                Err(StreamDeckError::HidError(hidapi::HidError::HidApiErrorEmpty)) => continue,
                Err(error) => {
                    let mapped = map_error(error);
                    if matches!(mapped, DeviceError::Disconnected) {
                        return Ok(None);
                    }
                    return Err(mapped);
                }
            };

            for update in updates {
                let event = match update {
                    DeviceStateUpdate::ButtonDown(index) => self
                        .descriptor
                        .grid
                        .position_of(index as usize)
                        .map(KeyEvent::Down),
                    DeviceStateUpdate::ButtonUp(index) => self
                        .descriptor
                        .grid
                        .position_of(index as usize)
                        .map(KeyEvent::Up),
                    // This layout has no encoders or touch strip.
                    _ => None,
                };
                if let Some(event) = event {
                    return Ok(Some(event));
                }
            }
        }
    }

    async fn close(&self) -> Result<(), DeviceError> {
        // Dropping the handle releases the HID device; flush first so the last
        // frame is not left half written.
        let _ = self.deck.flush().await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_five_by_three_kind_maps_to_the_mk2_grid() {
        assert_eq!(grid_of(Kind::Mk2), Grid::MK2);
        assert_eq!(grid_of(Kind::Original), Grid::MK2);
    }

    #[test]
    fn exclusive_access_failures_are_reported_as_busy() {
        let busy = map_error(StreamDeckError::HidError(hidapi::HidError::HidApiError {
            message: "Failed to open the device".to_string(),
        }));
        assert!(matches!(busy, DeviceError::Busy), "{busy}");
    }

    #[test]
    fn a_vanished_device_is_reported_as_disconnected() {
        let gone = map_error(StreamDeckError::HidError(hidapi::HidError::HidApiError {
            message: "No such device".to_string(),
        }));
        assert!(matches!(gone, DeviceError::Disconnected), "{gone}");
    }

    #[test]
    fn other_failures_keep_their_message() {
        let other = map_error(StreamDeckError::HidError(hidapi::HidError::HidApiError {
            message: "something unusual".to_string(),
        }));
        match other {
            DeviceError::Other(message) => assert!(message.contains("something unusual")),
            unexpected => panic!("expected a passthrough, got {unexpected}"),
        }
    }

    #[test]
    fn concurrent_enumeration_from_many_threads_is_safe() {
        // Enumerating from several threads at once used to crash the process; all
        // of it now runs on the one hid thread.
        let handles: Vec<_> = (0..6).map(|_| std::thread::spawn(discover)).collect();

        for handle in handles {
            match handle.join().expect("thread did not crash") {
                Ok(devices) => {
                    for device in devices {
                        assert!(!device.serial.is_empty());
                    }
                }
                // No hidapi on this machine is acceptable; a crash is not.
                Err(error) => assert!(matches!(error, DeviceError::Other(_)), "{error}"),
            }
        }
    }

    #[test]
    fn discovery_never_panics_even_with_no_device_attached() {
        // On CI there is no deck; discovery must simply report an empty list or a
        // clean hidapi error.
        match discover() {
            Ok(devices) => {
                for device in devices {
                    assert!(!device.serial.is_empty());
                }
            }
            Err(error) => assert!(matches!(error, DeviceError::Other(_)), "{error}"),
        }
    }
}
