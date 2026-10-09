//! The project bin (path bar, search, thumbnails, tools) and the Effects list.

use std::path::PathBuf;
use std::sync::Arc;

use libgui::*;
use ve_engine::Command;
use ve_model::*;
use ve_engine::{EffectInfo, Implementation};
use ve_time::{Rate, Time, Timecode};


use crate::widgets::{divider, icon_button, Icon};
use crate::{EditorUi, HostRequest, ASSET_PAYLOAD, FILES_PAYLOAD};

/// Payload kind for an effect dragged from the Effects panel, carrying `PluginRef`.
pub const EFFECT_PAYLOAD: &str = "effect";

pub fn panel(ui: &mut Ui, app: &mut EditorUi) {
    let t = ui.theme.clone();
    let col = Layout::column().width(Size::Grow(1.0)).height(Size::Grow(1.0));
    ui.container(col, Frame { fill: t.palette.bg_panel, clip: true, ..Frame::none() }, |ui| {
        // Files from the OS land anywhere on the bin.
        let zone = ui.drop_zone(&[FILES_PAYLOAD]);
        if let Some(p) = zone.dropped {
            if let Ok(paths) = p.take::<Vec<PathBuf>>() {
                app.engine.import(paths.iter().map(|p| p.to_string_lossy().into_owned()).collect());
            }
        }
        path_bar(ui, app);
        search_row(ui, app);
        grid(ui, app, zone.hovered);
        footer(ui, app);
    });
}

fn path_bar(ui: &mut Ui, app: &EditorUi) {
    let t = ui.theme.clone();
    let row = Layout::row()
        .width(Size::Grow(1.0))
        .height(Size::Fixed(22.0))
        .padding(Insets::xy(6.0, 0.0))
        .gap(6.0)
        .align(Align::Start, Align::Center);
    ui.container(row, Frame::none(), |ui| {
        let _ = icon_button(ui, "bin", Icon::Folder, 16.0, false);
        let name = match &app.st.file {
            Some(_) => format!("{}.veproj", app.snap().name),
            None => format!("{} (unsaved)", app.snap().name),
        };
        ui.text_with(&name, t.metrics.font_size, t.palette.text);
    });
}

fn search_row(ui: &mut Ui, app: &mut EditorUi) {
    let row = Layout::row()
        .width(Size::Grow(1.0))
        .height(Size::Fixed(28.0))
        .padding(Insets::xy(6.0, 0.0))
        .gap(6.0)
        .align(Align::Start, Align::Center);
    ui.container(row, Frame::none(), |ui| {
        ui.container(
            Layout::row().width(Size::Grow(1.0)).height(Size::Fixed(20.0)).gap(4.0).align(Align::Start, Align::Center),
            Frame::none(),
            |ui| {
                let _ = icon_button(ui, "find", Icon::Search, 16.0, false);
                ui.text_input("search", &mut app.view.search, "Search");
            },
        );
        let import = icon_button(ui, "import", Icon::NewBin, 18.0, false);
        ui.tooltip(&import, "Import… (Cmd+I)");
        if import.clicked {
            app.requests.push(HostRequest::ImportMedia);
        }
    });
}

