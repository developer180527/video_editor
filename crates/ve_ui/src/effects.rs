//! Effect Controls: the selected clip's effects as a property tree, each row
//! paired with its keyframe lane on the right so the two always line up.
//!
//! Everything here is generated from the effects' parameter descriptions
//! (`ParamInfo`), whether the effect is intrinsic (Motion, Opacity) or a
//! plugin. Dragging a value previews it locally; releasing commits one
//! `SetEffectParam`, so a drag is one undo step.

use std::sync::Arc;

use libgui::*;
use ve_engine::Command;
use ve_model::*;
use ve_engine::{EffectInfo, Implementation, ParamInfo, ParamKind};
use ve_time::{Time, Timecode};

use crate::theme::REEL;
use crate::widgets::{draw_icon, icon_button, prop_row, value, Icon, Prop};
use crate::{EditorUi, ParamEdit};

/// The clip's slice of the timeline the lanes show.
#[derive(Clone, Copy)]
struct Span {
    origin: f64,
    span: f64,
    clip_start: f64,
    clip_len: f64,
    playhead: f64,
}

impl Span {
    fn x(&self, r: Rect, t: f64) -> f32 {
        r.x + r.w * ((t - self.origin) / self.span) as f32
    }
}

pub fn panel(ui: &mut Ui, app: &mut EditorUi) {
    let t = ui.theme.clone();
    let col = Layout::column().width(Size::Grow(1.0)).height(Size::Grow(1.0));
    let clip = app
        .view
        .selection
        .first()
        .and_then(|id| app.snap().find_clip(*id))
        .map(|(s, ti, c)| (c.clone(), s.tracks[ti].kind == TrackKind::Video));
    ui.container(col, Frame { fill: t.palette.bg_panel, clip: true, ..Frame::none() }, |ui| {
        chips(ui, app, clip.as_ref().map(|(c, _)| &**c));
        match &clip {
            Some((c, video)) => tree(ui, app, c, *video),
            None => {
                ui.container(
                    Layout::column().width(Size::Grow(1.0)).height(Size::Grow(1.0)).align(Align::Center, Align::Center),
                    Frame::none(),
                    |ui| ui.text_with("(no clip selected)", t.metrics.font_size, t.palette.text_faint),
                );
            }
        }
        footer(ui, app);
    });
}

fn chips(ui: &mut Ui, app: &EditorUi, clip: Option<&Clip>) {
    let row = Layout::row()
        .width(Size::Grow(1.0))
        .height(Size::Fixed(30.0))
        .padding(Insets::xy(8.0, 0.0))
        .gap(6.0)
        .align(Align::Start, Align::Center);
    let name = clip.map(|c| c.name.replace(" [V]", "").replace(" [A]", "")).unwrap_or_else(|| "(no clip)".into());
    let seq = app.snap().active().map(|s| s.name.clone()).unwrap_or_default();
    ui.container(row, Frame { fill: Color::hex(0x1f1f1f), ..Frame::none() }, |ui| {
        chip(ui, "src", Icon::Panel, &format!("Source • {name}"), false);
        chip(ui, "seq", Icon::Effects, &format!("{seq} • {name}"), true);
        ui.flex();
        let _ = icon_button(ui, "split", Icon::Panel, 18.0, false);
    });
}

fn chip(ui: &mut Ui, key: &str, icon: Icon, label: &str, on: bool) {
    let t = ui.theme.clone();
    let id = ui.make_id(("chip", key));
    let r = ui.interact(id);
    let hot = ui.animate_bool(id, 0, r.hovered);
    let size = t.metrics.font_size;
    let w = ui.fonts.measure(ui.font, size, label).x + 34.0;
    let text = ui.frame_text(label);
    ui.add_leaf(id, Layout::leaf(Size::Fixed(w), Size::Fixed(22.0)), Vec2::ZERO, true, move |p, rect| {
        let fill = if on { Color::hex(0x2f2f2f) } else { Color::hex(0x272727) };
        p.rect_bordered(rect, fill.lerp(Color::hex(0x3a3a3a), hot), 3.0, 1.0, Color::hex(0x161616));
        let ic = Rect::new(rect.x + 4.0, rect.center().y - 7.0, 14.0, 14.0);
        draw_icon(p, ic, icon, if on { t.palette.accent } else { t.palette.text_faint });
        p.text_left(rect.shrink(22.0, 0.0, 6.0, 0.0), size, if on { t.palette.text } else { t.palette.text_muted }, text);
    });
}

