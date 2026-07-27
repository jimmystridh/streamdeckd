//! Low-latency audio controls backed directly by CoreAudio.

use std::ffi::c_void;
use std::mem::size_of;
use std::sync::{Mutex, MutexGuard};

use async_trait::async_trait;
use core_foundation::base::TCFType;
use core_foundation::string::{CFString, CFStringRef};
use streamdeck_core::integrations::audio::{self, AudioInventory, AudioStatus};
use streamdeck_core::model::AudioKind;

use super::{AudioAdapter, AudioError};

type AudioObjectId = u32;
type OsStatus = i32;

#[repr(C)]
#[derive(Clone, Copy)]
struct PropertyAddress {
    selector: u32,
    scope: u32,
    element: u32,
}

const SYSTEM_OBJECT: AudioObjectId = 1;
const UNKNOWN_OBJECT: AudioObjectId = 0;
const SCOPE_GLOBAL: u32 = fourcc(*b"glob");
const SCOPE_INPUT: u32 = fourcc(*b"inpt");
const SCOPE_OUTPUT: u32 = fourcc(*b"outp");
const ELEMENT_MAIN: u32 = 0;
const DEVICES: u32 = fourcc(*b"dev#");
const DEFAULT_INPUT: u32 = fourcc(*b"dIn ");
const DEFAULT_OUTPUT: u32 = fourcc(*b"dOut");
const NAME: u32 = fourcc(*b"lnam");
const STREAMS: u32 = fourcc(*b"stm#");
const VOLUME_SCALAR: u32 = fourcc(*b"volm");
const MUTE: u32 = fourcc(*b"mute");
const MAX_CHANNELS: u32 = 32;

const fn fourcc(bytes: [u8; 4]) -> u32 {
    ((bytes[0] as u32) << 24)
        | ((bytes[1] as u32) << 16)
        | ((bytes[2] as u32) << 8)
        | bytes[3] as u32
}

#[link(name = "CoreAudio", kind = "framework")]
extern "C" {
    fn AudioObjectHasProperty(object: AudioObjectId, address: *const PropertyAddress) -> u8;
    fn AudioObjectIsPropertySettable(
        object: AudioObjectId,
        address: *const PropertyAddress,
        settable: *mut u8,
    ) -> OsStatus;
    fn AudioObjectGetPropertyDataSize(
        object: AudioObjectId,
        address: *const PropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: *mut u32,
    ) -> OsStatus;
    fn AudioObjectGetPropertyData(
        object: AudioObjectId,
        address: *const PropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: *mut u32,
        data: *mut c_void,
    ) -> OsStatus;
    fn AudioObjectSetPropertyData(
        object: AudioObjectId,
        address: *const PropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: u32,
        data: *const c_void,
    ) -> OsStatus;
}

pub struct CoreAudioAdapter {
    transaction: Mutex<()>,
}

impl CoreAudioAdapter {
    pub fn new() -> Self {
        Self {
            transaction: Mutex::new(()),
        }
    }

    fn transaction(&self) -> MutexGuard<'_, ()> {
        self.transaction
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn status_now(&self) -> Result<AudioStatus, AudioError> {
        let output = default_device(AudioKind::Output)?;
        let input = default_device(AudioKind::Input)?;
        Ok(AudioStatus {
            current_output: device_name(output)?,
            current_input: device_name(input)?,
            output_volume: read_volume(output, AudioKind::Output)?,
            input_volume: read_volume(input, AudioKind::Input)?,
            output_muted: read_mute(output).unwrap_or(false),
        })
    }
}

impl Default for CoreAudioAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AudioAdapter for CoreAudioAdapter {
    async fn status(&self) -> Result<AudioStatus, AudioError> {
        let _transaction = self.transaction();
        self.status_now()
    }

