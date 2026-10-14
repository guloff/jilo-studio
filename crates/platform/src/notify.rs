//! System notifications: "the export is done" while the app is in the background, through the
//! macOS Notification Center (`UNUserNotificationCenter`). The first one asks the user for
//! permission (once; System Settings ▸ Notifications keeps the answer). Notifications need the app
//! bundle (its identifier); a bare binary (development builds, tests) reports that and shows
//! nothing. Other systems report that notifications are not available.

/// Whether this process can post notifications (macOS, inside an app bundle).
pub fn available() -> bool {
    imp::available()
}

/// Post a notification with `title` and `body`. Never blocks: the system shows it, or drops it
/// when the user said no.
pub fn send(title: &str, body: &str) -> Result<(), String> {
    if title.trim().is_empty() {
        return Err("a notification needs a title".into());
    }
    imp::send(title, body)
}

#[cfg(not(target_os = "macos"))]
mod imp {
    pub fn available() -> bool {
        false
    }
    pub fn send(_: &str, _: &str) -> Result<(), String> {
        Err("notifications need macOS".into())
    }
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod imp {
    use std::sync::Once;
    use std::sync::atomic::{AtomicU64, Ordering};

    use objc2::msg_send;
    use objc2::rc::{Retained, autoreleasepool};
    use objc2::runtime::{AnyClass, AnyObject, Bool};
    use objc2_foundation::NSString;

    #[link(name = "UserNotifications", kind = "framework")]
    unsafe extern "C" {}

    /// UNAuthorizationOptionSound | UNAuthorizationOptionAlert.
    const OPTIONS: usize = (1 << 1) | (1 << 2);

    fn class(name: &std::ffi::CStr) -> Option<&'static AnyClass> {
        AnyClass::get(name)
    }

    /// The main bundle's identifier (None for a bare binary).
    fn bundle_id() -> Option<String> {
        let bundle_cls = class(c"NSBundle")?;
        // SAFETY: `+[NSBundle mainBundle]` takes nothing and returns an autoreleased bundle (or nil).
        let bundle: Option<Retained<AnyObject>> = unsafe { msg_send![bundle_cls, mainBundle] };
        let bundle = bundle?;
        // SAFETY: `-[NSBundle bundleIdentifier]` returns an NSString or nil.
        let id: Option<Retained<NSString>> = unsafe { msg_send![&*bundle, bundleIdentifier] };
        id.map(|s| s.to_string()).filter(|s| !s.is_empty())
    }

    pub fn available() -> bool {
        autoreleasepool(|_| bundle_id().is_some() && class(c"UNUserNotificationCenter").is_some())
    }

    static ASKED: Once = Once::new();
    static NEXT: AtomicU64 = AtomicU64::new(1);

    pub fn send(title: &str, body: &str) -> Result<(), String> {
        autoreleasepool(|_| {
            if bundle_id().is_none() {
                return Err("notifications need the app bundle".to_string());
            }
            let center_cls = class(c"UNUserNotificationCenter").ok_or("no UserNotifications framework")?;
            let content_cls = class(c"UNMutableNotificationContent").ok_or("no UserNotifications framework")?;
            let request_cls = class(c"UNNotificationRequest").ok_or("no UserNotifications framework")?;
            let sound_cls = class(c"UNNotificationSound").ok_or("no UserNotifications framework")?;
            // SAFETY: `+currentNotificationCenter` returns the shared center of a bundled app.
            let center: Option<Retained<AnyObject>> = unsafe { msg_send![center_cls, currentNotificationCenter] };
            let center = center.ok_or("no notification center")?;
            ASKED.call_once(|| {
                let handler = block2::RcBlock::new(|_granted: Bool, _error: *mut AnyObject| {});
                // SAFETY: options are UNAuthorizationOptions bits; the handler is a live block of
                // the declared (BOOL, NSError *) signature, copied by the framework.
                unsafe {
                    let _: () = msg_send![&*center, requestAuthorizationWithOptions: OPTIONS, completionHandler: &*handler];
                }
            });
            // SAFETY: `+new` returns an owned, empty mutable content.
            let content: Retained<AnyObject> = unsafe { msg_send![content_cls, new] };
            let (t, b) = (NSString::from_str(title), NSString::from_str(body));
            // SAFETY: plain NSString property setters of UNMutableNotificationContent; the default
            // sound is a shared object.
            unsafe {
                let _: () = msg_send![&*content, setTitle: &*t];
                let _: () = msg_send![&*content, setBody: &*b];
                let sound: Option<Retained<AnyObject>> = msg_send![sound_cls, defaultSound];
                if let Some(sound) = sound {
                    let _: () = msg_send![&*content, setSound: &*sound];
                }
            }
            let id = NSString::from_str(&format!("jilo-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            let none: *const AnyObject = std::ptr::null();
            // SAFETY: identifier and content are live objects; a nil trigger delivers at once.
            let request: Option<Retained<AnyObject>> = unsafe { msg_send![request_cls, requestWithIdentifier: &*id, content: &*content, trigger: none] };
            let request = request.ok_or("the notification could not be made")?;
            // SAFETY: the request is live; a nil completion handler is allowed.
            unsafe {
                let _: () = msg_send![&*center, addNotificationRequest: &*request, withCompletionHandler: none];
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A test binary is not an app bundle: nothing is shown, and it says why.
    #[test]
    fn outside_an_app_bundle_nothing_is_posted() {
        assert!(!available());
        assert!(send("Export finished", "Main Edit").is_err());
        assert!(send(" ", "x").is_err());
    }
}
