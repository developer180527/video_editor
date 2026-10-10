//! The editing features beyond cuts: markers and in/out points, transitions,
//! speed and frame holds, nesting and multicam, generators and titles,
//! proxies and relinking, audio channel layouts — the actions behind them,
//! their dialogs, and the menu bar that reaches them all.
//!
//! Every action builds a command (or an edit tool's batch) and sends it, so
//! each is one undo step; nothing here changes the project itself.

use std::sync::Arc;

use libgui::*;
use ve_engine::{edit, intrinsic, Command, Edge, MarksOwner};
use ve_model::*;
use ve_time::Time;

use crate::EditorUi;

/// A dialog in front of the editor.
#[derive(Clone, Debug, PartialEq)]
pub enum Dialog {
    /// Speed/Duration for a clip: speed in percent, reversed, ripple.
    Speed { clip: ClipId, percent: f32, reverse: bool, ripple: bool },
    /// Edit (or create) a sequence marker.
    Marker { marker: Marker },
    /// Name the new sequence for "Nest…".
    Nest { name: String },
    /// Which source channels an audio clip plays.
    Channels { clip: ClipId, picked: Vec<bool> },
}

/// Marker colours as they are drawn.
pub fn marker_color(c: MarkerColor) -> Color {
    match c {
        MarkerColor::Green => Color::hex(0x58b85c),
        MarkerColor::Red => Color::hex(0xd94c4c),
        MarkerColor::Purple => Color::hex(0xa66bd6),
        MarkerColor::Orange => Color::hex(0xe8923a),
        MarkerColor::Yellow => Color::hex(0xe0c341),
        MarkerColor::White => Color::hex(0xe6e6e6),
        MarkerColor::Blue => Color::hex(0x4a90e2),
        MarkerColor::Cyan => Color::hex(0x4ac8d8),
    }
}

const COLORS: [MarkerColor; 8] =
    [MarkerColor::Green, MarkerColor::Red, MarkerColor::Purple, MarkerColor::Orange, MarkerColor::Yellow, MarkerColor::White, MarkerColor::Blue, MarkerColor::Cyan];
const COLOR_NAMES: [&str; 8] = ["Green", "Red", "Purple", "Orange", "Yellow", "White", "Blue", "Cyan"];

/// Payload kind for a transition dragged from the Effects panel, carrying `PluginRef`.
pub const TRANSITION_PAYLOAD: &str = "transition";
/// Payload kind for a generator dragged from the Effects panel, carrying its id (`String`).
pub const GENERATOR_PAYLOAD: &str = "generator";


impl EditorUi {
    pub(crate) fn active_seq(&self) -> Option<Arc<Sequence>> {
        self.snap().active().cloned()
    }

    /// The clip on a targeted track of `kind` under the playhead.
    fn targeted_clip_at(&self, kind: TrackKind, t: Time) -> Option<Arc<Clip>> {
        let seq = self.active_seq()?;
        seq.tracks.iter().filter(|tr| tr.kind == kind && self.view.targeted.contains(&tr.id)).find_map(|tr| tr.clip_at(t).cloned())
    }

    /// The selected clip, else the clip under the playhead on a targeted
    /// video track, else on a targeted audio track.
    pub(crate) fn subject_clip(&self) -> Option<Arc<Clip>> {
        if let Some(c) = self.view.selection.first().and_then(|id| self.snap().find_clip(*id)).map(|(_, _, c)| c.clone()) {
            return Some(c);
        }
        self.targeted_clip_at(TrackKind::Video, self.playhead).or_else(|| self.targeted_clip_at(TrackKind::Audio, self.playhead))
    }

    // ---- markers and in/out ------------------------------------------------

    /// Whose marks I, O and ⌥X set: the clip in the Source monitor when it
    /// is the active monitor, else the sequence.
    fn marks_owner(&self) -> Option<(MarksOwner, Marks)> {
        match (self.viewer(), self.engine.source()) {
            (ve_engine::Viewer::Source, Some(s)) => {
                let a = self.snap().assets.get(&s.asset)?;
                Some((MarksOwner::Asset(a.id), a.marks.clone()))
            }
            _ => self.active_seq().map(|s| (MarksOwner::Sequence(s.id), s.marks.clone())),
        }
    }

