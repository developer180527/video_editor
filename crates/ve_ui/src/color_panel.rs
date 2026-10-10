//! Lumetri Color: the selected clip's grade — white balance, tone,
//! saturation, and shadows / midtones / highlights colour wheels.
//!
//! The grade is the clip's `ve.grade` effect (also listed in Effect
//! Controls). Every control changes the picture as it is dragged; a drag is
//! one undo step (`EngineClient::execute_merging`). The first change adds
//! the effect, so undoing it takes the grade off again.

use std::sync::Arc;

use libgui::*;
use ve_engine::{intrinsic, Command};
use ve_model::*;
use ve_time::Time;

use crate::effects::set_at;
use crate::theme::REEL;
use crate::EditorUi;

/// The selected video clip, if any.
fn subject(app: &EditorUi) -> Option<Arc<Clip>> {
    let snap = app.snap();
    app.view.selection.iter().find_map(|id| snap.find_clip(*id).filter(|(s, ti, _)| s.tracks[*ti].kind == TrackKind::Video).map(|(_, _, c)| c.clone()))
}

/// The clip's grade effect: what is in the project, or the one just added.
fn grade_of(clip: &Clip) -> Option<Arc<Effect>> {
    clip.effects.iter().find(|e| e.plugin.id == intrinsic::GRADE).cloned()
}

/// A parameter's value now (its default when the clip has no grade yet).
fn value(app: &EditorUi, clip: &Clip, effect: Option<&Effect>, id: &str) -> Value {
    let info = app.st.plugins.find(&intrinsic::plugin_ref(intrinsic::GRADE));
    let p = info.and_then(|i| i.params.iter().find(|p| p.id == id));
    let default = p.map(|p| ve_engine::default_value(&p.kind, p.default)).unwrap_or(Value::Float(0.0));
    let t = (app.playhead - clip.timeline_start).clamp_to(Time::ZERO, clip.source_range.duration);
    effect.and_then(|e| e.params.get(id)).map(|p| p.value_at(t)).unwrap_or(default)
}

impl EditorUi {
    /// Set `param` of `clip`'s grade to `v`, as part of drag `gesture`;
    /// adds the grade first if the clip has none.
    fn set_grade(&mut self, clip: &Clip, param: &str, v: Value, gesture: u64) {
        let t = (self.playhead - clip.timeline_start).clamp_to(Time::ZERO, clip.source_range.duration);
        let existing = grade_of(clip);
        let pending = self.pending_grade.filter(|(c, _)| *c == clip.id).map(|(_, e)| e);
        let effect = match (existing, pending) {
            (Some(e), _) => {
                self.pending_grade = None;
                let stored = e.params.get(param).cloned().unwrap_or(Param::Constant(v.clone()));
                let cmd = Command::SetEffectParam { clip: clip.id, effect: e.id, param: param.into(), value: Some(set_at(&stored, t, v)) };
                self.engine.execute_merging(cmd, gesture);
                return;
            }
            // Added a moment ago; the engine has it even if this snapshot doesn't.
            (None, Some(id)) => id,
            (None, None) => {
                let Some(info) = self.st.plugins.find(&intrinsic::plugin_ref(intrinsic::GRADE)).cloned() else { return };
                let params = info.params.iter().map(|p| (p.id.clone(), Param::Constant(ve_engine::default_value(&p.kind, p.default)))).collect();
                let id = EffectId::new();
                let effect = Effect { id, plugin: info.plugin.clone(), enabled: true, params };
                let index = clip.effects.len();
                self.engine.execute_merging(Command::AddEffect { clip: clip.id, index, effect: Arc::new(effect) }, gesture);
                self.pending_grade = Some((clip.id, id));
                id
            }
        };
        let cmd = Command::SetEffectParam { clip: clip.id, effect, param: param.into(), value: Some(Param::Constant(v)) };
        self.engine.execute_merging(cmd, gesture);
    }
}

/// A control's track: plain, or a gradient that says what it does.
#[derive(Clone, Copy)]
enum Track {
    Plain,
    Gradient(Color, Color),
}

