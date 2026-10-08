//! The playhead while playing is not advanced by frame `dt`: it is *read*
//! from the audio device's count of played samples, so picture follows sound
//! and never drifts. The count moves in whole callback buffers, so between
//! callbacks it is extrapolated from the callback's time stamp — otherwise
//! the playhead would move in ~12 ms lurches and video would judder. Without
//! audio (no device, muted export preview) it uses the monotonic clock.

use std::time::Duration;
use ve_time::Time;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum State {
    Stopped,
    /// `rate` 1.0 forward, -1.0 reverse, 2.0 double speed (J/K/L).
    Playing { rate: f64 },
}

/// Where the clock was when playback started, in both clocks.
#[derive(Clone, Copy, Debug)]
struct Anchor {
    position: Time,
    audio_frames: Option<f64>,
    monotonic: Duration,
}

#[derive(Clone, Debug)]
pub struct Transport {
    state: State,
    position: Time,
    anchor: Option<Anchor>,
    sample_rate: u32,
    /// Playback stops (or loops) here.
    pub end: Time,
    pub looping: bool,
}

/// The current reading of both clocks.
#[derive(Clone, Copy, Debug)]
pub struct Clocks {
    /// Frames the audio device has played and when that count last moved,
    /// if a stream is open.
    pub audio: Option<(u64, Duration)>,
    /// Now, on the same epoch as the audio stamp.
    pub monotonic: Duration,
}

/// Extrapolation is capped: if callbacks stop (a stalled device), the clock
/// waits rather than running away from the audio.
const MAX_EXTRAPOLATION: Duration = Duration::from_millis(100);

impl Clocks {
    /// Frames played by now, extrapolated past the last callback.
    fn audio_frames(&self, rate: u32) -> Option<f64> {
        self.audio.map(|(frames, at)| {
            let since = self.monotonic.saturating_sub(at).min(MAX_EXTRAPOLATION);
            frames as f64 + since.as_secs_f64() * rate as f64
        })
    }
}

impl Transport {
    pub fn new(sample_rate: u32) -> Self {
        Transport { state: State::Stopped, position: Time::ZERO, anchor: None, sample_rate, end: Time::MAX, looping: false }
    }

    /// The audio device's rate, once known. Only while stopped.
    pub fn set_sample_rate(&mut self, rate: u32) {
        if self.anchor.is_none() {
            self.sample_rate = rate;
        }
    }

    pub fn state(&self) -> State {
        self.state
    }

    pub fn play(&mut self, rate: f64, now: Clocks) {
        self.position = self.position_at(now);
        self.anchor = Some(Anchor { position: self.position, audio_frames: now.audio_frames(self.sample_rate), monotonic: now.monotonic });
        self.state = State::Playing { rate };
    }

    pub fn stop(&mut self, now: Clocks) {
        self.position = self.position_at(now);
        self.anchor = None;
        self.state = State::Stopped;
    }

    pub fn seek(&mut self, t: Time, now: Clocks) {
        self.position = t.clamp_to(Time::ZERO, self.end);
        if self.anchor.is_some() {
            // Re-anchor so playback continues from the new place.
            self.anchor = Some(Anchor { position: self.position, audio_frames: now.audio_frames(self.sample_rate), monotonic: now.monotonic });
        }
    }

    /// The playhead at `now`.
    pub fn position_at(&self, now: Clocks) -> Time {
        let (State::Playing { rate }, Some(a)) = (self.state, self.anchor) else { return self.position };
        let elapsed = match (a.audio_frames, now.audio_frames(self.sample_rate)) {
            (Some(f0), Some(f1)) => Time::from_seconds_f64((f1 - f0).max(0.0) / self.sample_rate as f64),
            _ => Time::from_seconds_f64((now.monotonic.saturating_sub(a.monotonic)).as_secs_f64()),
        };
        let t = a.position + Time((elapsed.ticks() as f64 * rate) as i64);
        if self.looping && self.end > Time::ZERO && self.end != Time::MAX {
            Time(t.ticks().rem_euclid(self.end.ticks()))
        } else {
            t.clamp_to(Time::ZERO, self.end)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Audio stamped exactly now (no extrapolation).
    fn at(frames: Option<u64>, ms: u64) -> Clocks {
        let now = Duration::from_millis(ms);
        Clocks { audio: frames.map(|f| (f, now)), monotonic: now }
    }

    #[test]
    fn extrapolates_between_callbacks_smoothly() {
        let mut t = Transport::new(48_000);
        let cb = Duration::from_millis(1000);
        t.play(1.0, Clocks { audio: Some((0, cb)), monotonic: cb });
        // 5 ms after a callback that delivered nothing new, the playhead has
        // still moved 5 ms.
        let p = t.position_at(Clocks { audio: Some((0, cb)), monotonic: cb + Duration::from_millis(5) });
        assert_eq!(p, Time::from_seconds_f64(0.005));
        // The next callback (512 frames = 10.67 ms) arriving late does not
        // jump the playhead backwards or forwards beyond the elapsed time.
        let later = cb + Duration::from_millis(12);
        let p = t.position_at(Clocks { audio: Some((512, cb + Duration::from_micros(10_667))), monotonic: later });
        assert!((p.as_seconds_f64() - 0.012).abs() < 0.0005, "{}", p.as_seconds_f64());
        // A stalled device: the clock stops 100 ms after the last callback.
        let p = t.position_at(Clocks { audio: Some((512, cb)), monotonic: cb + Duration::from_secs(5) });
        assert!(p.as_seconds_f64() < 0.12);
    }

    #[test]
    fn follows_the_audio_clock_not_wall_time() {
        let mut t = Transport::new(48_000);
        t.play(1.0, at(Some(1000), 0));
        // Wall clock says 2 s, but the device has only played 1 s of samples.
        assert_eq!(t.position_at(at(Some(49_000), 2000)), Time::from_seconds(1));
    }

    #[test]
    fn falls_back_to_monotonic_without_audio() {
        let mut t = Transport::new(48_000);
        t.play(2.0, at(None, 0));
        assert_eq!(t.position_at(at(None, 500)), Time::from_seconds(1));
    }

    #[test]
    fn stop_holds_position_and_seek_moves_it() {
        let mut t = Transport::new(48_000);
        t.play(1.0, at(Some(0), 0));
        t.stop(at(Some(96_000), 0));
        assert_eq!(t.position_at(at(Some(500_000), 0)), Time::from_seconds(2));
        t.seek(Time::from_seconds(10), at(None, 0));
        assert_eq!(t.position_at(at(None, 0)), Time::from_seconds(10));
    }

    #[test]
    fn loops_and_clamps() {
        let mut t = Transport::new(48_000);
        t.end = Time::from_seconds(4);
        t.play(1.0, at(Some(0), 0));
        assert_eq!(t.position_at(at(Some(48_000 * 5), 0)), Time::from_seconds(4));
        t.looping = true;
        assert_eq!(t.position_at(at(Some(48_000 * 5), 0)), Time::from_seconds(1));
    }
}
