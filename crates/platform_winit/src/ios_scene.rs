//! The UIScene lifecycle, which iOS now requires: an app with no scene
//! delegate traps in UIKit at launch. winit 0.30 still starts the old way, so
//! this registers a minimal `UIWindowSceneDelegate` (named in Info.plist) and
//! puts winit's `UIWindow` on the scene. The window is created when winit
//! resumes and the scene connects separately; whichever comes second attaches.

use objc2::runtime::{AnyClass, AnyObject, AnyProtocol, ClassBuilder, Sel};
use objc2::{msg_send, sel};
use std::ptr::null_mut;
use std::sync::atomic::{AtomicPtr, Ordering};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

static WINDOW: AtomicPtr<AnyObject> = AtomicPtr::new(null_mut());
static SCENE: AtomicPtr<AnyObject> = AtomicPtr::new(null_mut());

/// Register `VeSceneDelegate`. Call before the event loop runs.
pub fn register() {
    let Some(superclass) = AnyClass::get("UIResponder") else { return };
    let Some(mut b) = ClassBuilder::new("VeSceneDelegate", superclass) else { return };
    if let Some(p) = AnyProtocol::get("UIWindowSceneDelegate") {
        b.add_protocol(p);
    }
    unsafe {
        b.add_method(
            sel!(scene:willConnectToSession:options:),
            will_connect as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject, *mut AnyObject, *mut AnyObject),
        );
        b.add_method(sel!(window), window as extern "C" fn(*mut AnyObject, Sel) -> *mut AnyObject);
        b.add_method(sel!(setWindow:), set_window as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject));
    }
    b.register();
}

/// winit's window was created: remember its `UIWindow`.
pub fn window_created(window: &Window) {
    let Ok(handle) = window.window_handle() else { return };
    if let RawWindowHandle::UiKit(h) = handle.as_raw() {
        let view = h.ui_view.as_ptr() as *mut AnyObject;
        let ui_window: *mut AnyObject = unsafe { msg_send![view, window] };
        if !ui_window.is_null() && WINDOW.load(Ordering::Acquire).is_null() {
            WINDOW.store(ui_window, Ordering::Release);
            attach();
        }
    }
}

extern "C" fn will_connect(_this: *mut AnyObject, _sel: Sel, scene: *mut AnyObject, _session: *mut AnyObject, _options: *mut AnyObject) {
    SCENE.store(scene, Ordering::Release);
    attach();
}

extern "C" fn window(_this: *mut AnyObject, _sel: Sel) -> *mut AnyObject {
    WINDOW.load(Ordering::Acquire)
}

extern "C" fn set_window(_this: *mut AnyObject, _sel: Sel, _window: *mut AnyObject) {}

fn attach() {
    let (w, s) = (WINDOW.load(Ordering::Acquire), SCENE.load(Ordering::Acquire));
    if w.is_null() || s.is_null() {
        return;
    }
    unsafe {
        let _: () = msg_send![w, setWindowScene: s];
        let _: () = msg_send![w, makeKeyAndVisible];
    }
}
