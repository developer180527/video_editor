//! The Source monitor: one clip from the bin, to watch, mark in and out,
//! and cut into the sequence — the other half of three-point editing.
//!
//! Double-click a clip in the bin to open it here. Its in and out are the
//! asset's own marks, so they stay with the clip. Insert (,) and Overwrite
//! (.) put the marked part into the sequence; the edit's position and
//! length come from three of the four points (source in/out, sequence
//! in/out, or the playhead), the way every NLE does it.

use std::sync::Arc;

use libgui::*;
use ve_engine::Viewer;
use ve_model::*;
use ve_time::{Time, TimeRange};

use crate::theme::REEL;
use crate::widgets::{divider, icon_button, Icon};
use crate::{EditorUi, ASSET_PAYLOAD};

/// Where a three-point edit lands and how much of the source it takes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ThreePoint {
    /// Where in the sequence.
    pub at: Time,
    /// Which part of the source.
    pub source: TimeRange,
}

/// The three-point rules. `source` is the clip's in/out (either may be
/// unset), `media` its length; `sequence` the sequence's in/out; `playhead`
/// the Program monitor's.
///
/// - Source in/out given: that much, at the sequence in (else the playhead),
///   or ending at the sequence out when only that is set (backtiming).
/// - Sequence in and out given and no source out: the sequence range's
///   length, from the source in.
pub(crate) fn three_point(source: (Option<Time>, Option<Time>), media: Time, sequence: (Option<Time>, Option<Time>), playhead: Time) -> Option<ThreePoint> {
    let src_in = source.0.unwrap_or(Time::ZERO).clamp_to(Time::ZERO, media);
    let src_out = source.1.unwrap_or(media).clamp_to(src_in, media);
    let marked = src_out - src_in;
    let (at, len) = match sequence {
        (Some(i), Some(o)) if source.1.is_none() && o > i => (i, (o - i).min(media - src_in)),
        (Some(i), _) => (i, marked),
        (None, Some(o)) => ((o - marked).max(Time::ZERO), marked),
        (None, None) => (playhead, marked),
    };
    (len > Time::ZERO).then(|| ThreePoint { at, source: TimeRange::new(src_in, len) })
}

impl EditorUi {
    /// Open `asset` in the Source monitor and bring the monitor forward.
    pub(crate) fn open_in_source(&mut self, asset: AssetId) {
        self.engine.set_source(asset);
        if let Some(at) = self.dock_mut().find_tab(|t| *t == crate::Tab::Source) {
            self.dock_mut().focus_tab(at);
        }
    }

    /// Insert (or overwrite) the Source monitor's marked part into the
    /// sequence by the three-point rules, then park the playhead after it.
    pub(crate) fn edit_from_source(&mut self, insert: bool) {
        let Some(s) = self.engine.source() else {
            self.errors.push("Open a clip in the Source monitor first (double-click it in the bin).".into());
            return;
        };
        let Some(a) = self.snap().assets.get(&s.asset).cloned() else { return };
        let Some(seq) = self.active_seq() else { return };
        let marks = (seq.marks.in_point, seq.marks.out_point);
        let Some(edit) = three_point((a.marks.in_point, a.marks.out_point), s.duration(), marks, self.playhead) else {
            self.errors.push("Nothing marked to edit in.".into());
            return;
        };
        self.place_range(a.id, edit.source, edit.at, None, insert);
        // After the edit lands: a seek now would be clamped to the old length.
        self.seek_after_edit = Some((edit.at + edit.source.duration, std::time::Instant::now()));
    }
}

pub(crate) fn panel(ui: &mut Ui, app: &mut EditorUi) {
    app.source_wanted = true;
    let t = ui.theme.clone();
    let source = app.engine.source();
    let asset = source.as_ref().and_then(|s| app.snap().assets.get(&s.asset).cloned());
    let col = Layout::column().width(Size::Grow(1.0)).height(Size::Grow(1.0));
    ui.container(col, Frame { fill: t.palette.bg_panel, clip: true, ..Frame::none() }, |ui| {
        picture(ui, app, asset.as_ref());
        readout(ui, app, asset.as_ref());
        scrub(ui, app, asset.as_ref());
        transport(ui, app);
    });
}

