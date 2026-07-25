#[cfg(target_os = "macos")]
use core_foundation::base::{CFType, TCFType};
#[cfg(target_os = "macos")]
use core_foundation::boolean::CFBoolean;
#[cfg(target_os = "macos")]
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
#[cfg(target_os = "macos")]
use core_foundation::string::CFString;

#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGSessionCopyCurrentDictionary() -> CFDictionaryRef;
}

#[cfg(target_os = "macos")]
pub fn screen_is_locked() -> bool {
    let raw = unsafe { CGSessionCopyCurrentDictionary() };
    if raw.is_null() {
        return false;
    }

    let session: CFDictionary<CFString, CFType> = unsafe { TCFType::wrap_under_create_rule(raw) };
    locked_value(&session)
}

#[cfg(target_os = "macos")]
fn locked_value(session: &CFDictionary<CFString, CFType>) -> bool {
    let key = CFString::from_static_string("CGSSessionScreenIsLocked");
    session
        .find(&key)
        .and_then(|value| value.downcast::<CFBoolean>())
        .is_some_and(bool::from)
}

#[cfg(not(target_os = "macos"))]
pub fn screen_is_locked() -> bool {
    false
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    fn dictionary(value: CFType) -> CFDictionary<CFString, CFType> {
        CFDictionary::from_CFType_pairs(&[(
            CFString::from_static_string("CGSSessionScreenIsLocked"),
            value,
        )])
    }

    #[test]
    fn the_session_boolean_reports_a_locked_screen() {
        let session = dictionary(CFBoolean::true_value().as_CFType());
        assert!(locked_value(&session));
    }

    #[test]
    fn false_and_unexpected_values_are_not_locked() {
        let unlocked = dictionary(CFBoolean::false_value().as_CFType());
        assert!(!locked_value(&unlocked));

        let unexpected = dictionary(CFString::from_static_string("yes").as_CFType());
        assert!(!locked_value(&unexpected));
    }

    #[test]
    fn the_live_session_can_be_queried() {
        let _current_state = screen_is_locked();
    }
}
