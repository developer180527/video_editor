//! The pieces an editor's chrome is made of: vector icons, toolbar buttons,
//! little value fields.
//!
//! The icons are drawn, not typed. A font is not guaranteed to have a play
//! triangle or a padlock, and a missing glyph is a tofu box in the middle of
//! your toolbar — so every one is a vector mask ([`crate::icons`]), tinted
//! with whatever colour the theme gives it.

use crate::theme::REEL;
use libgui::*;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Icon {
    Home,
    Share,
    Menu,
    Expand,
    Panel,
    Play,
    Pause,
    StepBack,
    StepForward,
    JumpStart,
    JumpEnd,
    MarkIn,
    MarkOut,
    Marker,
    Insert,
    Overwrite,
    Camera,
    Export,
    Settings,
    Wrench,
    Search,
    Folder,
    List,
    Grid,
    Freeform,
    Zoom,
    NewBin,
    Trash,
    Pen,
    Hand,
    Razor,
    Select,
    TrackSelect,
    Ripple,
    Rolling,
    Slip,
    Rect,
    Type,
    Lock,
    Eye,
    Mic,
    Speaker,
    Snap,
    LinkedSelection,
    Captions,
    Stopwatch,
    Reset,
    Chevron,
    ChevronRight,
    Sort,
    Effects,
    AddTrack,
}

impl Icon {
    pub const ALL: &'static [Icon] = &[
        Icon::Home,
        Icon::Share,
        Icon::Menu,
        Icon::Expand,
        Icon::Panel,
        Icon::Play,
        Icon::Pause,
        Icon::StepBack,
        Icon::StepForward,
        Icon::JumpStart,
        Icon::JumpEnd,
        Icon::MarkIn,
        Icon::MarkOut,
        Icon::Marker,
        Icon::Insert,
        Icon::Overwrite,
        Icon::Camera,
        Icon::Export,
        Icon::Settings,
        Icon::Wrench,
        Icon::Search,
        Icon::Folder,
        Icon::List,
        Icon::Grid,
        Icon::Freeform,
        Icon::Zoom,
        Icon::NewBin,
        Icon::Trash,
        Icon::Pen,
        Icon::Hand,
        Icon::Razor,
        Icon::Select,
        Icon::TrackSelect,
        Icon::Ripple,
        Icon::Rolling,
        Icon::Slip,
        Icon::Rect,
        Icon::Type,
        Icon::Lock,
        Icon::Eye,
        Icon::Mic,
        Icon::Speaker,
        Icon::Snap,
        Icon::LinkedSelection,
        Icon::Captions,
        Icon::Stopwatch,
        Icon::Reset,
        Icon::Chevron,
        Icon::ChevronRight,
        Icon::Sort,
        Icon::Effects,
        Icon::AddTrack,
    ];
}

/// Draw `icon` centred in `r`, in `c`: as large as fits, square.
pub fn draw_icon(p: &mut Painter, r: Rect, icon: Icon, c: Color) {
    let s = r.w.min(r.h);
    let m = r.center();
    p.fill_path(crate::icons::path(icon), Rect::new(m.x - s * 0.5, m.y - s * 0.5, s, s), c);
}

/// A square icon button, the toolbars' unit.
pub fn icon_button(ui: &mut Ui, key: impl std::hash::Hash, icon: Icon, size: f32, on: bool) -> Response {
    let t = ui.theme.clone();
    let id = ui.make_id(("icon", key, icon as u32));
    let r = ui.interact(id);
    let hot = ui.animate_bool(id, 0, r.hovered);
    if r.hovered {
        ui.cursor = Cursor::Pointer;
    }
    let bg = t.palette.surface;
    let accent = t.palette.accent;
    ui.add_leaf(id, Layout::leaf(Size::Fixed(size), Size::Fixed(size)), Vec2::ZERO, true, move |p, rect| {
        if on {
            p.rect(rect, accent.with_alpha(0.85), 2.0);
        } else if hot > 0.01 {
            p.rect(rect, bg.with_alpha(bg.a * hot), 2.0);
        }
        // Pure white on the dark theme, pure black on the light; white on
        // the accent when lit. Hover shows in the background.
        let c = if on { Color::WHITE } else { REEL.icon };
        draw_icon(p, rect.shrink(3.0, 3.0, 3.0, 3.0), icon, c);
    });
    r
}

/// A toolbar strip's separator.
pub fn divider(ui: &mut Ui, key: impl std::hash::Hash, vertical: bool, length: f32) {
    let c = ui.theme.palette.border_strong.with_alpha(0.5);
    let id = ui.make_id(("div", key));
    let layout = if vertical {
        Layout::leaf(Size::Fixed(1.0), Size::Fixed(length))
    } else {
        Layout::leaf(Size::Fixed(length), Size::Fixed(1.0))
    };
    ui.add_leaf(id, layout, Vec2::ZERO, false, move |p, r| p.rect(r, c, 0.0));
}

