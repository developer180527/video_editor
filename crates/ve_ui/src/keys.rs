//! Keyframes in Effect Controls' lanes: the diamonds right of an animated
//! row. Click one to select it and move the playhead there; drag it along
//! (on frames, between its neighbours; one undo step); right-click (or
//! touch and hold) for its interpolation or to delete it. Elsewhere in the
//! lane a click or drag scrubs, as in any lane.

use std::sync::Arc;

use libgui::*;
use ve_model::*;
use ve_time::{Rate, Time};

use crate::effects::Span;
use crate::graph::GraphKey;
use crate::theme::REEL;
use crate::{drag_from, EditorUi};

/// How close (px) the pointer must be to a diamond to take it.
const PICK: f32 = 6.0;

/// The interpolations a key can leave by, as the menus name them.
pub(crate) const INTERPS: [(&str, Interp); 5] =
    [("Linear", Interp::Linear), ("Ease In and Out", Interp::EASE_IN_OUT), ("Ease In", Interp::EASE_IN), ("Ease Out", Interp::EASE_OUT), ("Hold", Interp::Hold)];

/// The key at `t` (clip time), within half a frame.
pub(crate) fn key_at(keys: &[Keyframe], t: Time, rate: Rate) -> Option<usize> {
    let half = rate.frame_to_time(1).ticks() / 2;
    keys.iter().position(|k| (k.time.ticks() - t.ticks()).abs() <= half)
}

/// The nearest key before / after `t` (clip time), more than half a frame away.
pub(crate) fn neighbours(keys: &[Keyframe], t: Time, rate: Rate) -> (Option<Time>, Option<Time>) {
    let half = rate.frame_to_time(1).ticks() / 2;
    let prev = keys.iter().rev().find(|k| k.time.ticks() < t.ticks() - half).map(|k| k.time);
    let next = keys.iter().find(|k| k.time.ticks() > t.ticks() + half).map(|k| k.time);
    (prev, next)
}

/// `keys` with a key added at `t` (the value there, the curve's shape
/// kept) or, if there is one, removed. The last key is never removed.
pub(crate) fn toggle(param: &Param, t: Time, rate: Rate) -> Option<Vec<Keyframe>> {
    let Param::Animated(keys) = param else { return None };
    let mut next = keys.clone();
    match key_at(keys, t, rate) {
        Some(_) if keys.len() < 2 => return None,
        Some(i) => {
            next.remove(i);
        }
        None => {
            let i = next.partition_point(|k| k.time < t);
            let interp = i.checked_sub(1).map_or(Interp::Linear, |p| next[p].interp);
            next.insert(i, Keyframe { time: t, value: param.value_at(t), interp });
        }
    }
    Some(next)
}

/// Key `i` moved to `to` (clip time): on a frame, at least a frame clear of
/// its neighbours, never before the clip.
pub(crate) fn moved(keys: &[Keyframe], i: usize, to: Time, rate: Rate) -> Vec<Keyframe> {
    let mut next = keys.to_vec();
    let frame = rate.frame_to_time(1);
    let lo = if i > 0 { next[i - 1].time + frame } else { Time::ZERO };
    let hi = next.get(i + 1).map_or(Time::MAX, |k| k.time - frame);
    if lo <= hi {
        next[i].time = to.max(Time::ZERO).round_to_frame(rate).clamp_to(lo, hi);
    }
    next
}