/// One row: the tree part left, the lane right.
fn row<R, L>(ui: &mut Ui, key: impl std::hash::Hash, height: f32, left: impl FnOnce(&mut Ui) -> R, lane: impl FnOnce(&mut Ui) -> L) -> (R, L) {
    let id = ui.make_id(("fxrow", key));
    ui.container_id(id, Layout::row().width(Size::Grow(1.0)).height(Size::Fixed(height)), Frame::none(), |ui| {
        let out = ui.container(Layout::row().width(Size::Grow(1.5)).height(Size::Grow(1.0)).align(Align::Start, Align::Center), Frame::none(), left);
        let l = ui.container(Layout::row().width(Size::Grow(1.0)).height(Size::Grow(1.0)), Frame::none(), lane);
        (out, l)
    })
}

fn tree(ui: &mut Ui, app: &mut EditorUi, clip: &Arc<Clip>, video: bool) {
    let start = clip.timeline_start.as_seconds_f64();
    let len = clip.source_range.duration.as_seconds_f64();
    let span = Span {
        origin: (start - len * 0.3).max(0.0),
        span: (len * 1.6).max(0.5),
        clip_start: start,
        clip_len: len,
        playhead: app.playhead.as_seconds_f64(),
    };
    let clip_time = (app.playhead - clip.timeline_start).clamp_to(Time::ZERO, clip.source_range.duration);
    let registry = app.st.plugins.clone();
    let opts = ScrollOptions { gap: 0.0, ..ScrollOptions::new(Size::Grow(1.0)) };
    let name = clip.name.clone();
    let rate = app.rate();
    let mut seek: Option<Time> = None;
    ui.scroll_area_with("fx", opts, |ui| {
        let (_, s) = row(ui, "head", 42.0, |ui| section(ui, if video { "Video" } else { "Audio" }), |ui| ruler_lane(ui, rate, span, &name));
        seek = seek.or(s);
        for e in &clip.effects {
            let info = registry.find(&e.plugin);
            let label = info.map(|i| i.name.clone()).unwrap_or_else(|| format!("{} (missing)", e.plugin.id));
            let intrinsic = info.is_some_and(|i| matches!(i.implementation, Implementation::Intrinsic));
            let mut open = !app.view.closed.contains(&e.id);
            let (_, s) = row(ui, ("g", e.id), 20.0, |ui| group(ui, app, clip, e, &label, intrinsic, &mut open, info), |ui| lane(ui, span, None));
            seek = seek.or(s);
            if open {
                app.view.closed.remove(&e.id);
            } else {
                app.view.closed.insert(e.id);
                continue;
            }
            let Some(info) = info else { continue };
            for p in &info.params {
                seek = seek.or(param_row(ui, app, clip, e, p, clip_time, span));
            }
        }
    });
    if let Some(t) = seek {
        app.seek(t.round_to_frame(rate));
    }
}

fn section(ui: &mut Ui, label: &str) {
    let t = ui.theme.clone();
    let id = ui.make_id(("section", label));
    let text = ui.frame_text(label);
    let size = t.metrics.font_size;
    ui.add_leaf(id, Layout::leaf(Size::Grow(1.0), Size::Grow(1.0)), Vec2::ZERO, false, move |p, r| {
        let strip = Rect::new(r.x, r.bottom() - 20.0, r.w, 20.0);
        p.rect(strip, Color::hex(0x272727), 0.0);
        p.text_left(strip.shrink(8.0, 0.0, 0.0, 0.0), size, t.palette.text, text);
    });
}