pub(crate) fn panel(ui: &mut Ui, app: &mut EditorUi) {
    let t = ui.theme.clone();
    let Some(clip) = subject(app) else {
        ui.container(Layout::column().width(Size::Grow(1.0)).height(Size::Fixed(120.0)).align(Align::Center, Align::Center), Frame::none(), |ui| {
            ui.text_with("(no clip selected)", t.metrics.font_size, t.palette.text_faint);
        });
        return;
    };
    let effect = grade_of(&clip);
    let e = effect.as_deref();

    ui.container(Layout::row().width(Size::Grow(1.0)).height(Size::Fixed(22.0)).gap(6.0).align(Align::Start, Align::Center), Frame::none(), |ui| {
        ui.text_with(&clip.name, t.metrics.font_size, t.palette.text);
        ui.flex();
        if let Some(e) = e {
            let mut on = e.enabled;
            if ui.checkbox_keyed("grade-on", "On", &mut on).clicked {
                let mut c = (*clip).clone();
                if let Some(slot) = c.effects.iter_mut().find(|x| x.id == e.id) {
                    Arc::make_mut(slot).enabled = on;
                }
                app.run(Command::SetClip { clip: Arc::new(c) });
            }
            if ui.button_keyed("grade-reset", "Reset").clicked {
                app.run(Command::RemoveEffect { clip: clip.id, effect: e.id });
            }
        }
    });

    ui.section("Basic Correction");
    ui.text_with("White Balance", t.metrics.font_size_small, t.palette.text_muted);
    let sliders: [(&str, &str, f32, f32, f32, Track); 10] = [
        ("temperature", "Temperature", -100.0, 100.0, 0.0, Track::Gradient(Color::hex(0x3d6fd1), Color::hex(0xe0a33a))),
        ("tint", "Tint", -100.0, 100.0, 0.0, Track::Gradient(Color::hex(0x3fae4d), Color::hex(0xc24fc0))),
        ("exposure", "Exposure", -5.0, 5.0, 0.0, Track::Gradient(Color::hex(0x101010), Color::hex(0xe8e8e8))),
        ("contrast", "Contrast", -100.0, 100.0, 0.0, Track::Plain),
        ("highlights", "Highlights", -100.0, 100.0, 0.0, Track::Plain),
        ("shadows", "Shadows", -100.0, 100.0, 0.0, Track::Plain),
        ("whites", "Whites", -100.0, 100.0, 0.0, Track::Plain),
        ("blacks", "Blacks", -100.0, 100.0, 0.0, Track::Plain),
        ("saturation", "Saturation", 0.0, 200.0, 100.0, Track::Gradient(Color::hex(0x7a7a7a), Color::hex(0xd23c8c))),
        ("vibrance", "Vibrance", -100.0, 100.0, 0.0, Track::Gradient(Color::hex(0x7a7a7a), Color::hex(0x3cc8d2))),
    ];
    for (i, (id, label, min, max, default, track)) in sliders.into_iter().enumerate() {
        if i == 2 {
            ui.text_with("Tone", t.metrics.font_size_small, t.palette.text_muted);
        }
        if i == 9 {
            ui.section("Creative");
        }
        let Value::Float(v) = value(app, &clip, e, id) else { continue };
        if let Some((new, gesture)) = slider(ui, app, id, label, v as f32, min, max, default, track) {
            app.set_grade(&clip, id, Value::Float(new as f64), gesture);
        }
    }

    ui.section("Color Wheels");
    ui.container(Layout::row().width(Size::Grow(1.0)).height(Size::Fixed(176.0)).gap(10.0), Frame::none(), |ui| {
        for (id, level, label) in [("lift", "lift_level", "Shadows"), ("gamma", "gamma_level", "Midtones"), ("gain", "gain_level", "Highlights")] {
            ui.container(Layout::column().width(Size::Grow(1.0)).height(Size::Grow(1.0)).gap(4.0).align(Align::Center, Align::Start), Frame::none(), |ui| {
                ui.text_with(label, t.metrics.font_size_small, t.palette.text_muted);
                let Value::Vec2([x, y]) = value(app, &clip, e, id) else { return };
                if let Some((w, gesture)) = wheel(ui, app, id, [x as f32, y as f32]) {
                    app.set_grade(&clip, id, Value::Vec2([w[0] as f64, w[1] as f64]), gesture);
                }
                let Value::Float(l) = value(app, &clip, e, level) else { return };
                if let Some((new, gesture)) = slider(ui, app, level, "", l as f32, -1.0, 1.0, 0.0, Track::Gradient(Color::hex(0x101010), Color::hex(0xe8e8e8))) {
                    app.set_grade(&clip, level, Value::Float(new as f64), gesture);
                }
            });
        }
    });
    ui.space(6.0);
    ui.text_with("Drag to adjust; double-click a control to reset it.", t.metrics.font_size_small, t.palette.text_faint);
}

