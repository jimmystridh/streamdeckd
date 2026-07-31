//! Native access to the Mac's ambient-light sensor.

#[derive(Debug, thiserror::Error)]
pub enum AmbientLightError {
    #[error("macOS did not provide an HID event-system client")]
    ClientUnavailable,
    #[error("this Mac has no ambient-light sensor")]
    SensorUnavailable,
    #[error("the ambient-light sensor returned no reading")]
    ReadingUnavailable,
    #[error("the ambient-light sensor returned an invalid reading")]
    InvalidReading,
}

#[cfg(target_os = "macos")]
mod macos {
    use core::ffi::c_void;

    use core_foundation::array::{CFArray, CFArrayRef};
    use core_foundation::base::{CFGetTypeID, CFRelease, CFRetain, CFTypeRef, TCFType};
    use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
    use core_foundation::number::CFNumber;
    use core_foundation::string::{CFString, CFStringRef};

    use super::AmbientLightError;

    type IoObject = u32;
    type IOHIDEventSystemClientRef = *mut c_void;
    type IOHIDServiceClientRef = *mut c_void;
    type IOHIDEventRef = *mut c_void;

    const HID_USAGE_PAGE_SENSOR: u32 = 0x20;
    const HID_USAGE_SENSOR_AMBIENT_LIGHT: u32 = 0x41;
    const HID_EVENT_AMBIENT_LIGHT_SENSOR: i64 = 12;
    const HID_FIELD_AMBIENT_LIGHT_LEVEL: i32 = (HID_EVENT_AMBIENT_LIGHT_SENSOR as i32) << 16;

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOServiceGetMatchingService(main_port: u32, matching: CFDictionaryRef) -> IoObject;
        fn IORegistryEntryCreateCFProperty(
            entry: IoObject,
            key: CFStringRef,
            allocator: *const c_void,
            options: u32,
        ) -> CFTypeRef;
        fn IOObjectRelease(object: IoObject) -> i32;

