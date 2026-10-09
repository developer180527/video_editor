//! The program monitor: the picture, the readout, the scrub bar and the
//! transport.

use libgui::*;
use ve_time::Time;

use crate::theme::REEL;
use crate::widgets::{divider, icon_button, Icon};
use crate::EditorUi;

pub fn panel(ui: &mut Ui, app: &mut EditorUi) {
    let t = ui.theme.clone();
    let col = Layout::column().width(Size::Grow(1.0)).height(Size::Grow(1.0));
    ui.container(col, Frame { fill: t.palette.bg_panel, clip: true, ..Frame::none() }, |ui| {
        picture(ui, app);
        readout(ui, app);
        scrub(ui, app);
        transport(ui, app);
    });
}

/// The sequence's frame, letterboxed into the panel.
fn picture(ui: &mut Ui, app: &mut EditorUi) {
    let id = ui.make_id("picture");
    let r = ui.interact(id);
    if r.clicked {
        toggle_play(app);
    }
    let (fw, fh) = app.snap().active().map(|s| (s.format.width as f32, s.format.height as f32)).unwrap_or((16.0, 9.0));
    let zoom = [None, Some(1.0), Some(0.5), Some(0.25)][app.view.fit.min(3)];
    let tex = app.monitor;
    let note = ui.frame_text("Decoding…");
    let catching_up = app.catching_up && !app.engine.is_playing();
    let faint = ui.theme.palette.text_faint;
    ui.add_leaf(id, Layout::leaf(Size::Grow(1.0), Size::Grow(1.0)), Vec2::ZERO, true, move |p, rect| {
        p.rect(rect, REEL.line, 0.0);
        let scale = match zoom {
            None => (rect.w / fw).min(rect.h / fh),
            Some(z) => z,
        };
        let (w, h) = (fw * scale, fh * scale);
        let f = Rect::new((rect.center().x - w * 0.5).round(), (rect.center().y - h * 0.5).round(), w.round(), h.round());
        match tex {
            Some(tex) => p.image(f, tex, 0.0),
            None => p.rect(f, Color::BLACK, 0.0),
        }
        if catching_up {
            p.text_left(Rect::new(f.x + 10.0, f.bottom() - 22.0, f.w - 20.0, 16.0), 11.0, faint, note);
        }
    });
}

fn toggle_play(app: &mut EditorUi) {
    if app.engine.is_playing() {
        app.engine.stop();
    } else {
        app.engine.play(1.0);
    }
}

fn readout(ui: &mut Ui, app: &mut EditorUi) {
    let t = ui.theme.clone();
    let row = Layout::row()
        .width(Size::Grow(1.0))
        .height(Size::Fixed(26.0))
        .padding(Insets::xy(10.0, 0.0))
        .gap(8.0)
        .align(Align::Start, Align::Center);
    let end = app.snap().active().map(|s| s.duration()).unwrap_or(Time::ZERO);
    ui.container(row, Frame::none(), |ui| {
        ui.text_with(&app.timecode(app.playhead), 12.0, REEL.timecode);
        ui.space(6.0);
        ui.container(Layout::row().width(Size::Fixed(110.0)).height(Size::Fixed(20.0)), Frame::none(), |ui| {
            ui.combo("fit", &mut app.view.fit, &["Fit", "100%", "50%", "25%"]);
        });
        ui.flex();
        ui.container(Layout::row().width(Size::Fixed(96.0)).height(Size::Fixed(20.0)), Frame::none(), |ui| {
            ui.combo("quality", &mut app.view.quality, &["Full", "1/2", "1/4"]);
        });
        let proxies = icon_button(ui, "proxies", Icon::Wrench, 20.0, app.view.proxies);
        ui.tooltip(&proxies, if app.view.proxies { "Playing proxies (click for original media)" } else { "Toggle Proxies" });
        if proxies.clicked {
            app.toggle_proxies();
        }
        ui.text_with(&app.timecode(end), 12.0, t.palette.text_muted);
    });
}

