//! Observes `UIApplicationDidReceiveMemoryWarningNotification`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use objc2::runtime::{AnyClass, AnyObject, ClassBuilder, Sel};
use objc2::{class, msg_send, sel};

static START: OnceLock<Instant> = OnceLock::new();
/// Milliseconds since START at the last warning, plus one (0 = never).
static LAST: AtomicU64 = AtomicU64::new(0);

extern "C" fn warned(_this: *mut AnyObject, _sel: Sel, _note: *mut AnyObject) {
    let ms = START.get_or_init(Instant::now).elapsed().as_millis() as u64;
    LAST.store(ms + 1, Ordering::Relaxed);
}

pub fn since_last_warning() -> Option<Duration> {
    let last = LAST.load(Ordering::Relaxed);
    (last != 0).then(|| START.get_or_init(Instant::now).elapsed().saturating_sub(Duration::from_millis(last - 1)))
}

/// Register the observer, once.
pub fn observe() {
    static DONE: OnceLock<()> = OnceLock::new();
    DONE.get_or_init(|| {
        START.get_or_init(Instant::now);
        let Some(mut b) = ClassBuilder::new("VeMemoryObserver", class!(NSObject)) else { return };
        unsafe { b.add_method(sel!(memoryWarning:), warned as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject)) };
        let cls: &AnyClass = b.register();
        unsafe {
            let observer: *mut AnyObject = msg_send![cls, new];
            let name = std::ffi::CString::new("UIApplicationDidReceiveMemoryWarningNotification").unwrap();
            let name: *mut AnyObject = msg_send![class!(NSString), stringWithUTF8String: name.as_ptr()];
            let center: *mut AnyObject = msg_send![class!(NSNotificationCenter), defaultCenter];
            let _: () = msg_send![center, addObserver: observer, selector: sel!(memoryWarning:), name: name, object: std::ptr::null_mut::<AnyObject>()];
        }
    });
}