fn picture(ui: &mut Ui, app: &mut EditorUi, asset: Option<&Arc<Asset>>) {
    let id = ui.make_id("source-picture");
    let r = ui.interact_drag(id);
    // Drag the picture onto the timeline: the marked part goes in.
    if let Some(a) = asset {
        let (asset_id, label) = (a.id, a.name.clone());
        ui.drag_source_from(&r, move || Payload::new(ASSET_PAYLOAD, asset_id).with_label(label));
        if r.clicked {
            app.engine.set_viewer(Viewer::Source);
            if app.engine.is_playing() {
                app.engine.stop();
            } else {
                app.engine.play(1.0);
            }
        }
    }
    let size = asset.and_then(|a| a.info.as_ref()).and_then(|i| i.video.as_ref()).map(|v| (v.width as f32, v.height as f32)).unwrap_or((16.0, 9.0));
    let tex = asset.and(app.source_tex);
    let audio_only = asset.and_then(|a| a.info.as_ref()).is_some_and(|i| i.video.is_none());
    let hint = ui.frame_text(match (asset, audio_only) {
        (None, _) => "Double-click a clip in the Project panel to open it here.",
        (Some(_), true) => "Audio only",
        _ => "",
    });
    let faint = ui.theme.palette.text_faint;
    let fs = ui.theme.metrics.font_size;
    ui.add_leaf(id, Layout::leaf(Size::Grow(1.0), Size::Grow(1.0)), Vec2::ZERO, true, move |p, rect| {
        p.rect(rect, REEL.line, 0.0);
        let (fw, fh) = size;
        let scale = (rect.w / fw).min(rect.h / fh);
        let (w, h) = (fw * scale, fh * scale);
        let f = Rect::new((rect.center().x - w * 0.5).round(), (rect.center().y - h * 0.5).round(), w.round(), h.round());
        match tex {
            Some(tex) if !audio_only => p.image(f, tex, 0.0),
            _ => p.rect(f, Color::BLACK, 0.0),
        }
        p.text_centered(rect, fs, faint, hint);
    });
}

fn readout(ui: &mut Ui, app: &mut EditorUi, asset: Option<&Arc<Asset>>) {
    let t = ui.theme.clone();
    let row = Layout::row().width(Size::Grow(1.0)).height(Size::Fixed(26.0)).padding(Insets::xy(10.0, 0.0)).gap(8.0).align(Align::Start, Align::Center);
    let duration = app.engine.source().map(|s| s.duration()).unwrap_or(Time::ZERO);
    let marked = asset.map(|a| a.marks.out_point.unwrap_or(duration) - a.marks.in_point.unwrap_or(Time::ZERO)).unwrap_or(Time::ZERO);
    let active = app.viewer() == Viewer::Source;
    ui.container(row, Frame::none(), |ui| {
        ui.text_with(&app.timecode(app.source_playhead), 12.0, if active { REEL.timecode } else { t.palette.text_muted });
        ui.space(6.0);
        if let Some(a) = asset {
            ui.text_with(&a.name, t.metrics.font_size, t.palette.text);
        }
        ui.flex();
        // The marked duration, as Premiere shows it.
        ui.text_with(&app.timecode(marked), 12.0, t.palette.text_muted);
    });
}

/// Click or drag to move the Source playhead; the in/out range is shaded.
fn scrub(ui: &mut Ui, app: &mut EditorUi, asset: Option<&Arc<Asset>>) {
    let id = ui.make_id("source-scrub");
    let r = ui.interact_drag(id);
    let end = app.engine.source().map(|s| s.duration()).unwrap_or(Time::ZERO).max(Time::from_seconds(1));
    let rate = app.engine.source().map(|s| s.sequence.format.rate).unwrap_or(app.rate());
    if (r.active || r.pressed) && asset.is_some() {
        let f = ((r.mouse_pos.x - r.rect.x - 6.0) / (r.rect.w - 12.0).max(1.0)).clamp(0.0, 1.0);
        app.engine.stop();
        app.seek_source(Time::from_seconds_f64(end.as_seconds_f64() * f as f64).round_to_frame(rate));
    }
    if r.hovered {
        ui.cursor = Cursor::ResizeHorizontal;
    }
    let at = move |t: Time| (t.as_seconds_f64() / end.as_seconds_f64()).clamp(0.0, 1.0) as f32;
    let frac = at(app.source_playhead);
    let marks = asset.map(|a| (a.marks.in_point.map(at), a.marks.out_point.map(at))).unwrap_or((None, None));
    ui.add_leaf(id, Layout::leaf(Size::Grow(1.0), Size::Fixed(22.0)), Vec2::ZERO, true, move |p, rect| {
        let bar = Rect::new(rect.x + 6.0, rect.y + 8.0, rect.w - 12.0, 5.0);
        p.rect(bar, REEL.raised_hi, 2.5);
        if marks.0.is_some() || marks.1.is_some() {
            let a = bar.x + bar.w * marks.0.unwrap_or(0.0);
            let b = bar.x + bar.w * marks.1.unwrap_or(1.0);
            p.rect(Rect::new(a, bar.y - 2.0, (b - a).max(1.0), bar.h + 4.0), REEL.in_out_range, 0.0);
            if marks.0.is_some() {
                p.rect(Rect::new(a, bar.y - 3.0, 2.0, bar.h + 6.0), REEL.in_out, 0.0);
            }
            if marks.1.is_some() {
                p.rect(Rect::new(b - 2.0, bar.y - 3.0, 2.0, bar.h + 6.0), REEL.in_out, 0.0);
            }
        }
        let x = bar.x + bar.w * frac;
        p.rect(Rect::new(x - 5.0, rect.y + 3.0, 10.0, 14.0), REEL.playhead, 2.0);
    });
}

