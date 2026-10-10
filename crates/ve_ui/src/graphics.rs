//! Essential Graphics: titles from templates, and the editor for the
//! selected title's text and look. A title is a clip of the built-in title
//! generator (`ve.title`); its parameters are the title's look, so Effect
//! Controls, keyframes and undo all work on it as on any effect.
//!
//! Browse: the templates, drawn by the same generator. Double-click one,
//! press Add, or drag it to the timeline. Edit: the selected title (or the
//! one under the playhead) — text, size, tracking, leading, alignment,
//! fill, outline, background, shadow and position, live while dragged.
//! In the Program monitor the selected title wears its bounds: drag inside
//! them to move it; with the Type tool (T), click to make a new one there.

use std::sync::Arc;

use libgui::*;
use ve_engine::{edit, intrinsic, Command};
use ve_model::*;
use ve_render::generate::{title_bounds, TitleStyle};
use ve_time::Time;

use crate::effects::set_at;
use crate::theme::REEL;
use crate::EditorUi;

/// Payload kind for a template dragged from Essential Graphics: its index.
pub const TEMPLATE_PAYLOAD: &str = "title-template";

/// A title template: a name and the parameters it changes.
pub(crate) struct Template {
    pub name: &'static str,
    pub kind: &'static str,
    pub params: Vec<(&'static str, Value)>,
}

pub(crate) fn templates() -> Vec<Template> {
    let t = |s: &str| Value::Text(s.into());
    let at = |x: f64, y: f64| Value::Vec2([x, y]);
    let on = Value::Bool(true);
    vec![
        Template { name: "Title", kind: "Titles", params: vec![("text", t("Title")), ("size", Value::Float(120.0)), ("bold", on.clone())] },
        Template {
            name: "Lower Third – Bar",
            kind: "Lower Thirds",
            params: vec![
                ("text", t("Alex Morgan\nDirector of Photography")),
                ("size", Value::Float(48.0)),
                ("align", Value::Choice(0)),
                ("position", at(150.0, 880.0)),
                ("box", on.clone()),
                ("box_color", Value::Color([0.08, 0.28, 0.62, 0.9])),
                ("box_padding", Value::Float(20.0)),
            ],
        },
        Template {
            name: "Lower Third – Clean",
            kind: "Lower Thirds",
            params: vec![("text", t("Sam Rivera\nField Producer")), ("size", Value::Float(50.0)), ("align", Value::Choice(0)), ("position", at(150.0, 890.0)), ("shadow", on.clone())],
        },
        Template {
            name: "Caption",
            kind: "Captions",
            params: vec![("text", t("Caption text goes here")), ("size", Value::Float(44.0)), ("position", at(960.0, 960.0)), ("box", on.clone()), ("box_padding", Value::Float(12.0))],
        },
        Template {
            name: "Outline Title",
            kind: "Titles",
            params: vec![("text", t("OUTLINE")), ("size", Value::Float(150.0)), ("bold", on.clone()), ("tracking", Value::Float(80.0)), ("stroke", on.clone()), ("stroke_width", Value::Float(6.0))],
        },
        Template {
            name: "Credits",
            kind: "Titles",
            params: vec![("text", t("Directed by\nJordan Lee")), ("size", Value::Float(56.0)), ("leading", Value::Float(140.0)), ("shadow", on.clone())],
        },
        Template {
            name: "Callout",
            kind: "Captions",
            params: vec![
                ("text", t("NEW")),
                ("size", Value::Float(40.0)),
                ("bold", on.clone()),
                ("align", Value::Choice(2)),
                ("position", at(1770.0, 150.0)),
                ("color", Value::Color([0.08, 0.08, 0.08, 1.0])),
                ("box", on),
                ("box_color", Value::Color([0.98, 0.8, 0.2, 1.0])),
                ("box_padding", Value::Float(14.0)),
            ],
        },
    ]
}

/// The title generator's parameters at their defaults, then `overrides`.
fn title_params(app: &EditorUi, overrides: &[(&str, Value)]) -> Vec<(String, Value)> {
    let Some(info) = app.st.plugins.find(&intrinsic::plugin_ref(intrinsic::TITLE)) else { return Vec::new() };
    info.params
        .iter()
        .map(|p| {
            let v = overrides.iter().find(|(k, _)| *k == p.id).map(|(_, v)| v.clone()).unwrap_or_else(|| ve_engine::default_value(&p.kind, p.default));
            (p.id.clone(), v)
        })
        .collect()
}

/// A template's picture, small, for its card.
pub(crate) fn template_picture(app: &EditorUi, t: &Template) -> Option<(u32, u32, Vec<u8>)> {
    let (w, h) = (320, 180);
    let rgba = ve_render::generate::picture_rgba(intrinsic::TITLE, &title_params(app, &t.params), w, h, w as f32 / 1920.0)?;
    Some((w, h, rgba))
}

/// The clip a title edit applies to: the selected title, else the title
/// under the playhead (the topmost).
pub(crate) fn subject(app: &EditorUi) -> Option<Arc<Clip>> {
    let is_title = |c: &Clip| matches!(&c.source, ClipSource::Generator { plugin } if plugin.id == intrinsic::TITLE);
    let snap = app.snap();
    if let Some(c) = app.view.selection.iter().find_map(|id| snap.find_clip(*id).map(|(_, _, c)| c.clone()).filter(|c| is_title(c))) {
        return Some(c);
    }
    let seq = snap.active()?;
    seq.tracks.iter().rev().filter(|t| t.kind == TrackKind::Video && t.enabled).find_map(|t| t.clip_at(app.playhead).filter(|c| is_title(c)).cloned())
}

/// The title's generator effect.
fn title_effect(c: &Clip) -> Option<Arc<Effect>> {
    c.effects.iter().find(|e| e.plugin.id == intrinsic::TITLE).cloned()
}

/// The title's look now (at the playhead), from its effect.
pub(crate) fn style_of(app: &EditorUi, c: &Clip) -> Option<TitleStyle> {
    let e = title_effect(c)?;
    let t = (app.playhead - c.timeline_start).clamp_to(Time::ZERO, c.source_range.duration);
    let params: Vec<(String, Value)> = e.params.iter().map(|(k, p)| (k.clone(), p.value_at(t))).collect();
    Some(TitleStyle::from_params(&params))
}

impl EditorUi {
    /// Put a title from `overrides` into the active sequence at `at`: on
    /// `track` when given, else on the topmost video track free there, else
    /// on a new track above the rest.
    pub(crate) fn add_title(&mut self, overrides: &[(&str, Value)], at: Time, track: Option<TrackId>) {
        let Some(seq) = self.active_seq() else { return };
        let duration = Time::from_seconds_f64(self.settings().still_seconds as f64).round_to_frame(self.rate());
        let Some(mut clip) = ve_engine::make_generator_clip(&self.st.plugins, &seq.format, &intrinsic::plugin_ref(intrinsic::TITLE), duration) else { return };
        if let Some(e) = clip.effects.iter_mut().find(|e| e.plugin.id == intrinsic::TITLE) {
            let e = Arc::make_mut(e);
            for (k, v) in title_params(self, overrides) {
                e.params.insert(k, Param::Constant(v));
            }
        }
        clip.name = match overrides.iter().find(|(k, _)| *k == "text") {
            Some((_, Value::Text(t))) => t.lines().next().unwrap_or("Title").to_string(),
            _ => "Title".into(),
        };
        let range = ve_time::TimeRange::new(at, duration);
        let free = |t: &Track| !t.clips.iter().any(|c| c.timeline_range().overlaps(range));
        let target = track.filter(|t| seq.track(*t).is_some_and(|(_, tr)| tr.kind == TrackKind::Video)).or_else(|| {
            seq.tracks.iter().filter(|t| t.kind == TrackKind::Video && !t.locked).filter(|t| free(t)).last().map(|t| t.id)
        });
        let mut commands = Vec::new();
        let target = match target {
            Some(t) => t,
            None => {
                // A new video track, after the last one.
                let n = seq.tracks.iter().filter(|t| t.kind == TrackKind::Video).count() + 1;
                let index = seq.tracks.iter().rposition(|t| t.kind == TrackKind::Video).map_or(0, |i| i + 1);
                let track = Track::new(TrackKind::Video, format!("V{n}"));
                let id = track.id;
                commands.push(Command::AddTrack { sequence: seq.id, index, track: Arc::new(track) });
                id
            }
        };
        let clip = Arc::new(clip);
        self.view.selection = vec![clip.id];
        let base = match (Command::Batch { label: String::new(), commands: commands.clone() }).apply(self.snap()) {
            Ok(a) => a.project,
            Err(e) => return self.errors.push(e.to_string()),
        };
        let r = edit::overwrite(&base, seq.id, at, &[(target, clip)]).map(|e| match commands.is_empty() {
            true => e,
            false => Command::Batch { label: "New Title".into(), commands: commands.into_iter().chain([e]).collect() },
        });
        self.run_edit(r);
        self.view.graphics_tab = 1;
    }

