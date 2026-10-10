//! Audio Track Mixer: a strip per audio track (pan, mute, solo, fader, its
//! own meter) and the master (meters, loudness).
//!
//! Dragging a fader or pan knob is heard at once through the engine's
//! preview (no edit, no restart); letting go commits one `SetTrackState`,
//! one undo step.

use libgui::*;
use ve_engine::{Command, TrackState};
use ve_model::*;

use crate::theme::REEL;
use crate::EditorUi;

const STRIP_W: f32 = 92.0;
/// The fader's top, in dB.
const FADER_MAX: f32 = 6.0;

/// Fader travel (0 bottom, 1 top) for `db`: an audio taper, so the useful
/// range near unity gets most of the travel.
fn fader_pos(db: f32) -> f32 {
    if db <= -96.0 {
        return 0.0;
    }
    (10f32.powf(db / 20.0) / 10f32.powf(FADER_MAX / 20.0)).powf(0.25)
}

fn fader_db(pos: f32) -> f32 {
    if pos <= 0.001 {
        return -96.0;
    }
    (20.0 * (pos.powi(4) * 10f32.powf(FADER_MAX / 20.0)).log10()).clamp(-96.0, FADER_MAX)
}

fn db_text(db: f32) -> String {
    if db <= -96.0 {
        "-∞".into()
    } else {
        format!("{db:.1}")
    }
}

pub(crate) fn panel(ui: &mut Ui, app: &mut EditorUi) {
    let Some(seq) = app.active_seq() else {
        ui.label_muted("No sequence.");
        return;
    };
    let tracks: Vec<Track> = seq.tracks.iter().filter(|t| t.kind == TrackKind::Audio).map(|t| (**t).clone()).collect();
    let row = Layout::row().width(Size::Grow(1.0)).height(Size::Grow(1.0)).gap(1.0);
    ui.container(row, Frame { fill: REEL.line, clip: true, ..Frame::none() }, |ui| {
        let opts = ScrollOptions { gap: 1.0, ..ScrollOptions::horizontal(Size::Grow(1.0), Size::Grow(1.0)) };
        ui.scroll_area_with("mixer-strips", opts, |ui| {
            ui.container(Layout::row().height(Size::Grow(1.0)).gap(1.0), Frame::none(), |ui| {
                for t in &tracks {
                    strip(ui, app, seq.id, t);
                }
            });
        });
        master(ui, app);
    });
}

fn strip(ui: &mut Ui, app: &mut EditorUi, sequence: SequenceId, track: &Track) {
    let t = ui.theme.clone();
    // What the strip shows: the live drag, else the project.
    let (volume, pan) = match app.view.track_drag {
        Some((id, v, p)) if id == track.id => (v, p),
        _ => (track.volume_db, track.pan),
    };
    let meter = app.track_db.get(&track.id).copied().unwrap_or([-96.0; 2]);
    let col = Layout::column().width(Size::Fixed(STRIP_W)).height(Size::Grow(1.0)).padding(Insets::xy(6.0, 6.0)).gap(6.0).align(Align::Start, Align::Center);
    let strip_id = ui.make_id(("strip", track.id));
    ui.container_id(strip_id, col, Frame { fill: REEL.panel, ..Frame::none() }, |ui| {
        // Pan.
        let pan_r = knob(ui, ("pan", track.id), pan);
        ui.text_with(&pan_text(pan), t.metrics.font_size_small, REEL.timecode);
        // Mute and solo.
        let mut state = TrackState::of(track);
        let changed = ui.container(Layout::row().width(Size::Fit).gap(4.0).height(Size::Fixed(20.0)), Frame::none(), |ui| {
            let m = toggle(ui, ("m", track.id), "M", track.muted, REEL.meter_mid);
            let s = toggle(ui, ("s", track.id), "S", track.solo, REEL.meter_lo);
            if m {
                state.muted = !state.muted;
            }
            if s {
                state.solo = !state.solo;
            }
            m || s
        });
        if changed {
            app.run(Command::SetTrackState { sequence, track: track.id, state });
        }
        // Fader and meter.
        let fader_r = fader(ui, ("fader", track.id), volume, meter);
        ui.text_with(&db_text(volume), t.metrics.font_size_small, REEL.timecode);
        ui.text_with(&track.name, t.metrics.font_size, t.palette.text);

        // Live while dragging; one commit on release; double-click resets.
        let mut live = None;
        if let Some(pos) = fader_r.value {
            live = Some((fader_db(pos), pan));
        }
        if let Some(p) = pan_r.value {
            live = Some((volume, p));
        }
        if fader_r.reset {
            live = Some((0.0, pan));
        }
        if pan_r.reset {
            live = Some((volume, 0.0));
        }
        if let Some((v, p)) = live {
            app.view.track_drag = Some((track.id, v, p));
            app.engine.preview_track(track.id, Some((v, p)));
        }
        if fader_r.released || pan_r.released || fader_r.reset || pan_r.reset {
            if let Some((id, v, p)) = app.view.track_drag.take().filter(|d| d.0 == track.id) {
                let state = TrackState { volume_db: v, pan: p, ..TrackState::of(track) };
                app.run(Command::SetTrackState { sequence, track: id, state });
                app.engine.preview_track(id, None);
            }
        }
    });
}

