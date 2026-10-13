//! Keyframes on timeline clips: the rubber band across a clip's body —
//! Opacity on video, Volume on audio — as in Premiere.
//!
//! - Selection tool: drag the line up or down to change the level (a
//!   constant level, or the segment under the pointer); drag a key to move
//!   it in time and value; Cmd/Ctrl-click the line to add a key.
//! - Pen tool: click the line to add a key (and drag it at once).
//! - Right-click (touch and hold) a key: its interpolation, or delete it.
//!
//! The pure parts (mapping, hit tests, edits) live here; `timeline.rs`
//! routes the pointer to them and paints what [`shape`] returns.

use libgui::{Rect, Vec2};
use ve_model::*;
use ve_engine::intrinsic;
use ve_time::{Rate, Time};

/// How close (px) the pointer must be to the line, or to a key.
pub(crate) const LINE_PX: f32 = 4.0;
pub(crate) const KEY_PX: f32 = 6.0;
/// The quietest level the band shows; lower sits on the floor.
const FLOOR_DB: f64 = -60.0;
const TOP_DB: f64 = 15.0;

/// The parameter a clip's band shows: Opacity on video, Volume on audio.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Target {
    pub effect: EffectId,
    pub param: &'static str,
    pub audio: bool,
}

pub(crate) fn target(clip: &Clip, kind: TrackKind) -> Option<Target> {
    let (plugin, param, audio) = match kind {
        TrackKind::Video => (intrinsic::OPACITY, "opacity", false),
        TrackKind::Audio => (intrinsic::VOLUME, "level", true),
    };
    let e = clip.effects.iter().find(|e| e.plugin.id == plugin)?;
    Some(Target { effect: e.id, param, audio })
}

pub(crate) fn stored(clip: &Clip, t: Target) -> Option<&Param> {
    clip.effects.iter().find(|e| e.id == t.effect)?.params.get(t.param)
}

/// Where a level sits in the band, 0 (bottom) to 1 (top).
pub(crate) fn frac(audio: bool, v: f64) -> f32 {
    let f = if audio { (v - FLOOR_DB) / (TOP_DB - FLOOR_DB) } else { v / 100.0 };
    f.clamp(0.0, 1.0) as f32
}

/// The level at `f` (0 bottom, 1 top), as stored.
pub(crate) fn level(audio: bool, f: f32) -> f64 {
    let f = f.clamp(0.0, 1.0) as f64;
    if audio {
        let db = FLOOR_DB + f * (TOP_DB - FLOOR_DB);
        // The floor is silence, as a fader at the bottom.
        if f <= 0.0 { -96.0 } else { (db * 10.0).round() / 10.0 }
    } else {
        (f * 1000.0).round() / 10.0
    }
}

/// The band's area inside a clip drawn at `r`: under the name strip.
pub(crate) fn area(r: Rect, strip: f32) -> Rect {
    let top = r.y + strip + 3.0;
    Rect::new(r.x, top, r.w, (r.bottom() - 3.0 - top).max(1.0))
}

fn number(v: &Value) -> f64 {
    match v {
        Value::Float(f) => *f,
        Value::Int(i) => *i as f64,
        _ => 0.0,
    }
}

/// The value of `p` at clip time `t`, as a number.
pub(crate) fn value_at(p: &Param, t: Time) -> f64 {
    number(&p.value_at(t))
}

/// What to draw: the line (x in seconds from the clip's start, fraction)
/// and the keys (time from the clip's start in seconds, fraction).
/// Points of the band: (seconds from the clip's start, height 0–1).
pub(crate) type Points = Vec<(f32, f32)>;

pub(crate) fn shape(p: &Param, len: f64, audio: bool, samples: usize) -> (Points, Points) {
    let n = samples.max(2);
    let mut times: Vec<f64> = (0..=n).map(|i| len * i as f64 / n as f64).collect();
    let keys: Vec<(f32, f32)> = match p {
        Param::Animated(k) => k.iter().map(|k| (k.time.as_seconds_f64() as f32, frac(audio, number(&k.value)))).collect(),
        Param::Constant(_) => vec![],
    };
    // Keys (and the instant before a hold's step) are exact corners.
    if let Param::Animated(k) = p {
        for key in k {
            let t = key.time.as_seconds_f64();
            if t > 0.0 && t < len {
                times.push(t);
                times.push((t - 1e-4).max(0.0));
            }
        }
    }
    times.sort_by(f64::total_cmp);
    times.dedup();
    let line = times.into_iter().map(|t| (t as f32, frac(audio, value_at(p, Time::from_seconds_f64(t))))).collect();
    (line, keys)
}

/// What the pointer is over in a clip's band.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Over {
    Key(usize),
    Line,
}

/// Hit-test the band of a clip drawn at `r` (its full rect): `x_of` maps a
/// clip time to x.
/// `reach` scales the distances (more for a finger).
pub(crate) fn over(p: &Param, audio: bool, area: Rect, x_of: impl Fn(Time) -> f32, t_of: impl Fn(f32) -> Time, pos: Vec2, reach: f32) -> Option<Over> {
    let y_of = |f: f32| area.bottom() - f * area.h;
    if let Param::Animated(keys) = p {
        let best = keys
            .iter()
            .enumerate()
            .map(|(i, k)| (i, (x_of(k.time) - pos.x).abs().max((y_of(frac(audio, number(&k.value))) - pos.y).abs())))
            .filter(|(_, d)| *d <= KEY_PX * reach)
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((i, _)) = best {
            return Some(Over::Key(i));
        }
    }
    let y = y_of(frac(audio, value_at(p, t_of(pos.x))));
    ((pos.y - y).abs() <= LINE_PX * reach).then_some(Over::Line)
}