    /// Set title parameter `param` of `clip` to `v` (as part of drag
    /// `gesture` when dragging: one undo step per drag).
    pub(crate) fn set_title_param(&mut self, clip: &Clip, param: &str, v: Value, gesture: Option<u64>) {
        let Some(e) = title_effect(clip) else { return };
        let t = (self.playhead - clip.timeline_start).clamp_to(Time::ZERO, clip.source_range.duration);
        let stored = e.params.get(param).cloned().unwrap_or(Param::Constant(v.clone()));
        let cmd = Command::SetEffectParam { clip: clip.id, effect: e.id, param: param.into(), value: Some(set_at(&stored, t, v)) };
        match gesture {
            Some(g) => self.engine.execute_merging(cmd, g),
            None => self.run(cmd),
        }
    }
}

pub(crate) fn panel(ui: &mut Ui, app: &mut EditorUi) {
    app.graphics_wanted = true;
    let t = ui.theme.clone();
    ui.container(Layout::row().width(Size::Grow(1.0)).height(Size::Fixed(24.0)), Frame::none(), |ui| {
        ui.container(Layout::row().width(Size::Fixed(180.0)).height(Size::Fixed(20.0)), Frame::none(), |ui| {
            ui.segmented("eg-tab", &mut app.view.graphics_tab, &["Browse", "Edit"]);
        });
    });
    ui.space(6.0);
    if app.view.graphics_tab == 0 {
        browse(ui, app);
    } else {
        match subject(app) {
            Some(c) => edit_title(ui, app, &c),
            None => ui.text_with("Select a title in the timeline, or add one from Browse (or use the Type tool, T, on the Program monitor).", t.metrics.font_size_small, t.palette.text_faint),
        }
    }
}

fn browse(ui: &mut Ui, app: &mut EditorUi) {
    let t = ui.theme.clone();
    let all = templates();
    let mut add = None;
    for chunk in all.iter().enumerate().collect::<Vec<_>>().chunks(2) {
        ui.container(Layout::row().width(Size::Grow(1.0)).height(Size::Fixed(122.0)).gap(8.0), Frame::none(), |ui| {
            for (i, tpl) in chunk {
                let id = ui.make_id(("template", *i));
                let r = ui.interact_drag(id);
                let index = *i;
                let name = tpl.name.to_string();
                ui.drag_source_from(&r, move || Payload::new(TEMPLATE_PAYLOAD, index).with_label(name));
                if r.clicked {
                    app.view.template = index;
                }
                if r.double_clicked {
                    add = Some(index);
                }
                let on = app.view.template == index;
                let tex = app.template_tex.get(index).and_then(|t| t.as_ref()).map(|(_, id)| *id);
                let hot = ui.animate_bool(id, 0, r.hovered);
                let (name, kind) = (ui.frame_text(tpl.name), ui.frame_text(tpl.kind));
                let (size, accent) = (t.metrics.font_size, t.palette.accent);
                ui.add_leaf(id, Layout::leaf(Size::Grow(1.0), Size::Grow(1.0)), Vec2::ZERO, true, move |p, rect| {
                    p.rect_bordered(rect, REEL.raised.lerp(REEL.raised_hi, hot), 3.0, if on { 2.0 } else { 1.0 }, if on { accent } else { REEL.line });
                    let img = Rect::new(rect.x + 4.0, rect.y + 4.0, rect.w - 8.0, rect.h - 38.0);
                    // A dark stand-in for footage, under the title's alpha.
                    p.rect(img, Color::hex(0x2a3440), 2.0);
                    if let Some(tex) = tex {
                        let (w, h) = if img.w / img.h > 16.0 / 9.0 { (img.h * 16.0 / 9.0, img.h) } else { (img.w, img.w * 9.0 / 16.0) };
                        p.image(Rect::new(img.center().x - w / 2.0, img.center().y - h / 2.0, w, h), tex, 0.0);
                    }
                    p.text_left(Rect::new(rect.x + 6.0, rect.bottom() - 32.0, rect.w - 12.0, 14.0), size, REEL.text_soft, name);
                    p.text_left(Rect::new(rect.x + 6.0, rect.bottom() - 17.0, rect.w - 12.0, 12.0), size - 1.0, REEL.label, kind);
                });
            }
            if chunk.len() == 1 {
                ui.container(Layout::column().width(Size::Grow(1.0)).height(Size::Grow(1.0)), Frame::none(), |_| {});
            }
        });
    }
    ui.space(6.0);
    ui.row(|ui| {
        if ui.button_primary("Add to Sequence").clicked {
            add = Some(app.view.template);
        }
        ui.text_with("At the playhead, on the top free video track.", t.metrics.font_size_small, t.palette.text_faint);
    });
    if let Some(i) = add {
        let params = all[i.min(all.len() - 1)].params.clone();
        app.add_title(&params, app.playhead, None);
    }
}

/// A labelled row of controls.
fn line<R>(ui: &mut Ui, label: &str, body: impl FnOnce(&mut Ui) -> R) -> R {
    let t = ui.theme.clone();
    ui.container(Layout::row().width(Size::Grow(1.0)).height(Size::Fixed(24.0)).gap(8.0).align(Align::Start, Align::Center), Frame::none(), |ui| {
        ui.container(Layout::row().width(Size::Fixed(96.0)).height(Size::Grow(1.0)).align(Align::Start, Align::Center), Frame::none(), |ui| {
            ui.text_with(label, t.metrics.font_size, t.palette.text_muted)
        });
        body(ui)
    })
}

fn edit_title(ui: &mut Ui, app: &mut EditorUi, clip: &Arc<Clip>) {
    let t = ui.theme.clone();
    let Some(style) = style_of(app, clip) else { return };
    let cid = clip.id;
    let mut set: Vec<(&'static str, Value, bool)> = Vec::new(); // (param, value, part of a drag)

    ui.text_with(&clip.name, t.metrics.font_size_heading, t.palette.text);
    ui.space(4.0);
    // The text, edited in place: one undo step per stretch of typing.
    let mut text = style.text.clone();
    let r = ui.text_area(&format!("eg-text-{cid}"), &mut text, 3);
    if r.changed {
        if app.text_gesture.is_none_or(|(c, _)| c != cid) {
            app.text_gesture = Some((cid, app.next_gesture()));
        }
        set.push(("text", Value::Text(text), true));
    }
    if !r.focused {
        app.text_gesture = None;
    }
    if r.response.double_clicked {
        app.view.graphics_tab = 1;
    }
    ui.section("Text");
    line(ui, "Font", |ui| {
        ui.text_with("Inter", t.metrics.font_size, t.palette.text);
        let mut bold = style.bold;
        if ui.checkbox_keyed(("eg-bold", cid), "Bold", &mut bold).clicked {
            set.push(("bold", Value::Bool(bold), false));
        }
    });
    let drags: [(&'static str, &str, f32, f32, std::ops::RangeInclusive<f32>); 3] =
        [("size", "Size", style.size, 0.5, 1.0..=1000.0), ("tracking", "Tracking", style.tracking, 1.0, -100.0..=500.0), ("leading", "Leading (%)", style.leading, 0.5, 50.0..=300.0)];
    for (param, label, value, speed, range) in drags {
        line(ui, label, |ui| {
            let mut v = value;
            let r = ui.drag_value_range_keyed(("eg", param, cid), "", &mut v, speed, range);
            drag_commit(app, &r, param, Value::Float(v as f64), v != value, &mut set);
        });
    }
    line(ui, "Alignment", |ui| {
        let mut a = style.align as usize;
        ui.container(Layout::row().width(Size::Fixed(180.0)).height(Size::Fixed(20.0)), Frame::none(), |ui| {
            ui.segmented(&format!("eg-align-{cid}"), &mut a, &["Left", "Center", "Right"]);
        });
        if a as u32 != style.align {
            set.push(("align", Value::Choice(a as u32), false));
        }
    });
    line(ui, "Fill", |ui| color(ui, app, ("fill", cid), "color", style.color, &mut set));

    ui.section("Appearance");
    let toggles: [(&'static str, &str, bool); 3] = [("stroke", "Stroke", style.stroke.is_some()), ("box", "Background", style.boxed.is_some()), ("shadow", "Shadow", style.shadow.is_some())];
    for (param, label, on) in toggles {
        line(ui, label, |ui| {
            let mut v = on;
            if ui.checkbox_keyed(("eg-on", param, cid), "", &mut v).clicked {
                set.push((param, Value::Bool(v), false));
            }
            match param {
                "stroke" => {
                    let (c, w) = style.stroke.unwrap_or(([0.0, 0.0, 0.0, 1.0], 4.0));
                    color(ui, app, ("stroke", cid), "stroke_color", c, &mut set);
                    let mut v = w;
                    let r = ui.drag_value_range_keyed(("eg-sw", cid), "Width", &mut v, 0.2, 0.0..=50.0);
                    drag_commit(app, &r, "stroke_width", Value::Float(v as f64), v != w, &mut set);
                }
                "box" => {
                    let (c, pad) = style.boxed.unwrap_or(([0.0, 0.0, 0.0, 0.6], 24.0));
                    color(ui, app, ("box", cid), "box_color", c, &mut set);
                    let mut v = pad;
                    let r = ui.drag_value_range_keyed(("eg-bp", cid), "Padding", &mut v, 0.5, 0.0..=300.0);
                    drag_commit(app, &r, "box_padding", Value::Float(v as f64), v != pad, &mut set);
                }
                _ => {
                    let (c, d, soft) = style.shadow.unwrap_or(([0.0, 0.0, 0.0, 0.75], 6.0, 4.0));
                    color(ui, app, ("shadow", cid), "shadow_color", c, &mut set);
                    let (mut dv, mut sv) = (d, soft);
                    let r = ui.drag_value_range_keyed(("eg-sd", cid), "Distance", &mut dv, 0.2, 0.0..=200.0);
                    drag_commit(app, &r, "shadow_distance", Value::Float(dv as f64), dv != d, &mut set);
                    let r = ui.drag_value_range_keyed(("eg-ss", cid), "Softness", &mut sv, 0.2, 0.0..=100.0);
                    drag_commit(app, &r, "shadow_softness", Value::Float(sv as f64), sv != soft, &mut set);
                }
            }
        });
    }

    ui.section("Align and Transform");
    let pos = style.position.unwrap_or([960.0, 540.0]);
    line(ui, "Position", |ui| {
        let (mut x, mut y) = (pos[0], pos[1]);
        let rx = ui.drag_value_keyed(("eg-px", cid), "X", &mut x, 1.0);
        let ry = ui.drag_value_keyed(("eg-py", cid), "Y", &mut y, 1.0);
        let changed = x != pos[0] || y != pos[1];
        if rx.pressed || ry.pressed {
            app.drag_gesture = app.next_gesture();
        }
        if changed {
            set.push(("position", Value::Vec2([x as f64, y as f64]), rx.active || ry.active));
        }
    });
    line(ui, "Center", |ui| {
        let seq = app.active_seq().map(|s| (s.format.width as f64, s.format.height as f64)).unwrap_or((1920.0, 1080.0));
        if ui.button_keyed(("eg-ch", cid), "Horizontally").clicked {
            let x = match style.align {
                0 => seq.0 / 2.0 - title_bounds(&style, (seq.0 as u32, seq.1 as u32))[2] as f64 / 2.0,
                2 => seq.0 / 2.0 + title_bounds(&style, (seq.0 as u32, seq.1 as u32))[2] as f64 / 2.0,
                _ => seq.0 / 2.0,
            };
            set.push(("position", Value::Vec2([x, pos[1] as f64]), false));
        }
        if ui.button_keyed(("eg-cv", cid), "Vertically").clicked {
            set.push(("position", Value::Vec2([pos[0] as f64, seq.1 / 2.0]), false));
        }
    });

    for (param, v, drag) in set {
        let gesture = if param == "text" { app.text_gesture.map(|(_, g)| g) } else if drag { Some(app.drag_gesture) } else { None };
        app.set_title_param(clip, param, v, gesture);
    }
}

/// A drag value's change: part of its drag (one undo step) while held.
fn drag_commit(app: &mut EditorUi, r: &Response, param: &'static str, v: Value, changed: bool, set: &mut Vec<(&'static str, Value, bool)>) {
    if r.pressed {
        app.drag_gesture = app.next_gesture();
    }
    if changed {
        set.push((param, v, r.active));
    }
}

fn color(ui: &mut Ui, app: &mut EditorUi, key: (&str, ClipId), param: &'static str, c: [f32; 4], set: &mut Vec<(&'static str, Value, bool)>) {
    let mut col = Color::rgba(c[0], c[1], c[2], c[3]);
    let r = ui.color_button(&format!("eg-{}-{}", key.0, key.1), &mut col);
    if r.changed || r.finished {
        if r.changed && app.color_gesture.is_none_or(|(k, _)| k != param) {
            app.color_gesture = Some((param, app.next_gesture()));
        }
        let g = app.color_gesture.map(|(_, g)| g);
        set.push((param, Value::Color([col.r, col.g, col.b, col.a]), g.is_some()));
        if let Some(g) = g {
            app.drag_gesture = g;
        }
    }
    if r.finished {
        app.color_gesture = None;
    }
}
