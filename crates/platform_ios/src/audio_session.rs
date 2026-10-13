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

/// Whether a voice-over is being recorded (play-and-record) or not
/// (playback). Read by `activate`, which runs again after interruptions.
static RECORDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Switch between playback and play-and-record (voice-over), and make the
/// session active again.
pub fn recording(on: bool) {
    RECORDING.store(on, std::sync::atomic::Ordering::SeqCst);
    activate();
}

/// Playback category, movie mode (or play-and-record while recording);
/// then active.
fn activate() {
    let Some(s) = session() else { return };
    let recording = RECORDING.load(std::sync::atomic::Ordering::SeqCst);
    unsafe {
        let mut err: *mut AnyObject = std::ptr::null_mut();
        let (category, mode, options) = if recording {
            // Speaker rather than the earpiece; Bluetooth and AirPods mics allowed.
            ("AVAudioSessionCategoryPlayAndRecord", "AVAudioSessionModeVideoRecording", 0x8usize | 0x4)
        } else {
            ("AVAudioSessionCategoryPlayback", "AVAudioSessionModeMoviePlayback", 0)
        };
        let _: bool = msg_send![s, setCategory: ns_string(category), mode: ns_string(mode), options: options, error: &mut err];
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