/// A labelled slider. Returns the new value and the drag's gesture while it
/// changes; double-click resets to `default`.
#[allow(clippy::too_many_arguments)]
fn slider(ui: &mut Ui, app: &mut EditorUi, key: &str, label: &str, value: f32, min: f32, max: f32, default: f32, track: Track) -> Option<(f32, u64)> {
    let t = ui.theme.clone();
    let mut out = None;
    ui.container(Layout::row().width(Size::Grow(1.0)).height(Size::Fixed(22.0)).gap(8.0).align(Align::Start, Align::Center), Frame::none(), |ui| {
        if !label.is_empty() {
            ui.container(Layout::row().width(Size::Fixed(84.0)).height(Size::Grow(1.0)).align(Align::Start, Align::Center), Frame::none(), |ui| {
                ui.text_with(label, t.metrics.font_size, t.palette.text);
            });
        }
        let id = ui.make_id(("grade-slider", key));
        let r = ui.interact_drag(id);
        // One control is dragged at a time: its drag's gesture number.
        if r.pressed {
            app.drag_gesture = app.next_gesture();
        }
        let gesture = app.drag_gesture;
        let span = (r.rect.w - 12.0).max(1.0);
        if r.double_clicked {
            out = Some((default, app.next_gesture()));
        } else if let Some((v0, d)) = crate::drag_from(&mut app.drag_start, &r, [value, 0.0]) {
            // Moves by how far it is dragged; a click alone changes nothing.
            let v = (v0[0] + d.x / span * (max - min)).clamp(min, max);
            // Snap to the default near it, so "none" is easy to find.
            let v = if ((v - default) / (max - min)).abs() < 0.01 { default } else { v };
            if v != value {
                out = Some((v, gesture));
            }
        }
        if r.hovered {
            ui.cursor = Cursor::ResizeHorizontal;
        }
        let shown = out.map_or(value, |o| o.0);
        let f = (shown - min) / (max - min);
        let d = (default - min) / (max - min);
        let hot = ui.animate_bool(id, 0, r.hovered || r.active);
        let accent = t.palette.accent;
        ui.add_leaf(id, Layout::leaf(Size::Grow(1.0), Size::Fixed(18.0)), Vec2::ZERO, true, move |p, rect| {
            let bar = Rect::new(rect.x + 6.0, rect.center().y - 2.0, rect.w - 12.0, 4.0);
            match track {
                Track::Plain => p.rect(bar, REEL.raised_hi, 2.0),
                Track::Gradient(a, b) => {
                    let n = 24;
                    for i in 0..n {
                        let x0 = bar.x + bar.w * i as f32 / n as f32;
                        p.rect(Rect::new(x0, bar.y, bar.w / n as f32 + 0.5, bar.h), a.lerp(b, (i as f32 + 0.5) / n as f32), 0.0);
                    }
                }
            }
            // The default's tick, and the knob.
            p.rect(Rect::new((bar.x + bar.w * d).round(), bar.y - 3.0, 1.0, bar.h + 6.0), REEL.label, 0.0);
            let x = bar.x + bar.w * f;
            p.rect_bordered(Rect::new(x - 5.0, rect.center().y - 5.0, 10.0, 10.0), REEL.bright, 5.0, 1.0 + hot, accent.with_alpha(hot));
        });
        let text = if (max - min) <= 10.0 { format!("{shown:.2}") } else { format!("{shown:.1}") };
        ui.container(Layout::row().width(Size::Fixed(44.0)).height(Size::Grow(1.0)).align(Align::End, Align::Center), Frame::none(), |ui| {
            ui.text_with(&text, t.metrics.font_size_small, REEL.timecode);
        });
    });
    out
}

