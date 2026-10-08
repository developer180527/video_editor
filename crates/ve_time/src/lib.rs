//! Exact media time.
//!
//! Every time in a document is a whole number of **ticks**, 1/254 016 000 000
//! of a second. That base divides evenly by the frame and sample rates editors
//! meet — 23.976, 24, 25, 29.97, 30, 47.952, 48, 50, 59.94, 60, 120 fps and
//! 22.05, 44.1, 48, 88.2, 96, 192 kHz — so frame and sample boundaries are
//! exact integers and an edit never drifts. An `i64` of ticks spans about
//! 36 million seconds (over a year) either side of zero.
//!
//! Floats never enter the document. They appear only at the edges (a pixel
//! position on the timeline, a value shown to the user).

use serde::{Deserialize, Serialize};
use std::fmt;
use std::ops::{Add, AddAssign, Neg, Sub, SubAssign};

/// Ticks per second.
pub const TICKS_PER_SECOND: i64 = 254_016_000_000;

/// A point in time, or a length of time, in ticks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Time(pub i64);

impl Time {
    pub const ZERO: Time = Time(0);
    pub const MAX: Time = Time(i64::MAX);

    pub const fn from_ticks(ticks: i64) -> Self {
        Time(ticks)
    }

    pub const fn ticks(self) -> i64 {
        self.0
    }

    pub const fn from_seconds(s: i64) -> Self {
        Time(s * TICKS_PER_SECOND)
    }

    /// For display and for the UI's pixel maths only.
    pub fn as_seconds_f64(self) -> f64 {
        self.0 as f64 / TICKS_PER_SECOND as f64
    }

    /// Nearest tick to `s` seconds. For input from the UI, never for storage
    /// round trips.
    pub fn from_seconds_f64(s: f64) -> Self {
        Time((s * TICKS_PER_SECOND as f64).round() as i64)
    }

    /// The frame this time falls in at `rate` (floor).
    pub fn to_frame(self, rate: Rate) -> i64 {
        let per = rate.ticks_per_frame_x(); // ticks * num per frame, exact
        (self.0 as i128 * rate.num as i128).div_euclid(per) as i64
    }

    /// The nearest frame boundary at `rate`.
    pub fn round_to_frame(self, rate: Rate) -> Time {
        let per = rate.ticks_per_frame_x();
        let x = self.0 as i128 * rate.num as i128;
        let f = (x + per / 2).div_euclid(per);
        rate.frame_to_time(f as i64)
    }

    /// Clamp into `[lo, hi]`.
    pub fn clamp_to(self, lo: Time, hi: Time) -> Time {
        Time(self.0.clamp(lo.0, hi.0))
    }

    pub fn checked_add(self, o: Time) -> Option<Time> {
        self.0.checked_add(o.0).map(Time)
    }
}

impl Add for Time {
    type Output = Time;
    fn add(self, o: Time) -> Time {
        Time(self.0 + o.0)
    }
}
impl Sub for Time {
    type Output = Time;
    fn sub(self, o: Time) -> Time {
        Time(self.0 - o.0)
    }
}
impl AddAssign for Time {
    fn add_assign(&mut self, o: Time) {
        self.0 += o.0;
    }
}
impl SubAssign for Time {
    fn sub_assign(&mut self, o: Time) {
        self.0 -= o.0;
    }
}
impl Neg for Time {
    type Output = Time;
    fn neg(self) -> Time {
        Time(-self.0)
    }
}

/// A frame or sample rate as an exact fraction: 24000/1001 is 23.976 fps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rate {
    pub num: u32,
    pub den: u32,
}

impl Rate {
    pub const FPS_23_976: Rate = Rate::new(24000, 1001);
    pub const FPS_24: Rate = Rate::new(24, 1);
    pub const FPS_25: Rate = Rate::new(25, 1);
    pub const FPS_29_97: Rate = Rate::new(30000, 1001);
    pub const FPS_30: Rate = Rate::new(30, 1);
    pub const FPS_50: Rate = Rate::new(50, 1);
    pub const FPS_59_94: Rate = Rate::new(60000, 1001);
    pub const FPS_60: Rate = Rate::new(60, 1);
    pub const HZ_44100: Rate = Rate::new(44100, 1);
    pub const HZ_48000: Rate = Rate::new(48000, 1);