/// An effect's header: twirl, the `fx` badge (click to bypass), name, reset,
/// and remove for plugin effects.
#[allow(clippy::too_many_arguments)]
fn group(ui: &mut Ui, app: &mut EditorUi, clip: &Arc<Clip>, e: &Arc<Effect>, label: &str, intrinsic: bool, open: &mut bool, info: Option<&EffectInfo>) {
    let t = ui.theme.clone();
    let row = Layout::row()
        .width(Size::Grow(1.0))
        .height(Size::Fixed(20.0))
        .padding(Insets::xy(4.0, 0.0))
        .gap(4.0)
        .align(Align::Start, Align::Center);
    ui.container(row, Frame { fill: Color::hex(0x202020), ..Frame::none() }, |ui| {
        let r = icon_button(ui, ("tw", e.id), if *open { Icon::Chevron } else { Icon::ChevronRight }, 14.0, false);
        if r.clicked {
            *open = !*open;
        }
        let fx = icon_button(ui, ("fx", e.id), Icon::Effects, 16.0, !e.enabled);
        ui.tooltip(&fx, if e.enabled { "Bypass effect" } else { "Enable effect" });
        if fx.clicked {
            let mut c = (**clip).clone();
            if let Some(slot) = c.effects.iter_mut().find(|x| x.id == e.id) {
                let mut ne = (**slot).clone();
                ne.enabled = !ne.enabled;
                *slot = Arc::new(ne);
            }
            app.run(Command::SetClip { clip: Arc::new(c) });
        }
        let ink = if e.enabled { t.palette.text } else { t.palette.text_faint };
        ui.text_with(label, t.metrics.font_size, ink);
        ui.flex();
        if !intrinsic {
            let rm = icon_button(ui, ("rm", e.id), Icon::Trash, 14.0, false);
            ui.tooltip(&rm, "Remove effect");
            if rm.clicked {
                app.run(Command::RemoveEffect { clip: clip.id, effect: e.id });
            }
        }
        let rst = icon_button(ui, ("rst", e.id), Icon::Reset, 16.0, false);
        ui.tooltip(&rst, "Reset all parameters");
        if rst.clicked {
            if let Some(info) = info {
                let commands = info
                    .params
                    .iter()
                    .map(|p| Command::SetEffectParam {
                        clip: clip.id,
                        effect: e.id,
                        param: p.id.clone(),
                        value: Some(Param::Constant(default_for(app, clip, p))),
                    })
                    .collect();
                app.run(Command::Batch { label: format!("Reset {label}"), commands });
            }
        }
    });
}

/// The default a parameter resets to. Position and anchor depend on the frame
/// sizes, as when the clip was made.
fn default_for(app: &EditorUi, clip: &Clip, p: &ParamInfo) -> Value {
    let fmt = app.snap().active().map(|s| s.format.clone()).unwrap_or_default();
    let src = match &clip.source {
        ClipSource::Asset { asset, .. } => app.snap().assets.get(asset).and_then(|a| a.info.as_ref()).and_then(|i| i.video.clone()),
        _ => None,
    };
    let d = match p.id.as_str() {
        "position" => [fmt.width as f64 / 2.0, fmt.height as f64 / 2.0, 0.0, 0.0],
        "anchor" => {
            let (w, h) = src.map(|v| (v.width as f64, v.height as f64)).unwrap_or((fmt.width as f64, fmt.height as f64));
            [w / 2.0, h / 2.0, 0.0, 0.0]
        }
        _ => p.default,
    };
    ve_engine::default_value(&p.kind, d)
}

