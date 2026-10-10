//! The timeline: one custom surface with its own zoom, two-way scroll, track
//! headers, tools, snapping and drag-and-drop.
//!
//! Every tool works the same way. While the pointer is down the tool's edit
//! is **built** from the drag (with `ve_command::edit`) and applied to a copy
//! of the project, and that copy is what is drawn — so the preview is exactly
//! the edit that will happen. On release the same command is sent to the
//! engine as one undo step. An edit that would break the project (a move onto
//! another clip) is drawn as the original with a red outline and is not sent.

use std::sync::Arc;

use libgui::*;
use ve_engine::{edit, Command, CommandError, Edge, Snapshot};
use ve_model::*;
use ve_time::{Time, Timecode};

use crate::features::{marker_color, GENERATOR_PAYLOAD, TRANSITION_PAYLOAD};
use crate::project::{add_effect, EFFECT_PAYLOAD};
use crate::theme::REEL;
use crate::widgets::{draw_icon, icon_button, Icon};
use crate::{EditorUi, Tool, ASSET_PAYLOAD, FILES_PAYLOAD};

const HEADER_W: f32 = 148.0;
const RULER_H: f32 = 26.0;
const BAR: f32 = 11.0;
/// How close, in px, the pointer must be to an edge to grab it.
const EDGE_PX: f32 = 6.0;
/// How close, in px, the pointer must be to a marker to pick it.
const MARKER_PX: f32 = 5.0;
/// The name strip along a clip's top; below it is the body.
const STRIP_H: f32 = 15.0;
/// How close, in px, an edge must come to another to snap.
const SNAP_PX: f32 = 8.0;

const TOOLS: [(Icon, Tool, &str); 10] = [
    (Icon::Select, Tool::Select, "Selection (V)"),
    (Icon::TrackSelect, Tool::TrackSelect, "Track Select Forward (A)"),
    (Icon::Ripple, Tool::Ripple, "Ripple Edit (B)"),
    (Icon::Rolling, Tool::Rolling, "Rolling Edit (N)"),
    (Icon::Razor, Tool::Razor, "Razor (C)"),
    (Icon::Slip, Tool::Slip, "Slip (Y)"),
    (Icon::Pen, Tool::Pen, "Pen (P)"),
    (Icon::Rect, Tool::Rect, "Rectangle"),
    (Icon::Type, Tool::Type, "Type (T)"),
    (Icon::Hand, Tool::Hand, "Hand (H)"),
];

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Drag {
    Playhead,
    ScrollX,
    ScrollY,
    Hand,
    /// `anchor` is the time under the pointer at the press; deltas are taken
    /// from it, so the edit is recomputed from scratch every frame.
    Move { clip: ClipId, anchor: Time, track: TrackId },
    Trim { clip: ClipId, edge: Edge, ripple: bool, anchor: Time },
    Roll { left: ClipId, right: ClipId, anchor: Time },
    Slip { clip: ClipId, anchor: Time },
}

/// One track's row on screen.
#[derive(Clone)]
struct Row {
    id: TrackId,
    kind: TrackKind,
    name: String,
    top: f32,
    height: f32,
}

fn rows(app: &EditorUi, seq: &Sequence) -> Vec<Row> {
    let video = (0..seq.tracks.len()).filter(|&i| seq.tracks[i].kind == TrackKind::Video).rev();
    let audio = (0..seq.tracks.len()).filter(|&i| seq.tracks[i].kind == TrackKind::Audio);
    let first_v = seq.tracks.iter().position(|t| t.kind == TrackKind::Video);
    let first_a = seq.tracks.iter().position(|t| t.kind == TrackKind::Audio);
    let mut top = 0.0;
    video
        .chain(audio)
        .map(|i| {
            let t = &seq.tracks[i];
            let default = if Some(i) == first_v || Some(i) == first_a { 58.0 } else { 46.0 };
            let height = app.view.track_heights.get(&t.id).copied().unwrap_or(default);
            let r = Row { id: t.id, kind: t.kind, name: t.name.clone(), top, height };
            top += height + 1.0;
            r
        })
        .collect()
}

pub fn panel(ui: &mut Ui, app: &mut EditorUi) {
    let t = ui.theme.clone();
    let col = Layout::column().width(Size::Grow(1.0)).height(Size::Grow(1.0));
    ui.container(col, Frame { fill: t.palette.bg_panel, clip: true, ..Frame::none() }, |ui| {
        toolbar(ui, app);
        ui.container(Layout::row().width(Size::Grow(1.0)).height(Size::Grow(1.0)), Frame::none(), |ui| {
            tools(ui, app);
            ui.container(Layout::row().width(Size::Grow(1.0)).height(Size::Grow(1.0)), Frame::none(), |ui| surface(ui, app));
            meters(ui, app.meter_db);
        });
    });
}

fn toolbar(ui: &mut Ui, app: &mut EditorUi) {
    let row = Layout::row()
        .width(Size::Grow(1.0))
        .height(Size::Fixed(30.0))
        .padding(Insets::xy(8.0, 0.0))
        .gap(3.0)
        .align(Align::Start, Align::Center);
    ui.container(row, Frame { fill: REEL.chrome, ..Frame::none() }, |ui| {
        ui.text_with(&app.timecode(app.playhead), 14.0, REEL.timecode);
        ui.space(10.0);
        sequence_picker(ui, app);
        let nest = icon_button(ui, "nest", Icon::Insert, 22.0, false);
        ui.tooltip(&nest, "Nest… (the selected clips into a new sequence)");
        if nest.clicked {
            app.open_nest_dialog();
        }
        let snap = icon_button(ui, "snap", Icon::Snap, 22.0, app.view.snap);
        ui.tooltip(&snap, "Snap (S)");
        if snap.clicked {
            app.view.snap = !app.view.snap;
        }
        let link = icon_button(ui, "link", Icon::LinkedSelection, 22.0, app.view.linked);
        ui.tooltip(&link, "Linked Selection");
        if link.clicked {
            app.view.linked = !app.view.linked;
        }
        let marker = icon_button(ui, "markers", Icon::Marker, 22.0, false);
        ui.tooltip(&marker, "Add Marker (M)");
        if marker.clicked {
            app.add_marker();
        }
        ui.flex();
        let tool = TOOLS.iter().find(|(_, t, _)| *t == app.view.tool).map(|x| x.2).unwrap_or("");
        ui.text_with(tool, 11.0, ui.theme.palette.text_faint);
        ui.space(8.0);
        let out = icon_button(ui, "zoom-out", Icon::Zoom, 20.0, false);
        ui.tooltip(&out, "Zoom Out (-)");
        if out.clicked {
            app.view.pps = (app.view.pps / 1.25).max(4.0);
        }
        let zin = icon_button(ui, "zoom-in", Icon::Zoom, 20.0, false);
        ui.tooltip(&zin, "Zoom In (=)");
        if zin.clicked {
            app.view.pps = (app.view.pps * 1.25).min(1200.0);
        }
    });
}

/// Which sequence the timeline shows, when there is more than one.
fn sequence_picker(ui: &mut Ui, app: &mut EditorUi) {
    let seqs: Vec<(SequenceId, String)> = app.snap().sequences.values().map(|s| (s.id, s.name.clone())).collect();
    if seqs.len() < 2 {
        return;
    }
    let active = app.snap().active_sequence;
    let mut i = seqs.iter().position(|(id, _)| Some(*id) == active).unwrap_or(0);
    let before = i;
    let names: Vec<&str> = seqs.iter().map(|(_, n)| n.as_str()).collect();
    ui.container(Layout::row().width(Size::Fixed(150.0)).height(Size::Fixed(20.0)), Frame::none(), |ui| {
        ui.combo("sequence", &mut i, &names);
    });
    if i != before {
        app.open_sequence(seqs[i].0);
    }
    ui.space(4.0);
}

