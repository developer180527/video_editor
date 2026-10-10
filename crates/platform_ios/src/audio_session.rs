//! The app's audio session: playback, like a video app. iOS's default
//! (ambient) session is mixed and silenced at will, and an interruption (a
//! call, Siri, another app's sound) stops the output until the session is
//! made active again — with no callbacks the playhead, which follows the
//! sound, would wait forever.

use std::sync::OnceLock;

use objc2::runtime::{AnyClass, AnyObject, ClassBuilder, Sel};
use objc2::{class, msg_send, sel};

fn ns_string(s: &str) -> *mut AnyObject {
    let c = std::ffi::CString::new(s).unwrap();
    unsafe { msg_send![class!(NSString), stringWithUTF8String: c.as_ptr()] }
}

fn session() -> Option<*mut AnyObject> {
    let cls = AnyClass::get("AVAudioSession")?;
    let s: *mut AnyObject = unsafe { msg_send![cls, sharedInstance] };
    (!s.is_null()).then_some(s)
}

/// Playback category, movie mode; then active.
fn activate() {
    let Some(s) = session() else { return };
    unsafe {
        let mut err: *mut AnyObject = std::ptr::null_mut();
        let category = ns_string("AVAudioSessionCategoryPlayback");
        let mode = ns_string("AVAudioSessionModeMoviePlayback");
        let _: bool = msg_send![s, setCategory: category, mode: mode, options: 0usize, error: &mut err];
        let mut err: *mut AnyObject = std::ptr::null_mut();
        let ok: bool = msg_send![s, setActive: true, error: &mut err];
        if !ok {
            eprintln!("audio session: could not activate");
        }
    }
}

/// An interruption ended, or the media services were reset: back on.
extern "C" fn changed(_this: *mut AnyObject, _sel: Sel, _note: *mut AnyObject) {
    activate();
}

/// Set the session up and keep it active, once.
pub fn configure() {
    static DONE: OnceLock<()> = OnceLock::new();
    DONE.get_or_init(|| {
        activate();
        let Some(mut b) = ClassBuilder::new("VeAudioSessionObserver", class!(NSObject)) else { return };
        unsafe { b.add_method(sel!(sessionChanged:), changed as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject)) };
        let cls: &AnyClass = b.register();
        unsafe {
            let observer: *mut AnyObject = msg_send![cls, new];
            let center: *mut AnyObject = msg_send![class!(NSNotificationCenter), defaultCenter];
            // Re-activating while an interruption is still going on fails
            // harmlessly; when it ends, this one succeeds.
            for name in ["AVAudioSessionInterruptionNotification", "AVAudioSessionMediaServicesWereResetNotification"] {
                let _: () = msg_send![center, addObserver: observer, selector: sel!(sessionChanged:), name: ns_string(name), object: std::ptr::null_mut::<AnyObject>()];
            }
        }
    });
}
