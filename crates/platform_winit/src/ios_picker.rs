//! The iPadOS document picker, for Import and Open. Files are picked in
//! *import* mode: iOS copies them into the app's sandbox, so the engine can
//! read them by path like any other file. (Opening in place, with
//! security-scoped bookmarks kept in the project, is the next step.)
//!
//! Saving needs no picker: projects go to the app's Documents folder, which
//! the Files app shows (`UIFileSharingEnabled`).

use std::ffi::{c_char, CStr, CString};
use std::path::PathBuf;
use std::ptr::null_mut;
use std::sync::{Mutex, OnceLock};

use objc2::runtime::{AnyClass, AnyObject, AnyProtocol, Bool, ClassBuilder, Sel};
use objc2::{class, msg_send, sel};
use winit::event_loop::EventLoopProxy;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

use crate::{FileDialog, UserEvent};

/// The dialog in flight and where to send its answer.
static PENDING: Mutex<Option<(FileDialog, EventLoopProxy<UserEvent>)>> = Mutex::new(None);

fn finish(paths: Vec<PathBuf>) {
    if let Some((d, proxy)) = PENDING.lock().unwrap().take() {
        let _ = proxy.send_event(UserEvent::Picked(d, paths));
    }
}

extern "C" fn did_pick(_this: *mut AnyObject, _sel: Sel, _picker: *mut AnyObject, urls: *mut AnyObject) {
    let mut paths = Vec::new();
    unsafe {
        let n: usize = msg_send![urls, count];
        for i in 0..n {
            let url: *mut AnyObject = msg_send![urls, objectAtIndex: i];
            let path: *mut AnyObject = msg_send![url, path];
            if path.is_null() {
                continue;
            }
            let s: *const c_char = msg_send![path, UTF8String];
            if !s.is_null() {
                paths.push(PathBuf::from(CStr::from_ptr(s).to_string_lossy().into_owned()));
            }
        }
    }
    finish(paths);
}

extern "C" fn cancelled(_this: *mut AnyObject, _sel: Sel, _picker: *mut AnyObject) {
    finish(Vec::new());
}

fn delegate_class() -> &'static AnyClass {
    static CLASS: OnceLock<usize> = OnceLock::new();
    let ptr = *CLASS.get_or_init(|| {
        let mut b = ClassBuilder::new("VeDocumentPickerDelegate", class!(NSObject)).expect("delegate class");
        if let Some(p) = AnyProtocol::get("UIDocumentPickerDelegate") {
            b.add_protocol(p);
        }
        unsafe {
            b.add_method(
                sel!(documentPicker:didPickDocumentsAtURLs:),
                did_pick as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject, *mut AnyObject),
            );
            b.add_method(sel!(documentPickerWasCancelled:), cancelled as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject));
        }
        b.register() as *const AnyClass as usize
    });
    unsafe { &*(ptr as *const AnyClass) }
}

fn ns_string(s: &str) -> *mut AnyObject {
    let c = CString::new(s).unwrap();
    unsafe { msg_send![class!(NSString), stringWithUTF8String: c.as_ptr()] }
}

pub fn present(window: &Window, d: FileDialog, proxy: EventLoopProxy<UserEvent>) {
    if let FileDialog::SaveProject { default_name } | FileDialog::SaveMedia { default_name, .. } = &d {
        let home = std::env::var("HOME").unwrap_or_default();
        let path = PathBuf::from(home).join("Documents").join(default_name);
        let _ = proxy.send_event(UserEvent::Picked(d, vec![path]));
        return;
    }
    let types: &[&str] = match d {
        FileDialog::OpenMedia => &["public.movie", "public.audio", "public.image"],
        _ => &["public.data"],
    };
    let Ok(handle) = window.window_handle() else { return };
    let RawWindowHandle::UiKit(h) = handle.as_raw() else { return };
    *PENDING.lock().unwrap() = Some((d.clone(), proxy));
    unsafe {
        let view = h.ui_view.as_ptr() as *mut AnyObject;
        let ui_window: *mut AnyObject = msg_send![view, window];
        let root: *mut AnyObject = if ui_window.is_null() { null_mut() } else { msg_send![ui_window, rootViewController] };
        if root.is_null() {
            finish(Vec::new());
            return;
        }
        let strs: Vec<*mut AnyObject> = types.iter().map(|t| ns_string(t)).collect();
        let arr: *mut AnyObject = msg_send![class!(NSArray), arrayWithObjects: strs.as_ptr(), count: strs.len()];
        let picker: *mut AnyObject = msg_send![class!(UIDocumentPickerViewController), alloc];
        // UIDocumentPickerModeImport = 0: iOS copies the files into our sandbox.
        let picker: *mut AnyObject = msg_send![picker, initWithDocumentTypes: arr, inMode: 0usize];
        let _: () = msg_send![picker, setAllowsMultipleSelection: Bool::new(matches!(d, FileDialog::OpenMedia))];
        // The delegate lives for the app's lifetime: one per picker shown is a few bytes.
        let delegate: *mut AnyObject = msg_send![delegate_class(), new];
        let _: () = msg_send![picker, setDelegate: delegate];
        let _: () = msg_send![root, presentViewController: picker, animated: Bool::YES, completion: null_mut::<AnyObject>()];
    }
}