    fn set_marks(&mut self, f: impl FnOnce(&mut Marks)) {
        let Some((owner, published)) = self.marks_owner() else { return };
        // Marks sent a moment ago may not be published yet: build on them,
        // or a quick I then O would send O with the old In and lose it.
        let old = match &self.pending_marks {
            Some((o, m, at)) if *o == owner && *m != published && at.elapsed().as_secs_f32() < 1.0 => m.clone(),
            _ => published,
        };
        let mut marks = old.clone();
        f(&mut marks);
        // Keep in ≤ out: a new in after the out (or out before the in) clears the other.
        if let (Some(a), Some(b)) = (marks.in_point, marks.out_point) {
            if a > b {
                if old.in_point == marks.in_point {
                    marks.in_point = None;
                } else {
                    marks.out_point = None;
                }
            }
        }
        self.pending_marks = Some((owner, marks.clone(), std::time::Instant::now()));
        self.run(Command::SetMarks { owner, marks });
    }

    pub(crate) fn mark_in(&mut self) {
        let (t, _, _) = self.active_position();
        self.set_marks(|m| m.in_point = Some(t));
    }

    pub(crate) fn mark_out(&mut self) {
        // The out point is the end of the frame under the playhead.
        let (t, rate, _) = self.active_position();
        let t = rate.frame_to_time(t.to_frame(rate) + 1);
        self.set_marks(|m| m.out_point = Some(t));
    }

    pub(crate) fn clear_in_out(&mut self) {
        self.set_marks(|m| {
            m.in_point = None;
            m.out_point = None;
        });
    }

    /// A marker at the playhead, numbered.
    pub(crate) fn add_marker(&mut self) {
        let Some(seq) = self.active_seq() else { return };
        let t = self.playhead;
        if seq.marks.markers.iter().any(|m| m.time == t) {
            return;
        }
        let marker = Marker {
            id: MarkerId::new(),
            time: t,
            duration: Time::ZERO,
            name: format!("Marker {}", seq.marks.markers.len() + 1),
            comment: String::new(),
            color: MarkerColor::Green,
            kind: MarkerKind::Comment,
        };
        self.view.selected_marker = Some(marker.id);
        self.set_marks(|m| {
            let i = m.markers.iter().position(|x| x.time > t).unwrap_or(m.markers.len());
            m.markers.insert(i, marker);
        });
    }

    /// Replace a marker (matched by id), or remove it with `None`.
    pub(crate) fn put_marker(&mut self, id: MarkerId, marker: Option<Marker>) {
        self.set_marks(|m| {
            m.markers.retain(|x| x.id != id);
            if let Some(k) = marker {
                let i = m.markers.iter().position(|x| x.time > k.time).unwrap_or(m.markers.len());
                m.markers.insert(i, k);
            }
        });
    }

    pub(crate) fn edit_marker(&mut self, id: MarkerId) {
        if let Some(m) = self.active_seq().and_then(|s| s.marks.markers.iter().find(|m| m.id == id).cloned()) {
            self.view.dialog = Some(Dialog::Marker { marker: m });
        }
    }

    /// Move the playhead to the next (or previous) marker.
    pub(crate) fn go_to_marker(&mut self, forward: bool) {
        let Some(seq) = self.active_seq() else { return };
        let t = self.playhead;
        let next = if forward { seq.marks.markers.iter().find(|m| m.time > t) } else { seq.marks.markers.iter().rev().find(|m| m.time < t) };
        if let Some(m) = next.cloned() {
            self.view.selected_marker = Some(m.id);
            self.seek(m.time);
        }
    }

    // ---- transitions -------------------------------------------------------

