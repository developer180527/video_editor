//! The keyframe graph editor: an animated number's curve under its row in
//! Effect Controls, on the same time scale as the keyframe lanes.
//!
//! The top is the value graph: the curve, its keys (drag to move them in
//! time and value) and, for the selected key, the Bézier handles of the
//! segments either side of it (drag to shape the ease; a handle turns its
//! segment into a curve). Underneath, the velocity graph: how fast the
//! value changes, which is what makes an ease look smooth or not.
//!
//! Double-click the curve to add a key there; right-click a key for its
//! interpolation (Linear, Ease, Hold) or to delete it. Every drag changes
//! the picture as it moves and is one undo step.

use std::sync::Arc;

use libgui::*;
use ve_engine::Command;
use ve_model::*;
use ve_time::Time;

use crate::effects::Span;
use crate::theme::REEL;
use crate::{drag_from, EditorUi};

/// The graph row's height: the value graph and, under it, the velocity.
pub(crate) const HEIGHT: f32 = 170.0;
const VELOCITY_H: f32 = 42.0;
const PAD: f32 = 10.0;
/// How close (px) the pointer must be to pick a key or a handle.
const PICK: f32 = 7.0;

/// What a graph drag holds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Grab {
    Key(usize),
    /// The handle leaving key `i` (segment i → i+1).
    Out(usize),
    /// The handle arriving at key `i + 1` (segment i → i+1).
    In(usize),
}

/// The selected key of a graph: which effect parameter, which key.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct GraphKey {
    pub effect: EffectId,
    pub param: String,
    pub index: usize,
}

/// A key as the graph draws it: absolute time (s), value, interpolation.
#[derive(Clone, Copy)]
struct K {
    t: f64,
    v: f64,
    interp: Interp,
}

fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Float(f) => Some(*f),
        Value::Int(i) => Some(*i as f64),
        _ => None,
    }
}

/// Maps between the lane and (time, value).
#[derive(Clone, Copy)]
struct Axes {
    span: Span,
    lo: f64,
    hi: f64,
    rect: Rect,
}

impl Axes {
    fn value_rect(&self) -> Rect {
        Rect::new(self.rect.x, self.rect.y + PAD, self.rect.w, (self.rect.h - VELOCITY_H - 2.0 * PAD).max(10.0))
    }
    fn x(&self, t: f64) -> f32 {
        self.span.x(self.rect, t)
    }
    fn t(&self, x: f32) -> f64 {
        self.span.origin + ((x - self.rect.x) / self.rect.w.max(1.0)) as f64 * self.span.span
    }
    fn y(&self, v: f64) -> f32 {
        let r = self.value_rect();
        r.bottom() - ((v - self.lo) / (self.hi - self.lo)) as f32 * r.h
    }
    fn v(&self, y: f32) -> f64 {
        let r = self.value_rect();
        self.lo + ((r.bottom() - y) / r.h) as f64 * (self.hi - self.lo)
    }
}

/// The handle points of segment `i` (between keys i and i+1), when it is a
/// curve or can be shown as one.
fn handles(keys: &[K], i: usize) -> Option<((f64, f64), (f64, f64))> {
    let (a, b) = (keys.get(i)?, keys.get(i + 1)?);
    let (x1, y1, x2, y2) = a.interp.as_bezier()?;
    let (dt, dv) = (b.t - a.t, b.v - a.v);
    Some(((a.t + x1 as f64 * dt, a.v + y1 as f64 * dv), (a.t + x2 as f64 * dt, a.v + y2 as f64 * dv)))
}

/// The value at absolute time `t`, as the renderer will compute it.
fn sample(param: &Param, clip_start: f64, t: f64) -> f64 {
    number(&param.value_at(Time::from_seconds_f64((t - clip_start).max(0.0)))).unwrap_or(0.0)
}

impl EditorUi {
    /// Replace the parameter's keys, as part of drag `gesture` (one undo
    /// step per drag) or on their own.
    fn set_keys(&mut self, clip: ClipId, effect: EffectId, param: &str, keys: Vec<Keyframe>, gesture: Option<u64>) {
        let cmd = Command::SetEffectParam { clip, effect, param: param.into(), value: Some(Param::Animated(keys)) };
        match gesture {
            Some(g) => self.engine.execute_merging(cmd, g),
            None => self.run(cmd),
        }
    }
}