fn tools(ui: &mut Ui, app: &mut EditorUi) {
    let col = Layout::column()
        .width(Size::Fixed(30.0))
        .height(Size::Grow(1.0))
        .padding(Insets::xy(3.0, 6.0))
        .gap(2.0)
        .align(Align::Center, Align::Start);
    ui.container(col, Frame { fill: REEL.chrome, ..Frame::none() }, |ui| {
        for (i, (icon, tool, name)) in TOOLS.iter().enumerate() {
            let r = icon_button(ui, ("tool", i), *icon, 24.0, app.view.tool == *tool);
            ui.tooltip(&r, name);
            if r.clicked {
                app.view.tool = *tool;
            }
        }
    });
}

/// What the pointer is over.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Hit {
    Ruler,
    HBar,
    VBar,
    Header(TrackId, f32),
    /// A sequence marker on the ruler.
    Marker(MarkerId),
    /// A transition: the clip that owns it, and which edge.
    Transition { clip: ClipId, edge: Edge },
    Clip { clip: ClipId, track: TrackId, edge: Option<Edge> },
    /// Between two adjacent clips.
    Cut { left: ClipId, right: ClipId },
    Empty(TrackId),
    Nothing,
}

struct Geo {
    rect: Rect,
    track_x: f32,
    tracks_y: f32,
    view_w: f32,
    view_h: f32,
    pps: f32,
    scroll_x: f32,
    scroll_y: f32,
}

impl Geo {
    fn time_at(&self, x: f32) -> Time {
        Time::from_seconds_f64((((x - self.track_x + self.scroll_x) / self.pps).max(0.0)) as f64)
    }

    fn x_of(&self, t: Time) -> f32 {
        self.track_x + t.as_seconds_f64() as f32 * self.pps - self.scroll_x
    }

    fn row_at<'a>(&self, rows: &'a [Row], y: f32) -> Option<&'a Row> {
        let y = y - self.tracks_y + self.scroll_y;
        rows.iter().find(|r| y >= r.top && y < r.top + r.height + 1.0)
    }
}

fn hit(g: &Geo, rows: &[Row], seq: &Sequence, pos: Vec2) -> Hit {
    let r = g.rect;
    if pos.y >= r.bottom() - BAR {
        return Hit::HBar;
    }
    if pos.x >= r.right() - BAR {
        return Hit::VBar;
    }
    if pos.y < g.tracks_y {
        if pos.x < g.track_x {
            return Hit::Nothing;
        }
        // Markers sit along the ruler's lower half.
        if pos.y > r.y + RULER_H * 0.45 {
            let m = seq.marks.markers.iter().filter(|m| (g.x_of(m.time) - pos.x).abs() <= MARKER_PX).min_by(|a, b| {
                (g.x_of(a.time) - pos.x).abs().total_cmp(&(g.x_of(b.time) - pos.x).abs())
            });
            if let Some(m) = m {
                return Hit::Marker(m.id);
            }
        }
        return Hit::Ruler;
    }
    let Some(row) = g.row_at(rows, pos.y) else { return Hit::Nothing };
    if pos.x < g.track_x {
        return Hit::Header(row.id, pos.x - r.x);
    }
    let Some((_, track)) = seq.track(row.id) else { return Hit::Nothing };
    let t = g.time_at(pos.x);
    // In a clip's body a transition wins (it sits on the cut); in the name
    // strip the edges do, so a cut under a transition can still be trimmed.
    let in_body = pos.y - g.tracks_y + g.scroll_y - row.top >= STRIP_H;
    if in_body {
        for (clip, edge, a, b) in transition_spans(track) {
            if pos.x >= g.x_of(a) && pos.x <= g.x_of(b) {
                return Hit::Transition { clip, edge };
            }
        }
    }
    // Edges next: they are thin targets on top of the clip bodies.
    let near = |x: f32| (pos.x - x).abs() <= EDGE_PX;
    for (i, c) in track.clips.iter().enumerate() {
        let (x0, x1) = (g.x_of(c.timeline_start), g.x_of(c.timeline_range().end()));
        if near(x1) {
            if let Some(next) = track.clips.get(i + 1).filter(|n| n.timeline_start == c.timeline_range().end()) {
                return Hit::Cut { left: c.id, right: next.id };
            }
            return Hit::Clip { clip: c.id, track: row.id, edge: Some(Edge::End) };
        }
        if near(x0) {
            return Hit::Clip { clip: c.id, track: row.id, edge: Some(Edge::Start) };
        }
    }
    match track.clip_at(t) {
        Some(c) => Hit::Clip { clip: c.id, track: row.id, edge: None },
        None => Hit::Empty(row.id),
    }
}

/// Every transition on `track` that plays: its owner, edge, and the
/// timeline span it covers.
fn transition_spans(track: &Track) -> Vec<(ClipId, Edge, Time, Time)> {
    let mut v = Vec::new();
    for (i, c) in track.clips.iter().enumerate() {
        if let Some(t) = &c.transition_in {
            v.push((c.id, Edge::Start, c.timeline_start - t.before, c.timeline_start + t.after));
        }
        let end = c.timeline_range().end();
        // A tail fade only plays where no clip follows directly.
        let followed = track.clips.get(i + 1).is_some_and(|n| n.timeline_start == end);
        if let (Some(t), false) = (&c.transition_out, followed) {
            v.push((c.id, Edge::End, end - t.before, end + t.after));
        }
    }
    v
}

/// Nearest snap point to any of `edges` within the snap distance: the
/// correction to add.
fn snap(points: &[Time], edges: &[Time], g: &Geo) -> Time {
    let tol = Time::from_seconds_f64((SNAP_PX / g.pps) as f64);
    let mut best: Option<Time> = None;
    for &e in edges {
        for &p in points {
            let d = p - e;
            if Time(d.ticks().abs()) <= tol && best.is_none_or(|b| Time(d.ticks().abs()) < Time(b.ticks().abs())) {
                best = Some(d);
            }
        }
    }
    best.unwrap_or(Time::ZERO)
}

/// The edit a drag stands for right now.
fn pending(app: &EditorUi, snap_proj: &Project, seq: &Sequence, g: &Geo, drag: Drag, pointer: Vec2) -> Option<Result<Command, CommandError>> {
    let rate = seq.format.rate;
    let now = g.time_at(pointer.x);
    let linked = app.view.linked;
    let points = |except: &[ClipId]| if app.view.snap { edit::snap_points(seq, except, app.playhead) } else { vec![] };
    let delta_from = |anchor: Time| (now - anchor).round_to_frame(rate);
    Some(match drag {
        Drag::Move { clip, anchor, track } => {
            let (_, _, c) = snap_proj.find_clip(clip)?;
            let group = if linked { edit::linked(snap_proj, clip) } else { vec![clip] };
            let mut start = (c.timeline_start + delta_from(anchor)).max(Time::ZERO);
            let end = start + c.source_range.duration;
            start += snap(&points(&group), &[start, end], g);
            let start = start.max(Time::ZERO);
            // Onto a track of the same kind under the pointer.
            let rows = rows(app, seq);
            let kind = seq.track(track).map(|(_, t)| t.kind);
            let to = g.row_at(&rows, pointer.y).filter(|r| Some(r.kind) == kind).map_or(track, |r| r.id);
            if start == c.timeline_start && to == track {
                return None;
            }
            edit::move_clip(snap_proj, clip, to, start, linked)
        }
        Drag::Trim { clip, edge, ripple, anchor } => {
            let (_, _, c) = snap_proj.find_clip(clip)?;
            let mut delta = delta_from(anchor);
            let at = match edge {
                Edge::Start => c.timeline_start + delta,
                Edge::End => c.timeline_range().end() + delta,
            };
            if !ripple {
                delta += snap(&points(&edit::linked(snap_proj, clip)), &[at], g);
            }
            if delta == Time::ZERO {
                return None;
            }
            if ripple {
                edit::ripple_trim(snap_proj, clip, edge, delta, linked)
            } else {
                let mut b = edit::Builder::new(snap_proj);
                let group = if linked { edit::linked(snap_proj, clip) } else { vec![clip] };
                for id in group {
                    if let Err(e) = b.push(Command::TrimClip { clip: id, edge, delta }) {
                        return Some(Err(e));
                    }
                }
                b.finish("Trim")
            }
        }
        Drag::Roll { left, right, anchor } => {
            let (_, _, l) = snap_proj.find_clip(left)?;
            let mut delta = delta_from(anchor);
            let cut = l.timeline_range().end() + delta;
            delta += snap(&points(&[left, right]), &[cut], g);
            if delta == Time::ZERO {
                return None;
            }
            edit::roll(snap_proj, left, right, delta)
        }
        Drag::Slip { clip, anchor } => {
            let delta = delta_from(anchor);
            if delta == Time::ZERO {
                return None;
            }
            // Dragging right moves the picture right: earlier source frames.
            edit::slip(snap_proj, clip, -delta, linked)
        }
        _ => return None,
    })
}