    async fn inventory(&self) -> Result<AudioInventory, AudioError> {
        let _transaction = self.transaction();
        Ok(AudioInventory {
            outputs: device_names(AudioKind::Output)?,
            inputs: device_names(AudioKind::Input)?,
        })
    }

    async fn select(&self, kind: AudioKind, name: &str) -> Result<(), AudioError> {
        let _transaction = self.transaction();
        let matches: Vec<AudioObjectId> = devices(kind)?
            .into_iter()
            .filter_map(|device| {
                device_name(device)
                    .ok()
                    .filter(|candidate| candidate == name)
                    .map(|_| device)
            })
            .collect();
        let device = match matches.as_slice() {
            [device] => *device,
            [] => {
                return Err(AudioError::Unavailable {
                    label: name.to_string(),
                })
            }
            _ => {
                return Err(AudioError::Ambiguous {
                    label: name.to_string(),
                    count: matches.len(),
                })
            }
        };
        set_value(
            SYSTEM_OBJECT,
            address(default_selector(kind), SCOPE_GLOBAL, ELEMENT_MAIN),
            &device,
            "select the default audio device",
        )
    }

    async fn set_volume(&self, kind: AudioKind, volume: u8) -> Result<(), AudioError> {
        let _transaction = self.transaction();
        let device = default_device(kind)?;
        write_volume(device, kind, volume)
    }

    async fn set_output_muted(&self, muted: bool) -> Result<(), AudioError> {
        let _transaction = self.transaction();
        let device = default_device(AudioKind::Output)?;
        write_mute(device, muted)
    }

    async fn adjust_volume_relative(&self, kind: AudioKind, delta: i32) -> Result<u8, AudioError> {
        let _transaction = self.transaction();
        let device = default_device(kind)?;
        let current = read_volume(device, kind)?;
        let next = audio::next_volume(current, delta);
        write_volume(device, kind, next)?;
        Ok(next)
    }

    async fn toggle_mute_state(
        &self,
        kind: AudioKind,
        restore_volume: u8,
    ) -> Result<(bool, Option<u8>), AudioError> {
        let _transaction = self.transaction();
        match kind {
            AudioKind::Output => {
                let device = default_device(AudioKind::Output)?;
                let muted = !read_mute(device)?;
                write_mute(device, muted)?;
                Ok((muted, None))
            }
            AudioKind::Input => {
                let device = default_device(AudioKind::Input)?;
                let current = read_volume(device, AudioKind::Input)?;
                if current == 0 {
                    write_volume(device, AudioKind::Input, restore_volume.clamp(1, 100))?;
                    Ok((false, None))
                } else {
                    write_volume(device, AudioKind::Input, 0)?;
                    Ok((true, Some(current)))
                }
            }
        }
    }
}

fn devices(kind: AudioKind) -> Result<Vec<AudioObjectId>, AudioError> {
    let devices = read_vec::<AudioObjectId>(
        SYSTEM_OBJECT,
        address(DEVICES, SCOPE_GLOBAL, ELEMENT_MAIN),
        "list audio devices",
    )?;
    Ok(devices
        .into_iter()
        .filter(|device| {
            data_size(*device, address(STREAMS, scope(kind), ELEMENT_MAIN))
                .is_some_and(|size| size > 0)
        })
        .collect())
}

fn device_names(kind: AudioKind) -> Result<Vec<String>, AudioError> {
    let mut names: Vec<String> = devices(kind)?
        .into_iter()
        .filter_map(|device| device_name(device).ok())
        .collect();
    names.sort();
    names.dedup();
    Ok(names)
}

fn default_device(kind: AudioKind) -> Result<AudioObjectId, AudioError> {
    let device = read_value::<AudioObjectId>(
        SYSTEM_OBJECT,
        address(default_selector(kind), SCOPE_GLOBAL, ELEMENT_MAIN),
        "read the default audio device",
    )?;
    if device == UNKNOWN_OBJECT {
        Err(AudioError::Unavailable {
            label: match kind {
                AudioKind::Output => "default output".to_string(),
                AudioKind::Input => "default input".to_string(),
            },
        })
    } else {
        Ok(device)
    }
}