fn grid(ui: &mut Ui, app: &mut EditorUi, drop_hover: bool) {
    let t = ui.theme.clone();
    let q = app.view.search.to_lowercase();
    let assets: Vec<Arc<Asset>> =
        app.snap().assets.values().filter(|a| q.is_empty() || a.name.to_lowercase().contains(&q)).cloned().collect();
    let total = app.snap().assets.len();
    let selected = app.view.selected_asset;
    let count_line = match selected {
        Some(_) => format!("1 of {total} items selected"),
        None => format!("{total} items"),
    };
    ui.container(
        Layout::row().width(Size::Grow(1.0)).height(Size::Fixed(18.0)).padding(Insets::xy(8.0, 0.0)).align(Align::Start, Align::Center),
        Frame::none(),
        |ui| ui.text_with(&count_line, t.metrics.font_size, t.palette.text_muted),
    );
    let across = 1 + (app.view.thumb * 3.0).round() as usize; // 1..4
    let across = 5 - across.clamp(1, 4); // bigger thumbnails, fewer across
    let rate = app.rate();
    let opts = ScrollOptions { padding: Insets::all(8.0), gap: 8.0, ..ScrollOptions::new(Size::Grow(1.0)) };
    let mut picked = None;
    let mut append = None;
    ui.scroll_area_with("bin", opts, |ui| {
        if assets.is_empty() {
            let msg = if drop_hover { "Drop to import" } else { "Drop media here, or use Import (Cmd+I)." };
            ui.text_with(msg, t.metrics.font_size, t.palette.text_faint);
        }
        for chunk in assets.chunks(across) {
            ui.container(Layout::row().width(Size::Grow(1.0)).height(Size::Fixed(64.0 + 260.0 / across as f32)).gap(8.0), Frame::none(), |ui| {
                for a in chunk {
                    // A poster frame a moment in, past any fade from black.
                    let at = a.info.as_ref().map(|i| Time::from_seconds(1).min(Time(i.duration.ticks() / 2))).unwrap_or(Time::ZERO);
                    let tex = if a.info.as_ref().is_some_and(|i| i.video.is_some()) { app.thumb_tex(a, at) } else { None };
                    let peaks = if tex.is_none() { app.peaks(a, 0) } else { None };
                    let r = thumbnail(ui, a, selected == Some(a.id), rate, tex, peaks);
                    if r.clicked {
                        picked = Some(a.id);
                    }
                    if r.double_clicked {
                        append = Some(a.id);
                    }
                }
                for _ in chunk.len()..across {
                    ui.container(Layout::column().width(Size::Grow(1.0)).height(Size::Grow(1.0)), Frame::none(), |_| {});
                }
            });
        }
    });
    if let Some(id) = picked {
        app.view.selected_asset = Some(id);
    }
    if let Some(id) = append {
        // Double-click: onto the end of the sequence.
        let end = app.snap().active().map(|s| s.duration()).unwrap_or(Time::ZERO);
        app.place_asset(id, end, None, false);
    }
}

/// Two colours from the name, standing in for a frame until thumbnails are
/// decoded (Phase C).
fn tint(name: &str) -> (Color, Color) {
    let h = name.bytes().fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619));
    let hue = |x: u32| Color::hex(0x204060 + (x & 0x3f3f3f));
    (hue(h), hue(h.rotate_left(11)).lerp(Color::hex(0xd8d0c0), 0.5))
}