    /// A new transition of `plugin` for `clip`'s `edge` on `track`, one
    /// second long: centred on a cut, at the clip for a fade, never longer
    /// than the clips allow.
    fn new_transition(&self, track: &Track, clip: &Clip, edge: Edge, plugin: PluginRef) -> Option<(ClipId, Edge, Transition)> {
        let i = track.clips.iter().position(|c| c.id == clip.id)?;
        let full = Time::from_seconds_f64(self.settings().transition_seconds as f64).round_to_frame(self.rate());
        // The tail of a clip followed directly by another is that clip's cut.
        let (owner, edge) = match edge {
            Edge::End => match track.clips.get(i + 1).filter(|n| n.timeline_start == clip.timeline_range().end()) {
                Some(n) => (n.clone(), Edge::Start),
                None => (track.clips[i].clone(), Edge::End),
            },
            Edge::Start => (track.clips[i].clone(), Edge::Start),
        };
        let j = track.clips.iter().position(|c| c.id == owner.id)?;
        let dur = owner.source_range.duration;
        let (before, after) = match edge {
            Edge::Start => match j.checked_sub(1).map(|p| &track.clips[p]).filter(|p| p.timeline_range().end() == owner.timeline_start) {
                // A cut: centred, half on each side, within both clips.
                Some(prev) => {
                    let half = Time(full.ticks() / 2);
                    let room_before = prev.source_range.duration - prev.transition_in.as_ref().map_or(Time::ZERO, |t| t.after);
                    let room_after = dur - owner.transition_out.as_ref().map_or(Time::ZERO, |t| t.before);
                    (half.min(room_before).max(Time::ZERO), half.min(room_after).max(Time::ZERO))
                }
                // From nothing: starts at the clip.
                None => (Time::ZERO, full.min(dur - owner.transition_out.as_ref().map_or(Time::ZERO, |t| t.before))),
            },
            Edge::End => (full.min(dur - owner.transition_in.as_ref().map_or(Time::ZERO, |t| t.after)), Time::ZERO),
        };
        if before + after <= Time::ZERO {
            return None;
        }
        Some((owner.id, edge, Transition { id: EffectId::new(), plugin, before, after, params: OrdMap::new() }))
    }

    /// Put a `plugin` transition at `clip`'s `edge`.
    pub(crate) fn add_transition(&mut self, clip: ClipId, edge: Edge, plugin: PluginRef) {
        let Some((seq, ti, c)) = self.snap().find_clip(clip).map(|(s, ti, c)| (s.clone(), ti, c.clone())) else { return };
        match self.new_transition(&seq.tracks[ti], &c, edge, plugin) {
            Some((owner, edge, tr)) => {
                self.view.selected_transition = Some((owner, edge));
                let r = edit::set_transition(self.snap(), owner, edge, Some(tr));
                self.run_edit(r);
            }
            None => self.errors.push("No room for a transition there.".into()),
        }
    }

    /// The default transition (Cross Dissolve, or Constant Power for audio)
    /// at the edit point nearest the playhead on the targeted tracks of
    /// `kind` — or at the selected clips' heads.
    pub(crate) fn apply_default_transition(&mut self, kind: TrackKind) {
        let plugin = intrinsic::plugin_ref(if kind == TrackKind::Video { intrinsic::DISSOLVE } else { intrinsic::CROSSFADE });
        self.apply_transition(kind, plugin);
    }

    /// `plugin` at the selected clips' heads (those on `kind` tracks), else
    /// at the edit point nearest the playhead on the targeted `kind` tracks.
    pub(crate) fn apply_transition(&mut self, kind: TrackKind, plugin: PluginRef) {
        let Some(seq) = self.active_seq() else { return };
        let selected: Vec<ClipId> = self
            .view
            .selection
            .iter()
            .filter(|id| self.snap().find_clip(**id).is_some_and(|(s, ti, _)| s.tracks[ti].kind == kind))
            .copied()
            .collect();
        if !selected.is_empty() {
            for c in selected {
                self.add_transition(c, Edge::Start, plugin.clone());
            }
            return;
        }
        let t = self.playhead;
        let nearest = seq
            .tracks
            .iter()
            .filter(|tr| tr.kind == kind && self.view.targeted.contains(&tr.id))
            .flat_map(|tr| tr.clips.iter().flat_map(|c| [(c.timeline_start, c.id, Edge::Start), (c.timeline_range().end(), c.id, Edge::End)]))
            .min_by_key(|(at, ..)| (*at - t).ticks().abs());
        match nearest {
            Some((_, clip, edge)) => self.add_transition(clip, edge, plugin),
            None => self.errors.push("No edit point on the targeted tracks.".into()),
        }
    }