fn surface(ui: &mut Ui, app: &mut EditorUi) {
    let snap_proj: Snapshot = app.st.snapshot.clone();
    let Some(seq) = snap_proj.active().cloned() else {
        ui.label_muted("No sequence.");
        return;
    };
    let rows = rows(app, &seq);

    // Media and effects dropped onto the timeline.
    let zone = ui.drop_zone(&[ASSET_PAYLOAD, FILES_PAYLOAD, EFFECT_PAYLOAD, TRANSITION_PAYLOAD, GENERATOR_PAYLOAD, crate::graphics::TEMPLATE_PAYLOAD]);

    let id = ui.make_id("timeline");
    let r = ui.interact_drag(id);
    let rect = r.rect;
    let content_h = rows.last().map_or(0.0, |r| r.top + r.height + 1.0);
    let end = seq.duration().as_seconds_f64() as f32 + 10.0;
    let mut g = Geo {
        rect,
        track_x: rect.x + HEADER_W,
        tracks_y: rect.y + RULER_H,
        view_w: (rect.w - HEADER_W - BAR).max(1.0),
        view_h: (rect.h - RULER_H - BAR).max(1.0),
        pps: app.view.pps,
        scroll_x: app.view.scroll_x,
        scroll_y: app.view.scroll_y,
    };
    let content_w = end * g.pps;
    let max_x = (content_w - g.view_w).max(0.0);
    let max_y = (content_h - g.view_h).max(0.0);

    // --- wheel: Cmd/Ctrl zooms about the pointer, Shift scrolls sideways ----
    if r.hovered || r.active {
        let s = ui.input().scroll;
        let m = ui.input().modifiers;
        if m.ctrl || m.logo {
            let anchor = (r.mouse_pos.x - g.track_x + app.view.scroll_x) / app.view.pps;
            app.view.pps = (app.view.pps * (1.0 + s.y * 0.0015).clamp(0.5, 2.0)).clamp(4.0, 1200.0);
            app.view.scroll_x = (anchor * app.view.pps - (r.mouse_pos.x - g.track_x)).clamp(0.0, (end * app.view.pps - g.view_w).max(0.0));
        } else if m.shift {
            app.view.scroll_x = (app.view.scroll_x - s.y - s.x).clamp(0.0, max_x);
        } else {
            app.view.scroll_y = (app.view.scroll_y - s.y).clamp(0.0, max_y);
            app.view.scroll_x = (app.view.scroll_x - s.x).clamp(0.0, max_x);
        }
        g.pps = app.view.pps;
        g.scroll_x = app.view.scroll_x;
        g.scroll_y = app.view.scroll_y;
    }

    let under = hit(&g, &rows, &seq, r.mouse_pos);
    let shift = r.modifiers.shift;
    let alt = r.modifiers.alt;

    // --- press: what the drag will be ---------------------------------------
    if r.pressed {
        // A press anywhere else in the tracks lets go of a picked transition
        // or marker, so Delete then means the clips.
        if matches!(under, Hit::Clip { .. } | Hit::Cut { .. } | Hit::Empty(_) | Hit::Ruler) {
            app.view.selected_transition = None;
            app.view.selected_marker = None;
        }
        app.view.drag = match under {
            Hit::HBar if max_x > 0.0 => Some(Drag::ScrollX),
            Hit::VBar if max_y > 0.0 => Some(Drag::ScrollY),
            Hit::Ruler => {
                app.engine.stop();
                app.seek(g.time_at(r.mouse_pos.x).round_to_frame(seq.format.rate));
                Some(Drag::Playhead)
            }
            Hit::Header(track, x) => {
                header_click(app, &seq, track, x);
                None
            }
            Hit::Marker(id) => {
                app.engine.stop();
                app.view.selected_marker = Some(id);
                app.view.selected_transition = None;
                if let Some(m) = seq.marks.markers.iter().find(|m| m.id == id) {
                    app.seek(m.time);
                }
                None
            }
            Hit::Transition { clip, edge } => {
                app.view.selected_transition = Some((clip, edge));
                app.view.selected_marker = None;
                app.view.selection.clear();
                None
            }
            _ if app.view.tool == Tool::Hand => Some(Drag::Hand),
            hit => press_tool(app, &snap_proj, &seq, &g, hit, r.mouse_pos, shift, alt),
        };
    }

    // --- double-click: edit a marker, open a nest -------------------------------
    if r.double_clicked {
        match under {
            Hit::Marker(id) => app.edit_marker(id),
            Hit::Clip { clip, .. } => {
                if let Some(ClipSource::Sequence { sequence, .. }) = snap_proj.find_clip(clip).map(|(_, _, c)| c.source.clone()) {
                    app.view.drag = None;
                    app.open_sequence(sequence);
                    return;
                }
            }
            _ => {}
        }
    }

    // --- drag ----------------------------------------------------------------
    let mut preview: Option<(Project, bool)> = None; // (project to draw, valid)
    if let Some(drag) = app.view.drag {
        if r.active {
            match drag {
                Drag::Playhead => app.seek(g.time_at(r.mouse_pos.x).round_to_frame(seq.format.rate)),
                Drag::ScrollX => {
                    let span = (g.view_w / content_w.max(1.0) * g.view_w).max(28.0);
                    let travel = (g.view_w - span).max(1.0);
                    app.view.scroll_x = (app.view.scroll_x + r.drag_delta.x * max_x / travel).clamp(0.0, max_x);
                }
                Drag::ScrollY => {
                    let span = (g.view_h / content_h.max(1.0) * g.view_h).max(28.0);
                    let travel = (g.view_h - span).max(1.0);
                    app.view.scroll_y = (app.view.scroll_y + r.drag_delta.y * max_y / travel).clamp(0.0, max_y);
                }
                Drag::Hand => {
                    app.view.scroll_x = (app.view.scroll_x - r.drag_delta.x).clamp(0.0, max_x);
                    app.view.scroll_y = (app.view.scroll_y - r.drag_delta.y).clamp(0.0, max_y);
                }
                _ => {}
            }
        }
        match pending(app, &snap_proj, &seq, &g, drag, r.mouse_pos) {
            Some(Ok(cmd)) => match cmd.apply(&snap_proj) {
                Ok(a) => {
                    if r.released {
                        app.run(cmd);
                    }
                    preview = Some((a.project, true));
                }
                Err(_) => preview = Some(((*snap_proj).clone(), false)),
            },
            Some(Err(_)) => preview = Some(((*snap_proj).clone(), false)),
            None => {}
        }
        if r.released || !r.active {
            app.view.drag = None;
        }
    }

    // --- cursor ----------------------------------------------------------------
    if r.hovered || r.active {
        ui.cursor = match (app.view.tool, under) {
            (_, Hit::Ruler) => Cursor::ResizeHorizontal,
            (Tool::Hand, _) => Cursor::Grab,
            (Tool::Razor, Hit::Clip { .. }) => Cursor::Crosshair,
            (Tool::Slip, Hit::Clip { .. }) => Cursor::ResizeHorizontal,
            (Tool::Select | Tool::Ripple, Hit::Clip { edge: Some(_), .. }) => Cursor::ResizeHorizontal,
            (_, Hit::Cut { .. }) => Cursor::ResizeHorizontal,
            (_, Hit::Header(..) | Hit::Marker(_) | Hit::Transition { .. }) => Cursor::Pointer,
            _ => Cursor::Default,
        };
    }

    // --- drops -------------------------------------------------------------------
    let mut drop_line = None;
    if zone.hovered {
        let t = g.time_at(zone.pointer.x).round_to_frame(seq.format.rate);
        drop_line = Some(t);
    }
    if let Some(p) = zone.dropped {
        let t = g.time_at(zone.pointer.x).round_to_frame(seq.format.rate);
        let track = g.row_at(&rows, zone.pointer.y).map(|r| r.id);
        let insert = ui.input().modifiers.logo || ui.input().modifiers.ctrl;
        match p.kind() {
            ASSET_PAYLOAD => {
                if let Ok(asset) = p.take::<AssetId>() {
                    app.place_asset(asset, t, track, insert);
                }
            }
            EFFECT_PAYLOAD => {
                if let Ok(plugin) = p.take::<PluginRef>() {
                    let target = track.and_then(|tr| seq.track(tr)).and_then(|(_, tr)| tr.clip_at(t).map(|c| c.id));
                    match target {
                        Some(c) => {
                            app.view.selection = vec![c];
                            add_effect(app, c, &plugin);
                        }
                        None => app.errors.push("Drop effects onto a clip.".into()),
                    }
                }
            }
            TRANSITION_PAYLOAD => {
                if let Ok(plugin) = p.take::<PluginRef>() {
                    let exact = g.time_at(zone.pointer.x);
                    let near = |at: Time| ((at - exact).as_seconds_f64() as f32 * g.pps).abs();
                    let target = track.and_then(|tr| seq.track(tr)).and_then(|(_, tr)| match tr.clip_at(exact) {
                        // On a clip: its nearer edge.
                        Some(c) => Some(if near(c.timeline_start) <= near(c.timeline_range().end()) { (c.id, Edge::Start) } else { (c.id, Edge::End) }),
                        // In a gap: the nearest edge, if it is close.
                        None => tr
                            .clips
                            .iter()
                            .flat_map(|c| [(c.timeline_start, c.id, Edge::Start), (c.timeline_range().end(), c.id, Edge::End)])
                            .filter(|(at, ..)| near(*at) < 40.0)
                            .min_by(|a, b| near(a.0).total_cmp(&near(b.0)))
                            .map(|(_, c, e)| (c, e)),
                    });
                    match target {
                        Some((clip, edge)) => app.add_transition(clip, edge, plugin),
                        None => app.errors.push("Drop transitions onto a clip's edge.".into()),
                    }
                }
            }
            crate::graphics::TEMPLATE_PAYLOAD => {
                if let Ok(i) = p.take::<usize>() {
                    let all = crate::graphics::templates();
                    if let Some(tpl) = all.get(i) {
                        // Onto the video track under the pointer, else the top free one.
                        let video = track.filter(|t| seq.track(*t).is_some_and(|(_, tr)| tr.kind == TrackKind::Video));
                        app.add_title(&tpl.params, t, video);
                    }
                }
            }
            GENERATOR_PAYLOAD => {
                if let Ok(plugin) = p.take::<PluginRef>() {
                    app.new_generator(&plugin, t, track);
                }
            }
            _ => {
                if let Ok(paths) = p.take::<Vec<std::path::PathBuf>>() {
                    app.engine.import(paths.iter().map(|p| p.to_string_lossy().into_owned()).collect());
                }
            }
        }
    }

    // Keep the playhead in view while playing.
    if app.engine.is_playing() {
        let x = app.playhead.as_seconds_f64() as f32 * app.view.pps - app.view.scroll_x;
        if x > g.view_w * 0.9 || x < 0.0 {
            app.view.scroll_x = (app.playhead.as_seconds_f64() as f32 * app.view.pps - g.view_w * 0.1).clamp(0.0, max_x);
        }
    }

    // --- paint ---------------------------------------------------------------
    let (draw_proj, valid) = match &preview {
        Some((p, v)) => (p, *v),
        None => (&*snap_proj, true),
    };
    let draw_seq = draw_proj.sequence(seq.id).cloned().unwrap_or_else(|| Arc::new((*seq).clone()));
    let shot = Shot::new(ui, app, &draw_seq, draw_proj, &rows, &g, content_w, content_h, valid, drop_line);
    ui.add_leaf(id, Layout::leaf(Size::Grow(1.0), Size::Grow(1.0)), Vec2::ZERO, true, move |p, rect| shot.paint(p, rect));
}