fn thumbnail(ui: &mut Ui, a: &Arc<Asset>, selected: bool, rate: Rate, tex: Option<TextureId>, peaks: Option<Arc<Vec<f32>>>) -> Response {
    let t = ui.theme.clone();
    let id = ui.make_id(("asset", a.id));
    let r = ui.interact_drag(id);
    let label = a.name.clone();
    let asset = a.id;
    ui.drag_source_from(&r, move || Payload::new(ASSET_PAYLOAD, asset).with_label(label));
    if r.hovered {
        ui.cursor = Cursor::Pointer;
    }
    let hot = ui.animate_bool(id, 0, r.hovered);
    let (c0, c1) = tint(&a.name);
    let audio_only = a.info.as_ref().is_some_and(|i| i.video.is_none() && !i.audio.is_empty());
    let has_audio = a.info.as_ref().is_some_and(|i| !i.audio.is_empty());
    let dur = a
        .info
        .as_ref()
        .map(|i| {
            let tc = Timecode::from_time(i.duration, rate, false);
            if tc.hours > 0 { format!("{}:{:02}:{:02}", tc.hours, tc.minutes, tc.seconds) } else { format!("{}:{:02}", tc.minutes, tc.seconds) }
        })
        .unwrap_or_else(|| "—".into());
    let (name, dur) = (ui.frame_text(&a.name), ui.frame_text(&dur));
    let size = t.metrics.font_size_small;
    let accent = t.palette.accent;
    ui.add_leaf(id, Layout::leaf(Size::Grow(1.0), Size::Grow(1.0)), Vec2::ZERO, true, move |p, rect| {
        let fill = if selected { Color::hex(0x3a3a3a) } else { Color::hex(0x2a2a2a).lerp(Color::hex(0x333333), hot) };
        p.rect_bordered(rect, fill, 3.0, if selected { 2.0 } else { 1.0 }, if selected { accent } else { Color::hex(0x191919) });
        let img = Rect::new(rect.x + 4.0, rect.y + 4.0, rect.w - 8.0, rect.h - 32.0);
        if let Some(tex) = tex {
            // The picture, fitted (16:9 into the card), on black.
            p.rect(img, Color::BLACK, 2.0);
            let (w, h) = if img.w / img.h > 16.0 / 9.0 { (img.h * 16.0 / 9.0, img.h) } else { (img.w, img.w * 9.0 / 16.0) };
            p.image(Rect::new(img.center().x - w / 2.0, img.center().y - h / 2.0, w, h), tex, 0.0);
        } else if audio_only {
            p.rect(img, Color::hex(0x1b3b49), 2.0);
            let mid = img.center().y;
            let cols = (img.w as usize / 3).max(1);
            for k in 0..cols {
                let x = img.x + k as f32 * 3.0;
                let a = match &peaks {
                    // The whole file, squeezed across the card.
                    Some(pk) if !pk.is_empty() => {
                        let (i0, i1) = (k * pk.len() / cols, ((k + 1) * pk.len() / cols).max(k * pk.len() / cols + 1));
                        pk[i0..i1.min(pk.len())].iter().cloned().fold(0.0, f32::max).sqrt()
                    }
                    _ => 0.05,
                };
                let h = a * img.h * 0.9;
                p.rect(Rect::new(x, mid - h * 0.5, 2.0, h.max(1.0)), Color::hex(0x7fc6e8), 0.0);
            }
        } else {
            let bands = 12;
            for b in 0..bands {
                let f = b as f32 / bands as f32;
                p.rect(Rect::new(img.x, img.y + img.h * f, img.w, img.h / bands as f32 + 1.0), c0.lerp(c1, f), 0.0);
            }
            p.rect(Rect::new(img.x, img.y + img.h * 0.62, img.w, img.h * 0.38), c0.lerp(Color::hex(0x2f5d33), 0.6), 0.0);
        }
        // Corner badges: film strip, and the "has audio" mark.
        let mut bx = img.right() - 24.0;
        if !audio_only {
            let badge = Rect::new(bx, img.bottom() - 12.0, 22.0, 10.0);
            p.rect(badge, Color::rgba(0.0, 0.0, 0.0, 0.55), 1.5);
            for k in 0..5 {
                p.rect(Rect::new(badge.x + 2.0 + k as f32 * 4.0, badge.y + 2.0, 2.0, 6.0), Color::hex(0xbfc7cf), 0.0);
            }
            bx -= 26.0;
        }
        if has_audio {
            let badge = Rect::new(bx, img.bottom() - 12.0, 22.0, 10.0);
            p.rect(badge, Color::rgba(0.0, 0.0, 0.0, 0.55), 1.5);
            for k in 0..5 {
                let h = [3.0, 6.0, 4.0, 7.0, 3.0][k];
                p.rect(Rect::new(badge.x + 2.0 + k as f32 * 4.0, badge.center().y - h * 0.5, 2.0, h), Color::hex(0x7fc6e8), 0.0);
            }
        }
        p.text_left(Rect::new(rect.x + 6.0, rect.bottom() - 26.0, rect.w - 60.0, 14.0), size, Color::hex(0xdcdcdc), name);
        p.text_right(Rect::new(rect.x, rect.bottom() - 26.0, rect.w - 6.0, 14.0), size, Color::hex(0x9a9a9a), dur);
    });
    r
}

/// The bare zoom slider a bin wears: no label, no readout.
fn thumb_slider(ui: &mut Ui, app: &mut EditorUi) {
    let id = ui.make_id("thumb");
    let r = ui.interact_drag(id);
    if r.active {
        app.view.thumb = ((r.mouse_pos.x - r.rect.x) / r.rect.w.max(1.0)).clamp(0.0, 1.0);
    }
    if r.hovered {
        ui.cursor = Cursor::Pointer;
    }
    let f = app.view.thumb;
    ui.add_leaf(id, Layout::leaf(Size::Fixed(104.0), Size::Fixed(20.0)), Vec2::ZERO, true, move |p, rect| {
        let line = Rect::new(rect.x + 6.0, rect.center().y - 1.0, rect.w - 12.0, 2.0);
        p.rect(line, Color::hex(0x3d3d3d), 1.0);
        let x = line.x + line.w * f;
        p.rect(Rect::new(x - 5.0, rect.center().y - 5.0, 10.0, 10.0), Color::hex(0xb0b0b0), 5.0);
    });
}