        fn IOHIDEventSystemClientCreateSimpleClient(
            allocator: *const c_void,
        ) -> IOHIDEventSystemClientRef;
        fn IOHIDEventSystemClientCopyServices(client: IOHIDEventSystemClientRef) -> CFArrayRef;
        fn IOHIDServiceClientConformsTo(
            service: IOHIDServiceClientRef,
            usage_page: u32,
            usage: u32,
        ) -> u32;
        fn IOHIDServiceClientCopyEvent(
            service: IOHIDServiceClientRef,
            event_type: i64,
            options: i32,
            matching_event: i64,
        ) -> IOHIDEventRef;
        fn IOHIDEventGetFloatValue(event: IOHIDEventRef, field: i32) -> f64;
    }

    enum Backend {
        Registry {
            service: IoObject,
        },
        Hid {
            client: Option<IOHIDEventSystemClientRef>,
            service: IOHIDServiceClientRef,
            bezel_framework: Option<*mut c_void>,
        },
    }

    pub struct AmbientLightSensor {
        backend: Backend,
    }

    impl AmbientLightSensor {
        pub fn open() -> Result<Self, AmbientLightError> {
            if let Some(service) = current_lux_service() {
                return Ok(Self {
                    backend: Backend::Registry { service },
                });
            }

            if let Some((service, framework)) = copy_bezel_als_service() {
                return Ok(Self {
                    backend: Backend::Hid {
                        client: None,
                        service,
                        bezel_framework: Some(framework),
                    },
                });
            }

            let client = unsafe { IOHIDEventSystemClientCreateSimpleClient(std::ptr::null()) };
            if client.is_null() {
                return Err(AmbientLightError::ClientUnavailable);
            }

            let services = unsafe { IOHIDEventSystemClientCopyServices(client) };
            if services.is_null() {
                unsafe { CFRelease(client as CFTypeRef) };
                return Err(AmbientLightError::SensorUnavailable);
            }
            let services = unsafe { CFArray::<*const c_void>::wrap_under_create_rule(services) };
            let service = services
                .get_all_values()
                .into_iter()
                .find(|service| unsafe {
                    IOHIDServiceClientConformsTo(
                        *service as IOHIDServiceClientRef,
                        HID_USAGE_PAGE_SENSOR,
                        HID_USAGE_SENSOR_AMBIENT_LIGHT,
                    ) != 0
                });
            let Some(service) = service else {
                unsafe { CFRelease(client as CFTypeRef) };
                return Err(AmbientLightError::SensorUnavailable);
            };
            unsafe { CFRetain(service as CFTypeRef) };

            Ok(Self {
                backend: Backend::Hid {
                    client: Some(client),
                    service: service as IOHIDServiceClientRef,
                    bezel_framework: None,
                },
            })
        }

        pub fn lux(&self) -> Result<f64, AmbientLightError> {
            let lux = match self.backend {
                Backend::Registry { service } => registry_lux(service)?,
                Backend::Hid { service, .. } => hid_lux(service)?,
            };
            if !lux.is_finite() || lux < 0.0 {
                return Err(AmbientLightError::InvalidReading);
            }
            Ok(lux)
        }
    }

    impl Drop for AmbientLightSensor {
        fn drop(&mut self) {
            match self.backend {
                Backend::Registry { service } => unsafe {
                    let _ = IOObjectRelease(service);
                },
                Backend::Hid {
                    client,
                    service,
                    bezel_framework,
                } => unsafe {
                    CFRelease(service as CFTypeRef);
                    if let Some(client) = client {
                        CFRelease(client as CFTypeRef);
                    }
                    if let Some(framework) = bezel_framework {
                        libc::dlclose(framework);
                    }
                },
            }
        }
    }

    fn current_lux_service() -> Option<IoObject> {
        let matching = CFDictionary::from_CFType_pairs(&[(
            CFString::new("IOPropertyExistsMatch"),
            CFString::new("CurrentLux"),
        )]);
        let service = unsafe { IOServiceGetMatchingService(0, matching.as_concrete_TypeRef()) };
        // IOServiceGetMatchingService consumes the matching dictionary.
        std::mem::forget(matching);
        (service != 0).then_some(service)
    }

    fn registry_lux(service: IoObject) -> Result<f64, AmbientLightError> {
        let key = CFString::new("CurrentLux");
        let property = unsafe {
            IORegistryEntryCreateCFProperty(service, key.as_concrete_TypeRef(), std::ptr::null(), 0)
        };
        if property.is_null() {
            return Err(AmbientLightError::ReadingUnavailable);
        }
        if unsafe { CFGetTypeID(property) } != CFNumber::type_id() {
            unsafe { CFRelease(property) };
            return Err(AmbientLightError::InvalidReading);
        }
        let number = unsafe { CFNumber::wrap_under_create_rule(property.cast_mut().cast()) };
        number.to_f64().ok_or(AmbientLightError::InvalidReading)
    }

    fn hid_lux(service: IOHIDServiceClientRef) -> Result<f64, AmbientLightError> {
        let event =
            unsafe { IOHIDServiceClientCopyEvent(service, HID_EVENT_AMBIENT_LIGHT_SENSOR, 0, 0) };
        if event.is_null() {
            return Err(AmbientLightError::ReadingUnavailable);
        }
        let lux = unsafe { IOHIDEventGetFloatValue(event, HID_FIELD_AMBIENT_LIGHT_LEVEL) };
        unsafe { CFRelease(event as CFTypeRef) };
        Ok(lux)
    }

    fn copy_bezel_als_service() -> Option<(IOHIDServiceClientRef, *mut c_void)> {
        type CopyService = unsafe extern "C" fn() -> IOHIDServiceClientRef;

        let path = c"/System/Library/PrivateFrameworks/BezelServices.framework/BezelServices";
        let symbol = c"ALCALSCopyALSServiceClient";
        let framework = unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL) };
        if framework.is_null() {
            return None;
        }
        let function = unsafe { libc::dlsym(framework, symbol.as_ptr()) };
        if function.is_null() {
            unsafe { libc::dlclose(framework) };
            return None;
        }
        let copy_service: CopyService = unsafe { std::mem::transmute(function) };
        let service = unsafe { copy_service() };
        if service.is_null() {
            unsafe { libc::dlclose(framework) };
            return None;
        }
        Some((service, framework))
    }
}

#[cfg(target_os = "macos")]
pub use macos::AmbientLightSensor;

#[cfg(not(target_os = "macos"))]
pub struct AmbientLightSensor;

#[cfg(not(target_os = "macos"))]
impl AmbientLightSensor {
    pub fn open() -> Result<Self, AmbientLightError> {
        Err(AmbientLightError::SensorUnavailable)
    }

    pub fn lux(&self) -> Result<f64, AmbientLightError> {
        Err(AmbientLightError::SensorUnavailable)
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn the_live_sensor_returns_a_plausible_lux_reading_when_available() {
        let sensor = match AmbientLightSensor::open() {
            Ok(sensor) => sensor,
            Err(error) => {
                eprintln!("ambient-light sensor unavailable: {error}");
                return;
            }
        };
        let lux = sensor.lux().expect("ambient-light reading");
        eprintln!("ambient light: {lux:.2} lux");
        assert!((0.0..=200_000.0).contains(&lux), "{lux}");
    }
}
