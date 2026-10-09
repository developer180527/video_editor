//! macOS: the app's title bar in place of the system one (the traffic
//! lights moved onto it), and the menus in the system menu bar.

use std::cell::RefCell;
use std::sync::Mutex;

use objc2_06::rc::Retained;
use objc2_06::runtime::{AnyObject, NSObject, Sel};
use objc2_06::{define_class, msg_send, sel, AllocAnyThread, MainThreadMarker};
use objc2_app_kit::{NSApplication, NSControlStateValueOn, NSEventModifierFlags, NSMenu, NSMenuItem, NSView, NSWindowButton};
use objc2_foundation::{NSPoint, NSString};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

use crate::{MenuEntry, NativeMenu, Waker};

/// Where the traffic lights' left edge goes, and the gap after them.
const LIGHTS_X: f64 = 13.0;
const LIGHTS_GAP: f64 = 12.0;

fn ns_view(window: &Window) -> Option<Retained<NSView>> {
    let RawWindowHandle::AppKit(h) = window.window_handle().ok()?.as_raw() else { return None };
    // SAFETY: winit hands out a valid NSView for the window's lifetime.
    unsafe { Retained::retain(h.ns_view.as_ptr().cast::<NSView>()) }
}

/// Centre the traffic lights vertically in a bar `bar_h` logical px tall at
/// the window's top, and say how far right they reach (what the bar keeps
/// clear). AppKit puts them back on some changes, so call after resizes,
/// focus changes and full-screen transitions too.
pub fn place_traffic_lights(window: &Window, bar_h: f64) -> f32 {
    let Some(view) = ns_view(window) else { return 0.0 };
    let Some(win) = view.window() else { return 0.0 };
    let buttons: Vec<_> = [NSWindowButton::CloseButton, NSWindowButton::MiniaturizeButton, NSWindowButton::ZoomButton]
        .into_iter()
        .filter_map(|b| win.standardWindowButton(b))
        .collect();
    let [close, mini, zoom] = buttons.as_slice() else { return 0.0 };
    // The buttons live in the title bar container: make it as tall as our
    // bar, so they can sit in its middle.
    // SAFETY: plain view-hierarchy queries on the main thread.
    let Some(container) = (unsafe { close.superview().and_then(|v| v.superview()) }) else { return 0.0 };
    let mut frame = container.frame();
    frame.size.height = bar_h;
    frame.origin.y = win.frame().size.height - bar_h;
    container.setFrame(frame);
    let spacing = mini.frame().origin.x - close.frame().origin.x;
    for (i, b) in [close, mini, zoom].into_iter().enumerate() {
        let h = b.frame().size.height;
        b.setFrameOrigin(NSPoint::new(LIGHTS_X + i as f64 * spacing, ((bar_h - h) / 2.0).round()));
    }
    (LIGHTS_X + 2.0 * spacing + zoom.frame().size.width + LIGHTS_GAP) as f32
}

// ---- the menu bar -------------------------------------------------------------

/// Item ids chosen in the menu bar since last asked.
static CHOSEN: Mutex<Vec<u32>> = Mutex::new(Vec::new());
static WAKER: Mutex<Option<Waker>> = Mutex::new(None);

thread_local! {
    static TARGET: RefCell<Option<Retained<MenuTarget>>> = const { RefCell::new(None) };
}

/// The id of "Quit", which the shell handles itself.
pub const QUIT: u32 = u32::MAX;

define_class!(
    // SAFETY: NSObject has no subclassing requirements and this has no Drop.
    #[unsafe(super(NSObject))]
    #[name = "VideoEditorMenuTarget"]
    struct MenuTarget;

    impl MenuTarget {
        #[unsafe(method(menuAction:))]
        fn menu_action(&self, sender: &NSMenuItem) {
            CHOSEN.lock().unwrap().push(sender.tag() as u32);
            if let Some(w) = WAKER.lock().unwrap().as_ref() {
                w();
            }
        }
    }
);

impl MenuTarget {
    fn new() -> Retained<Self> {
        let this = Self::alloc().set_ivars(());
        // SAFETY: NSObject's designated initializer.
        unsafe { msg_send![super(this), init] }
    }
}

/// Wake the shell through `waker` when a menu item is chosen.
pub fn install(waker: Waker) {
    *WAKER.lock().unwrap() = Some(waker);
}

/// Menu items chosen since the last call.
pub fn take_chosen() -> Vec<u32> {
    std::mem::take(&mut *CHOSEN.lock().unwrap())
}