    pub(crate) fn remove_transition(&mut self, clip: ClipId, edge: Edge) {
        self.view.selected_transition = None;
        let r = edit::set_transition(self.snap(), clip, edge, None);
        self.run_edit(r);
    }

    // ---- speed, holds ------------------------------------------------------

    pub(crate) fn open_speed_dialog(&mut self) {
        let Some(c) = self.subject_clip() else {
            self.errors.push("Select a clip to change its speed.".into());
            return;
        };
        let (percent, reverse) = match c.retime {
            Retime::Speed(r) => ((r.as_f64().abs() * 100.0) as f32, r.num < 0),
            Retime::Remap(_) => (100.0, false),
        };
        self.view.dialog = Some(Dialog::Speed { clip: c.id, percent, reverse, ripple: false });
    }

    /// Speed as an exact ratio: whole hundredths of a percent. With linked
    /// selection on, the clip's linked partners change with it, so picture
    /// and sound stay in sync — one undo step.
    pub(crate) fn apply_speed(&mut self, clip: ClipId, percent: f32, reverse: bool, ripple: bool) {
        let num = (percent.clamp(1.0, 10000.0) * 100.0).round() as i64;
        let ratio = Ratio::new(if reverse { -num } else { num }, 10_000);
        let group = if self.view.linked { edit::linked(self.snap(), clip) } else { vec![clip] };
        let mut proj: Project = (**self.snap()).clone();
        let mut commands = Vec::new();
        for id in group {
            let step = edit::set_speed(&proj, id, ratio, ripple).and_then(|c| Ok((c.apply(&proj)?.project, c)));
            match step {
                Ok((next, c)) => {
                    proj = next;
                    commands.push(c);
                }
                Err(e) => return self.run_edit(Err(e)),
            }
        }
        self.run(Command::Batch { label: "Speed/Duration".into(), commands });
    }

    pub(crate) fn frame_hold(&mut self) {
        let Some(c) = self.targeted_clip_at(TrackKind::Video, self.playhead).or_else(|| self.subject_clip()) else {
            self.errors.push("No clip under the playhead to hold.".into());
            return;
        };
        let r = edit::frame_hold(self.snap(), c.id, self.playhead);
        self.run_edit(r);
    }

    // ---- nesting, multicam -------------------------------------------------

    pub(crate) fn open_nest_dialog(&mut self) {
        if self.view.selection.is_empty() {
            self.errors.push("Select the clips to nest.".into());
            return;
        }
        let n = self.snap().sequences.len() + 1;
        self.view.dialog = Some(Dialog::Nest { name: format!("Nested Sequence {n}") });
    }

    pub(crate) fn nest(&mut self, name: &str) {
        let Some(seq) = self.active_seq() else { return };
        let plugins = self.st.plugins.clone();
        let format = seq.format.clone();
        let sel = std::mem::take(&mut self.view.selection);
        let r = edit::nest(self.snap(), seq.id, &sel, name, |s, k, l| ve_engine::make_sequence_clip(&plugins, &format, s, k, l));
        self.run_edit(r);
    }

    /// Open a sequence in the timeline (a nest, from a double-click).
    pub(crate) fn open_sequence(&mut self, id: SequenceId) {
        self.view.selection.clear();
        self.view.selected_transition = None;
        self.run(Command::SetActiveSequence { sequence: Some(id) });
    }

    /// Multicam: from the playhead, the clip on the targeted video track
    /// shows angle `n` (counting from 1).
    pub(crate) fn switch_angle(&mut self, n: u32) {
        let Some(c) = self.targeted_clip_at(TrackKind::Video, self.playhead).filter(|c| matches!(c.source, ClipSource::Sequence { .. })) else {
            return;
        };
        let r = edit::switch_angle(self.snap(), c.id, self.playhead, n - 1);
        self.run_edit(r);
    }

    // ---- generators --------------------------------------------------------