/// The graph lane for `param` of `effect` on `clip`. Returns where to move
/// the playhead, if a click asked for it.
pub(crate) fn lane(ui: &mut Ui, app: &mut EditorUi, clip: &Arc<Clip>, effect: &Arc<Effect>, param: &str, stored: &Param, span: Span) -> Option<Time> {
    let Param::Animated(model) = stored else { return None };
    let keys: Vec<K> = model.iter().filter_map(|k| Some(K { t: span.clip_start + k.time.as_seconds_f64(), v: number(&k.value)?, interp: k.interp })).collect();
    if keys.len() != model.len() {
        return None; // not a number
    }
    let int = matches!(model[0].value, Value::Int(_));
    let rate = app.rate();
    let id = ui.make_id(("graph", effect.id, param));
    let r = ui.interact_drag(id);
    app.graph_rect = Some(r.rect);

    // The value range: the curve over the lane, the keys and the handles.
    let n = 160;
    let samples: Vec<f64> = (0..=n).map(|i| sample(stored, span.clip_start, span.origin + span.span * i as f64 / n as f64)).collect();
    let mut lo = samples.iter().chain(keys.iter().map(|k| &k.v)).cloned().fold(f64::INFINITY, f64::min);
    let mut hi = samples.iter().chain(keys.iter().map(|k| &k.v)).cloned().fold(f64::NEG_INFINITY, f64::max);
    for i in 0..keys.len().saturating_sub(1) {
        if let Some((a, b)) = handles(&keys, i) {
            lo = lo.min(a.1).min(b.1);
            hi = hi.max(a.1).max(b.1);
        }
    }
    if hi - lo < 1e-9 {
        (lo, hi) = (lo - 1.0, hi + 1.0);
    }
    let pad = (hi - lo) * 0.08;
    // The scale fits the curve, except during a drag: re-fitting under the
    // pointer would move what it holds, and the next frame further still.
    let (lo, hi) = match (app.view.graph_drag, app.view.graph_scale) {
        (Some(_), Some(frozen)) if r.active => frozen,
        _ => (lo - pad, hi + pad),
    };
    let axes = Axes { span, lo, hi, rect: r.rect };

    let selected = app.view.graph_key.as_ref().filter(|g| g.effect == effect.id && g.param == param).map(|g| g.index).filter(|i| *i < keys.len());

    // What the pointer is over: the selected key's handles first, then keys.
    let near = |p: Vec2, q: (f64, f64)| {
        let (x, y) = (axes.x(q.0), axes.y(q.1));
        (p.x - x).abs() <= PICK && (p.y - y).abs() <= PICK
    };
    let hit = |p: Vec2| -> Option<Grab> {
        if let Some(s) = selected {
            if let Some((out, _)) = handles(&keys, s) {
                if near(p, out) {
                    return Some(Grab::Out(s));
                }
            }
            if let Some(prev) = s.checked_sub(1) {
                if let Some((_, inn)) = handles(&keys, prev) {
                    if near(p, inn) {
                        return Some(Grab::In(prev));
                    }
                }
            }
        }
        keys.iter().position(|k| near(p, (k.t, k.v))).map(Grab::Key)
    };

    let mut seek = None;
    if r.pressed {
        let grab = hit(r.mouse_pos);
        app.view.graph_drag = grab;
        app.view.graph_scale = Some((axes.lo, axes.hi));
        app.drag_gesture = app.next_gesture();
        match grab {
            Some(Grab::Key(i)) => app.view.graph_key = Some(GraphKey { effect: effect.id, param: param.into(), index: i }),
            Some(_) => {}
            // Empty graph: move the playhead there, as a lane does.
            None => seek = Some(Time::from_seconds_f64(axes.t(r.mouse_pos.x).max(0.0))),
        }
    }
    // Dragging: past the dead zone, the grabbed thing follows the pointer.
    let moved = drag_from(&mut app.drag_start, &r, [0.0, 0.0]).is_some();
    if let (true, Some(grab)) = (moved && r.active, app.view.graph_drag) {
        let mut next = model.clone();
        let t_at = |x: f32| axes.t(x) - span.clip_start;
        match grab {
            Grab::Key(i) if i < next.len() => {
                // In time between its neighbours (a frame apart), on a frame.
                let frame = rate.frame_to_time(1);
                let lo = if i > 0 { next[i - 1].time + frame } else { Time::ZERO };
                let hi = next.get(i + 1).map_or(Time::MAX, |k| k.time - frame);
                if lo <= hi {
                    next[i].time = Time::from_seconds_f64(t_at(r.mouse_pos.x).max(0.0)).round_to_frame(rate).clamp_to(lo, hi);
                }
                let v = axes.v(r.mouse_pos.y);
                next[i].value = if int { Value::Int(v.round() as i64) } else { Value::Float(v) };
            }
            Grab::Out(i) | Grab::In(i) if i + 1 < next.len() => {
                let (a, b) = (keys[i], keys[i + 1]);
                let (x1, y1, x2, y2) = a.interp.as_bezier().unwrap_or((0.33, 0.0, 0.67, 1.0));
                let fx = ((axes.t(r.mouse_pos.x) - a.t) / (b.t - a.t)).clamp(0.0, 1.0) as f32;
                // A flat segment has no height to shape: its handles slide only.
                let dv = b.v - a.v;
                let fy = |old: f32| if dv.abs() < 1e-9 { old } else { ((axes.v(r.mouse_pos.y) - a.v) / dv) as f32 };
                next[i].interp = match grab {
                    Grab::Out(_) => Interp::Bezier { x1: fx, y1: fy(y1), x2, y2 },
                    _ => Interp::Bezier { x1, y1, x2: fx, y2: fy(y2) },
                };
            }
            _ => {}
        }
        if next != *model {
            app.set_keys(clip.id, effect.id, param, next, Some(app.drag_gesture));
        }
    }
    if r.released {
        app.view.graph_drag = None;
        app.view.graph_scale = None;
    }
    // Double-click on the curve: a key there, keeping the curve's shape.
    if r.double_clicked && hit(r.mouse_pos).is_none() {
        let t = Time::from_seconds_f64((axes.t(r.mouse_pos.x) - span.clip_start).max(0.0)).round_to_frame(rate);
        if !model.iter().any(|k| k.time == t) {
            let mut next = model.clone();
            let i = next.partition_point(|k| k.time < t);
            let interp = i.checked_sub(1).map_or(Interp::Linear, |p| next[p].interp);
            next.insert(i, Keyframe { time: t, value: stored.value_at(t), interp });
            app.view.graph_key = Some(GraphKey { effect: effect.id, param: param.into(), index: i });
            app.set_keys(clip.id, effect.id, param, next, None);
        }
    }
    // Right-click a key: its interpolation, or delete it.
    if r.secondary_pressed {
        if let Some(Grab::Key(i)) = hit(r.mouse_pos) {
            app.view.graph_key = Some(GraphKey { effect: effect.id, param: param.into(), index: i });
        }
    }
    let menu_key = app.view.graph_key.as_ref().filter(|g| g.effect == effect.id && g.param == param).map(|g| g.index).filter(|i| *i < keys.len());
    if let Some(i) = menu_key {
        let choice = ui.context_menu(&r, |ui| {
            let mut c = None;
            for (label, interp) in [("Linear", Interp::Linear), ("Ease In and Out", Interp::EASE_IN_OUT), ("Ease In", Interp::EASE_IN), ("Ease Out", Interp::EASE_OUT), ("Hold", Interp::Hold)] {
                if ui.menu_item_ex(label, None, i + 1 < model.len() || label == "Hold").clicked {
                    c = Some(Some(interp));
                }
            }
            ui.menu_separator();
            if ui.menu_item_ex("Delete Keyframe", None, model.len() > 1).clicked {
                c = Some(None);
            }
            c
        });
        match choice.flatten() {
            Some(Some(interp)) => {
                let mut next = model.clone();
                next[i].interp = interp;
                app.set_keys(clip.id, effect.id, param, next, None);
            }
            Some(None) => delete_key(app, clip.id, effect.id, param, model, i),
            None => {}
        }
    }
    if r.hovered {
        ui.cursor = match hit(r.mouse_pos) {
            Some(Grab::Key(_)) => Cursor::Grab,
            Some(_) => Cursor::Crosshair,
            None => Cursor::Default,
        };
    }

    // ---- paint ---------------------------------------------------------------
    let size = ui.theme.metrics.font_size_small;
    let accent = ui.theme.palette.accent;
    let labels = [ui.frame_text(&fmt(axes.hi)), ui.frame_text(&fmt((axes.hi + axes.lo) / 2.0)), ui.frame_text(&fmt(axes.lo))];
    let vel_label = ui.frame_text("Velocity");
    let stored = stored.clone();
    let clip_start = span.clip_start;
    ui.add_leaf(id, Layout::leaf(Size::Grow(1.0), Size::Grow(1.0)), Vec2::ZERO, true, move |p, rect| {
        let axes = Axes { rect, ..axes };
        let vr = axes.value_rect();
        p.rect(rect, REEL.inset, 0.0);
        // The clip's extent, and the value grid with its scale.
        let (cx0, cx1) = (axes.x(span.clip_start), axes.x(span.clip_start + span.clip_len));
        p.rect(Rect::new(cx0, rect.y, (cx1 - cx0).max(0.0), rect.h), REEL.chrome, 0.0);
        for (k, label) in labels.iter().enumerate() {
            let y = (vr.y + vr.h * k as f32 / 2.0).round();
            p.rect(Rect::new(rect.x, y, rect.w, 1.0), REEL.line, 0.0);
            p.text_left(Rect::new(rect.x + 4.0, y - 13.0, 80.0, 12.0), size - 1.0, REEL.label, *label);
        }
        // The curve, a point per two pixels, as the renderer computes it.
        let step = 2.0;
        let mut prev: Option<Vec2> = None;
        let mut x = rect.x;
        let mut speeds = Vec::new();
        while x <= rect.right() {
            let t = axes.t(x);
            let v = sample(&stored, clip_start, t);
            let q = Vec2::new(x, axes.y(v));
            if let Some(a) = prev {
                p.line(a, q, 1.5, REEL.timecode);
            }
            let dt = (axes.t(x + step) - t).max(1e-6);
            speeds.push(((sample(&stored, clip_start, t + dt) - v) / dt).abs());
            prev = Some(q);
            x += step;
        }
        // Handles of the selected key's segments.
        if let Some(s) = selected {
            for (i, which) in [(s, 0), (s.wrapping_sub(1), 1)] {
                if let Some((out, inn)) = handles(&keys, i) {
                    let (anchor, h) = if which == 0 { ((keys[i].t, keys[i].v), out) } else { ((keys[i + 1].t, keys[i + 1].v), inn) };
                    let (a, b) = (Vec2::new(axes.x(anchor.0), axes.y(anchor.1)), Vec2::new(axes.x(h.0), axes.y(h.1)));
                    p.line(a, b, 1.0, REEL.text_soft.with_alpha(0.7));
                    p.rect_bordered(Rect::new(b.x - 4.0, b.y - 4.0, 8.0, 8.0), REEL.inset, 4.0, 1.5, REEL.bright);
                }
            }
        }
        // Keys: diamonds, the selected one lit.
        for (i, k) in keys.iter().enumerate() {
            let c = Vec2::new(axes.x(k.t), axes.y(k.v));
            let on = selected == Some(i);
            let fill = if on { accent } else { REEL.bright };
            p.draw.triangle(Vec2::new(c.x - 5.0, c.y), Vec2::new(c.x, c.y - 5.0), Vec2::new(c.x + 5.0, c.y), 0b011, fill);
            p.draw.triangle(Vec2::new(c.x - 5.0, c.y), Vec2::new(c.x + 5.0, c.y), Vec2::new(c.x, c.y + 5.0), 0b110, fill);
        }
        // Velocity, under a rule: how fast the value changes.
        let band = Rect::new(rect.x, rect.bottom() - VELOCITY_H, rect.w, VELOCITY_H - 4.0);
        p.rect(Rect::new(rect.x, band.y - 2.0, rect.w, 1.0), REEL.line, 0.0);
        p.text_left(Rect::new(rect.x + 4.0, band.y, 80.0, 12.0), size - 1.0, REEL.label, vel_label);
        let top = speeds.iter().cloned().fold(0.0, f64::max).max(1e-9);
        let mut prev: Option<Vec2> = None;
        for (i, s) in speeds.iter().enumerate() {
            let q = Vec2::new(rect.x + i as f32 * step, band.bottom() - (s / top) as f32 * (band.h - 12.0));
            if let Some(a) = prev {
                p.line(a, q, 1.0, REEL.meter_mid.with_alpha(0.85));
            }
            prev = Some(q);
        }
        // The playhead.
        let x = axes.x(span.playhead).round();
        if x >= rect.x && x <= rect.right() {
            p.rect(Rect::new(x - 0.5, rect.y, 1.0, rect.h), REEL.playhead, 0.0);
        }
    });
    seek
}