/// AppKit's key-equivalent string for a key, if it has one.
fn key_equivalent(key: libgui::Key) -> Option<String> {
    use libgui::Key::*;
    let s = format!("{key:?}");
    Some(match key {
        A | B | C | D | E | F | G | H | I | J | K | L | M | N | O | P | Q | R | S | T | U | V | W | X | Y | Z => s.to_lowercase(),
        Num0 | Num1 | Num2 | Num3 | Num4 | Num5 | Num6 | Num7 | Num8 | Num9 => s[3..].to_string(),
        Comma => ",".into(),
        Period => ".".into(),
        Slash => "/".into(),
        Minus => "-".into(),
        Equal => "=".into(),
        _ => return None,
    })
}

fn item(mtm: MainThreadMarker, title: &str, action: Option<Sel>, key: &str) -> Retained<NSMenuItem> {
    // SAFETY: a fresh item; the selector is either ours or a standard responder action.
    unsafe { NSMenuItem::initWithTitle_action_keyEquivalent(mtm.alloc(), &NSString::from_str(title), action, &NSString::from_str(key)) }
}

fn submenu(mtm: MainThreadMarker, title: &str, items: impl IntoIterator<Item = Retained<NSMenuItem>>) -> Retained<NSMenuItem> {
    let menu = NSMenu::initWithTitle(mtm.alloc(), &NSString::from_str(title));
    menu.setAutoenablesItems(false);
    for i in items {
        menu.addItem(&i);
    }
    let top = item(mtm, title, None, "");
    top.setSubmenu(Some(&menu));
    top
}

/// Replace the system menu bar with the app menu, `menus`, and Window.
pub fn set_menu(app_name: &str, menus: &[NativeMenu]) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let target = TARGET.with(|t| t.borrow_mut().get_or_insert_with(MenuTarget::new).clone());
    let ours = |title: &str, id: u32, key: &str, mods: NSEventModifierFlags| {
        let i = item(mtm, title, Some(sel!(menuAction:)), key);
        i.setKeyEquivalentModifierMask(mods);
        i.setTag(id as isize);
        // SAFETY: the target lives for the program (held in TARGET).
        unsafe { i.setTarget(Some(&*(&*target as *const MenuTarget as *const AnyObject))) };
        i
    };
    let bar = NSMenu::new(mtm);
    let cmd = NSEventModifierFlags::Command;
    bar.addItem(&submenu(
        mtm,
        app_name,
        [
            item(mtm, &format!("About {app_name}"), Some(sel!(orderFrontStandardAboutPanel:)), ""),
            NSMenuItem::separatorItem(mtm),
            item(mtm, &format!("Hide {app_name}"), Some(sel!(hide:)), "h"),
            {
                let i = item(mtm, "Hide Others", Some(sel!(hideOtherApplications:)), "h");
                i.setKeyEquivalentModifierMask(cmd | NSEventModifierFlags::Option);
                i
            },
            item(mtm, "Show All", Some(sel!(unhideAllApplications:)), ""),
            NSMenuItem::separatorItem(mtm),
            ours(&format!("Quit {app_name}"), QUIT, "q", cmd),
        ],
    ));
    for m in menus {
        let items = m.entries.iter().map(|e| match e {
            MenuEntry::Separator => NSMenuItem::separatorItem(mtm),
            MenuEntry::Item { id, label, shortcut, enabled, checked } => {
                // Only chords with ⌘ become key equivalents: a bare key
                // there would be taken from text fields.
                let (key, mods) = match shortcut.filter(|s| s.mods.logo).and_then(|s| key_equivalent(s.key).map(|k| (k, s.mods))) {
                    Some((k, m)) => {
                        let mut f = NSEventModifierFlags::Command;
                        if m.shift {
                            f |= NSEventModifierFlags::Shift;
                        }
                        if m.alt {
                            f |= NSEventModifierFlags::Option;
                        }
                        if m.ctrl {
                            f |= NSEventModifierFlags::Control;
                        }
                        (k, f)
                    }
                    None => (String::new(), NSEventModifierFlags::empty()),
                };
                let i = ours(label, *id, &key, mods);
                i.setEnabled(*enabled);
                if *checked {
                    i.setState(NSControlStateValueOn);
                }
                i
            }
        });
        bar.addItem(&submenu(mtm, &m.title, items.collect::<Vec<_>>()));
    }
    let window = submenu(
        mtm,
        "Window",
        [item(mtm, "Minimize", Some(sel!(performMiniaturize:)), ""), item(mtm, "Zoom", Some(sel!(performZoom:)), "")],
    );
    bar.addItem(&window);
    NSApplication::sharedApplication(mtm).setMainMenu(Some(&bar));
}
