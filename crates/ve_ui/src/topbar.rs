//! The application bar, which is also the window's title bar: the OS's
//! window buttons (or ours), the menus, workspace tabs (Import / Edit /
//! Export), the project title, and the right side's utilities.

use crate::theme::REEL;
use libgui::*;

use crate::widgets::{icon_button, Icon};
use crate::{EditorUi, HostRequest, WindowAction, WindowControls};

/// The bar's height.
pub const HEIGHT: f32 = 38.0;
/// One drawn window button's width, as Chrome and Windows draw them.
const BUTTON_W: f32 = 46.0;
/// All three drawn window buttons.
pub const CONTROLS_W: f32 = BUTTON_W * 3.0;

pub fn bar(ui: &mut Ui, app: &mut EditorUi) {
    let controls = app.frame.controls;
    // Drawn window buttons sit flush with the window's right edge.
    let right = if matches!(controls, WindowControls::Drawn { .. }) { 0.0 } else { 10.0 };
    let row = Layout::row()
        .width(Size::Grow(1.0))
        .height(Size::Fixed(HEIGHT))
        .padding(Insets { left: 10.0, top: 0.0, right, bottom: 0.0 })
        .gap(6.0)
        .align(Align::Start, Align::Center);
    ui.container(row, Frame { fill: REEL.chrome, ..Frame::none() }, |ui| {
        if let WindowControls::Leading { inset } = controls {
            // The traffic lights are drawn here by the OS.
            ui.space((inset - 10.0).max(0.0));
        }
        let home = icon_button(ui, "home", Icon::Home, 24.0, false);
        ui.tooltip(&home, "Open Project…");
        if home.clicked {
            app.requests.push(HostRequest::OpenProject);
        }
        if !app.frame.system_menu {
            app.menu_bar(ui);
        }
        ui.space(8.0);
        for (i, name) in ["Import", "Edit", "Export"].iter().enumerate() {
            if workspace_tab(ui, i, name, &mut app.view.workspace) && i != 1 {
                // Import and Export open their dialogs and return to Edit.
                if i == 0 {
                    app.requests.push(HostRequest::ImportMedia);
                } else {
                    app.show_export = true;
                }
                app.view.workspace = 1;
            }
        }
        ui.flex();
        title(ui, app);
        ui.flex();
        if let Some(busy) = &app.st.busy {
            ui.text_with(busy, 11.0, ui.theme.palette.text_muted);
        }
        if let Some(job) = app.export.clone() {
            ui.text_with(&format!("Exporting {} — {:.0}%", job.name, job.progress() * 100.0), 11.0, ui.theme.palette.text_muted);
            ui.container(Layout::row().width(Size::Fixed(120.0)).height(Size::Fixed(20.0)).align(Align::Start, Align::Center), Frame::none(), |ui| {
                ui.progress("export", Some(job.progress()));
            });
            if ui.button_keyed("cancel-export", "Cancel").clicked {
                job.cancel();
            }
        }
        let save = icon_button(ui, "save", Icon::Panel, 24.0, false);
        ui.tooltip(&save, "Save Project As…");
        if save.clicked {
            app.requests.push(HostRequest::SaveProjectAs);
        }
        let share = icon_button(ui, "share", Icon::Share, 24.0, false);
        ui.tooltip(&share, "Export… (Cmd+M)");
        if share.clicked {
            app.show_export = true;
        }
        let _ = icon_button(ui, "full", Icon::Expand, 24.0, false);
        if let WindowControls::Drawn { maximized } = controls {
            ui.space(4.0);
            for a in window_controls(ui, maximized, HEIGHT) {
                app.window_actions.push((SurfaceId::MAIN, a));
            }
        }
    });
}