/// Click or drag anywhere on it to move the playhead.
fn scrub(ui: &mut Ui, app: &mut EditorUi) {
    let id = ui.make_id("scrub");
    let r = ui.interact_drag(id);
    let end = app.snap().active().map(|s| s.duration()).unwrap_or(Time::ZERO).max(Time::from_seconds(1));
    if r.active || r.pressed {
        let f = ((r.mouse_pos.x - r.rect.x - 6.0) / (r.rect.w - 12.0).max(1.0)).clamp(0.0, 1.0);
        let t = Time::from_seconds_f64(end.as_seconds_f64() * f as f64).round_to_frame(app.rate());
        app.seek(t);
    }
    if r.hovered {
        ui.cursor = Cursor::ResizeHorizontal;
    }
    let frac = (app.playhead.as_seconds_f64() / end.as_seconds_f64()).clamp(0.0, 1.0) as f32;
    let at = move |t: Time| (t.as_seconds_f64() / end.as_seconds_f64()).clamp(0.0, 1.0) as f32;
    let marks = app.snap().active().map(|s| s.marks.clone()).unwrap_or_default();
    let (mark_in, mark_out) = (marks.in_point.map(at), marks.out_point.map(at));
    let markers: Vec<(f32, Color)> = marks.markers.iter().map(|m| (at(m.time), crate::features::marker_color(m.color))).collect();
    ui.add_leaf(id, Layout::leaf(Size::Grow(1.0), Size::Fixed(22.0)), Vec2::ZERO, true, move |p, rect| {
        let bar = Rect::new(rect.x + 6.0, rect.y + 8.0, rect.w - 12.0, 5.0);
        p.rect(bar, REEL.raised_hi, 2.5);
        for i in 0..=10 {
            let x = bar.x + bar.w * (i as f32 / 10.0);
            p.rect(Rect::new(x, bar.bottom() + 2.0, 1.0, 4.0), REEL.tick, 0.0);
        }
        let x = bar.x + bar.w * frac;
        p.rect(Rect::new(bar.x, bar.y, (x - bar.x).max(0.0), bar.h), REEL.tick, 2.5);
        // In/out: the range shaded, brackets at its ends.
        if mark_in.is_some() || mark_out.is_some() {
            let a = bar.x + bar.w * mark_in.unwrap_or(0.0);
            let b = bar.x + bar.w * mark_out.unwrap_or(1.0);
            p.rect(Rect::new(a, bar.y - 2.0, (b - a).max(1.0), bar.h + 4.0), REEL.in_out_range, 0.0);
            if mark_in.is_some() {
                p.rect(Rect::new(a, bar.y - 3.0, 2.0, bar.h + 6.0), REEL.in_out, 0.0);
            }
            if mark_out.is_some() {
                p.rect(Rect::new(b - 2.0, bar.y - 3.0, 2.0, bar.h + 6.0), REEL.in_out, 0.0);
            }
        }
        for (f, c) in &markers {
            let mx = (bar.x + bar.w * f).round();
            p.rect(Rect::new(mx - 2.0, rect.y + 1.0, 4.0, 5.0), *c, 1.0);
        }
        p.rect(Rect::new(x - 5.0, rect.y + 3.0, 10.0, 14.0), REEL.playhead, 2.0);
    });
}

fn transport(ui: &mut Ui, app: &mut EditorUi) {
    let row = Layout::row()
        .width(Size::Grow(1.0))
        .height(Size::Fixed(34.0))
        .padding(Insets::xy(8.0, 0.0))
        .gap(2.0)
        .align(Align::Center, Align::Center);
    let playing = app.engine.is_playing();
    ui.container(row, Frame { fill: REEL.chrome, ..Frame::none() }, |ui| {
        let tip = |ui: &mut Ui, r: &Response, s: &str| ui.tooltip(r, s);
        let r = icon_button(ui, "add-marker", Icon::Marker, 22.0, false);
        tip(ui, &r, "Add Marker (M)");
        if r.clicked {
            app.add_marker();
        }
        let r = icon_button(ui, "in", Icon::MarkIn, 22.0, false);
        tip(ui, &r, "Mark In (I)");
        if r.clicked {
            app.mark_in();
        }
        let r = icon_button(ui, "out", Icon::MarkOut, 22.0, false);
        tip(ui, &r, "Mark Out (O)");
        if r.clicked {
            app.mark_out();
        }
        divider(ui, "t1", true, 18.0);
        let r = icon_button(ui, "start", Icon::JumpStart, 22.0, false);
        tip(ui, &r, "Go to Start (Home)");
        if r.clicked {
            app.seek(Time::ZERO);
        }
        let r = icon_button(ui, "back", Icon::StepBack, 22.0, false);
        tip(ui, &r, "Step Back (Left)");
        if r.clicked {
            app.seek(app.step(-1));
        }
        let r = icon_button(ui, "play", if playing { Icon::Pause } else { Icon::Play }, 26.0, playing);
        tip(ui, &r, "Play/Stop (Space)");
        if r.clicked {
            toggle_play(app);
        }
        let r = icon_button(ui, "fwd", Icon::StepForward, 22.0, false);
        tip(ui, &r, "Step Forward (Right)");
        if r.clicked {
            app.seek(app.step(1));
        }
        let r = icon_button(ui, "end", Icon::JumpEnd, 22.0, false);
        tip(ui, &r, "Go to End (End)");
        if r.clicked {
            let end = app.snap().active().map(|s| s.duration()).unwrap_or(Time::ZERO);
            app.seek(end);
        }
        divider(ui, "t2", true, 18.0);
        let r = icon_button(ui, "insert", Icon::Insert, 22.0, false);
        tip(ui, &r, "Insert selected media at the playhead");
        if r.clicked {
            if let Some(a) = app.view.selected_asset {
                app.place_asset(a, app.playhead, None, true);
            }
        }
        let r = icon_button(ui, "overwrite", Icon::Overwrite, 22.0, false);
        tip(ui, &r, "Overwrite selected media at the playhead");
        if r.clicked {
            if let Some(a) = app.view.selected_asset {
                app.place_asset(a, app.playhead, None, false);
            }
        }
        ui.flex();
    });
}
