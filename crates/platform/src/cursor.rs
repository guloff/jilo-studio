//! The mouse position in screen pixels, for when the windowing layer has none.
//!
//! While files are dragged in from the OS file manager, winit on Windows delivers no cursor events
//! (it only reports hovered and dropped files), so the UI cannot learn where a drop landed. The
//! same holds for winit on macOS.

/// The cursor's position on the virtual screen in physical pixels, or `None` where the OS cannot
/// say or this platform's windowing layer already reports it during a drag.
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
pub fn cursor_screen_position() -> Option<(i32, i32)> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut p = POINT { x: 0, y: 0 };
    // SAFETY: `p` is a valid, writable POINT for the duration of the call.
    unsafe { GetCursorPos(&mut p) }.ok()?;
    Some((p.x, p.y))
}

/// macOS: winit (0.30) handles no `draggingUpdated:`, so a drag from Finder moves no pointer either.
/// `CGEventGetLocation` gives global display points with the main display's top left as the
/// origin, which is the space winit's `inner_position` uses; they are scaled to the pixels of the
/// display under the cursor (the window's own scale whenever the drop is over it).
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
pub fn cursor_screen_position() -> Option<(i32, i32)> {
    use objc2_core_graphics::{CGDirectDisplayID, CGDisplayCopyDisplayMode, CGDisplayMode, CGError, CGEvent, CGGetDisplaysWithPoint};
    let event = CGEvent::new(None)?;
    let p = CGEvent::location(Some(&event));
    if !(p.x.is_finite() && p.y.is_finite()) {
        return None;
    }
    let mut display: CGDirectDisplayID = 0;
    let mut count = 0u32;
    // SAFETY: both out-pointers are valid, writable locals; at most one display ID is written.
    let err = unsafe { CGGetDisplaysWithPoint(p, 1, &mut display, &mut count) };
    let scale = if err == CGError::Success && count == 1 {
        CGDisplayCopyDisplayMode(display)
            .map(|m| (CGDisplayMode::pixel_width(Some(&m)), CGDisplayMode::width(Some(&m))))
            .filter(|&(px, pt)| px > 0 && pt > 0)
            .map_or(1.0, |(px, pt)| px as f64 / pt as f64)
    } else {
        1.0
    };
    Some(((p.x * scale).round() as i32, (p.y * scale).round() as i32))
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn cursor_screen_position() -> Option<(i32, i32)> {
    None
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    /// CoreGraphics reports a cursor position in global display space (negative on a display to the
    /// left of or above the main one, so only its size is checked).
    #[test]
    fn macos_reports_a_cursor_position() {
        let (x, y) = super::cursor_screen_position().expect("CoreGraphics reports the cursor");
        assert!(x.abs() < 1 << 20 && y.abs() < 1 << 20, "({x}, {y})");
    }
}
