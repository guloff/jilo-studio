//! Files the OS asks FilmCraft to open: on macOS a double-click on a `.fcproj` in Finder, Open With,
//! or files dropped on the Dock icon arrive as an "open documents" Apple Event, not as command-line
//! arguments, and winit (0.30) handles no such event. [`install`] registers a handler for it; the
//! paths queue up until the UI [`take`]s them, and a [`set_waker`] callback repaints the UI so it
//! does so promptly. [`note_recent`] adds a project to the system's recent documents (the Dock
//! icon's menu).
//!
//! Elsewhere files come as arguments: [`install`] and [`note_recent`] do nothing, [`take`] is empty.

use std::sync::{Mutex, PoisonError};

type Waker = Box<dyn Fn() + Send>;

static QUEUE: Mutex<Vec<String>> = Mutex::new(Vec::new());
static WAKER: Mutex<Option<Waker>> = Mutex::new(None);

/// The paths the OS asked to open since the last call, oldest first.
pub fn take() -> Vec<String> {
    std::mem::take(&mut *QUEUE.lock().unwrap_or_else(PoisonError::into_inner))
}

/// Called whenever paths arrive (and right away if some are already waiting).
pub fn set_waker(waker: Waker) {
    let pending = !QUEUE.lock().unwrap_or_else(PoisonError::into_inner).is_empty();
    if pending {
        waker();
    }
    *WAKER.lock().unwrap_or_else(PoisonError::into_inner) = Some(waker);
}

fn push(paths: Vec<String>) {
    if paths.is_empty() {
        return;
    }
    QUEUE.lock().unwrap_or_else(PoisonError::into_inner).extend(paths);
    if let Some(w) = WAKER.lock().unwrap_or_else(PoisonError::into_inner).as_ref() {
        w();
    }
}

#[cfg(target_os = "macos")]
pub use mac::{install, note_recent};

/// Not macOS: files arrive as arguments.
#[cfg(not(target_os = "macos"))]
pub fn install() {}