fn footer(ui: &mut Ui, app: &mut EditorUi) {
    let row = Layout::row()
        .width(Size::Grow(1.0))
        .height(Size::Fixed(28.0))
        .padding(Insets::xy(6.0, 0.0))
        .gap(3.0)
        .align(Align::Start, Align::Center);
    ui.container(row, Frame { fill: Color::hex(0x1f1f1f), ..Frame::none() }, |ui| {
        let _ = icon_button(ui, "pen", Icon::Pen, 20.0, true);
        let _ = icon_button(ui, "list", Icon::List, 20.0, false);
        let _ = icon_button(ui, "grid", Icon::Grid, 20.0, true);
        let _ = icon_button(ui, "free", Icon::Freeform, 20.0, false);
        divider(ui, "p1", true, 16.0);
        thumb_slider(ui, app);
        ui.flex();
        let _ = icon_button(ui, "sortbin", Icon::Sort, 20.0, false);
        let _ = icon_button(ui, "zoomin", Icon::Zoom, 20.0, false);
        let newbin = icon_button(ui, "newbin", Icon::NewBin, 20.0, false);
        ui.tooltip(&newbin, "Import…");
        if newbin.clicked {
            app.requests.push(HostRequest::ImportMedia);
        }
        let trash = icon_button(ui, "trash", Icon::Trash, 20.0, false);
        ui.tooltip(&trash, "Remove from project");
        if trash.clicked {
            if let Some(a) = app.view.selected_asset.take() {
                app.run(Command::RemoveAsset { asset: a });
            }
        }
    });
}

/// The Effects panel: every effect the engine knows, by category. Drag one
/// onto a clip, or double-click to apply it to the selected clip.
pub fn effects_list(ui: &mut Ui, app: &mut EditorUi) {
    let t = ui.theme.clone();
    let registry = app.st.plugins.clone();
    let mut effects: Vec<&EffectInfo> =
        registry.effects().iter().filter(|e| !matches!(e.implementation, Implementation::Intrinsic)).collect();
    effects.sort_by(|a, b| (&a.category, &a.name).cmp(&(&b.category, &b.name)));
    if effects.is_empty() {
        ui.label_muted("No plugin effects loaded.");
    }
    let mut last = String::new();
    let mut apply = None;
    for e in effects {
        if e.category != last {
            ui.section(&e.category);
            last = e.category.clone();
        }
        let id = ui.make_id(("effect", &e.plugin.id));
        let r = ui.selectable_keyed(&e.plugin.id, &e.name, false);
        let plugin = e.plugin.clone();
        let label = e.name.clone();
        ui.drag_source_from(&r, move || Payload::new(EFFECT_PAYLOAD, plugin).with_label(label));
        ui.tooltip(&r, &format!("{} · {:?} v{} · {} parameter(s){}", e.plugin.id, e.plugin.api, e.plugin.major_version, e.params.len(), if e.wgsl.is_some() { " · GPU" } else { "" }));
        if r.double_clicked {
            apply = Some(e.plugin.clone());
        }
        let _ = id;
    }
    ui.space(8.0);
    ui.text_with("Drag onto a clip, or double-click to apply to the selection.", t.metrics.font_size_small, t.palette.text_faint);
    if let Some(plugin) = apply {
        if let Some(&clip) = app.view.selection.first() {
            add_effect(app, clip, &plugin);
        }
    }
}

/// Append a new instance of `plugin` to `clip`'s effects, at defaults.
pub fn add_effect(app: &mut EditorUi, clip: ClipId, plugin: &PluginRef) {
    let Some(info) = app.st.plugins.find(plugin) else { return };
    let Some((_, _, c)) = app.snap().find_clip(clip) else { return };
    let params = info.params.iter().map(|p| (p.id.clone(), Param::Constant(ve_engine::default_value(&p.kind, p.default)))).collect();
    let effect = Effect { id: EffectId::new(), plugin: plugin.clone(), enabled: true, params };
    let index = c.effects.len();
    app.run(Command::AddEffect { clip, index, effect: Arc::new(effect) });
}