/// A number the user can drag, as an effect panel is mostly made of. Blue,
/// underlined, and it scrubs (Alt for fine steps). The value changes every
/// frame of the drag; `released` on the response is when to commit it.
pub fn value(ui: &mut Ui, key: impl std::hash::Hash, v: &mut f64, speed: f64, range: (f64, f64), decimals: usize, suffix: &str) -> Response {
    let t = ui.theme.clone();
    let id = ui.make_id(("value", key));
    let r = ui.interact_drag(id);
    if r.hovered || r.active {
        ui.cursor = Cursor::ResizeHorizontal;
    }
    if r.active && r.drag_delta.x != 0.0 {
        let fine = if ui.input().modifiers.alt { 0.1 } else { 1.0 };
        *v = (*v + r.drag_delta.x as f64 * speed * fine).clamp(range.0, range.1);
    }
    let hot = ui.animate_bool(id, 0, r.hovered || r.active);
    let text = if suffix.is_empty() { format!("{v:.decimals$}") } else { format!("{v:.decimals$} {suffix}") };
    let size = t.metrics.font_size;
    let w = ui.fonts.measure(ui.font, size, &text).x.max(28.0) + 4.0;
    let text = ui.frame_text(&text);
    let c = Color::hex(0x5aa9e6);
    ui.add_leaf(id, Layout::leaf(Size::Fixed(w), Size::Fixed(16.0)), Vec2::ZERO, true, move |p, rect| {
        let y = rect.bottom() - 2.0;
        p.text_left(rect, size, c.lerp(Color::hex(0x8ecbf5), hot), text);
        let tw = p.measure(size, text).x;
        p.rect(Rect::new(rect.x, y, tw, 1.0), c.with_alpha(0.35 + 0.45 * hot), 0.0);
    });
    r
}

/// A row in a properties tree: an optional twirl arrow, a stopwatch, a label,
/// and whatever the caller puts on the right.
pub struct Prop<'a> {
    pub depth: usize,
    pub label: &'a str,
    /// `Some` draws a twirl arrow that toggles the flag.
    pub twirl: Option<&'a mut bool>,
    /// `Some(animated)` draws a keyframe stopwatch, lit when animated.
    pub stopwatch: Option<bool>,
    /// Draw the reset button.
    pub reset: bool,
    /// Greyed out, the way a value governed by another one is.
    pub dim: bool,
}

impl<'a> Prop<'a> {
    pub fn new(label: &'a str) -> Self {
        Self { depth: 1, label, twirl: None, stopwatch: None, reset: true, dim: false }
    }

    pub fn twirl(mut self, open: &'a mut bool) -> Self {
        self.twirl = Some(open);
        self
    }

    pub fn stopwatch(mut self, animated: bool) -> Self {
        self.stopwatch = Some(animated);
        self
    }

    pub fn no_reset(mut self) -> Self {
        self.reset = false;
        self
    }

    pub fn dim(mut self, dim: bool) -> Self {
        self.dim = dim;
        self
    }
}

pub struct PropResponse<R> {
    pub stopwatch_clicked: bool,
    pub reset_clicked: bool,
    pub out: R,
}

pub fn prop_row<R>(ui: &mut Ui, key: impl std::hash::Hash + Copy, prop: Prop<'_>, right: impl FnOnce(&mut Ui) -> R) -> PropResponse<R> {
    let t = ui.theme.clone();
    let row = Layout::row()
        .width(Size::Grow(1.0))
        .height(Size::Fixed(20.0))
        .padding(Insets::xy(4.0 + prop.depth as f32 * 12.0, 0.0))
        .gap(4.0)
        .align(Align::Start, Align::Center);
    let id = ui.make_id(("prop", key));
    ui.container_id(id, row, Frame::none(), |ui| {
        match prop.twirl {
            Some(open) => {
                let r = icon_button(ui, (key, "tw"), if *open { Icon::Chevron } else { Icon::ChevronRight }, 14.0, false);
                if r.clicked {
                    *open = !*open;
                }
            }
            None => ui.space(14.0),
        }
        let mut stopwatch_clicked = false;
        match prop.stopwatch {
            Some(animated) => {
                let r = icon_button(ui, (key, "sw"), Icon::Stopwatch, 16.0, animated);
                ui.tooltip(&r, "Toggle animation");
                stopwatch_clicked = r.clicked;
            }
            None => ui.space(16.0),
        }
        let c = if prop.dim { t.palette.text_faint } else { t.palette.text };
        ui.text_with(prop.label, t.metrics.font_size, c);
        ui.flex();
        let out = right(ui);
        ui.space(2.0);
        let reset_clicked = if prop.reset {
            let r = icon_button(ui, (key, "rst"), Icon::Reset, 16.0, false);
            ui.tooltip(&r, "Reset to default");
            r.clicked
        } else {
            ui.space(14.0);
            false
        };
        PropResponse { stopwatch_clicked, reset_clicked, out }
    })
}