fn device_name(device: AudioObjectId) -> Result<String, AudioError> {
    let mut value: CFStringRef = std::ptr::null();
    let mut size = size_of::<CFStringRef>() as u32;
    let status = unsafe {
        AudioObjectGetPropertyData(
            device,
            &address(NAME, SCOPE_GLOBAL, ELEMENT_MAIN),
            0,
            std::ptr::null(),
            &mut size,
            (&mut value as *mut CFStringRef).cast(),
        )
    };
    check(status, "read an audio device name")?;
    if value.is_null() {
        return Err(AudioError::CoreAudio {
            operation: "read an audio device name",
            status: -1,
        });
    }
    Ok(unsafe { CFString::wrap_under_create_rule(value) }.to_string())
}

fn read_volume(device: AudioObjectId, kind: AudioKind) -> Result<u8, AudioError> {
    let values = readable_values::<f32>(device, VOLUME_SCALAR, scope(kind));
    if values.is_empty() {
        return Err(AudioError::Unsupported {
            device: device_name(device).unwrap_or_else(|_| format!("#{device}")),
            property: "volume",
        });
    }
    let average = values.iter().sum::<f32>() / values.len() as f32;
    Ok((average.clamp(0.0, 1.0) * 100.0).round() as u8)
}

fn write_volume(device: AudioObjectId, kind: AudioKind, volume: u8) -> Result<(), AudioError> {
    let scalar = f32::from(volume.min(100)) / 100.0;
    if write_elements(device, VOLUME_SCALAR, scope(kind), &scalar)? {
        Ok(())
    } else {
        Err(AudioError::Unsupported {
            device: device_name(device).unwrap_or_else(|_| format!("#{device}")),
            property: "volume",
        })
    }
}

fn read_mute(device: AudioObjectId) -> Result<bool, AudioError> {
    let values = readable_values::<u32>(device, MUTE, SCOPE_OUTPUT);
    if values.is_empty() {
        return Err(AudioError::Unsupported {
            device: device_name(device).unwrap_or_else(|_| format!("#{device}")),
            property: "mute",
        });
    }
    Ok(values.into_iter().any(|value| value != 0))
}

fn write_mute(device: AudioObjectId, muted: bool) -> Result<(), AudioError> {
    let value = u32::from(muted);
    if write_elements(device, MUTE, SCOPE_OUTPUT, &value)? {
        Ok(())
    } else {
        Err(AudioError::Unsupported {
            device: device_name(device).unwrap_or_else(|_| format!("#{device}")),
            property: "mute",
        })
    }
}

fn readable_values<T: Copy + Default>(object: AudioObjectId, selector: u32, scope: u32) -> Vec<T> {
    if let Ok(value) = read_value(
        object,
        address(selector, scope, ELEMENT_MAIN),
        "read an audio property",
    ) {
        return vec![value];
    }
    (1..=MAX_CHANNELS)
        .filter_map(|element| {
            read_value(
                object,
                address(selector, scope, element),
                "read an audio channel property",
            )
            .ok()
        })
        .collect()
}

fn write_elements<T>(
    object: AudioObjectId,
    selector: u32,
    scope: u32,
    value: &T,
) -> Result<bool, AudioError> {
    let main = address(selector, scope, ELEMENT_MAIN);
    if is_settable(object, main) {
        set_value(object, main, value, "set an audio property")?;
        return Ok(true);
    }

    let mut wrote = false;
    for element in 1..=MAX_CHANNELS {
        let channel = address(selector, scope, element);
        if is_settable(object, channel) {
            set_value(object, channel, value, "set an audio channel property")?;
            wrote = true;
        }
    }
    Ok(wrote)
}