/// Minimize, maximize (or restore) and close, drawn the way Chrome and
/// Windows draw them: flat, full height, the close button red on hover.
pub fn window_controls(ui: &mut Ui, maximized: bool, height: f32) -> Vec<WindowAction> {
    let mut out = Vec::new();
    for (i, action) in [WindowAction::Minimize, WindowAction::ToggleMaximize, WindowAction::Close].into_iter().enumerate() {
        let id = ui.make_id(("window-button", i));
        let r = ui.interact(id);
        if r.clicked {
            out.push(action);
        }
        let hot = ui.animate_bool(id, 0, r.hovered);
        let pressed = r.active;
        let ink = ui.theme.palette.text;
        ui.add_leaf(id, Layout::leaf(Size::Fixed(BUTTON_W), Size::Fixed(height)), Vec2::ZERO, true, move |p, rect| {
            let close = action == WindowAction::Close;
            let fill = if close { Color::hex(0xc42b1c) } else { REEL.raised_hi };
            let a = if pressed { 0.75 } else { hot };
            p.rect(rect, fill.with_alpha(fill.a * a), 0.0);
            let ink = if close { ink.lerp(Color::WHITE, hot) } else { ink };
            let c = Vec2::new(rect.center().x.round() + 0.5, rect.center().y.round() + 0.5);
            let s = 5.0; // half the glyph: a 10 px square
            match action {
                WindowAction::Minimize => p.line(Vec2::new(c.x - s, c.y), Vec2::new(c.x + s, c.y), 1.0, ink),
                WindowAction::ToggleMaximize if maximized => {
                    // Restore: two overlapping squares.
                    p.rect_bordered(Rect::new(c.x - s, c.y - s + 2.0, 2.0 * s - 2.0, 2.0 * s - 2.0), Color::TRANSPARENT, 1.0, 1.0, ink);
                    p.line(Vec2::new(c.x - s + 2.0, c.y - s), Vec2::new(c.x + s, c.y - s), 1.0, ink);
                    p.line(Vec2::new(c.x + s, c.y - s), Vec2::new(c.x + s, c.y + s - 2.0), 1.0, ink);
                }
                WindowAction::ToggleMaximize => {
                    p.rect_bordered(Rect::new(c.x - s, c.y - s, 2.0 * s, 2.0 * s), Color::TRANSPARENT, 1.0, 1.0, ink);
                }
                WindowAction::Close => {
                    p.line(Vec2::new(c.x - s, c.y - s), Vec2::new(c.x + s, c.y + s), 1.0, ink);
                    p.line(Vec2::new(c.x + s, c.y - s), Vec2::new(c.x - s, c.y + s), 1.0, ink);
                }
            }
        });
    }
    out
}

/// One of the workspace names: white and underlined when active.
fn workspace_tab(ui: &mut Ui, i: usize, name: &str, selected: &mut usize) -> bool {
    let t = ui.theme.clone();
    let id = ui.make_id(("ws", i));
    let r = ui.interact(id);
    if r.clicked {
        *selected = i;
    }
    if r.hovered {
        ui.cursor = Cursor::Pointer;
    }
    let on = *selected == i;
    let hot = ui.animate_bool(id, 0, r.hovered);
    let size = 13.0;
    let w = ui.fonts.measure(ui.font, size, name).x + 20.0;
    let text = ui.frame_text(name);
    ui.add_leaf(id, Layout::leaf(Size::Fixed(w), Size::Fixed(30.0)), Vec2::ZERO, true, move |p, rect| {
        let c = if on { REEL.bright } else { t.palette.text_muted.lerp(REEL.text_soft, hot) };
        p.text_centered(rect, size, c, text);
        if on {
            let tw = p.measure(size, text).x;
            p.rect(Rect::new(rect.center().x - tw * 0.5, rect.bottom() - 5.0, tw, 2.0), REEL.bright, 1.0);
        }
    });
    r.clicked
}

fn title(ui: &mut Ui, app: &EditorUi) {
    let t = ui.theme.clone();
    let id = ui.make_id("title");
    let name = ui.frame_text(&app.snap().name);
    let state = ui.frame_text(if app.st.dirty { " - Edited" } else { "" });
    let size = 14.0;
    ui.add_leaf(id, Layout::leaf(Size::Fit, Size::Fixed(20.0)), Vec2::new(150.0, 20.0), false, move |p, rect| {
        let w = p.measure(size, name).x;
        p.text_left(rect, size, REEL.bright, name);
        p.text_left(rect.shrink(w, 0.0, 0.0, 0.0), size, t.palette.text_muted, state);
    });
}