/// A colour wheel: drag the puck towards a hue; the distance is how much.
/// Double-click recentres it.
fn wheel(ui: &mut Ui, app: &mut EditorUi, key: &str, value: [f32; 2]) -> Option<([f32; 2], u64)> {
    let id = ui.make_id(("grade-wheel", key));
    let r = ui.interact_drag(id);
    if r.pressed {
        app.drag_gesture = app.next_gesture();
    }
    let gesture = app.drag_gesture;
    let radius = (r.rect.w.min(r.rect.h) / 2.0 - 6.0).max(1.0);
    let mut out = None;
    if r.double_clicked {
        out = Some(([0.0, 0.0], app.next_gesture()));
    } else if let Some((v0, d)) = crate::drag_from(&mut app.drag_start, &r, value) {
        // Fine control: the puck moves at a quarter of the pointer's speed.
        let (mut x, mut y) = (v0[0] + d.x / radius * 0.25, v0[1] - d.y / radius * 0.25);
        let len = (x * x + y * y).sqrt();
        if len > 1.0 {
            (x, y) = (x / len, y / len);
        }
        out = Some(([x, y], gesture));
    }
    if r.hovered {
        ui.cursor = Cursor::Crosshair;
    }
    let shown = out.map_or(value, |o| o.0);
    ui.add_leaf(id, Layout::leaf(Size::Fixed(120.0), Size::Fixed(120.0)), Vec2::ZERO, true, move |p, rect| {
        let c = rect.center();
        let rad = rect.w.min(rect.h) / 2.0 - 6.0;
        // The hue ring, as the scope places hues (red up-left, blue right).
        let n = 72;
        for i in 0..n {
            let a0 = i as f32 / n as f32 * std::f32::consts::TAU;
            let a1 = (i + 1) as f32 / n as f32 * std::f32::consts::TAU;
            let (wx, wy) = (a0.cos(), -a0.sin());
            let rgb = [1.402 * wy, -0.344136 * wx - 0.714136 * wy, 1.772 * wx];
            let lo = rgb.iter().cloned().fold(f32::INFINITY, f32::min);
            let hi = rgb.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
            let col = Color::rgba((rgb[0] - lo) / (hi - lo), (rgb[1] - lo) / (hi - lo), (rgb[2] - lo) / (hi - lo), 1.0);
            p.line(Vec2::new(c.x + rad * a0.cos(), c.y + rad * a0.sin()), Vec2::new(c.x + rad * a1.cos(), c.y + rad * a1.sin()), 4.0, col);
        }
        p.rect(Rect::new(c.x - rad + 3.0, c.y - rad + 3.0, 2.0 * rad - 6.0, 2.0 * rad - 6.0), REEL.inset, rad - 3.0);
        p.line(Vec2::new(c.x - 6.0, c.y), Vec2::new(c.x + 6.0, c.y), 1.0, REEL.tick);
        p.line(Vec2::new(c.x, c.y - 6.0), Vec2::new(c.x, c.y + 6.0), 1.0, REEL.tick);
        let puck = Vec2::new(c.x + shown[0] * (rad - 6.0), c.y - shown[1] * (rad - 6.0));
        p.rect_bordered(Rect::new(puck.x - 5.0, puck.y - 5.0, 10.0, 10.0), Color::TRANSPARENT, 5.0, 2.0, REEL.bright);
    });
    out
}
