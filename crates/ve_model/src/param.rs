//! Effect parameters: constants or keyframed curves. The plugin declares
//! them; the host stores, animates and undoes them.

use serde::{Deserialize, Serialize};
use ve_time::Time;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Float(f64),
    Vec2([f64; 2]),
    /// Linear-light RGBA.
    Color([f32; 4]),
    Choice(u32),
    Text(String),
}

impl Value {
    /// Interpolates numeric values; others hold until the next key.
    pub fn lerp(&self, o: &Value, t: f64) -> Value {
        match (self, o) {
            (Value::Float(a), Value::Float(b)) => Value::Float(a + (b - a) * t),
            (Value::Vec2(a), Value::Vec2(b)) => Value::Vec2([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]),
            (Value::Color(a), Value::Color(b)) => {
                let t = t as f32;
                Value::Color([0, 1, 2, 3].map(|i| a[i] + (b[i] - a[i]) * t))
            }
            (Value::Int(a), Value::Int(b)) => Value::Int(a + ((b - a) as f64 * t).round() as i64),
            _ => self.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Interp {
    Hold,
    Linear,
    /// Smooth in and out.
    Ease,
    /// A curve from this key to the next, shaped by two handles in the
    /// segment's unit square — time across, the change in value up — as
    /// CSS's `cubic-bezier(x1, y1, x2, y2)`. Times stay within the segment
    /// (0..1), so the curve never runs backwards; values may overshoot.
    /// Normalised, so one curve times a number, a position or a colour.
    Bezier { x1: f32, y1: f32, x2: f32, y2: f32 },
}

impl Interp {
    /// The common eases as curves (what the graph editor starts from).
    pub const EASE_IN_OUT: Interp = Interp::Bezier { x1: 0.42, y1: 0.0, x2: 0.58, y2: 1.0 };
    pub const EASE_IN: Interp = Interp::Bezier { x1: 0.42, y1: 0.0, x2: 1.0, y2: 1.0 };
    pub const EASE_OUT: Interp = Interp::Bezier { x1: 0.0, y1: 0.0, x2: 0.58, y2: 1.0 };

    /// How far from the first key's value to the next's (0..1, beyond
    /// when overshooting) at fraction `x` of the segment's time.
    pub fn progress(self, x: f64) -> f64 {
        let x = x.clamp(0.0, 1.0);
        match self {
            Interp::Hold => 0.0,
            Interp::Linear => x,
            Interp::Ease => x * x * (3.0 - 2.0 * x),
            Interp::Bezier { x1, y1, x2, y2 } => bezier(x1.clamp(0.0, 1.0) as f64, y1 as f64, x2.clamp(0.0, 1.0) as f64, y2 as f64, x),
        }
    }

    /// The handles this interpolation looks like, as a curve (Linear and
    /// Ease have exact or close equivalents; Hold has none).
    pub fn as_bezier(self) -> Option<(f32, f32, f32, f32)> {
        match self {
            Interp::Hold => None,
            Interp::Linear => Some((1.0 / 3.0, 1.0 / 3.0, 2.0 / 3.0, 2.0 / 3.0)),
            Interp::Ease => Some((1.0 / 3.0, 0.0, 2.0 / 3.0, 1.0)),
            Interp::Bezier { x1, y1, x2, y2 } => Some((x1, y1, x2, y2)),
        }
    }
}

/// The y of a cubic Bézier from (0,0) to (1,1) with handles (x1,y1),
/// (x2,y2), at the point whose x is `x`. x(s) only rises (handles' x are
/// within 0..1), so Newton's method with a bisection fallback finds s.
fn bezier(x1: f64, y1: f64, x2: f64, y2: f64, x: f64) -> f64 {
    let curve = |a: f64, b: f64, s: f64| {
        let u = 1.0 - s;
        3.0 * u * u * s * a + 3.0 * u * s * s * b + s * s * s
    };
    let slope = |a: f64, b: f64, s: f64| {
        let u = 1.0 - s;
        3.0 * u * u * a + 6.0 * u * s * (b - a) + 3.0 * s * s * (1.0 - b)
    };
    let mut s = x;
    for _ in 0..8 {
        let err = curve(x1, x2, s) - x;
        if err.abs() < 1e-7 {
            return curve(y1, y2, s);
        }
        let d = slope(x1, x2, s);
        if d.abs() < 1e-6 {
            break;
        }
        s = (s - err / d).clamp(0.0, 1.0);
    }
    let (mut lo, mut hi) = (0.0, 1.0);
    for _ in 0..40 {
        s = (lo + hi) / 2.0;
        if curve(x1, x2, s) < x {
            lo = s;
        } else {
            hi = s;
        }
    }
    curve(y1, y2, s)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Keyframe {
    /// Relative to the clip's start.
    pub time: Time,
    pub value: Value,
    /// How to get from this key to the next.
    pub interp: Interp,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Param {
    Constant(Value),
    /// Sorted by time, at least one key.
    Animated(Vec<Keyframe>),
}

impl Param {
    /// The value at clip-relative time `t`.
    pub fn value_at(&self, t: Time) -> Value {
        match self {
            Param::Constant(v) => v.clone(),
            Param::Animated(keys) => {
                let i = keys.partition_point(|k| k.time <= t);
                match (i.checked_sub(1).map(|j| &keys[j]), keys.get(i)) {
                    (None, Some(next)) => next.value.clone(),
                    (Some(prev), None) => prev.value.clone(),
                    (Some(prev), Some(next)) => {
                        let span = (next.time - prev.time).ticks() as f64;
                        let x = (t - prev.time).ticks() as f64 / span;
                        if prev.interp == Interp::Hold {
                            return prev.value.clone();
                        }
                        prev.value.lerp(&next.value, prev.interp.progress(x))
                    }
                    (None, None) => unreachable!("animated param with no keys"),
                }
            }
        }
    }
}
