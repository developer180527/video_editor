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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Interp {
    Hold,
    Linear,
    /// Smooth in and out.
    Ease,
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
                        let mut x = (t - prev.time).ticks() as f64 / span;
                        match prev.interp {
                            Interp::Hold => return prev.value.clone(),
                            Interp::Linear => {}
                            Interp::Ease => x = x * x * (3.0 - 2.0 * x),
                        }
                        prev.value.lerp(&next.value, x)
                    }
                    (None, None) => unreachable!("animated param with no keys"),
                }
            }
        }
    }
}
