//! Files opened from Finder (double click, Open With, a drop on the Dock
//! icon), on macOS.
//!
//! macOS does not pass them as arguments: it sends the app an Apple Event,
//! "open documents" (`aevt`/`odoc`), whose direct parameter is a list of
//! file URLs. winit does not expose it, so this module installs a handler
//! for it with `NSAppleEventManager` and queues the paths; the app takes them
//! each frame (`take`). The handler object is a small Objective-C class
//! defined here, since a handler is a target and a selector.
//!
//! When to install it matters: `NSApplication` puts its own handler for
//! this event in place while it finishes launching, replacing any installed
//! earlier -- installing in `main` before the event loop was tried, and
//! files were silently ignored. Apple's advice is to install it in
//! `applicationWillFinishLaunching:`, which belongs to winit's delegate; the
//! same moment is posted as a notification, though, so `install` observes
//! that notification and installs the handler then. That is still before
//! the event that launched the app is delivered.

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{AnyThread, define_class, msg_send, sel};
use std::ffi::{CStr, c_char};
use std::sync::{Mutex, OnceLock};

/// `kCoreEventClass` and `kAEOpenDocuments`, as four-character codes.
const CORE_EVENT_CLASS: u32 = u32::from_be_bytes(*b"aevt");
const OPEN_DOCUMENTS: u32 = u32::from_be_bytes(*b"odoc");
/// `keyDirectObject`.
const DIRECT_OBJECT: u32 = u32::from_be_bytes(*b"----");

static PENDING: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// Woken when a file arrives, so it opens without waiting for input.
static WAKE: OnceLock<eframe::egui::Context> = OnceLock::new();

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and this class
    // implements no Drop.
    #[unsafe(super(NSObject))]
    #[name = "VoxferroOpenDocuments"]
    struct OpenDocuments;

    impl OpenDocuments {
        #[unsafe(method(willFinishLaunching:))]
        fn will_finish_launching(&self, _notification: &AnyObject) {
            // SAFETY: as in `install`.
            unsafe {
                let manager: Retained<AnyObject> = msg_send![NSAppleEventManager(), sharedAppleEventManager];
                let _: () = msg_send![
                    &*manager,
                    setEventHandler: self,
                    andSelector: sel!(handleOpen:withReply:),
                    forEventClass: CORE_EVENT_CLASS,
                    andEventID: OPEN_DOCUMENTS
                ];
            }
        }

        #[unsafe(method(handleOpen:withReply:))]
        fn handle_open(&self, event: &AnyObject, _reply: &AnyObject) {
            let paths = paths_in(event);
            if let Ok(mut pending) = PENDING.lock() {
                pending.extend(paths);
            }
            if let Some(ctx) = WAKE.get() {
                ctx.request_repaint();
            }
        }
    }
);

/// The file paths in an "open documents" event.
fn paths_in(event: &AnyObject) -> Vec<String> {
    let mut out = Vec::new();
    // SAFETY: these are NSAppleEventDescriptor and NSURL methods with the
    // argument and return types the SDK declares.
    unsafe {
        let list: Option<Retained<AnyObject>> = msg_send![event, paramDescriptorForKeyword: DIRECT_OBJECT];
        let Some(list) = list else { return out };
        let count: isize = msg_send![&*list, numberOfItems];
        // Descriptor lists count from 1.
        for i in 1..=count {
            let item: Option<Retained<AnyObject>> = msg_send![&*list, descriptorAtIndex: i];
            let Some(item) = item else { continue };
            let url: Option<Retained<AnyObject>> = msg_send![&*item, fileURLValue];
            let Some(url) = url else { continue };
            let path: Option<Retained<AnyObject>> = msg_send![&*url, path];
            let Some(path) = path else { continue };
            let utf8: *const c_char = msg_send![&*path, UTF8String];
            if !utf8.is_null() {
                out.push(CStr::from_ptr(utf8).to_string_lossy().into_owned());
            }
        }
    }
    out
}

/// Arranges for the handler to be installed as the app finishes launching.
/// Call on the main thread, before the event loop.
pub fn install() {
    let handler: Retained<OpenDocuments> = unsafe { msg_send![OpenDocuments::alloc(), init] };
    // SAFETY: NSNotificationCenter's documented observer method; the
    // selector takes the notification. The handler must outlive both the
    // observation and the Apple Event registration, neither of which
    // retains it, so it is leaked on purpose.
    unsafe {
        let name: Retained<AnyObject> = msg_send![
            objc2::class!(NSString),
            stringWithUTF8String: c"NSApplicationWillFinishLaunchingNotification".as_ptr()
        ];
        let center: Retained<AnyObject> = msg_send![objc2::class!(NSNotificationCenter), defaultCenter];
        let none: *const AnyObject = std::ptr::null();
        let _: () = msg_send![
            &*center,
            addObserver: &*handler,
            selector: sel!(willFinishLaunching:),
            name: &*name,
            object: none
        ];
    }
    std::mem::forget(handler);
}

#[allow(non_snake_case)]
fn NSAppleEventManager() -> &'static objc2::runtime::AnyClass {
    objc2::class!(NSAppleEventManager)
}

/// Lets an arriving file wake the UI.
pub fn set_waker(ctx: &eframe::egui::Context) {
    let _ = WAKE.set(ctx.clone());
}

/// Paths opened since the last call.
pub fn take() -> Vec<String> {
    PENDING.lock().map(|mut p| std::mem::take(&mut *p)).unwrap_or_default()
}