/// A press in the track area, by tool.
#[allow(clippy::too_many_arguments)]
fn press_tool(app: &mut EditorUi, proj: &Project, seq: &Sequence, g: &Geo, hit: Hit, pos: Vec2, shift: bool, alt: bool) -> Option<Drag> {
    let rate = seq.format.rate;
    let at = g.time_at(pos.x).round_to_frame(rate);
    let anchor = g.time_at(pos.x);
    let select = |app: &mut EditorUi, clip: ClipId| {
        let group = if app.view.linked { edit::linked(proj, clip) } else { vec![clip] };
        if shift {
            if app.view.selection.contains(&clip) {
                app.view.selection.retain(|c| !group.contains(c));
            } else {
                app.view.selection.extend(group);
            }
        } else if !app.view.selection.contains(&clip) {
            app.view.selection = group;
        }
        // The clicked clip is the one Effect Controls shows.
        if let Some(i) = app.view.selection.iter().position(|c| *c == clip) {
            app.view.selection.swap(0, i);
        }
    };
    match (app.view.tool, hit) {
        (Tool::Razor, Hit::Clip { clip, .. }) => {
            // Snap the cut to the playhead when close.
            let at = if ((at - app.playhead).as_seconds_f64() as f32 * g.pps).abs() < SNAP_PX { app.playhead } else { at };
            let r = edit::razor(proj, &[clip], at, app.view.linked && !alt);
            app.run_edit(r);
            None
        }
        (Tool::TrackSelect, Hit::Clip { track, .. } | Hit::Empty(track)) => {
            app.view.selection = seq
                .tracks
                .iter()
                .filter(|t| shift || t.id == track)
                .flat_map(|t| t.clips.iter())
                .filter(|c| c.timeline_range().end() > at)
                .map(|c| c.id)
                .collect();
            None
        }
        (Tool::Rolling | Tool::Select | Tool::Ripple, Hit::Cut { left, right }) => {
            app.view.selection = vec![left];
            match app.view.tool {
                Tool::Rolling => Some(Drag::Roll { left, right, anchor }),
                Tool::Ripple => Some(Drag::Trim { clip: left, edge: Edge::End, ripple: true, anchor }),
                _ => Some(Drag::Trim { clip: left, edge: Edge::End, ripple: false, anchor }),
            }
        }
        (Tool::Select | Tool::Ripple | Tool::Rolling, Hit::Clip { clip, edge: Some(edge), .. }) => {
            select(app, clip);
            Some(Drag::Trim { clip, edge, ripple: app.view.tool == Tool::Ripple, anchor })
        }
        (Tool::Slip, Hit::Clip { clip, .. }) => {
            select(app, clip);
            Some(Drag::Slip { clip, anchor })
        }
        (_, Hit::Clip { clip, track, .. }) => {
            select(app, clip);
            Some(Drag::Move { clip, anchor, track })
        }
        (_, Hit::Empty(_)) => {
            if !shift {
                app.view.selection.clear();
            }
            None
        }
        _ => None,
    }
}

