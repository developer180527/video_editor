//! How a clip's own time maps to its media: speed and time remapping.
//!
//! A clip occupies `source_range.duration` of the timeline whatever its
//! speed — that length is the clip's own time, 0 to its duration. A
//! [`Retime`] maps clip time to an offset into the media from
//! `source_range.start`. At normal speed the two are the same, so editing
//! maths (trims, ripples, rolls) never needs to know about speed; only what
//! is *shown* changes.

use serde::{Deserialize, Serialize};
use ve_time::Time;

/// An exact ratio, `num / den`, kept reduced with a positive denominator.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Ratio {
    pub num: i64,
    pub den: i64,
}

impl Ratio {
    pub const ONE: Ratio = Ratio { num: 1, den: 1 };

    /// `num / den`, reduced. Panics if `den` is 0.
    pub fn new(num: i64, den: i64) -> Ratio {
        assert!(den != 0, "ratio with zero denominator");
        let (mut a, mut b) = (num.unsigned_abs(), den.unsigned_abs());
        while b != 0 {
            (a, b) = (b, a % b);
        }
        let g = a.max(1) as i64;
        let sign = if den < 0 { -1 } else { 1 };
        Ratio { num: sign * num / g, den: sign * den / g }
    }

    pub fn as_f64(self) -> f64 {
        self.num as f64 / self.den as f64
    }

    /// `t × self`, rounded to the nearest tick, halves away from zero — so
    /// `apply(-t) == -apply(t)` and an edit and its undo cancel exactly.
    pub fn apply(self, t: Time) -> Time {
        Time(div_round(t.ticks() as i128 * self.num as i128, self.den as i128) as i64)
    }
}

/// `a / b` rounded to nearest, halves away from zero (`b > 0`).
fn div_round(a: i128, b: i128) -> i128 {
    let q = (a.abs() + b / 2) / b;
    if a < 0 {
        -q
    } else {
        q
    }
}

/// One point of a time remap: at clip time `time`, the media `offset` from
/// the clip's `source_range.start`. Clip-relative, like effect keyframes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemapKey {
    pub time: Time,
    pub offset: Time,
}

/// How a clip's time maps to its media.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Retime {
    /// The media advances `speed` ticks per tick of clip time: 2 is double
    /// speed, 1/2 slow motion, -1 backwards (from `source_range.start`
    /// down). 24000/25025 conforms 25 fps material to 23.976.
    Speed(Ratio),
    /// Time remapping: media offsets at clip times, linear between keys and
    /// held before the first and after the last. Sorted by time, at least
    /// one key. Freezes, ramps and reversals are all just keys.
    Remap(Vec<RemapKey>),
}

impl Default for Retime {
    fn default() -> Self {
        Retime::Speed(Ratio::ONE)
    }
}

impl Retime {
    pub fn is_normal(&self) -> bool {
        matches!(self, Retime::Speed(r) if *r == Ratio::ONE)
    }

    /// The media offset at clip time `t`.
    pub fn offset(&self, t: Time) -> Time {
        match self {
            Retime::Speed(r) => r.apply(t),
            Retime::Remap(keys) => {
                let i = keys.partition_point(|k| k.time <= t);
                match (i.checked_sub(1).map(|j| keys[j]), keys.get(i)) {
                    (Some(a), Some(b)) => {
                        let span = (b.time - a.time).ticks() as i128;
                        let along = (t - a.time).ticks() as i128;
                        a.offset + Time(div_round((b.offset - a.offset).ticks() as i128 * along, span) as i64)
                    }
                    (Some(a), None) => a.offset,
                    (None, Some(b)) => b.offset,
                    (None, None) => t,
                }
            }
        }
    }

    /// The same mapping for a clip whose start moved `d` later in clip time
    /// (a head trim, the right half of a split): the media offset that start
    /// now has, and the retime to use from there. Keys stay on their media.
    pub fn advanced(&self, d: Time) -> (Time, Retime) {
        match self {
            Retime::Speed(r) => (r.apply(d), self.clone()),
            Retime::Remap(keys) => (Time::ZERO, Retime::Remap(keys.iter().map(|k| RemapKey { time: k.time - d, offset: k.offset }).collect())),
        }
    }

    /// The media the clip plays over clip times `0..duration`, as a
    /// half-open span of offsets. Points the clip shows (its start, remap
    /// keys inside it) are included; the end is exclusive in whichever
    /// direction the media runs there — `[offset(0), offset(duration))`
    /// forwards, `(offset(duration), offset(0)]` backwards.
    pub fn extent(&self, duration: Time) -> (Time, Time) {
        let mut shown = vec![self.offset(Time::ZERO)];
        if let Retime::Remap(keys) = self {
            shown.extend(keys.iter().filter(|k| k.time > Time::ZERO && k.time < duration).map(|k| k.offset));
        }
        let end = self.offset(duration);
        let lo = shown.iter().copied().chain([end + Time(1)]).min().unwrap();
        let hi = shown.iter().map(|o| *o + Time(1)).chain([end]).max().unwrap();
        (lo, hi)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(x: i64) -> Time {
        Time::from_seconds(x)
    }

    #[test]
    fn ratios_reduce_and_round_symmetrically() {
        assert_eq!(Ratio::new(4, -8), Ratio { num: -1, den: 2 });
        let third = Ratio::new(1, 3);
        let t = Time(10);
        assert_eq!(third.apply(t), Time(3));
        assert_eq!(third.apply(-t), Time(-3), "undo cancels");
        assert_eq!(Ratio::new(1, 2).apply(Time(5)), Time(3)); // 2.5 → 3
        assert_eq!(Ratio::new(1, 2).apply(Time(-5)), Time(-3));
    }

    #[test]
    fn speed_and_remap_offsets() {
        assert_eq!(Retime::Speed(Ratio::new(2, 1)).offset(s(3)), s(6));
        assert_eq!(Retime::Speed(Ratio::new(1, 1)).extent(s(4)), (Time::ZERO, s(4)));
        assert_eq!(Retime::Speed(Ratio::new(-1, 1)).extent(s(4)), (s(-4) + Time(1), Time(1)));
        // Ramp: normal for 2 s, then freeze at media 2 s.
        let r = Retime::Remap(vec![RemapKey { time: Time::ZERO, offset: Time::ZERO }, RemapKey { time: s(2), offset: s(2) }, RemapKey { time: s(4), offset: s(2) }]);
        assert_eq!(r.offset(s(1)), s(1));
        assert_eq!(r.offset(s(3)), s(2));
        assert_eq!(r.offset(s(9)), s(2), "held after the last key");
        // A head trim keeps every key on its media.
        let (start, moved) = r.advanced(s(1));
        assert_eq!(start, Time::ZERO);
        assert_eq!(moved.offset(Time::ZERO), s(1));
        assert_eq!(moved.offset(s(2)), s(2));
    }
}
