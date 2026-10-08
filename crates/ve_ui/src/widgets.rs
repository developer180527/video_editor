//! The pieces an editor's chrome is made of: vector icons, toolbar buttons,
//! little value fields.
//!
//! The icons are drawn, not typed. A font is not guaranteed to have a play
//! triangle or a padlock, and a missing glyph is a tofu box in the middle of
//! your toolbar — so every one of these is rectangles, lines and circles.

use libgui::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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

/// Draw `icon` centred in `r`, in `c`. Sizes are relative to the box, so the
/// same call works in a 14 px toolbar and a 24 px transport row.
pub fn draw_icon(p: &mut Painter, r: Rect, icon: Icon, c: Color) {
    let m = r.center();
    let s = r.w.min(r.h);
    let u = s / 16.0; // a 16-unit grid, like the icon sets these copy
    let line = (u * 1.4).max(1.0);
    let bar = |p: &mut Painter, x: f32, y: f32, w: f32, h: f32| {
        p.rect(Rect::new(m.x + x * u - w * u * 0.5, m.y + y * u - h * u * 0.5, w * u, h * u), c, 0.0);
    };
    let tri_right = |p: &mut Painter, cx: f32, cy: f32, w: f32, h: f32| {
        // A triangle out of columns: the shader has no polygon.
        let steps = (h * u).max(3.0) as usize;
        for i in 0..steps {
            let t = i as f32 / steps as f32;
            let hh = h * u * (1.0 - t);
            let x = m.x + cx * u + t * w * u;
            p.rect(Rect::new(x, m.y + cy * u - hh * 0.5, (w * u / steps as f32).ceil(), hh), c, 0.0);
        }
    };
    let tri_left = |p: &mut Painter, cx: f32, cy: f32, w: f32, h: f32| {
        let steps = (h * u).max(3.0) as usize;
        for i in 0..steps {
            let t = i as f32 / steps as f32;
            let hh = h * u * (1.0 - t);
            let x = m.x + cx * u - t * w * u;
            p.rect(Rect::new(x - (w * u / steps as f32).ceil(), m.y + cy * u - hh * 0.5, (w * u / steps as f32).ceil(), hh), c, 0.0);
        }
    };
    let ring = |p: &mut Painter, rad: f32, w: f32| {
        p.rect_bordered(Rect::new(m.x - rad * u, m.y - rad * u, rad * 2.0 * u, rad * 2.0 * u), Color::TRANSPARENT, rad * u, w, c);
    };

    match icon {
        Icon::Home => {
            // Roof from stacked rows, then the body.
            let steps = (5.0 * u).max(3.0) as usize;
            for i in 0..steps {
                let t = i as f32 / steps as f32;
                let w = 12.0 * u * t;
                let y = m.y - 6.0 * u + t * 5.0 * u;
                p.rect(Rect::new(m.x - w * 0.5, y, w, (5.0 * u / steps as f32).ceil()), c, 0.0);
            }
            p.rect_bordered(Rect::new(m.x - 4.0 * u, m.y - 1.0 * u, 8.0 * u, 7.0 * u), Color::TRANSPARENT, 0.0, line, c);
        }
        Icon::Share | Icon::Export => {
            bar(p, 0.0, 1.0, 1.4, 8.0);
            tri_right(p, -3.5, -4.5, 3.5, 5.0);
            tri_left(p, 3.5, -4.5, 3.5, 5.0);
            bar(p, 0.0, 6.0, 12.0, 1.4);
        }
        Icon::Menu => {
            for y in [-4.0, 0.0, 4.0] {
                bar(p, 0.0, y, 13.0, 1.4);
            }
        }
        Icon::Expand => {
            for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                bar(p, sx * 4.5, sy * 6.0, 5.0, 1.4);
                bar(p, sx * 6.0, sy * 4.5, 1.4, 5.0);
            }
        }
        Icon::Panel => {
            p.rect_bordered(Rect::new(m.x - 6.0 * u, m.y - 5.0 * u, 12.0 * u, 10.0 * u), Color::TRANSPARENT, 1.0, line, c);
            bar(p, -1.0, 0.0, 1.2, 10.0);
        }
        Icon::Play => tri_right(p, -3.0, 0.0, 7.0, 10.0),
        Icon::Pause => {
            bar(p, -2.2, 0.0, 2.2, 10.0);
            bar(p, 2.2, 0.0, 2.2, 10.0);
        }
        Icon::StepBack => {
            tri_left(p, 2.0, 0.0, 6.0, 9.0);
            bar(p, -3.5, 0.0, 1.4, 9.0);
        }
        Icon::StepForward => {
            tri_right(p, -2.0, 0.0, 6.0, 9.0);
            bar(p, 3.5, 0.0, 1.4, 9.0);
        }
        Icon::JumpStart => {
            bar(p, -5.0, 0.0, 1.6, 10.0);
            tri_left(p, 5.0, 0.0, 7.0, 10.0);
        }
        Icon::JumpEnd => {
            bar(p, 5.0, 0.0, 1.6, 10.0);
            tri_right(p, -5.0, 0.0, 7.0, 10.0);
        }
        Icon::MarkIn => {
            bar(p, -4.0, 0.0, 1.6, 11.0);
            bar(p, 0.5, -4.7, 8.0, 1.6);
            bar(p, 0.5, 4.7, 8.0, 1.6);
        }
        Icon::MarkOut => {
            bar(p, 4.0, 0.0, 1.6, 11.0);
            bar(p, -0.5, -4.7, 8.0, 1.6);
            bar(p, -0.5, 4.7, 8.0, 1.6);
        }
        Icon::Marker => {
            p.rect(Rect::new(m.x - 4.0 * u, m.y - 5.0 * u, 8.0 * u, 7.0 * u), c, 1.0);
            tri_down(p, m, u, c, 4.0, 2.0, 3.0);
        }
        Icon::Insert | Icon::Overwrite => {
            p.rect_bordered(Rect::new(m.x - 6.0 * u, m.y - 4.0 * u, 12.0 * u, 8.0 * u), Color::TRANSPARENT, 1.0, line, c);
            if icon == Icon::Overwrite {
                p.rect(Rect::new(m.x - 6.0 * u, m.y - 4.0 * u, 6.0 * u, 8.0 * u), c, 0.0);
            } else {
                bar(p, 0.0, 0.0, 1.4, 8.0);
            }
        }
        Icon::Camera => {
            p.rect_bordered(Rect::new(m.x - 6.5 * u, m.y - 4.0 * u, 13.0 * u, 9.0 * u), Color::TRANSPARENT, 1.5, line, c);
            ring(p, 2.6, line);
            bar(p, 3.5, -5.5, 4.0, 1.6);
        }
        Icon::Settings | Icon::Wrench => {
            ring(p, 3.0, line);
            for (dx, dy) in [(0.0, 6.0), (0.0, -6.0), (6.0, 0.0), (-6.0, 0.0)] {
                bar(p, dx * 0.85, dy * 0.85, if dx == 0.0 { 1.6 } else { 4.0 }, if dx == 0.0 { 4.0 } else { 1.6 });
            }
        }
        Icon::Search | Icon::Zoom => {
            p.rect_bordered(Rect::new(m.x - 6.0 * u, m.y - 6.0 * u, 9.0 * u, 9.0 * u), Color::TRANSPARENT, 4.5 * u, line, c);
            p.line(Vec2::new(m.x + 2.0 * u, m.y + 2.0 * u), Vec2::new(m.x + 6.0 * u, m.y + 6.0 * u), line, c);
        }
        Icon::Folder | Icon::NewBin => {
            p.rect(Rect::new(m.x - 6.5 * u, m.y - 4.5 * u, 6.0 * u, 2.0 * u), c, 0.5);
            p.rect_bordered(Rect::new(m.x - 6.5 * u, m.y - 3.0 * u, 13.0 * u, 8.5 * u), Color::TRANSPARENT, 1.0, line, c);
            if icon == Icon::NewBin {
                bar(p, 0.0, 1.5, 5.0, 1.3);
                bar(p, 0.0, 1.5, 1.3, 5.0);
            }
        }
        Icon::List | Icon::Sort => {
            for (i, y) in [-4.0, 0.0, 4.0].iter().enumerate() {
                let w = if icon == Icon::Sort { 11.0 - i as f32 * 3.0 } else { 9.0 };
                p.rect(Rect::new(m.x - 6.0 * u, m.y + y * u - line * 0.5, w * u, line), c, 0.0);
                if icon == Icon::List {
                    p.rect(Rect::new(m.x - 8.0 * u, m.y + y * u - line * 0.5, line, line), c, 0.0);
                }
            }
        }
        Icon::Grid | Icon::Freeform => {
            for (ix, iy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let (x, y) = (m.x - 6.0 * u + ix as f32 * 7.0 * u, m.y - 6.0 * u + iy as f32 * 7.0 * u);
                if icon == Icon::Grid {
                    p.rect(Rect::new(x, y, 5.0 * u, 5.0 * u), c, 0.5);
                } else {
                    p.rect_bordered(Rect::new(x, y, 5.0 * u, 5.0 * u), Color::TRANSPARENT, 0.5, line, c);
                }
            }
        }
        Icon::Trash => {
            bar(p, 0.0, -5.0, 10.0, 1.4);
            p.rect_bordered(Rect::new(m.x - 4.0 * u, m.y - 3.5 * u, 8.0 * u, 9.5 * u), Color::TRANSPARENT, 1.0, line, c);
        }
        Icon::Pen => {
            p.line(Vec2::new(m.x - 5.0 * u, m.y + 5.0 * u), Vec2::new(m.x + 4.0 * u, m.y - 4.0 * u), line * 1.6, c);
            tri_right(p, 3.0, -5.0, 3.0, 3.0);
        }
        Icon::Hand => {
            p.rect(Rect::new(m.x - 4.0 * u, m.y - 2.0 * u, 8.0 * u, 7.0 * u), c, 2.0 * u);
            for i in 0..3 {
                p.rect(Rect::new(m.x - 4.0 * u + i as f32 * 3.0 * u, m.y - 6.0 * u, 2.2 * u, 5.0 * u), c, 1.0 * u);
            }
        }
        Icon::Razor => {
            p.rect(Rect::new(m.x - 5.0 * u, m.y - 6.0 * u, 4.0 * u, 7.0 * u), c, 0.5);
            p.rect_bordered(Rect::new(m.x - 5.0 * u, m.y + 1.0 * u, 10.0 * u, 5.0 * u), Color::TRANSPARENT, 0.5, line, c);
        }
        Icon::Select => {
            // Arrow: a triangle with a tail.
            let steps = (11.0 * u).max(4.0) as usize;
            for i in 0..steps {
                let t = i as f32 / steps as f32;
                let w = 7.0 * u * (1.0 - t * 0.75);
                p.rect(Rect::new(m.x - 4.0 * u, m.y - 6.0 * u + t * 11.0 * u, w, (11.0 * u / steps as f32).ceil()), c, 0.0);
            }
            p.line(Vec2::new(m.x - 0.5 * u, m.y + 2.0 * u), Vec2::new(m.x + 2.5 * u, m.y + 6.5 * u), line * 1.6, c);
        }
        Icon::TrackSelect => {
            tri_right(p, -6.0, 0.0, 6.0, 9.0);
            bar(p, 3.0, 0.0, 1.4, 9.0);
            bar(p, 6.0, 0.0, 1.4, 9.0);
        }
        Icon::Ripple => {
            bar(p, -5.0, 0.0, 1.4, 10.0);
            tri_right(p, -2.0, 0.0, 5.0, 8.0);
            bar(p, 5.0, 0.0, 1.4, 10.0);
        }
        Icon::Rolling => {
            bar(p, 0.0, 0.0, 1.4, 11.0);
            tri_left(p, -2.0, 0.0, 4.0, 7.0);
            tri_right(p, 2.0, 0.0, 4.0, 7.0);
        }
        Icon::Slip => {
            bar(p, 0.0, -4.0, 12.0, 1.4);
            bar(p, 0.0, 4.0, 12.0, 1.4);
            tri_left(p, -3.0, 0.0, 3.0, 5.0);
            tri_right(p, 3.0, 0.0, 3.0, 5.0);
        }
        Icon::Rect => p.rect_bordered(Rect::new(m.x - 6.0 * u, m.y - 4.5 * u, 12.0 * u, 9.0 * u), Color::TRANSPARENT, 1.0, line, c),
        Icon::Lock => {
            p.rect(Rect::new(m.x - 4.5 * u, m.y - 0.5 * u, 9.0 * u, 7.0 * u), c, 1.0 * u);
            p.rect_bordered(Rect::new(m.x - 3.0 * u, m.y - 6.0 * u, 6.0 * u, 7.0 * u), Color::TRANSPARENT, 3.0 * u, line, c);
        }
        Icon::Eye => {
            ring(p, 2.2, line);
            p.rect_bordered(Rect::new(m.x - 7.0 * u, m.y - 4.5 * u, 14.0 * u, 9.0 * u), Color::TRANSPARENT, 4.5 * u, line, c);
        }
        Icon::Mic => {
            p.rect(Rect::new(m.x - 2.0 * u, m.y - 6.5 * u, 4.0 * u, 8.0 * u), c, 2.0 * u);
            p.rect_bordered(Rect::new(m.x - 4.5 * u, m.y - 2.0 * u, 9.0 * u, 6.0 * u), Color::TRANSPARENT, 4.5 * u, line, c);
            bar(p, 0.0, 5.5, 1.4, 3.0);
        }
        Icon::Speaker => {
            p.rect(Rect::new(m.x - 6.0 * u, m.y - 2.5 * u, 4.0 * u, 5.0 * u), c, 0.0);
            tri_right(p, -2.0, 0.0, 4.0, 10.0);
        }
        Icon::Snap => {
            bar(p, -3.5, 0.0, 1.4, 11.0);
            bar(p, 3.5, 0.0, 1.4, 11.0);
            bar(p, 0.0, 0.0, 5.0, 1.4);
        }
        Icon::LinkedSelection => {
            p.rect_bordered(Rect::new(m.x - 6.5 * u, m.y - 3.0 * u, 7.0 * u, 6.0 * u), Color::TRANSPARENT, 3.0 * u, line, c);
            p.rect_bordered(Rect::new(m.x - 0.5 * u, m.y - 3.0 * u, 7.0 * u, 6.0 * u), Color::TRANSPARENT, 3.0 * u, line, c);
        }
        Icon::Captions => {
            p.rect_bordered(Rect::new(m.x - 7.0 * u, m.y - 5.0 * u, 14.0 * u, 10.0 * u), Color::TRANSPARENT, 1.5 * u, line, c);
            bar(p, -2.0, 1.0, 4.0, 1.3);
            bar(p, 3.0, 1.0, 3.0, 1.3);
        }
        Icon::Stopwatch => {
            ring(p, 4.5, line);
            bar(p, 0.0, -6.0, 4.0, 1.4);
        }
        Icon::Reset => {
            // An open circle with an arrow head: "go back to the default".
            p.rect_bordered(Rect::new(m.x - 5.0 * u, m.y - 5.0 * u, 10.0 * u, 10.0 * u), Color::TRANSPARENT, 5.0 * u, line, c);
            p.rect(Rect::new(m.x - 1.0 * u, m.y - 6.5 * u, 6.0 * u, 3.0 * u), Color::TRANSPARENT, 0.0);
            tri_left(p, -2.5, -5.0, 3.0, 4.0);
        }
        Icon::Chevron => tri_down(p, m, u, c, 4.5, 0.0, 3.5),
        Icon::ChevronRight => tri_right(p, -2.0, 0.0, 4.5, 7.0),
        Icon::Effects => {
            ring(p, 5.0, line);
            bar(p, 0.0, 0.0, 9.0, 1.3);
        }
        Icon::AddTrack => {
            p.rect_bordered(Rect::new(m.x - 6.0 * u, m.y - 5.0 * u, 12.0 * u, 10.0 * u), Color::TRANSPARENT, 1.0, line, c);
            bar(p, 0.0, 0.0, 6.0, 1.3);
            bar(p, 0.0, 0.0, 1.3, 6.0);
        }
    }
}

fn tri_down(p: &mut Painter, m: Vec2, u: f32, c: Color, w: f32, top: f32, h: f32) {
    let steps = (h * u).max(3.0) as usize;
    for i in 0..steps {
        let t = i as f32 / steps as f32;
        let ww = w * u * (1.0 - t);
        p.rect(Rect::new(m.x - ww * 0.5, m.y + top * u + t * h * u, ww, (h * u / steps as f32).ceil()), c, 0.0);
    }
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
    let (fg, bg) = (t.palette.text_muted, t.palette.surface);
    let accent = t.palette.accent;
    ui.add_leaf(id, Layout::leaf(Size::Fixed(size), Size::Fixed(size)), Vec2::ZERO, true, move |p, rect| {
        if on {
            p.rect(rect, accent.with_alpha(0.85), 2.0);
        } else if hot > 0.01 {
            p.rect(rect, bg.with_alpha(bg.a * hot), 2.0);
        }
        let c = if on { Color::WHITE } else { fg.lerp(Color::hex(0xe4e4e4), hot) };
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