/// Header buttons: lock, target patch, eye / mute, solo, mono / stereo.
fn header_click(app: &mut EditorUi, seq: &Sequence, track: TrackId, x: f32) {
    let Some((_, t)) = seq.track(track) else { return };
    let mut st = ve_engine::TrackState::of(t);
    let video = t.kind == TrackKind::Video;
    match x {
        x if (5.0..23.0).contains(&x) => st.locked = !st.locked,
        x if (28.0..48.0).contains(&x) => {
            if !app.view.targeted.remove(&track) {
                app.view.targeted.insert(track);
            }
            return;
        }
        x if (84.0..104.0).contains(&x) => {
            if video {
                st.enabled = !st.enabled
            } else {
                st.muted = !st.muted
            }
        }
        x if (104.0..124.0).contains(&x) && !video => st.solo = !st.solo,
        x if (126.0..144.0).contains(&x) && !video => {
            app.toggle_layout(track);
            return;
        }
        _ => return,
    }
    app.run(Command::SetTrackState { sequence: seq.id, track, state: st });
}

// ---- paint ------------------------------------------------------------------

/// Everything the paint closure needs, copied out before it runs.
struct Shot {
    rows: Vec<RowShot>,
    clips: Vec<ClipShot>,
    ticks: Vec<(f32, FrameText)>,
    track_x_off: f32,
    pps: f32,
    scroll_x: f32,
    scroll_y: f32,
    playhead: f32,
    work_end: f32,
    content_w: f32,
    content_h: f32,
    view_w: f32,
    view_h: f32,
    valid: bool,
    drop_line: Option<f32>,
    /// In and out points, seconds.
    mark_in: Option<f32>,
    mark_out: Option<f32>,
    /// Markers: seconds, colour, selected.
    markers: Vec<(f32, Color, bool)>,
    transitions: Vec<TransitionShot>,
    text: Color,
    faint: Color,
    accent: Color,
    size: f32,
}

struct RowShot {
    name: FrameText,
    top: f32,
    height: f32,
    video: bool,
    locked: bool,
    on: bool,
    solo: bool,
    targeted: bool,
    /// Audio tracks: "S" or "M".
    layout: Option<FrameText>,
}

struct TransitionShot {
    row: usize,
    /// Seconds.
    start: f32,
    end: f32,
    name: FrameText,
    selected: bool,
}

struct ClipShot {
    name: FrameText,
    row: usize,
    start: f32,
    len: f32,
    kind: ClipKind,
    fx: bool,
    enabled: bool,
    selected: bool,
    tint: Color,
    /// Source time at the clip's start, seconds.
    src_start: f32,
    /// Filmstrip cells: offset from the clip's left edge, and the thumbnail
    /// once it exists.
    cells: Vec<(f32, Option<TextureId>)>,
    peaks: Option<std::sync::Arc<Vec<f32>>>,
}

#[derive(Clone, Copy, PartialEq)]
enum ClipKind {
    Video,
    Audio,
    Title,
    Nest,
}

fn tint_of(name: &str) -> Color {
    let h = name.bytes().fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619));
    Color::hex(0x204060 + (h & 0x3f3f3f))
}

impl Shot {
    #[allow(clippy::too_many_arguments)]
    fn new(ui: &mut Ui, app: &mut EditorUi, seq: &Sequence, proj: &Project, rows: &[Row], g: &Geo, content_w: f32, content_h: f32, valid: bool, drop_line: Option<Time>) -> Self {
        let t = ui.theme.clone();
        let step = tick_step(g.pps);
        let first = (g.scroll_x / g.pps / step).floor() * step;
        let mut ticks = Vec::new();
        let mut time = first;
        while time * g.pps - g.scroll_x < g.view_w + 80.0 {
            let tc = Timecode::from_time(Time::from_seconds_f64(time as f64), seq.format.rate, true).to_string();
            ticks.push((time, ui.frame_text(&tc)));
            time += step;
        }
        let mut row_shots = Vec::new();
        let mut clips = Vec::new();
        let mut transitions = Vec::new();
        for (ri, row) in rows.iter().enumerate() {
            let Some((_, tr)) = seq.track(row.id) else { continue };
            row_shots.push(RowShot {
                name: ui.frame_text(&row.name),
                top: row.top,
                height: row.height,
                video: row.kind == TrackKind::Video,
                locked: tr.locked,
                on: if row.kind == TrackKind::Video { tr.enabled } else { !tr.muted },
                solo: tr.solo,
                targeted: app.view.targeted.contains(&row.id),
                layout: (row.kind == TrackKind::Audio).then(|| ui.frame_text(if tr.layout == ChannelLayout::Mono { "M" } else { "S" })),
            });
            for (clip, edge, a, b) in transition_spans(tr) {
                let name = tr
                    .clips
                    .iter()
                    .find(|c| c.id == clip)
                    .and_then(|c| if edge == Edge::Start { c.transition_in.clone() } else { c.transition_out.clone() })
                    .and_then(|t| app.st.plugins.find(&t.plugin).map(|i| i.name.clone()))
                    .unwrap_or_default();
                transitions.push(TransitionShot {
                    row: ri,
                    start: a.as_seconds_f64() as f32,
                    end: b.as_seconds_f64() as f32,
                    name: ui.frame_text(&name),
                    selected: app.view.selected_transition == Some((clip, edge)),
                });
            }
            for c in &tr.clips {
                let kind = match (&c.source, row.kind) {
                    (ClipSource::Generator { .. }, _) => ClipKind::Title,
                    (ClipSource::Sequence { .. }, _) => ClipKind::Nest,
                    (_, TrackKind::Video) => ClipKind::Video,
                    (_, TrackKind::Audio) => ClipKind::Audio,
                };
                let asset_name = match &c.source {
                    ClipSource::Asset { asset, .. } => proj.assets.get(asset).map(|a| a.name.clone()).unwrap_or_default(),
                    _ => c.name.clone(),
                };
                let fx = c.effects.iter().any(|e| {
                    !e.plugin.id.starts_with("ve.") || e.params.values().any(|p| matches!(p, Param::Animated(_)))
                });
                let asset = match &c.source {
                    ClipSource::Asset { asset, .. } => proj.assets.get(asset).cloned(),
                    _ => None,
                };
                let start = c.timeline_start.as_seconds_f64() as f32;
                let len = c.source_range.duration.as_seconds_f64() as f32;
                let src_start = c.source_range.start.as_seconds_f64() as f32;
                // Thumbnails for the filmstrip cells that are on screen.
                let mut cells = Vec::new();
                if let (ClipKind::Video, Some(a)) = (kind, &asset) {
                    let body_h = (row.height - 1.0 - 15.0).max(1.0);
                    let cell_w = body_h * 16.0 / 9.0;
                    let clip_x = start * g.pps - g.scroll_x; // relative to the track area
                    for k in 0..256 {
                        let off = k as f32 * cell_w;
                        if off >= len * g.pps || clip_x + off > g.view_w {
                            break;
                        }
                        if clip_x + off + cell_w < 0.0 {
                            continue;
                        }
                        let t = Time::from_seconds_f64((src_start + off / g.pps) as f64);
                        cells.push((off, app.thumb_tex(a, t)));
                    }
                }
                let peaks = match (kind, &asset, &c.source) {
                    (ClipKind::Audio, Some(a), ClipSource::Asset { audio_stream, .. }) => app.peaks(a, *audio_stream as usize),
                    _ => None,
                };
                // The name, with what is unusual about its timing, and the
                // angle a multicam clip shows.
                let mut label = c.name.clone();
                if let ClipSource::Sequence { angle: Some(a), .. } = &c.source {
                    label = format!("{label} · Angle {}", a + 1);
                }
                if let Some(b) = speed_badge(&c.retime) {
                    label = format!("{label} [{b}]");
                }
                clips.push(ClipShot {
                    name: ui.frame_text(&label),
                    row: ri,
                    start,
                    len,
                    kind,
                    fx,
                    enabled: c.enabled && tr.enabled,
                    selected: app.view.selection.contains(&c.id),
                    tint: tint_of(&asset_name),
                    src_start,
                    cells,
                    peaks,
                });
            }
        }
        Shot {
            rows: row_shots,
            clips,
            ticks,
            track_x_off: HEADER_W,
            pps: g.pps,
            scroll_x: g.scroll_x,
            scroll_y: g.scroll_y,
            playhead: app.playhead.as_seconds_f64() as f32,
            work_end: seq.duration().as_seconds_f64() as f32,
            content_w,
            content_h,
            view_w: g.view_w,
            view_h: g.view_h,
            valid,
            drop_line: drop_line.map(|t| t.as_seconds_f64() as f32),
            mark_in: seq.marks.in_point.map(|t| t.as_seconds_f64() as f32),
            mark_out: seq.marks.out_point.map(|t| t.as_seconds_f64() as f32),
            markers: seq
                .marks
                .markers
                .iter()
                .map(|m| (m.time.as_seconds_f64() as f32, marker_color(m.color), app.view.selected_marker == Some(m.id)))
                .collect(),
            transitions,
            text: t.palette.text,
            faint: t.palette.text_faint,
            accent: t.palette.accent,
            size: t.metrics.font_size,
        }
    }