/// `p` with a key added at clip time `t` (on the line). Returns the new
/// keys and the index of the added one.
pub(crate) fn add_key(p: &Param, t: Time, rate: Rate) -> (Vec<Keyframe>, usize) {
    let t = t.max(Time::ZERO).round_to_frame(rate);
    let mut keys = match p {
        Param::Animated(k) => k.clone(),
        Param::Constant(_) => vec![],
    };
    if let Some(i) = crate::keys::key_at(&keys, t, rate) {
        return (keys, i);
    }
    let i = keys.partition_point(|k| k.time < t);
    let interp = i.checked_sub(1).map_or(Interp::Linear, |p| keys[p].interp);
    keys.insert(i, Keyframe { time: t, value: p.value_at(t), interp });
    (keys, i)
}

fn with(keys: &[Keyframe], i: usize, v: f64) -> Value {
    match keys[i].value {
        Value::Int(_) => Value::Int(v.round() as i64),
        _ => Value::Float(v),
    }
}

/// The line dragged by `dy` of the band's height (up is positive): a
/// constant level moves; on an animated one, the two keys around clip time
/// `t` do (or the one key, before the first or after the last).
pub(crate) fn lift(orig: &Param, t: Time, df: f32, audio: bool) -> Param {
    let shift = |v: f64| level(audio, frac(audio, v) + df);
    match orig {
        Param::Constant(v) => Param::Constant(Value::Float(shift(number(v)))),
        Param::Animated(keys) => {
            let mut next = keys.clone();
            let after = keys.partition_point(|k| k.time <= t);
            let which: Vec<usize> = match (after.checked_sub(1), (after < keys.len()).then_some(after)) {
                (Some(a), Some(b)) => vec![a, b],
                (Some(a), None) => vec![a],
                (None, Some(b)) => vec![b],
                (None, None) => vec![],
            };
            for i in which {
                next[i].value = with(keys, i, shift(number(&keys[i].value)));
            }
            Param::Animated(next)
        }
    }
}

/// Key `i` of `orig` dragged to clip time `to` and band fraction `f`.
pub(crate) fn drag_key(orig: &[Keyframe], i: usize, to: Time, f: f32, audio: bool, rate: Rate, len: Time) -> Vec<Keyframe> {
    let mut next = crate::keys::moved(orig, i, to.min(len), rate);
    next[i].value = with(orig, i, level(audio, f));
    next
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: Rate = Rate::FPS_25;

    fn k(f: i64, v: f64) -> Keyframe {
        Keyframe { time: R.frame_to_time(f), value: Value::Float(v), interp: Interp::Linear }
    }

    #[test]
    fn levels_map_both_ways() {
        assert_eq!(level(false, frac(false, 37.5)), 37.5);
        assert_eq!(level(true, frac(true, -6.0)), -6.0);
        assert_eq!(frac(true, -96.0), 0.0);
        assert_eq!(level(true, 0.0), -96.0, "the floor is silence");
        assert_eq!(frac(false, 100.0), 1.0);
    }

    #[test]
    fn adding_a_key_keeps_the_level_and_animates_a_constant() {
        let (keys, i) = add_key(&Param::Constant(Value::Float(80.0)), R.frame_to_time(5), R);
        assert_eq!((keys.len(), i), (1, 0));
        assert_eq!(keys[0].value, Value::Float(80.0));
        let p = Param::Animated(vec![k(0, 0.0), k(10, 100.0)]);
        let (keys, i) = add_key(&p, R.frame_to_time(5), R);
        assert_eq!((keys.len(), i), (3, 1));
        assert_eq!(keys[1].value, Value::Float(50.0));
    }

    #[test]
    fn lifting_moves_the_segment_under_the_pointer() {
        assert_eq!(lift(&Param::Constant(Value::Float(100.0)), Time::ZERO, -0.5, false), Param::Constant(Value::Float(50.0)));
        let p = Param::Animated(vec![k(0, 20.0), k(10, 40.0), k(20, 60.0)]);
        let Param::Animated(next) = lift(&p, R.frame_to_time(15), 0.1, false) else { panic!() };
        assert_eq!(next.iter().map(|k| k.value.clone()).collect::<Vec<_>>(), vec![Value::Float(20.0), Value::Float(50.0), Value::Float(70.0)]);
        let Param::Animated(next) = lift(&p, R.frame_to_time(25), 0.1, false) else { panic!() };
        assert_eq!(next[2].value, Value::Float(70.0));
        assert_eq!(next[1].value, Value::Float(40.0));
    }

    #[test]
    fn the_line_and_keys_are_hit() {
        let p = Param::Animated(vec![k(0, 0.0), k(25, 100.0)]);
        let area = Rect::new(0.0, 0.0, 100.0, 100.0);
        let x_of = |t: Time| t.as_seconds_f64() as f32 * 100.0;
        let t_of = |x: f32| Time::from_seconds_f64(x as f64 / 100.0);
        assert_eq!(over(&p, false, area, x_of, t_of, Vec2::new(1.0, 99.0), 1.0), Some(Over::Key(0)));
        assert_eq!(over(&p, false, area, x_of, t_of, Vec2::new(50.0, 51.0), 1.0), Some(Over::Line));
        assert_eq!(over(&p, false, area, x_of, t_of, Vec2::new(50.0, 20.0), 1.0), None);
    }
}