/// `param` with `v` set at `t`: replaces a constant, or sets a keyframe at
/// `t` when the parameter is animated.
fn set_at(param: &Param, t: Time, v: Value) -> Param {
    match param {
        Param::Constant(_) => Param::Constant(v),
        Param::Animated(keys) => {
            let mut keys = keys.clone();
            match keys.iter_mut().find(|k| k.time == t) {
                Some(k) => k.value = v,
                None => {
                    let i = keys.partition_point(|k| k.time < t);
                    keys.insert(i, Keyframe { time: t, value: v, interp: Interp::Linear });
                }
            }
            Param::Animated(keys)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn param_row(ui: &mut Ui, app: &mut EditorUi, clip: &Arc<Clip>, e: &Arc<Effect>, p: &ParamInfo, t: Time, span: Span) -> Option<Time> {
    let stored = e.params.get(&p.id).cloned().unwrap_or_else(|| Param::Constant(ve_engine::default_value(&p.kind, p.default)));
    let animated = matches!(stored, Param::Animated(_));
    let editing = app.view.param_edit.as_ref().filter(|pe| pe.clip == clip.id && pe.effect == e.id && pe.param == p.id).map(|pe| pe.value.clone());
    let current = editing.unwrap_or_else(|| stored.value_at(t));
    let keys: Vec<f64> = match &stored {
        Param::Animated(k) => k.iter().map(|k| span.clip_start + k.time.as_seconds_f64()).collect(),
        _ => vec![],
    };
    let mut commit: Option<Value> = None;
    let mut preview: Option<Value> = None;
    let mut cancel_preview = false;
    let mut prop = Prop::new(&p.label).dim(!e.enabled);
    if p.animatable {
        prop = prop.stopwatch(animated);
    }
    let key = (e.id, p.id.as_str());
    let (resp, seek) = row(
        ui,
        ("p", e.id, &p.id),
        20.0,
        |ui| {
            prop_row(ui, key, prop, |ui| {
                let speed = if p.max.is_finite() && p.min.is_finite() { ((p.max - p.min) / 400.0).max(0.01) } else { 0.5 };
                let decimals = if p.max.is_finite() && p.max - p.min <= 1.0 { 2 } else { 1 };
                let drag = |ui: &mut Ui, k: usize, v: &mut f64, suffix: &str| -> (bool, bool) {
                    let r = value(ui, (key, k), v, speed, (p.min, p.max), decimals, suffix);
                    (r.active, r.released)
                };
                match (&p.kind, current.clone()) {
                    (ParamKind::Float, Value::Float(mut v)) => {
                        let suffix = match p.id.as_str() {
                            "opacity" | "crop_left" | "crop_top" | "crop_right" | "crop_bottom" => "%",
                            "level" => "dB",
                            _ => "",
                        };
                        let (active, released) = drag(ui, 0, &mut v, suffix);
                        if active {
                            preview = Some(Value::Float(v));
                        }
                        if released {
                            commit = Some(Value::Float(v));
                        }
                    }
                    (ParamKind::Int, Value::Int(v)) => {
                        let mut f = v as f64;
                        let (active, released) = drag(ui, 0, &mut f, "");
                        if active {
                            preview = Some(Value::Int(f.round() as i64));
                        }
                        if released {
                            commit = Some(Value::Int(f.round() as i64));
                        }
                    }
                    (ParamKind::Vec2, Value::Vec2([mut x, mut y])) => {
                        let (ax, rx) = drag(ui, 0, &mut x, "");
                        let (ay, ry) = drag(ui, 1, &mut y, "");
                        if ax || ay {
                            preview = Some(Value::Vec2([x, y]));
                        }
                        if rx || ry {
                            commit = Some(Value::Vec2([x, y]));
                        }
                    }
                    (ParamKind::Bool, Value::Bool(mut b)) => {
                        if ui.checkbox_keyed(key, "", &mut b).clicked {
                            commit = Some(Value::Bool(b));
                        }
                    }
                    (ParamKind::Choice(options), Value::Choice(c)) => {
                        let opts: Vec<&str> = options.iter().map(String::as_str).collect();
                        let mut sel = c as usize;
                        ui.container(Layout::row().width(Size::Fixed(110.0)).height(Size::Fixed(20.0)), Frame::none(), |ui| {
                            ui.combo(&format!("{}-{}", e.id, p.id), &mut sel, &opts);
                            if sel != c as usize {
                                commit = Some(Value::Choice(sel as u32));
                            }
                        });
                    }
                    (ParamKind::Color, Value::Color(c)) => {
                        let mut col = Color::rgba(c[0], c[1], c[2], c[3]);
                        let r = ui.color_button(&format!("{}-{}", e.id, p.id), &mut col);
                        if r.changed {
                            preview = Some(Value::Color([col.r, col.g, col.b, col.a]));
                        }
                        if r.finished {
                            commit = Some(Value::Color([col.r, col.g, col.b, col.a]));
                        }
                    }
                    (_, other) => {
                        ui.text_with(&format!("{other:?}"), 11.0, Color::hex(0x8a8a8a));
                    }
                }
            })
        },
        |ui| lane(ui, span, Some(&keys)),
    );
    if let Some(v) = preview {
        app.view.param_edit = Some(ParamEdit { clip: clip.id, effect: e.id, param: p.id.clone(), value: v });
    }
    let set = |value: Param| Command::SetEffectParam { clip: clip.id, effect: e.id, param: p.id.clone(), value: Some(value) };
    if let Some(v) = commit {
        cancel_preview = true;
        app.run(set(set_at(&stored, t, v)));
    }
    if resp.stopwatch_clicked {
        // Constant → one keyframe here; animated → the value here, held.
        let next = match &stored {
            Param::Constant(v) => Param::Animated(vec![Keyframe { time: t, value: v.clone(), interp: Interp::Linear }]),
            Param::Animated(_) => Param::Constant(stored.value_at(t)),
        };
        app.run(set(next));
    }
    if resp.reset_clicked {
        let d = default_for(app, clip, p);
        app.run(set(set_at(&stored, t, d)));
    }
    if cancel_preview {
        app.view.param_edit = None;
    }
    seek
}

/// The lane right of a row: background, keyframe diamonds, the playhead.
/// Click or drag to scrub.
fn lane(ui: &mut Ui, span: Span, keys: Option<&[f64]>) -> Option<Time> {
    let id = ui.make_id("lane");
    let r = ui.interact_drag(id);
    let seek = scrub_to(&r, span);
    let keys: Vec<f64> = keys.map(|k| k.to_vec()).unwrap_or_default();
    let has_keys = !keys.is_empty();
    ui.add_leaf(id, Layout::leaf(Size::Grow(1.0), Size::Grow(1.0)), Vec2::ZERO, true, move |p, rect| {
        p.rect(rect, Color::hex(0x1d1d1d), 0.0);
        p.rect(Rect::new(rect.x, rect.bottom() - 1.0, rect.w, 1.0), Color::hex(0x262626), 0.0);
        if has_keys {
            let y = rect.center().y;
            p.rect(Rect::new(span.x(rect, keys[0]), y - 0.5, span.x(rect, *keys.last().unwrap()) - span.x(rect, keys[0]), 1.0), Color::hex(0x5a5a5a), 0.0);
            for k in &keys {
                let x = span.x(rect, *k);
                // A diamond from stacked rows.
                for i in 0..5i32 {
                    let w = (5 - (i - 2).abs() * 2) as f32;
                    p.rect(Rect::new(x - w * 0.5, y - 2.5 + i as f32, w.max(1.0), 1.0), Color::hex(0xcfcfcf), 0.0);
                }
            }
        }
        let x = span.x(rect, span.playhead).round();
        if x >= rect.x && x <= rect.right() {
            p.rect(Rect::new(x - 0.5, rect.y, 1.0, rect.h), REEL.playhead, 0.0);
        }
    });
    seek
}

/// Where a press or drag in a lane puts the playhead.
fn scrub_to(r: &Response, span: Span) -> Option<Time> {
    (r.pressed || r.active).then(|| {
        let f = ((r.mouse_pos.x - r.rect.x) / r.rect.w.max(1.0)).clamp(0.0, 1.0) as f64;
        Time::from_seconds_f64(span.origin + f * span.span)
    })
}

/// The lanes' header: a ruler and the clip's bar, under which the lanes run.
fn ruler_lane(ui: &mut Ui, rate: ve_time::Rate, span: Span, name: &str) -> Option<Time> {
    let id = ui.make_id("fx-ruler");
    let r = ui.interact_drag(id);
    let seek = scrub_to(&r, span);
    let ticks: Vec<FrameText> = (0..4)
        .map(|i| ui.frame_text(&Timecode::from_time(Time::from_seconds_f64(span.origin + span.span * i as f64 / 4.0), rate, true).to_string()))
        .collect();
    let name = ui.frame_text(name);
    let size = ui.theme.metrics.font_size_small;
    ui.add_leaf(id, Layout::leaf(Size::Grow(1.0), Size::Grow(1.0)), Vec2::ZERO, true, move |p, rect| {
        p.rect(rect, Color::hex(0x1d1d1d), 0.0);
        let ruler = Rect::new(rect.x, rect.y, rect.w, 18.0);
        p.rect(ruler, Color::hex(0x252525), 0.0);
        p.rect(Rect::new(rect.x, ruler.bottom(), rect.w, 1.0), Color::hex(0x101010), 0.0);
        // Every label when there is room, every other one when the panel is narrow.
        let every = if rect.w / 4.0 < 78.0 { 2 } else { 1 };
        for (i, label) in ticks.iter().enumerate().step_by(every) {
            let x = (rect.x + rect.w * (i as f32 / 4.0)).round();
            p.rect(Rect::new(x, ruler.y + 5.0, 1.0, 13.0), Color::hex(0x484848), 0.0);
            p.text_left(Rect::new(x + 3.0, ruler.y + 2.0, 72.0, 14.0), size - 1.0, Color::hex(0x7a7a7a), *label);
        }
        let x0 = span.x(rect, span.clip_start);
        let x1 = span.x(rect, span.clip_start + span.clip_len);
        let bar = Rect::new(x0, ruler.bottom() + 3.0, (x1 - x0).max(4.0), 19.0);
        p.rect(bar, REEL.video_fill, 2.0);
        if bar.w > 30.0 {
            p.text_left(bar.shrink(4.0, 0.0, 4.0, 0.0), size - 1.0, Color::hex(0xe8f1f8), name);
        }
        let x = span.x(rect, span.playhead).round();
        if x >= rect.x && x <= rect.right() {
            p.rect(Rect::new(x - 0.5, rect.y, 1.0, rect.h), REEL.playhead, 0.0);
            p.rect(Rect::new(x - 5.0, rect.y, 10.0, 9.0), REEL.playhead, 1.5);
        }
    });
    seek
}

fn footer(ui: &mut Ui, app: &mut EditorUi) {
    let row = Layout::row()
        .width(Size::Grow(1.0))
        .height(Size::Fixed(26.0))
        .padding(Insets::xy(8.0, 0.0))
        .gap(4.0)
        .align(Align::Start, Align::Center);
    ui.container(row, Frame { fill: Color::hex(0x1f1f1f), ..Frame::none() }, |ui| {
        ui.text_with(&app.timecode(app.playhead), 11.0, REEL.timecode);
        ui.flex();
        let _ = icon_button(ui, "filter", Icon::Sort, 20.0, false);
        let _ = icon_button(ui, "audio", Icon::Speaker, 20.0, false);
        let _ = icon_button(ui, "export-fx", Icon::Export, 20.0, false);
    });
}
