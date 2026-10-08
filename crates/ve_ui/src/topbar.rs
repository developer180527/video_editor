//! The application bar: workspace tabs (Import / Edit / Export), the project
//! title, and the right side's utilities.

use libgui::*;

use crate::widgets::{icon_button, Icon};
use crate::{EditorUi, HostRequest};

pub fn bar(ui: &mut Ui, app: &mut EditorUi) {
    let row = Layout::row()
        .width(Size::Grow(1.0))
        .height(Size::Fixed(38.0))
        .padding(Insets::xy(10.0, 0.0))
        .gap(6.0)
        .align(Align::Start, Align::Center);
    ui.container(row, Frame { fill: Color::hex(0x1c1c1c), ..Frame::none() }, |ui| {
        let home = icon_button(ui, "home", Icon::Home, 24.0, false);
        ui.tooltip(&home, "Open Project…");
        if home.clicked {
            app.requests.push(HostRequest::OpenProject);
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
        let _ = icon_button(ui, "menu", Icon::Menu, 24.0, false);
        let _ = icon_button(ui, "full", Icon::Expand, 24.0, false);
    });
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
        let c = if on { Color::hex(0xf2f2f2) } else { t.palette.text_muted.lerp(Color::hex(0xd8d8d8), hot) };
        p.text_centered(rect, size, c, text);
        if on {
            let tw = p.measure(size, text).x;
            p.rect(Rect::new(rect.center().x - tw * 0.5, rect.bottom() - 5.0, tw, 2.0), Color::hex(0xf2f2f2), 1.0);
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
        p.text_left(rect, size, Color::hex(0xe8e8e8), name);
        p.text_left(rect.shrink(w, 0.0, 0.0, 0.0), size, t.palette.text_muted, state);
    });
}