fn pan_text(pan: f32) -> String {
    let v = (pan * 100.0).round() as i32;
    match v {
        0 => "C".into(),
        v if v < 0 => format!("L{}", -v),
        v => format!("R{v}"),
    }
}

/// What a drag did this frame.
#[derive(Default)]
struct DragResult {
    /// The new value while dragging.
    value: Option<f32>,
    released: bool,
    /// Double-clicked: back to the default.
    reset: bool,
}

/// A pan knob: drag up/right to turn right; -1..1.
fn knob(ui: &mut Ui, key: impl std::hash::Hash, value: f32) -> DragResult {
    let id = ui.make_id(key);
    let r = ui.interact_drag(id);
    let mut out = DragResult { released: r.released, reset: r.double_clicked, ..Default::default() };
    if r.active && (r.drag_delta.x != 0.0 || r.drag_delta.y != 0.0) {
        out.value = Some((value + (r.drag_delta.x - r.drag_delta.y) / 100.0).clamp(-1.0, 1.0));
    }
    if r.hovered {
        ui.cursor = Cursor::ResizeHorizontal;
    }
    ui.add_leaf(id, Layout::leaf(Size::Fixed(34.0), Size::Fixed(34.0)), Vec2::ZERO, true, move |p, rect| {
        let c = rect.center();
        let rad = rect.w / 2.0 - 2.0;
        // The dial, and the arc from centre to the value.
        let n = 40;
        for i in 0..n {
            let a0 = std::f32::consts::PI * 0.75 + i as f32 / n as f32 * std::f32::consts::PI * 1.5;
            let a1 = a0 + std::f32::consts::PI * 1.5 / n as f32;
            p.line(Vec2::new(c.x + rad * a0.cos(), c.y + rad * a0.sin()), Vec2::new(c.x + rad * a1.cos(), c.y + rad * a1.sin()), 2.0, REEL.raised_hi);
        }
        let angle = |v: f32| -std::f32::consts::FRAC_PI_2 + v * std::f32::consts::PI * 0.75;
        let (from, to) = (angle(0.0).min(angle(value)), angle(0.0).max(angle(value)));
        let steps = ((to - from) / 0.08).ceil().max(1.0) as usize;
        for i in 0..steps {
            let a0 = from + (to - from) * i as f32 / steps as f32;
            let a1 = from + (to - from) * (i + 1) as f32 / steps as f32;
            p.line(Vec2::new(c.x + rad * a0.cos(), c.y + rad * a0.sin()), Vec2::new(c.x + rad * a1.cos(), c.y + rad * a1.sin()), 2.0, REEL.timecode);
        }
        let a = angle(value);
        p.line(c, Vec2::new(c.x + (rad - 4.0) * a.cos(), c.y + (rad - 4.0) * a.sin()), 2.0, REEL.bright);
    });
    out
}

fn toggle(ui: &mut Ui, key: impl std::hash::Hash, label: &str, on: bool, color: Color) -> bool {
    let id = ui.make_id(key);
    let r = ui.interact(id);
    let hot = ui.animate_bool(id, 0, r.hovered);
    let text = ui.frame_text(label);
    let size = ui.theme.metrics.font_size_small;
    ui.add_leaf(id, Layout::leaf(Size::Fixed(22.0), Size::Fixed(18.0)), Vec2::ZERO, true, move |p, rect| {
        let fill = if on { color } else { REEL.raised.lerp(REEL.raised_hi, hot) };
        p.rect(rect, fill, 2.0);
        p.text_centered(rect, size, if on { Color::BLACK } else { REEL.text_soft }, text);
    });
    r.clicked
}