    pub const fn new(num: u32, den: u32) -> Self {
        assert!(num > 0 && den > 0, "rate must be positive");
        Rate { num, den }
    }

    /// Whether frame boundaries at this rate land on whole ticks. True for
    /// every standard rate; a false one still works, rounding to the tick.
    pub fn is_exact(self) -> bool {
        (TICKS_PER_SECOND as i128 * self.den as i128) % self.num as i128 == 0
    }

    pub fn as_f64(self) -> f64 {
        self.num as f64 / self.den as f64
    }

    /// Ticks per frame, multiplied by `num` so it is always an integer.
    fn ticks_per_frame_x(self) -> i128 {
        TICKS_PER_SECOND as i128 * self.den as i128
    }

    /// Start of `frame`.
    pub fn frame_to_time(self, frame: i64) -> Time {
        let x = frame as i128 * self.ticks_per_frame_x();
        Time((x / self.num as i128) as i64)
    }

    /// Length of one frame.
    pub fn frame_duration(self) -> Time {
        self.frame_to_time(1)
    }

    /// Frames per second, rounded: the "timebase" timecode counts in.
    pub fn nominal_fps(self) -> u32 {
        (self.num + self.den / 2) / self.den
    }

    /// 29.97 and 59.94 use drop-frame timecode by convention.
    pub fn is_drop_frame_rate(self) -> bool {
        self.den == 1001 && matches!(self.num, 30000 | 60000)
    }
}

/// A half-open span `[start, start + duration)`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TimeRange {
    pub start: Time,
    pub duration: Time,
}

impl TimeRange {
    pub const fn new(start: Time, duration: Time) -> Self {
        TimeRange { start, duration }
    }

    pub fn from_bounds(start: Time, end: Time) -> Self {
        TimeRange { start, duration: end - start }
    }

    pub fn end(self) -> Time {
        self.start + self.duration
    }

    pub fn is_empty(self) -> bool {
        self.duration.0 <= 0
    }

    pub fn contains(self, t: Time) -> bool {
        t >= self.start && t < self.end()
    }

    pub fn overlaps(self, o: TimeRange) -> bool {
        self.start < o.end() && o.start < self.end()
    }

    pub fn intersection(self, o: TimeRange) -> Option<TimeRange> {
        let s = self.start.max(o.start);
        let e = self.end().min(o.end());
        (s < e).then(|| TimeRange::from_bounds(s, e))
    }

    pub fn shifted(self, by: Time) -> TimeRange {
        TimeRange { start: self.start + by, duration: self.duration }
    }
}

/// SMPTE timecode for display: `hh:mm:ss:ff`, or `hh:mm:ss;ff` drop-frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timecode {
    pub hours: u32,
    pub minutes: u32,
    pub seconds: u32,
    pub frames: u32,
    pub drop_frame: bool,
    pub negative: bool,
}

impl Timecode {
    /// Timecode of the frame containing `t`. Drop-frame is used for 29.97 and
    /// 59.94 when `drop_frame` is requested.
    pub fn from_time(t: Time, rate: Rate, drop_frame: bool) -> Self {
        let negative = t.0 < 0;
        let mut frame = (if negative { -t } else { t }).to_frame(rate);
        let fps = rate.nominal_fps() as i64;
        let drop = drop_frame && rate.is_drop_frame_rate();
        if drop {
            // Skip frame numbers 0 and 1 (2 and 3 at 59.94) at the start of
            // every minute except each tenth.
            let d = fps / 15; // 2 at 29.97, 4 at 59.94
            let per_10min = fps * 60 * 10 - d * 9;
            let per_min = fps * 60 - d;
            let tens = frame / per_10min;
            let rem = frame % per_10min;
            let extra = if rem > d { d * ((rem - d) / per_min) } else { 0 };
            frame += 9 * d * tens + extra;
        }
        Timecode {
            hours: (frame / (fps * 3600)) as u32,
            minutes: ((frame / (fps * 60)) % 60) as u32,
            seconds: ((frame / fps) % 60) as u32,
            frames: (frame % fps) as u32,
            drop_frame: drop,
            negative,
        }
    }
}