/// Remove key `i` (never the last one: an animated parameter keeps a key).
pub(crate) fn delete_key(app: &mut EditorUi, clip: ClipId, effect: EffectId, param: &str, keys: &[Keyframe], i: usize) {
    if keys.len() < 2 || i >= keys.len() {
        return;
    }
    let mut next = keys.to_vec();
    next.remove(i);
    app.view.graph_key = None;
    app.set_keys(clip, effect, param, next, None);
}

fn fmt(v: f64) -> String {
    if v.abs() >= 100.0 { format!("{v:.0}") } else if v.abs() >= 10.0 { format!("{v:.1}") } else { format!("{v:.2}") }
}

/// The selected graph key's interpolation and position, for the row's
/// left side: its time, value and a way to change how it leaves.
pub(crate) fn key_panel(ui: &mut Ui, app: &mut EditorUi, clip: &Arc<Clip>, effect: &Arc<Effect>, param: &str, stored: &Param) {
    let t = ui.theme.clone();
    let Param::Animated(keys) = stored else { return };
    let col = Layout::column().width(Size::Grow(1.0)).height(Size::Grow(1.0)).padding(Insets::xy(28.0, 8.0)).gap(6.0);
    ui.container(col, Frame::none(), |ui| {
        let selected = app.view.graph_key.as_ref().filter(|g| g.effect == effect.id && g.param == param).map(|g| g.index).filter(|i| *i < keys.len());
        let Some(i) = selected else {
            ui.text_with("Click a keyframe to edit it; double-click the curve to add one.", t.metrics.font_size_small, t.palette.text_faint);
            return;
        };
        let k = &keys[i];
        let value = number(&k.value).map(fmt).unwrap_or_default();
        ui.text_with(&format!("Keyframe {} · {} · {value}", i + 1, app.timecode(clip.timeline_start + k.time)), t.metrics.font_size_small, t.palette.text);
        if i + 1 < keys.len() {
            let names = ["Linear", "Bézier", "Hold"];
            let current = match k.interp {
                Interp::Linear => 0,
                Interp::Hold => 2,
                _ => 1,
            };
            let mut choice = current;
            ui.container(Layout::row().width(Size::Fixed(200.0)).height(Size::Fixed(20.0)), Frame::none(), |ui| {
                ui.segmented(&format!("interp-{}-{param}", effect.id), &mut choice, &names);
            });
            if choice != current {
                let mut next = keys.clone();
                next[i].interp = [Interp::Linear, Interp::EASE_IN_OUT, Interp::Hold][choice];
                app.set_keys(clip.id, effect.id, param, next, None);
            }
        }
        if keys.len() > 1 && ui.button_keyed(("del-key", effect.id, param), "Delete Keyframe").clicked {
            delete_key(app, clip.id, effect.id, param, keys, i);
        }
    });
}