    fn paint(&self, p: &mut Painter, rect: Rect) {
        let track_x = rect.x + self.track_x_off;
        let tracks_y = rect.y + RULER_H;
        let view = Rect::new(track_x, tracks_y, self.view_w, self.view_h);
        p.rect(rect, REEL.track_bg, 0.0);
        self.ruler(p, rect, track_x);
        self.tracks(p, view, track_x, tracks_y);
        self.headers(p, rect, tracks_y);
        self.playhead(p, rect, track_x);
        self.scrollbars(p, rect);
    }

    fn ruler(&self, p: &mut Painter, rect: Rect, track_x: f32) {
        let r = Rect::new(rect.x, rect.y, rect.w, RULER_H);
        p.rect(r, REEL.ruler_bg, 0.0);
        p.rect(Rect::new(rect.x, r.bottom() - 1.0, rect.w, 1.0), REEL.line, 0.0);
        if let Some(clip) = p.draw.clip().intersect(&Rect::new(track_x, r.y, self.view_w, r.h)) {
            p.draw.push_clip(clip);
            // The work area: the sequence's length.
            p.rect(Rect::new(track_x - self.scroll_x, r.y, self.work_end * self.pps, 3.0), REEL.in_out.with_alpha(0.85), 0.0);
            for (time, label) in &self.ticks {
                let x = (track_x + time * self.pps - self.scroll_x).round();
                p.rect(Rect::new(x, r.y + 6.0, 1.0, RULER_H - 7.0), REEL.tick, 0.0);
                p.text_left(Rect::new(x + 4.0, r.y + 5.0, 90.0, 14.0), self.size - 1.0, self.faint, *label);
            }
            let xs = |t: f32| (track_x + t * self.pps - self.scroll_x).round();
            if let Some((a, b)) = self.in_out_span() {
                p.rect(Rect::new(xs(a), r.y + 3.0, xs(b) - xs(a), RULER_H - 4.0), REEL.in_out_range, 0.0);
                // Brackets: [ at the in, ] at the out.
                if let Some(t) = self.mark_in {
                    let x = xs(t);
                    p.rect(Rect::new(x, r.y + 3.0, 2.0, RULER_H - 4.0), REEL.in_out, 0.0);
                    p.rect(Rect::new(x, r.y + 3.0, 6.0, 2.0), REEL.in_out, 0.0);
                    p.rect(Rect::new(x, r.bottom() - 3.0, 6.0, 2.0), REEL.in_out, 0.0);
                }
                if let Some(t) = self.mark_out {
                    let x = xs(t);
                    p.rect(Rect::new(x - 2.0, r.y + 3.0, 2.0, RULER_H - 4.0), REEL.in_out, 0.0);
                    p.rect(Rect::new(x - 6.0, r.y + 3.0, 6.0, 2.0), REEL.in_out, 0.0);
                    p.rect(Rect::new(x - 6.0, r.bottom() - 3.0, 6.0, 2.0), REEL.in_out, 0.0);
                }
            }
            // Markers: a flag along the ruler's foot.
            for (t, c, selected) in &self.markers {
                let x = xs(*t);
                let (top, mid) = (r.bottom() - 13.0, r.bottom() - 6.0);
                if *selected {
                    p.rect(Rect::new(x - 5.0, top - 1.0, 10.0, 8.0), REEL.selected, 1.0);
                }
                p.rect(Rect::new(x - 4.0, top, 8.0, mid - top), *c, 1.0);
                p.draw.triangle(Vec2::new(x - 4.0, mid), Vec2::new(x + 4.0, mid), Vec2::new(x, r.bottom() - 2.0), 0b110, *c);
            }
            p.draw.pop_clip();
        }
    }

    /// The in-to-out span in seconds, when either is set (to the sequence's
    /// ends otherwise).
    fn in_out_span(&self) -> Option<(f32, f32)> {
        if self.mark_in.is_none() && self.mark_out.is_none() {
            return None;
        }
        Some((self.mark_in.unwrap_or(0.0), self.mark_out.unwrap_or(self.work_end)))
    }

    fn tracks(&self, p: &mut Painter, view: Rect, track_x: f32, tracks_y: f32) {
        let Some(clip) = p.draw.clip().intersect(&view) else { return };
        p.draw.push_clip(clip);
        for (i, tr) in self.rows.iter().enumerate() {
            let y = tracks_y + tr.top - self.scroll_y;
            let bg = if i % 2 == 0 { REEL.track_bg } else { REEL.track_bg_alt };
            p.rect(Rect::new(track_x, y, self.view_w, tr.height), bg, 0.0);
            p.rect(Rect::new(track_x, y + tr.height, self.view_w, 1.0), REEL.line, 0.0);
        }
        for c in &self.clips {
            let Some(tr) = self.rows.get(c.row) else { continue };
            let x = track_x + c.start * self.pps - self.scroll_x;
            let w = c.len * self.pps;
            if x + w < view.x || x > view.right() {
                continue;
            }
            let y = tracks_y + tr.top - self.scroll_y;
            self.clip(p, Rect::new(x, y, w, tr.height - 1.0), c);
        }
        for t in &self.transitions {
            let Some(tr) = self.rows.get(t.row) else { continue };
            let x0 = track_x + t.start * self.pps - self.scroll_x;
            let x1 = track_x + t.end * self.pps - self.scroll_x;
            let y = tracks_y + tr.top - self.scroll_y;
            transition(p, Rect::new(x0, y + STRIP_H, (x1 - x0).max(4.0), (tr.height - 1.0 - STRIP_H).max(4.0)), t, self.size, self.accent);
        }
        // The in-to-out range, faintly across the tracks.
        if let Some((a, b)) = self.in_out_span() {
            let (xa, xb) = (track_x + a * self.pps - self.scroll_x, track_x + b * self.pps - self.scroll_x);
            p.rect(Rect::new(xa, view.y, xb - xa, view.h), REEL.in_out_range.with_alpha(REEL.in_out_range.a * 0.35), 0.0);
        }
        if let Some(t) = self.drop_line {
            let x = (track_x + t * self.pps - self.scroll_x).round();
            p.rect(Rect::new(x - 1.0, view.y, 2.0, view.h), Color::WHITE.with_alpha(0.8), 0.0);
        }
        if !self.valid {
            p.rect_bordered(view.shrink(1.0, 1.0, 1.0, 1.0), Color::TRANSPARENT, 0.0, 2.0, REEL.meter_hi);
        }
        p.draw.pop_clip();
    }