/// The lane of an animated parameter. Returns where to move the playhead.
pub(crate) fn lane(ui: &mut Ui, app: &mut EditorUi, clip: &Arc<Clip>, effect: &Arc<Effect>, param: &str, stored: &Param, span: Span) -> Option<Time> {
    let Param::Animated(model) = stored else { return None };
    let rate = app.rate();
    let id = ui.make_id(("keys", effect.id, param));
    let r = ui.interact_drag(id);
    let rect = r.rect;
    let reach = if ui.input().pointer_kind == PointerKind::Touch { 2.5 } else { 1.0 };
    let at = |k: &Keyframe| span.x(rect, span.clip_start + k.time.as_seconds_f64());
    let hit = |p: Vec2| -> Option<usize> {
        let y = rect.center().y;
        if (p.y - y).abs() > (PICK + 4.0) * reach {
            return None;
        }
        // The closest diamond within reach.
        model
            .iter()
            .enumerate()
            .map(|(i, k)| (i, (at(k) - p.x).abs()))
            .filter(|(_, d)| *d <= PICK * reach)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    };
    let to_clip = |x: f32| {
        let s = span.origin + ((x - rect.x) / rect.w.max(1.0)).clamp(0.0, 1.0) as f64 * span.span;
        Time::from_seconds_f64(s - span.clip_start)
    };
    let selected = app.view.graph_key.as_ref().filter(|g| g.effect == effect.id && g.param == param).map(|g| g.index).filter(|i| *i < model.len());
    let select = |app: &mut EditorUi, i: usize| app.view.graph_key = Some(GraphKey { effect: effect.id, param: param.into(), index: i });

    let mut seek = None;
    if r.pressed {
        app.view.key_drag = hit(r.mouse_pos).map(|i| (effect.id, param.to_string(), i));
        app.drag_gesture = app.next_gesture();
        match app.view.key_drag {
            Some((_, _, i)) => {
                select(app, i);
                seek = Some(clip.timeline_start + model[i].time);
            }
            None => {
                app.view.graph_key = None;
                seek = Some(Time::from_seconds_f64((span.clip_start + to_clip(r.mouse_pos.x).as_seconds_f64()).max(0.0)));
            }
        }
    }
    let grabbed = app.view.key_drag.as_ref().filter(|(e, p, _)| *e == effect.id && p == param).map(|(_, _, i)| *i).filter(|i| *i < model.len());
    let moved_past_slop = drag_from(&mut app.drag_start, &r, [0.0, 0.0]).is_some();
    if r.active {
        match grabbed {
            Some(i) if moved_past_slop => {
                let next = moved(model, i, to_clip(r.mouse_pos.x), rate);
                if next != *model {
                    seek = Some(clip.timeline_start + next[i].time);
                    app.set_keys(clip.id, effect.id, param, next, Some(app.drag_gesture));
                }
            }
            Some(_) => {}
            None => seek = Some(Time::from_seconds_f64((span.clip_start + to_clip(r.mouse_pos.x).as_seconds_f64()).max(0.0))),
        }
    }
    if r.released {
        app.view.key_drag = None;
    }
    // Right-click (touch and hold) a diamond: its interpolation, or delete.
    if r.secondary_pressed {
        if let Some(i) = hit(r.mouse_pos) {
            select(app, i);
        }
    }
    let numeric = matches!(model[0].value, Value::Float(_) | Value::Int(_) | Value::Vec2(_) | Value::Color(_));
    if let Some(i) = selected.or_else(|| app.view.graph_key.as_ref().filter(|g| g.effect == effect.id && g.param == param).map(|g| g.index)) {
        if i < model.len() {
            let choice = ui.context_menu(&r, |ui| {
                let mut c = None;
                if numeric {
                    for (label, interp) in INTERPS {
                        if ui.menu_item_ex(label, None, i + 1 < model.len() || interp == Interp::Hold).clicked {
                            c = Some(Some(interp));
                        }
                    }
                    ui.menu_separator();
                }
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
                Some(None) => crate::graph::delete_key(app, clip.id, effect.id, param, model, i),
                None => {}
            }
        }
    }
    if r.hovered && hit(r.mouse_pos).is_some() {
        ui.cursor = Cursor::Grab;
    }

    // ---- paint ---------------------------------------------------------------
    let keys: Vec<(f64, Interp)> = model.iter().map(|k| (span.clip_start + k.time.as_seconds_f64(), k.interp)).collect();
    let accent = ui.theme.palette.accent;
    ui.add_leaf(id, Layout::leaf(Size::Grow(1.0), Size::Grow(1.0)), Vec2::ZERO, true, move |p, rect| {
        p.rect(rect, REEL.chrome, 0.0);
        p.rect(Rect::new(rect.x, rect.bottom() - 1.0, rect.w, 1.0), REEL.panel, 0.0);
        let y = rect.center().y;
        if let (Some(first), Some(last)) = (keys.first(), keys.last()) {
            let (a, b) = (span.x(rect, first.0), span.x(rect, last.0));
            p.rect(Rect::new(a, y - 0.5, b - a, 1.0), REEL.tick, 0.0);
        }
        for (i, (t, interp)) in keys.iter().enumerate() {
            let x = span.x(rect, *t);
            let on = selected == Some(i);
            let fill = if on { accent } else { REEL.text_soft };
            let s = if on { 6.0 } else { 5.0 };
            // The shape says how the key leaves: a diamond (linear), a
            // circle (eased), a square (hold).
            match interp {
                Interp::Hold => p.rect(Rect::new(x - s * 0.75, y - s * 0.75, s * 1.5, s * 1.5), fill, 0.0),
                Interp::Linear => {
                    p.draw.triangle(Vec2::new(x - s, y), Vec2::new(x, y - s), Vec2::new(x + s, y), 0b011, fill);
                    p.draw.triangle(Vec2::new(x - s, y), Vec2::new(x + s, y), Vec2::new(x, y + s), 0b110, fill);
                }
                _ => p.rect(Rect::new(x - s * 0.8, y - s * 0.8, s * 1.6, s * 1.6), fill, s * 0.8),
            }
        }
        let x = span.x(rect, span.playhead).round();
        if x >= rect.x && x <= rect.right() {
            p.rect(Rect::new(x - 0.5, rect.y, 1.0, rect.h), REEL.playhead, 0.0);
        }
    });
    seek
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: Rate = Rate::FPS_25;

    fn k(f: i64, v: f64) -> Keyframe {
        Keyframe { time: R.frame_to_time(f), value: Value::Float(v), interp: Interp::Linear }
    }

    #[test]
    fn toggle_adds_on_the_curve_and_removes() {
        let p = Param::Animated(vec![k(0, 0.0), k(10, 100.0)]);
        let added = toggle(&p, R.frame_to_time(5), R).unwrap();
        assert_eq!(added.len(), 3);
        assert_eq!(added[1].value, Value::Float(50.0), "the key takes the value already there");
        let removed = toggle(&Param::Animated(added), R.frame_to_time(5), R).unwrap();
        assert_eq!(removed.len(), 2);
        assert!(toggle(&Param::Animated(vec![k(0, 1.0)]), Time::ZERO, R).is_none(), "the last key stays");
    }

    #[test]
    fn neighbours_skip_the_key_under_the_playhead() {
        let keys = vec![k(0, 0.0), k(10, 1.0), k(20, 2.0)];
        assert_eq!(neighbours(&keys, R.frame_to_time(10), R), (Some(R.frame_to_time(0)), Some(R.frame_to_time(20))));
        assert_eq!(neighbours(&keys, R.frame_to_time(25), R), (Some(R.frame_to_time(20)), None));
        assert_eq!(key_at(&keys, R.frame_to_time(10), R), Some(1));
    }

    #[test]
    fn a_dragged_key_stays_between_its_neighbours_on_frames() {
        let keys = vec![k(0, 0.0), k(10, 1.0), k(20, 2.0)];
        assert_eq!(moved(&keys, 1, R.frame_to_time(30), R)[1].time, R.frame_to_time(19));
        assert_eq!(moved(&keys, 1, R.frame_to_time(-3), R)[1].time, R.frame_to_time(1));
        let off_frame = R.frame_to_time(14) + Time::from_ticks(R.frame_to_time(1).ticks() / 3);
        assert_eq!(moved(&keys, 1, off_frame, R)[1].time, R.frame_to_time(14));
        assert_eq!(moved(&keys, 0, R.frame_to_time(-5), R)[0].time, Time::ZERO);
    }
}