    /// A new generator clip (`ve.color`, `ve.bars`, `ve.title`) at `at` on
    /// `track` (the targeted video track when `None`).
    pub(crate) fn new_generator(&mut self, id: &str, at: Time, track: Option<TrackId>) {
        let Some(seq) = self.active_seq() else { return };
        let Some(track) = track
            .filter(|t| seq.track(*t).is_some_and(|(_, tr)| tr.kind == TrackKind::Video))
            .or_else(|| seq.tracks.iter().find(|t| t.kind == TrackKind::Video && self.view.targeted.contains(&t.id)).map(|t| t.id))
        else {
            self.errors.push("Target a video track first.".into());
            return;
        };
        let duration = Time::from_seconds_f64(self.settings().still_seconds as f64).round_to_frame(self.rate());
        let Some(clip) = ve_engine::make_generator_clip(&self.st.plugins, &seq.format, &intrinsic::plugin_ref(id), duration) else { return };
        self.view.selection = vec![clip.id];
        let r = edit::overwrite(self.snap(), seq.id, at, &[(track, Arc::new(clip))]);
        self.run_edit(r);
    }

    // ---- media -------------------------------------------------------------

    pub(crate) fn remove_proxy(&mut self, asset: AssetId) {
        if let Some(a) = self.snap().assets.get(&asset) {
            let variants = a.variants.iter().filter(|v| v.kind != VariantKind::Proxy).cloned().collect();
            self.run(Command::SetAssetVariants { asset, variants });
        }
    }

    /// Proxies on or off for playback (export always uses originals).
    pub(crate) fn toggle_proxies(&mut self) {
        self.view.proxies = !self.view.proxies;
    }

    // ---- audio channels ----------------------------------------------------

    pub(crate) fn toggle_layout(&mut self, track: TrackId) {
        let Some(seq) = self.active_seq() else { return };
        let Some((_, t)) = seq.track(track) else { return };
        let layout = if t.layout == ChannelLayout::Stereo { ChannelLayout::Mono } else { ChannelLayout::Stereo };
        self.run(Command::SetTrackLayout { sequence: seq.id, track, layout });
    }

    /// The channel count of an audio clip's source stream, if known.
    pub(crate) fn source_channels(&self, clip: &Clip) -> Option<u16> {
        match &clip.source {
            ClipSource::Asset { asset, audio_stream } => self.snap().assets.get(asset)?.info.as_ref()?.audio.get(*audio_stream as usize).map(|s| s.channels),
            _ => None,
        }
    }

    pub(crate) fn open_channels_dialog(&mut self) {
        let Some(c) = self.subject_clip() else { return };
        let Some(n) = self.source_channels(&c) else {
            self.errors.push("Select an audio clip.".into());
            return;
        };
        let picked = (0..n).map(|i| c.channels.is_empty() || c.channels.contains(&i)).collect();
        self.view.dialog = Some(Dialog::Channels { clip: c.id, picked });
    }

    fn apply_channels(&mut self, clip: ClipId, picked: &[bool]) {
        let Some((_, _, c)) = self.snap().find_clip(clip) else { return };
        let mut c = (**c).clone();
        let chosen: Vec<u16> = picked.iter().enumerate().filter(|(_, on)| **on).map(|(i, _)| i as u16).collect();
        // All of them is the same as "all": keep the default.
        c.channels = if chosen.len() == picked.len() { Vec::new() } else { chosen };
        self.run(Command::SetClip { clip: Arc::new(c) });
    }

    // ---- dialogs -----------------------------------------------------------