    fn clip(&self, p: &mut Painter, r: Rect, c: &ClipShot) {
        let (fill, head) = match c.kind {
            ClipKind::Video => (REEL.video_fill, REEL.video_head),
            ClipKind::Audio => (REEL.audio_fill, REEL.audio_head),
            ClipKind::Title => (REEL.title_fill, REEL.title_head),
            ClipKind::Nest => (REEL.nest_fill, REEL.nest_head),
        };
        let dim = if c.enabled { 1.0 } else { 0.45 };
        p.rect(r, fill.with_alpha(dim), 3.0);
        let strip = Rect::new(r.x, r.y, r.w, 15.0_f32.min(r.h));
        p.rect(strip, head.with_alpha(dim), 3.0);
        p.rect(Rect::new(r.x, strip.bottom() - 3.0, r.w, 3.0), head.with_alpha(dim), 0.0);
        if r.w > 26.0 {
            p.text_left(strip.shrink(5.0, 0.0, 18.0, 0.0), self.size - 1.0, REEL.clip_text, c.name);
        }
        if c.fx && r.w > 40.0 {
            let b = Rect::new(r.right() - 16.0, r.y + 2.0, 13.0, 11.0);
            p.rect(b, Color::rgba(0.0, 0.0, 0.0, 0.35), 2.0);
            draw_icon(p, b, Icon::Effects, Color::hex(0xc9e2f5));
        }
        let body = Rect::new(r.x, strip.bottom(), r.w, (r.bottom() - strip.bottom()).max(0.0));
        if body.h > 4.0 {
            match c.kind {
                ClipKind::Audio => waveform(p, body, c.peaks.as_ref().map(|v| v.as_slice()), c.src_start, self.pps),
                ClipKind::Title => p.rect(body.shrink(4.0, 3.0, 4.0, 3.0), Color::WHITE.with_alpha(0.10), 2.0),
                ClipKind::Nest => nest_body(p, body),
                ClipKind::Video => filmstrip(p, body, c.tint, &c.cells),
            }
        }
        let border = if c.selected { REEL.selected } else { REEL.clip_border };
        p.rect_bordered(r, Color::TRANSPARENT, 3.0, if c.selected { 2.0 } else { 1.0 }, border);
    }

    fn headers(&self, p: &mut Painter, rect: Rect, tracks_y: f32) {
        let col = Rect::new(rect.x, tracks_y, HEADER_W, self.view_h);
        let Some(clip) = p.draw.clip().intersect(&col) else { return };
        p.draw.push_clip(clip);
        p.rect(col, REEL.chrome_deep, 0.0);
        for tr in &self.rows {
            let y = tracks_y + tr.top - self.scroll_y;
            let r = Rect::new(col.x, y, HEADER_W, tr.height);
            p.rect(r, REEL.track_head, 0.0);
            p.rect(Rect::new(r.x, r.bottom(), r.w, 1.0), REEL.line, 0.0);
            let mid = r.y + 9.0;
            let icon = |p: &mut Painter, x: f32, icon: Icon, on: bool| {
                let b = Rect::new(r.x + x, mid, 16.0, 16.0);
                if on {
                    p.rect(b, REEL.raised_hi, 2.0);
                }
                draw_icon(p, b.shrink(3.0, 3.0, 3.0, 3.0), icon, if on { REEL.bright } else { REEL.label });
            };
            icon(p, 5.0, Icon::Lock, tr.locked);
            let patch = Rect::new(r.x + 28.0, mid, 20.0, 16.0);
            p.rect(patch, if tr.targeted { self.accent } else { REEL.raised }, 2.0);
            p.text_centered(patch, self.size - 1.0, if tr.targeted { Color::WHITE } else { self.faint }, tr.name);
            p.text_left(Rect::new(r.x + 54.0, mid, 30.0, 16.0), self.size, self.text, tr.name);
            if tr.video {
                icon(p, 86.0, Icon::Eye, tr.on);
            } else {
                icon(p, 86.0, Icon::Speaker, tr.on);
                icon(p, 106.0, Icon::Mic, tr.solo);
                if let Some(l) = tr.layout {
                    let b = Rect::new(r.x + 127.0, mid, 16.0, 16.0);
                    p.rect(b, REEL.raised, 2.0);
                    p.text_centered(b, self.size - 1.0, REEL.text_soft, l);
                }
            }
            if tr.locked {
                // Locked tracks wear diagonal hatching, as NLEs show them.
                let lane = Rect::new(rect.x + HEADER_W, r.y, self.view_w, r.h);
                let _ = lane;
            }
        }
        p.draw.pop_clip();
    }

    fn playhead(&self, p: &mut Painter, rect: Rect, track_x: f32) {
        let x = (track_x + self.playhead * self.pps - self.scroll_x).round();
        if x < track_x || x > track_x + self.view_w {
            return;
        }
        let area = Rect::new(track_x, rect.y, self.view_w, rect.h - BAR);
        let Some(clip) = p.draw.clip().intersect(&area) else { return };
        p.draw.push_clip(clip);
        p.rect(Rect::new(x - 0.5, rect.y, 1.0, area.h), REEL.playhead, 0.0);
        p.rect(Rect::new(x - 6.0, rect.y, 12.0, 11.0), REEL.playhead, 2.0);
        p.draw.pop_clip();
    }

    fn scrollbars(&self, p: &mut Painter, rect: Rect) {
        let track = Rect::new(rect.x + HEADER_W, rect.bottom() - BAR, self.view_w, BAR);
        p.rect(Rect::new(rect.x, track.y, rect.w, BAR), REEL.chrome_deep, 0.0);
        if self.content_w > self.view_w {
            let span = (self.view_w / self.content_w * self.view_w).max(28.0);
            let travel = (self.view_w - span).max(1.0);
            let t = (self.scroll_x / (self.content_w - self.view_w).max(1.0)).clamp(0.0, 1.0);
            p.rect(Rect::new(track.x + travel * t, track.y + 2.0, span, BAR - 4.0), REEL.tick, (BAR - 4.0) * 0.5);
        }
        let vt = Rect::new(rect.right() - BAR, rect.y + RULER_H, BAR, self.view_h);
        p.rect(vt, REEL.chrome_deep, 0.0);
        if self.content_h > self.view_h {
            let span = (self.view_h / self.content_h * self.view_h).max(28.0);
            let travel = (self.view_h - span).max(1.0);
            let t = (self.scroll_y / (self.content_h - self.view_h).max(1.0)).clamp(0.0, 1.0);
            p.rect(Rect::new(vt.x + 2.0, vt.y + travel * t, BAR - 4.0, span), REEL.tick, (BAR - 4.0) * 0.5);
        }
    }
}

