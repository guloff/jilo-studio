//! The display the app is on: how much brighter than SDR white it can show right now (EDR).

/// The current extended-dynamic-range headroom of the screen with the key window: the largest
/// pixel value it shows, with SDR white at 1.0 (macOS EDR: e.g. 2–16 on an XDR display, depending
/// on the brightness setting; 1.0 on an SDR display). 1.0 elsewhere, off the main thread, or when
/// the system cannot say.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
pub fn edr_headroom() -> f32 {
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject};
    if objc2::MainThreadMarker::new().is_none() {
        return 1.0;
    }
    let Some(cls) = AnyClass::get(c"NSScreen") else { return 1.0 };
    // SAFETY: `mainScreen` returns the screen with the key window or nil; on the main thread
    // (checked above), as AppKit requires. `maximumExtendedDynamicRangeColorComponentValue` is a
    // CGFloat (f64) property of NSScreen (macOS 10.15+).
    let v: f64 = unsafe {
        let screen: *mut AnyObject = msg_send![cls, mainScreen];
        match screen.as_ref() {
            Some(screen) => msg_send![screen, maximumExtendedDynamicRangeColorComponentValue],
            None => 1.0,
        }
    };
    if v.is_finite() { (v as f32).clamp(1.0, 64.0) } else { 1.0 }
}

/// Not macOS: no EDR.
#[cfg(not(target_os = "macos"))]
pub fn edr_headroom() -> f32 {
    1.0
}

#[cfg(test)]
mod tests {
    /// Off the main thread (tests) the answer is the SDR 1.0; never below it.
    #[test]
    fn headroom_is_at_least_sdr_white() {
        assert!(super::edr_headroom() >= 1.0);
    }
}