    /// The open dialog, if any.
    pub(crate) fn dialogs(&mut self, ui: &mut Ui) {
        let Some(dialog) = self.view.dialog.clone() else { return };
        let opts = ModalOptions { width: 380.0, ..Default::default() };
        match dialog {
            Dialog::Speed { clip, mut percent, mut reverse, mut ripple } => {
                let dur = self.snap().find_clip(clip).map(|(_, _, c)| c.source_range.duration);
                let r = ui.modal("speed", "Clip Speed / Duration", &opts, |ui| {
                    ui.drag_value_range("Speed (%)", &mut percent, 1.0, 1.0..=10000.0);
                    if let Some(d) = dur {
                        // What the duration will become at this speed.
                        let now = self.snap().find_clip(clip).map(|(_, _, c)| c.media_extent().duration).unwrap_or(d);
                        let new = Time::from_seconds_f64(now.as_seconds_f64() * 100.0 / percent.max(1.0) as f64);
                        ui.label_muted(&format!("Duration: {}", self.timecode(new)));
                    }
                    ui.checkbox("Reverse Speed", &mut reverse);
                    ui.checkbox("Ripple Edit, Shifting Trailing Clips", &mut ripple);
                    ui.space(8.0);
                    buttons(ui)
                });
                self.view.dialog = Some(Dialog::Speed { clip, percent, reverse, ripple });
                let (cancel, ok) = r.inner;
                if ok || r.submitted {
                    self.view.dialog = None;
                    self.apply_speed(clip, percent, reverse, ripple);
                } else if cancel || r.cancelled || r.clicked_outside {
                    self.view.dialog = None;
                }
            }
            Dialog::Marker { mut marker } => {
                let mut color = COLORS.iter().position(|c| *c == marker.color).unwrap_or(0);
                let mut chapter = marker.kind == MarkerKind::Chapter;
                let tc = self.timecode(marker.time);
                let r = ui.modal("marker", "Marker", &opts, |ui| {
                    ui.label_muted(&format!("At {tc}"));
                    ui.text_input("marker-name", &mut marker.name, "Name");
                    ui.text_input("marker-comment", &mut marker.comment, "Comment");
                    ui.combo("Color", &mut color, &COLOR_NAMES);
                    ui.checkbox("Chapter marker", &mut chapter);
                    ui.space(8.0);
                    ui.row(|ui| {
                        let delete = ui.button("Delete").clicked;
                        ui.flex();
                        (delete, ui.button("Cancel").clicked, ui.button_primary("OK").clicked)
                    })
                });
                marker.color = COLORS[color.min(7)];
                marker.kind = if chapter { MarkerKind::Chapter } else { MarkerKind::Comment };
                let id = marker.id;
                self.view.dialog = Some(Dialog::Marker { marker: marker.clone() });
                let (delete, cancel, ok) = r.inner;
                if delete {
                    self.view.dialog = None;
                    self.put_marker(id, None);
                } else if ok || r.submitted {
                    self.view.dialog = None;
                    self.put_marker(id, Some(marker));
                } else if cancel || r.cancelled || r.clicked_outside {
                    self.view.dialog = None;
                }
            }
            Dialog::Nest { mut name } => {
                let r = ui.modal("nest", "Nested Sequence Name", &opts, |ui| {
                    ui.text_input("nest-name", &mut name, "Name");
                    ui.space(8.0);
                    buttons(ui)
                });
                self.view.dialog = Some(Dialog::Nest { name: name.clone() });
                let (cancel, ok) = r.inner;
                if ok || r.submitted {
                    self.view.dialog = None;
                    self.nest(if name.trim().is_empty() { "Nested Sequence" } else { name.trim() });
                } else if cancel || r.cancelled || r.clicked_outside {
                    self.view.dialog = None;
                }
            }
            Dialog::Channels { clip, mut picked } => {
                let layout = self.snap().find_clip(clip).map(|(s, ti, _)| s.tracks[ti].layout).unwrap_or_default();
                let r = ui.modal("channels", "Audio Channels", &opts, |ui| {
                    ui.label_muted(&format!("Source channels mixed to this {} track:", if layout == ChannelLayout::Mono { "mono" } else { "stereo" }));
                    for (i, on) in picked.iter_mut().enumerate() {
                        ui.checkbox_keyed(("ch", i), &format!("Channel {}", i + 1), on);
                    }
                    ui.space(8.0);
                    buttons(ui)
                });
                self.view.dialog = Some(Dialog::Channels { clip, picked: picked.clone() });
                let (cancel, ok) = r.inner;
                if (ok || r.submitted) && picked.iter().any(|p| *p) {
                    self.view.dialog = None;
                    self.apply_channels(clip, &picked);
                } else if cancel || r.cancelled || r.clicked_outside {
                    self.view.dialog = None;
                }
            }
        }
    }
}

/// A dialog's Cancel / OK row: `(cancel, ok)`.
fn buttons(ui: &mut Ui) -> (bool, bool) {
    ui.row(|ui| {
        ui.flex();
        (ui.button("Cancel").clicked, ui.button_primary("OK").clicked)
    })
}
