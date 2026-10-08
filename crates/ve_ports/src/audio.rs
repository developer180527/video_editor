use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use thiserror::Error;

/// Time since a process-wide epoch. Audio callbacks stamp the clock with it
/// and the playhead is read against it, so the two always agree.
pub fn clock_now() -> Duration {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed()
}

/// The audio device's position: frames handed to the device, and when.
///
/// The count moves in whole callback buffers (512 frames is ~11.6 ms at
/// 44.1 kHz). Reading the stamp too lets a reader extrapolate between
/// callbacks, so a playhead built on it moves smoothly.
#[derive(Default)]
pub struct AudioClock {
    frames: AtomicU64,
    stamp_ns: AtomicU64,
    /// The last callback was full of counted frames, so the count is moving
    /// and may be extrapolated. A stalled count (an underrun, a restart
    /// filling up) must not be: extrapolating it would run ahead of the sound.
    live: AtomicBool,
}

impl AudioClock {
    pub fn new() -> Self {
        Self::default()
    }

    /// The device took `n` more frames, now. Real-time safe.
    pub fn advance(&self, n: u64) {
        self.tick(n, true);
    }

    /// A callback counted `n` frames; `live` when that was the whole buffer.
    /// Real-time safe.
    pub fn tick(&self, n: u64, live: bool) {
        self.stamp_ns.store(clock_now().as_nanos() as u64, Ordering::Release);
        self.frames.fetch_add(n, Ordering::AcqRel);
        self.live.store(live, Ordering::Release);
    }

    /// Frames so far, and when the count last moved (on [`clock_now`]'s
    /// epoch). A stalled count is stamped "now", so readers do not
    /// extrapolate it.
    pub fn read(&self) -> (u64, Duration) {
        let frames = self.frames.load(Ordering::Acquire);
        let at = if self.live.load(Ordering::Acquire) {
            Duration::from_nanos(self.stamp_ns.load(Ordering::Acquire))
        } else {
            clock_now()
        };
        (frames, at)
    }
}

#[derive(Debug, Error)]
pub enum AudioError {
    #[error("no audio output device")]
    NoDevice,
    #[error("{0}")]
    Other(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioConfig {
    pub sample_rate: u32,
    pub channels: u16,
    /// Frames per callback the engine would like; the device may differ.
    pub buffer_frames: u32,
}

/// Fills an interleaved buffer. Runs on the real-time audio thread: it must
/// not allocate, lock, or wait.
pub type RenderCallback = Box<dyn FnMut(&mut [f32]) + Send>;

/// An open output stream.
pub trait AudioStream: Send {
    fn play(&mut self) -> Result<(), AudioError>;
    fn pause(&mut self) -> Result<(), AudioError>;
    /// Sample frames the device has actually played since the stream opened.
    /// **This is the playback master clock.**
    fn frames_played(&self) -> u64 {
        self.clock().read().0
    }
    /// The same count with its time stamp, shared, so other threads (the
    /// UI) can read the clock without asking the engine.
    fn clock(&self) -> Arc<AudioClock>;
    /// Delay from the callback to the speaker, in frames.
    fn latency_frames(&self) -> u32;
    fn config(&self) -> AudioConfig;
}

pub trait AudioOutput: Send + Sync {
    /// The device's own format (rate, channels). Opening with it never
    /// fails for being unsupported; the mixer resamples to match.
    fn preferred(&self) -> Option<AudioConfig> {
        None
    }
    fn open(&self, want: AudioConfig, render: RenderCallback) -> Result<Box<dyn AudioStream>, AudioError>;
}