impl fmt::Display for Timecode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sep = if self.drop_frame { ';' } else { ':' };
        let sign = if self.negative { "-" } else { "" };
        write!(f, "{sign}{:02}:{:02}:{:02}{sep}{:02}", self.hours, self.minutes, self.seconds, self.frames)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_rates_are_exact() {
        for r in [
            Rate::FPS_23_976,
            Rate::FPS_24,
            Rate::FPS_25,
            Rate::FPS_29_97,
            Rate::FPS_30,
            Rate::FPS_50,
            Rate::FPS_59_94,
            Rate::FPS_60,
            Rate::new(120, 1),
            Rate::new(48000, 1001),
            Rate::HZ_44100,
            Rate::HZ_48000,
            Rate::new(96000, 1),
            Rate::new(192000, 1),
        ] {
            assert!(r.is_exact(), "{r:?}");
        }
    }

    #[test]
    fn frames_round_trip() {
        for r in [Rate::FPS_23_976, Rate::FPS_25, Rate::FPS_29_97, Rate::FPS_60] {
            for f in [-5_i64, 0, 1, 23, 24, 1000, 86_400 * 60] {
                let t = r.frame_to_time(f);
                assert_eq!(t.to_frame(r), f, "{r:?} frame {f}");
                // One tick before the boundary is still the previous frame.
                assert_eq!((t - Time(1)).to_frame(r), f - 1);
            }
        }
    }

    #[test]
    fn no_drift_over_an_hour() {
        let r = Rate::FPS_29_97;
        let hour_of_frames = 107_892; // 29.97 * 3600, as broadcast counts it
        let t = r.frame_to_time(hour_of_frames);
        let summed = (0..hour_of_frames).fold(Time::ZERO, |a, _| a + r.frame_duration());
        assert_eq!(t, summed);
    }

    #[test]
    fn rounding_to_frames() {
        let r = Rate::FPS_24;
        let d = r.frame_duration();
        assert_eq!(Time(d.0 / 2 - 1).round_to_frame(r), Time::ZERO);
        assert_eq!(Time(d.0 / 2 + 1).round_to_frame(r), d);
    }

    #[test]
    fn ranges() {
        let s = Time::from_seconds;
        let a = TimeRange::new(s(0), s(10));
        let b = TimeRange::new(s(5), s(10));
        let c = TimeRange::new(s(10), s(1));
        assert!(a.overlaps(b));
        assert!(!a.overlaps(c), "half-open: touching is not overlapping");
        assert_eq!(a.intersection(b), Some(TimeRange::new(s(5), s(5))));
        assert!(a.contains(s(0)) && !a.contains(s(10)));
    }

    #[test]
    fn timecode_ndf() {
        let r = Rate::FPS_24;
        let t = r.frame_to_time(24 * 3661 + 7);
        assert_eq!(Timecode::from_time(t, r, false).to_string(), "01:01:01:07");
    }

    #[test]
    fn timecode_drop_frame() {
        let r = Rate::FPS_29_97;
        let tc = |f: i64| Timecode::from_time(r.frame_to_time(f), r, true).to_string();
        assert_eq!(tc(0), "00:00:00;00");
        assert_eq!(tc(1799), "00:00:59;29");
        assert_eq!(tc(1800), "00:01:00;02"); // ;00 and ;01 are dropped
        assert_eq!(tc(17982), "00:10:00;00"); // tenth minute keeps them
        assert_eq!(tc(107_892), "01:00:00;00");
    }
}