/// Not macOS: no system recent-documents list is kept.
#[cfg(not(target_os = "macos"))]
pub fn note_recent(_path: &str) {}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod mac {
    use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject, NSObject};
    use objc2::{AllocAnyThread, define_class, msg_send, sel};
    use objc2_foundation::{NSAppleEventDescriptor, NSAppleEventManager, NSString, NSURL};

    /// `kCoreEventClass`, `kAEOpenDocuments` and `keyDirectObject` (four-character codes).
    const CORE_EVENT_CLASS: u32 = u32::from_be_bytes(*b"aevt");
    const OPEN_DOCUMENTS: u32 = u32::from_be_bytes(*b"odoc");
    const DIRECT_OBJECT: u32 = u32::from_be_bytes(*b"----");

    define_class!(
        // SAFETY: NSObject has no subclassing requirements and the class implements no Drop.
        #[unsafe(super(NSObject))]
        #[name = "FilmCraftOpenDocumentsHandler"]
        struct Handler;

        impl Handler {
            #[unsafe(method(handleOpenDocuments:withReplyEvent:))]
            fn handle_open_documents(&self, event: &NSAppleEventDescriptor, _reply: &NSAppleEventDescriptor) {
                super::push(paths(event));
            }

            /// `NSApplicationWillFinishLaunchingNotification`: AppKit has just installed its own
            /// Apple Event handlers and has not yet delivered the event that launched the app.
            #[unsafe(method(applicationWillFinishLaunching:))]
            fn application_will_finish_launching(&self, _note: &AnyObject) {
                register(self);
            }
        }
    );

    /// The file paths in an "open documents" event's direct object (a list of file URLs).
    fn paths(event: &NSAppleEventDescriptor) -> Vec<String> {
        // SAFETY: `paramDescriptorForKeyword:` takes an AEKeyword (a four-character code, u32) and
        // returns a descriptor or nil; the event is a valid descriptor for the duration of the call.
        let list: Option<Retained<NSAppleEventDescriptor>> = unsafe { msg_send![event, paramDescriptorForKeyword: DIRECT_OBJECT] };
        let Some(list) = list else { return Vec::new() };
        // descriptor lists are 1-based; a single file may also come as the item itself
        let n = list.numberOfItems();
        let items: Vec<Retained<NSAppleEventDescriptor>> = if n > 0 { (1..=n).filter_map(|i| list.descriptorAtIndex(i)).collect() } else { vec![list] };
        items.iter().filter_map(|d| d.fileURLValue()).filter_map(|u| u.path()).map(|p| p.to_string()).collect()
    }

    /// The handler object; the Apple Event manager does not retain it, so it lives for the process.
    static HANDLER: AtomicPtr<Handler> = AtomicPtr::new(std::ptr::null_mut());

    static OBSERVING: AtomicBool = AtomicBool::new(false);

    /// Handle "open documents" events. Call it before the event loop starts: AppKit installs its
    /// own handler while it finishes launching (it would hand the events to winit's delegate, which
    /// ignores them), so the handler is installed again on `NSApplicationWillFinishLaunching`, in
    /// time for the event that launched FilmCraft. Calling it again later is harmless.
    pub fn install() {
        let h = handler();
        register(h);
        if !OBSERVING.swap(true, Ordering::AcqRel)
            && let Some(center) = AnyClass::get(c"NSNotificationCenter")
        {
            let name = NSString::from_str("NSApplicationWillFinishLaunchingNotification");
            // SAFETY: `defaultCenter` is never nil; the observer is never released (and so never
            // needs removing) and implements `applicationWillFinishLaunching:` taking the
            // notification; a nil object observes the notification from any sender.
            unsafe {
                let center: *mut AnyObject = msg_send![center, defaultCenter];
                if let Some(center) = center.as_ref() {
                    let _: () = msg_send![
                        center,
                        addObserver: h,
                        selector: sel!(applicationWillFinishLaunching:),
                        name: &*name,
                        object: std::ptr::null::<AnyObject>()
                    ];
                }
            }
        }
    }

    /// The process-wide handler object (created on first use, never released).
    fn handler() -> &'static Handler {
        let mut h = HANDLER.load(Ordering::Acquire);
        if h.is_null() {
            // SAFETY: plain NSObject subclass allocation and `init`.
            let obj: Retained<Handler> = unsafe { msg_send![Handler::alloc(), init] };
            h = Retained::into_raw(obj);
            if let Err(existing) = HANDLER.compare_exchange(std::ptr::null_mut(), h, Ordering::AcqRel, Ordering::Acquire) {
                // SAFETY: `h` came from `Retained::into_raw` just above and was not published.
                drop(unsafe { Retained::from_raw(h) });
                h = existing;
            }
        }
        // SAFETY: `h` is non-null, came from `Retained::into_raw` and is never released.
        unsafe { &*h }
    }

    /// Make `h` the Apple Event manager's handler of "open documents".
    fn register(h: &Handler) {
        let manager = NSAppleEventManager::sharedAppleEventManager();
        // SAFETY: `h` is a live, never-released object implementing the selector with the
        // `(NSAppleEventDescriptor*, NSAppleEventDescriptor*)` signature the manager calls it with;
        // the class and id are four-character codes (u32).
        unsafe {
            let _: () = msg_send![
                &*manager,
                setEventHandler: h,
                andSelector: sel!(handleOpenDocuments:withReplyEvent:),
                forEventClass: CORE_EVENT_CLASS,
                andEventID: OPEN_DOCUMENTS
            ];
        }
    }

    /// Add `path` to the system's recent documents (shown in the Dock icon's menu). Main thread
    /// only, as AppKit requires; elsewhere it does nothing.
    pub fn note_recent(path: &str) {
        if objc2::MainThreadMarker::new().is_none() {
            return;
        }
        let Some(cls) = AnyClass::get(c"NSDocumentController") else { return };
        let url = NSURL::fileURLWithPath(&NSString::from_str(path));
        // SAFETY: `sharedDocumentController` returns the app's shared controller (never nil) and
        // `noteNewRecentDocumentURL:` takes a file URL; both on the main thread (checked above).
        unsafe {
            let controller: *mut AnyObject = msg_send![cls, sharedDocumentController];
            if let Some(controller) = controller.as_ref() {
                let _: () = msg_send![controller, noteNewRecentDocumentURL: &*url];
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// An "open documents" event built like Finder's: a list of file URLs as the direct object.
        #[test]
        fn open_documents_event_yields_the_paths() {
            let list = NSAppleEventDescriptor::listDescriptor();
            for (i, p) in ["/tmp/a b.fcproj", "/tmp/клип.mov"].iter().enumerate() {
                let url = NSURL::fileURLWithPath(&NSString::from_str(p));
                let d: Retained<NSAppleEventDescriptor> =
                    // SAFETY: `descriptorWithFileURL:` takes a file URL and returns a descriptor.
                    unsafe { msg_send![objc2::class!(NSAppleEventDescriptor), descriptorWithFileURL: &*url] };
                list.insertDescriptor_atIndex(&d, (i + 1) as isize);
            }
            // an empty record carries the direct object, as a real Apple Event does
            let event = NSAppleEventDescriptor::recordDescriptor();
            // SAFETY: `setParamDescriptor:forKeyword:` takes a descriptor and an AEKeyword (u32).
            let _: () = unsafe { msg_send![&*event, setParamDescriptor: &*list, forKeyword: DIRECT_OBJECT] };
            assert_eq!(paths(&event), ["/tmp/a b.fcproj", "/tmp/клип.mov"]);
            install();
            install();
            assert!(!HANDLER.load(Ordering::Acquire).is_null());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queued_paths_wake_and_drain() {
        let woken = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let w = woken.clone();
        set_waker(Box::new(move || {
            w.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }));
        push(vec!["/x/a.fcproj".into()]);
        push(Vec::new());
        assert_eq!(take(), ["/x/a.fcproj"]);
        assert!(take().is_empty());
        assert_eq!(woken.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}