/// A transition across the body of the clip(s) it joins: a light box with
/// the diagonal NLEs draw, and its name when there is room.
fn transition(p: &mut Painter, r: Rect, t: &TransitionShot, size: f32, accent: Color) {
    p.rect(r, REEL.transition.with_alpha(0.88), 2.0);
    p.line(Vec2::new(r.x + 1.0, r.bottom() - 1.0), Vec2::new(r.right() - 1.0, r.y + 1.0), 1.0, REEL.clip_border.with_alpha(0.55));
    if r.w > 60.0 && r.h > 12.0 {
        p.text_centered(r, size - 1.0, REEL.transition_text, t.name);
    }
    let (w, c) = if t.selected { (2.0, accent) } else { (1.0, REEL.clip_border) };
    p.rect_bordered(r, Color::TRANSPARENT, 2.0, w, c);
}

/// A nest's body: stacked bars, standing for the tracks inside.
fn nest_body(p: &mut Painter, r: Rect) {
    let inner = r.shrink(4.0, 3.0, 4.0, 3.0);
    let n = ((inner.h / 7.0) as usize).clamp(1, 4);
    for k in 0..n {
        let y = inner.y + k as f32 * (inner.h / n as f32);
        p.rect(Rect::new(inner.x, y, inner.w, (inner.h / n as f32 - 2.0).max(1.0)), Color::WHITE.with_alpha(0.08), 1.5);
    }
}

/// "200%", "Reverse 50%", "Hold", "Remap" — or nothing at normal speed.
fn speed_badge(r: &Retime) -> Option<String> {
    match r {
        Retime::Speed(s) if s.num == s.den => None,
        Retime::Speed(s) => {
            let pct = s.as_f64().abs() * 100.0;
            let pct = if (pct - pct.round()).abs() < 0.05 { format!("{pct:.0}%") } else { format!("{pct:.1}%") };
            Some(if s.num < 0 { format!("Reverse {pct}") } else { pct })
        }
        Retime::Remap(k) if k.len() == 1 => Some("Hold".into()),
        Retime::Remap(_) => Some("Remap".into()),
    }
}

/// A tick every frame, ¼ s, ½ s, 1, 2, 5 … s, whichever keeps labels apart.
fn tick_step(pps: f32) -> f32 {
    for step in [1.0 / 24.0, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 300.0] {
        if step * pps >= 76.0 {
            return step;
        }
    }
    600.0
}

/// The clip's waveform from its asset's peaks (100 per second): each 3 px
/// column shows the loudest peak it covers. Square-rooted, so quiet passages
/// still read. A flat line until the peaks are computed.
fn waveform(p: &mut Painter, r: Rect, peaks: Option<&[f32]>, src_start: f32, pps: f32) {
    let mid = r.center().y;
    p.rect(Rect::new(r.x, mid - 0.5, r.w, 1.0), REEL.wave.with_alpha(0.35), 0.0);
    let Some(peaks) = peaks else { return };
    let rate = ve_engine::PEAKS_PER_SECOND as f32;
    let (left, right) = (r.x.max(p.draw.clip().x - 3.0), r.right().min(p.draw.clip().right() + 3.0));
    let mut x = r.x + ((left - r.x) / 3.0).floor() * 3.0;
    while x < right {
        let t0 = src_start + (x - r.x) / pps;
        let t1 = src_start + (x + 3.0 - r.x) / pps;
        let (i0, i1) = ((t0 * rate) as usize, ((t1 * rate) as usize).max((t0 * rate) as usize + 1));
        let a = peaks.get(i0..i1.min(peaks.len())).map(|s| s.iter().cloned().fold(0.0, f32::max)).unwrap_or(0.0);
        let h = (a.sqrt() * r.h * 0.95).max(1.0);
        p.rect(Rect::new(x, mid - h * 0.5, 2.0, h), REEL.wave.with_alpha(0.8), 0.0);
        x += 3.0;
    }
}

/// Thumbnail cells along a video clip; a tinted placeholder where a thumbnail
/// is still being made.
fn filmstrip(p: &mut Painter, r: Rect, tint: Color, cells: &[(f32, Option<TextureId>)]) {
    let w = r.h * 16.0 / 9.0;
    if w < 6.0 {
        return;
    }
    for (off, tex) in cells {
        let cell = Rect::new(r.x + off, r.y, w.min(r.right() - r.x - off), r.h);
        if cell.w <= 0.0 {
            continue;
        }
        match tex {
            // Cropped at the clip's end rather than squeezed.
            Some(t) => p.image_uv(cell, *t, [0.0, 0.0, cell.w / w, 1.0], 0.0),
            None => {
                p.rect(Rect::new(cell.x, cell.y, cell.w, cell.h * 0.6), tint.lerp(Color::hex(0xbcd4e8), 0.55), 0.0);
                p.rect(Rect::new(cell.x, cell.y + cell.h * 0.6, cell.w, cell.h * 0.4), tint.lerp(Color::hex(0x1e3a22), 0.35), 0.0);
            }
        }
        p.rect(Rect::new(cell.right() - 1.0, cell.y, 1.0, cell.h), Color::rgba(0.0, 0.0, 0.0, 0.25), 0.0);
    }
}

/// The audio meters down the right edge: peak dBFS per channel.
fn meters(ui: &mut Ui, levels: [f32; 2]) {
    let id = ui.make_id("meters");
    let size = ui.theme.metrics.font_size_small;
    let marks = [0, -6, -12, -18, -24, -30, -36, -42, -48];
    let labels: Vec<FrameText> = marks.iter().map(|d| ui.frame_text(&d.to_string())).collect();
    let db = ui.frame_text("dB");
    let ss = ui.frame_text("S");
    ui.add_leaf(id, Layout::leaf(Size::Fixed(62.0), Size::Grow(1.0)), Vec2::ZERO, false, move |p, r| {
        p.rect(r, REEL.chrome, 0.0);
        p.rect(Rect::new(r.x, r.y, 1.0, r.h), REEL.line, 0.0);
        let top = r.y + 8.0;
        let bottom = r.bottom() - 30.0;
        let h = (bottom - top).max(10.0);
        for (i, label) in labels.iter().enumerate() {
            let y = top + h * (i as f32 / (labels.len() - 1) as f32);
            p.text_right(Rect::new(r.x + 24.0, y - 6.0, 34.0, 12.0), size - 1.0, REEL.label, *label);
            p.rect(Rect::new(r.x + 20.0, y, 3.0, 1.0), REEL.raised_hi, 0.0);
        }
        p.text_right(Rect::new(r.x + 24.0, bottom + 4.0, 34.0, 12.0), size - 1.0, REEL.label, db);
        for (i, level) in levels.iter().enumerate() {
            let bar = Rect::new(r.x + 5.0 + i as f32 * 8.0, top, 6.0, h);
            p.rect(bar, REEL.inset, 1.0);
            // The scale runs 0 dB at the top to -48 at the bottom.
            let lit = ((level + 48.0) / 48.0).clamp(0.0, 1.0) * h;
            if lit > 0.0 {
                let y0 = bar.bottom() - lit;
                let warn = top + h * (6.0 / 48.0); // -6 dB
                let clip = top + h * (0.5 / 48.0);
                p.rect(Rect::new(bar.x, y0.max(warn), bar.w, bar.bottom() - y0.max(warn)), REEL.meter_lo, 1.0);
                if y0 < warn {
                    p.rect(Rect::new(bar.x, y0.max(clip), bar.w, warn - y0.max(clip)), REEL.meter_mid, 0.0);
                }
                if y0 < clip {
                    p.rect(Rect::new(bar.x, y0, bar.w, clip - y0), REEL.meter_hi, 0.0);
                }
            }
        }
        for i in 0..2 {
            let b = Rect::new(r.x + 5.0 + i as f32 * 8.0, r.bottom() - 16.0, 6.0, 10.0);
            p.rect(b, REEL.raised, 1.0);
            p.text_centered(b, size - 2.0, REEL.label, ss);
        }
    });
}