/// A fader with the track's stereo meter beside it; `value` in dB.
fn fader(ui: &mut Ui, key: impl std::hash::Hash, value: f32, meter: [f32; 2]) -> DragResult {
    let id = ui.make_id(key);
    let r = ui.interact_drag(id);
    let mut out = DragResult { released: r.released, reset: r.double_clicked, ..Default::default() };
    let travel = (r.rect.h - 16.0).max(1.0);
    if r.active && r.drag_delta.y != 0.0 {
        out.value = Some((fader_pos(value) - r.drag_delta.y / travel).clamp(0.0, 1.0));
    }
    if r.hovered {
        ui.cursor = Cursor::ResizeVertical;
    }
    let size = ui.theme.metrics.font_size_small;
    let marks: Vec<(f32, FrameText)> = [6.0, 0.0, -6.0, -12.0, -24.0, -48.0].iter().map(|d| (*d, ui.frame_text(&format!("{d:.0}")))).collect();
    ui.add_leaf(id, Layout::leaf(Size::Fixed(76.0), Size::Grow(1.0)), Vec2::ZERO, true, move |p, rect| {
        let (top, h) = (rect.y + 8.0, (rect.h - 16.0).max(1.0));
        let y_of = |pos: f32| top + h * (1.0 - pos);
        // Scale.
        for (d, label) in &marks {
            let y = y_of(fader_pos(*d)).round();
            p.rect(Rect::new(rect.x + 22.0, y, 6.0, 1.0), REEL.tick, 0.0);
            p.text_right(Rect::new(rect.x, y - 6.0, 20.0, 12.0), size - 1.0, REEL.label, *label);
        }
        // The slot and the cap.
        let slot = Rect::new(rect.x + 32.0, top, 4.0, h);
        p.rect(slot, REEL.inset, 2.0);
        let cy = y_of(fader_pos(value));
        let cap = Rect::new(slot.center().x - 9.0, cy - 6.0, 18.0, 12.0);
        p.rect_bordered(cap, REEL.raised_hi, 2.0, 1.0, REEL.line);
        p.rect(Rect::new(cap.x + 3.0, cy - 0.5, cap.w - 6.0, 1.0), REEL.bright, 0.0);
        // The meter, -48 to 0 dBFS, same colours as the master's.
        for (i, db) in meter.iter().enumerate() {
            let bar = Rect::new(rect.x + 50.0 + i as f32 * 8.0, top, 6.0, h);
            p.rect(bar, REEL.inset, 1.0);
            let lit = ((db + 48.0) / 48.0).clamp(0.0, 1.0) * h;
            if lit > 0.0 {
                let color = if *db > -0.5 { REEL.meter_hi } else if *db > -6.0 { REEL.meter_mid } else { REEL.meter_lo };
                p.rect(Rect::new(bar.x, bar.bottom() - lit, bar.w, lit), color, 1.0);
            }
        }
    });
    out
}

/// The master: what the whole mix peaks at, and how loud it is.
fn master(ui: &mut Ui, app: &mut EditorUi) {
    let t = ui.theme.clone();
    let meter = app.meter_db;
    let l = app.loudness;
    let col = Layout::column().width(Size::Fixed(120.0)).height(Size::Grow(1.0)).padding(Insets::xy(8.0, 6.0)).gap(6.0).align(Align::Start, Align::Center);
    ui.container(col, Frame { fill: REEL.chrome, ..Frame::none() }, |ui| {
        ui.text_with("Master", t.metrics.font_size, t.palette.text);
        let id = ui.make_id("master-meter");
        ui.add_leaf(id, Layout::leaf(Size::Fixed(40.0), Size::Grow(1.0)), Vec2::ZERO, false, move |p, rect| {
            for (i, db) in meter.iter().enumerate() {
                let bar = Rect::new(rect.x + 6.0 + i as f32 * 16.0, rect.y, 12.0, rect.h);
                p.rect(bar, REEL.inset, 1.0);
                let lit = ((db + 48.0) / 48.0).clamp(0.0, 1.0) * rect.h;
                if lit > 0.0 {
                    let color = if *db > -0.5 { REEL.meter_hi } else if *db > -6.0 { REEL.meter_mid } else { REEL.meter_lo };
                    p.rect(Rect::new(bar.x, bar.bottom() - lit, bar.w, lit), color, 1.0);
                }
            }
        });
        let lufs = |v: f32| if v.is_finite() { format!("{v:.1}") } else { "—".into() };
        for (name, v, unit) in [("M", l.momentary, "LUFS"), ("S", l.short_term, "LUFS"), ("I", l.integrated, "LUFS"), ("TP", l.true_peak, "dBTP")] {
            let color = if name == "I" { REEL.timecode } else { t.palette.text_muted };
            ui.container(Layout::row().width(Size::Grow(1.0)).height(Size::Fixed(14.0)).gap(4.0), Frame::none(), |ui| {
                ui.text_with(name, t.metrics.font_size_small, t.palette.text_muted);
                ui.flex();
                ui.text_with(&format!("{} {unit}", lufs(v)), t.metrics.font_size_small, color);
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fader_taper_round_trips() {
        for db in [6.0, 0.0, -6.0, -20.0, -60.0] {
            assert!((fader_db(fader_pos(db)) - db).abs() < 0.01, "{db}");
        }
        assert_eq!(fader_pos(-96.0), 0.0);
        assert!((fader_pos(FADER_MAX) - 1.0).abs() < 1e-6);
        assert!(fader_pos(0.0) > 0.8, "unity sits high, with room above");
    }
}