fn transport(ui: &mut Ui, app: &mut EditorUi) {
    let row = Layout::row().width(Size::Grow(1.0)).height(Size::Fixed(34.0)).padding(Insets::xy(8.0, 0.0)).gap(2.0).align(Align::Center, Align::Center);
    let playing = app.viewer() == Viewer::Source && app.engine.is_playing();
    let has = app.engine.source().is_some();
    ui.container(row, Frame { fill: REEL.chrome, ..Frame::none() }, |ui| {
        let tip = |ui: &mut Ui, r: &Response, s: &str| ui.tooltip(r, s);
        // Every button here acts on the Source monitor.
        let act = |app: &mut EditorUi| {
            app.engine.set_viewer(Viewer::Source);
        };
        let r = icon_button(ui, "src-in", Icon::MarkIn, 22.0, false);
        tip(ui, &r, "Mark In (I)");
        if r.clicked && has {
            act(app);
            app.mark_in();
        }
        let r = icon_button(ui, "src-out", Icon::MarkOut, 22.0, false);
        tip(ui, &r, "Mark Out (O)");
        if r.clicked && has {
            act(app);
            app.mark_out();
        }
        divider(ui, "src-t1", true, 18.0);
        let in_point = app.engine.source().and_then(|s| app.snap().assets.get(&s.asset).and_then(|a| a.marks.in_point));
        let r = icon_button(ui, "src-go-in", Icon::JumpStart, 22.0, false);
        tip(ui, &r, "Go to In (Shift+I)");
        if r.clicked && has {
            app.seek_source(in_point.unwrap_or(Time::ZERO));
        }
        let r = icon_button(ui, "src-back", Icon::StepBack, 22.0, false);
        tip(ui, &r, "Step Back (Left)");
        if r.clicked && has {
            act(app);
            app.nudge_source(-1);
        }
        let r = icon_button(ui, "src-play", if playing { Icon::Pause } else { Icon::Play }, 26.0, playing);
        tip(ui, &r, "Play/Stop (Space) · Shuttle (J/K/L)");
        if r.clicked && has {
            act(app);
            if playing {
                app.engine.stop();
            } else {
                app.engine.play(1.0);
            }
        }
        let r = icon_button(ui, "src-fwd", Icon::StepForward, 22.0, false);
        tip(ui, &r, "Step Forward (Right)");
        if r.clicked && has {
            act(app);
            app.nudge_source(1);
        }
        let out_point = app.engine.source().and_then(|s| app.snap().assets.get(&s.asset).and_then(|a| a.marks.out_point));
        let end = app.engine.source().map(|s| s.duration()).unwrap_or(Time::ZERO);
        let r = icon_button(ui, "src-go-out", Icon::JumpEnd, 22.0, false);
        tip(ui, &r, "Go to Out (Shift+O)");
        if r.clicked && has {
            app.seek_source(out_point.unwrap_or(end));
        }
        divider(ui, "src-t2", true, 18.0);
        let r = icon_button(ui, "src-insert", Icon::Insert, 22.0, false);
        tip(ui, &r, "Insert (,)");
        if r.clicked {
            app.edit_from_source(true);
        }
        let r = icon_button(ui, "src-overwrite", Icon::Overwrite, 22.0, false);
        tip(ui, &r, "Overwrite (.)");
        if r.clicked {
            app.edit_from_source(false);
        }
    });
}

impl EditorUi {
    fn nudge_source(&mut self, frames: i64) {
        let Some(s) = self.engine.source() else { return };
        let r = s.sequence.format.rate;
        self.engine.stop();
        self.seek_source(r.frame_to_time(self.source_playhead.to_frame(r) + frames));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(x: i64) -> Time {
        Time::from_seconds(x)
    }

    #[test]
    fn three_point_rules() {
        let media = s(20);
        // Source in/out, nothing on the sequence: at the playhead.
        let e = three_point((Some(s(2)), Some(s(5))), media, (None, None), s(10)).unwrap();
        assert_eq!((e.at, e.source), (s(10), TimeRange::new(s(2), s(3))));
        // Sequence in wins over the playhead.
        let e = three_point((Some(s(2)), Some(s(5))), media, (Some(s(4)), None), s(10)).unwrap();
        assert_eq!(e.at, s(4));
        // Sequence in and out, source in only: the sequence range's length.
        let e = three_point((Some(s(2)), None), media, (Some(s(4)), Some(s(10))), s(0)).unwrap();
        assert_eq!((e.at, e.source), (s(4), TimeRange::new(s(2), s(6))));
        // ...never past the end of the media.
        let e = three_point((Some(s(18)), None), media, (Some(s(4)), Some(s(10))), s(0)).unwrap();
        assert_eq!(e.source, TimeRange::new(s(18), s(2)));
        // Only a sequence out: backtimed, ending there.
        let e = three_point((Some(s(2)), Some(s(5))), media, (None, Some(s(10))), s(0)).unwrap();
        assert_eq!(e.at, s(7));
        // Nothing marked: the whole clip.
        let e = three_point((None, None), media, (None, None), s(1)).unwrap();
        assert_eq!(e.source, TimeRange::new(s(0), s(20)));
    }
}