fn read_value<T: Copy + Default>(
    object: AudioObjectId,
    address: PropertyAddress,
    operation: &'static str,
) -> Result<T, AudioError> {
    if !has_property(object, address) {
        return Err(AudioError::CoreAudio {
            operation,
            status: -1,
        });
    }
    let mut value = T::default();
    let mut size = size_of::<T>() as u32;
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            &address,
            0,
            std::ptr::null(),
            &mut size,
            (&mut value as *mut T).cast(),
        )
    };
    check(status, operation)?;
    if size as usize != size_of::<T>() {
        return Err(AudioError::CoreAudio {
            operation,
            status: -1,
        });
    }
    Ok(value)
}

fn read_vec<T: Copy + Default>(
    object: AudioObjectId,
    address: PropertyAddress,
    operation: &'static str,
) -> Result<Vec<T>, AudioError> {
    let size = data_size(object, address).ok_or(AudioError::CoreAudio {
        operation,
        status: -1,
    })?;
    if size == 0 {
        return Ok(Vec::new());
    }
    let mut values = vec![T::default(); size as usize / size_of::<T>()];
    let mut actual = size;
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            &address,
            0,
            std::ptr::null(),
            &mut actual,
            values.as_mut_ptr().cast(),
        )
    };
    check(status, operation)?;
    values.truncate(actual as usize / size_of::<T>());
    Ok(values)
}

fn set_value<T>(
    object: AudioObjectId,
    address: PropertyAddress,
    value: &T,
    operation: &'static str,
) -> Result<(), AudioError> {
    let status = unsafe {
        AudioObjectSetPropertyData(
            object,
            &address,
            0,
            std::ptr::null(),
            size_of::<T>() as u32,
            (value as *const T).cast(),
        )
    };
    check(status, operation)
}

fn data_size(object: AudioObjectId, address: PropertyAddress) -> Option<u32> {
    if !has_property(object, address) {
        return None;
    }
    let mut size = 0;
    let status =
        unsafe { AudioObjectGetPropertyDataSize(object, &address, 0, std::ptr::null(), &mut size) };
    (status == 0).then_some(size)
}

fn has_property(object: AudioObjectId, address: PropertyAddress) -> bool {
    unsafe { AudioObjectHasProperty(object, &address) != 0 }
}

fn is_settable(object: AudioObjectId, address: PropertyAddress) -> bool {
    if !has_property(object, address) {
        return false;
    }
    let mut settable = 0;
    unsafe { AudioObjectIsPropertySettable(object, &address, &mut settable) == 0 && settable != 0 }
}

fn check(status: OsStatus, operation: &'static str) -> Result<(), AudioError> {
    if status == 0 {
        Ok(())
    } else {
        Err(AudioError::CoreAudio { operation, status })
    }
}

const fn address(selector: u32, scope: u32, element: u32) -> PropertyAddress {
    PropertyAddress {
        selector,
        scope,
        element,
    }
}

const fn scope(kind: AudioKind) -> u32 {
    match kind {
        AudioKind::Output => SCOPE_OUTPUT,
        AudioKind::Input => SCOPE_INPUT,
    }
}

const fn default_selector(kind: AudioKind) -> u32 {
    match kind {
        AudioKind::Output => DEFAULT_OUTPUT,
        AudioKind::Input => DEFAULT_INPUT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_character_codes_match_the_coreaudio_headers() {
        assert_eq!(fourcc(*b"dOut"), 0x644f_7574);
        assert_eq!(fourcc(*b"volm"), 0x766f_6c6d);
    }

    #[tokio::test]
    async fn live_status_and_inventory_include_the_current_devices() {
        let adapter = CoreAudioAdapter::new();
        let status = adapter.status().await.expect("status");
        let inventory = adapter.inventory().await.expect("inventory");

        assert!(inventory.outputs.contains(&status.current_output));
        assert!(inventory.inputs.contains(&status.current_input));
        assert!(status.output_volume <= 100);
        assert!(status.input_volume <= 100);
    }
}
