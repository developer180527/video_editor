//! Real-time playback: a `Mixer` running ahead of the audio device.
//!
//! The mixer thread renders blocks from the play position into a lock-free
//! ring; the device's callback only pops samples and measures levels — no
//! locks, no allocation, no decoding on the real-time thread. Restarting
//! (play from elsewhere, an edit) bumps a generation number; the callback
//! drops samples of an old generation.
//!
//! Per-track levels and loudness are measured by the mixer as it renders,
//! which is ahead of what is heard; each block's readings wait in
//! [`Meters`] until the callback has played that block, so meters move with
//! the sound.
//!
//! [`Playback::clock`] counts only the mixed samples the callback actually
//! delivered — not the silence it plays while the mixer starts up or falls
//! behind. That count is the playhead's clock, so the picture waits for the
//! sound instead of running ahead of it.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rtrb::{Consumer, Producer, RingBuffer};
use ve_model::{SequenceId, Snapshot};
use ve_ports::{AudioClock, RenderCallback};
use ve_time::Time;

use crate::mixer::{Levels, TrackMix};
use crate::Mixer;

const BLOCK: usize = 1024;

/// Peak levels of the last callback, per channel, as linear amplitude; and
/// the mixer's per-track levels and loudness, as of what has been heard.
#[derive(Default)]
pub struct Meters {
    left: AtomicU32,
    right: AtomicU32,
    /// Readings rendered but maybe not heard yet: (generation, the sample
    /// count at the block's end, levels).
    pending: Mutex<VecDeque<(u64, u64, Levels)>>,
    /// The generation the callback is playing, and samples of it heard.
    heard_gen: AtomicU64,
    heard: AtomicU64,
    /// The last reading that was heard, and when.
    last: Mutex<(Levels, Option<Instant>)>,
}

/// Readings stop moving this long after the sound stops.
const HOLD: Duration = Duration::from_millis(150);

impl Meters {
    pub fn peaks(&self) -> [f32; 2] {
        [f32::from_bits(self.left.load(Ordering::Relaxed)), f32::from_bits(self.right.load(Ordering::Relaxed))]
    }

    /// Per-track peaks and loudness as of the sound being heard now. Track
    /// peaks fall to zero when nothing plays; loudness holds its last value.
    pub fn levels(&self) -> Levels {
        let (g, heard) = (self.heard_gen.load(Ordering::Acquire), self.heard.load(Ordering::Acquire));
        let mut last = self.last.lock().unwrap();
        let mut pending = self.pending.lock().unwrap();
        while let Some((pg, at, _)) = pending.front() {
            if *pg < g || (*pg == g && *at <= heard) {
                let (pg, _, l) = pending.pop_front().unwrap();
                if pg == g {
                    *last = (l, Some(Instant::now()));
                }
            } else {
                break;
            }
        }
        let mut out = last.0.clone();
        if last.1.is_none_or(|t| t.elapsed() > HOLD) {
            for (_, p) in &mut out.tracks {
                *p = [0.0; 2];
            }
        }
        out
    }

    fn rendered(&self, g: u64, at: u64, levels: Levels) {
        let mut pending = self.pending.lock().unwrap();
        // Half a second of blocks at most; older ones were never heard.
        if pending.len() >= 64 {
            pending.pop_front();
        }
        pending.push_back((g, at, levels));
    }
}

struct Shared {
    /// What to play: the project, the sequence, from where, which generation.
    job: Mutex<Option<(Snapshot, SequenceId, Time, u64)>>,
    generation: AtomicU64,
    playing: AtomicBool,
    quit: AtomicBool,
}

pub struct Playback {
    shared: Arc<Shared>,
    pub meters: Arc<Meters>,
    overrides: TrackMix,
    clock: Arc<AudioClock>,
    rate: u32,
}

impl Playback {
    /// Start the mixer thread. Returns the playback controller and the
    /// callback to hand to the audio device, which plays `channels` channels
    /// at `mixer.rate()`.
    pub fn new(mixer: Mixer, channels: u16) -> (Playback, RenderCallback) {
        let rate = mixer.rate();
        // Half a second of stereo, tagged with its generation every block.
        let (prod, cons) = RingBuffer::<(u64, [f32; 2])>::new(rate as usize / 2);
        let shared = Arc::new(Shared { job: Mutex::new(None), generation: AtomicU64::new(0), playing: AtomicBool::new(false), quit: AtomicBool::new(false) });
        let meters = Arc::new(Meters::default());
        let clock = Arc::new(AudioClock::new());
        let overrides = mixer.overrides();
        let (s, m) = (shared.clone(), meters.clone());
        std::thread::Builder::new().name("ve-mixer".into()).spawn(move || mix_thread(mixer, prod, s, m)).expect("mixer thread");
        let callback = make_callback(cons, shared.clone(), meters.clone(), clock.clone(), channels as usize);
        (Playback { shared, meters, overrides, clock, rate }, callback)
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Hear `track` at this fader (dB) and pan while the user drags them,
    /// without restarting; `None` goes back to the project's values.
    pub fn preview_track(&self, track: ve_model::TrackId, mix: Option<(f32, f32)>) {
        let mut o = self.overrides.lock().unwrap();
        match mix {
            Some(m) => o.insert(track, m),
            None => o.remove(&track),
        };
    }

    /// Mixed frames heard so far: the clock the playhead follows.
    pub fn clock(&self) -> Arc<AudioClock> {
        self.clock.clone()
    }

    /// Play `seq` of `project` from `at` (restarts if already playing).
    pub fn play(&self, project: Snapshot, seq: SequenceId, at: Time) {
        let g = self.shared.generation.fetch_add(1, Ordering::SeqCst) + 1;
        // `playing` first: the mixer drops a job it finds while not playing.
        self.shared.playing.store(true, Ordering::SeqCst);
        *self.shared.job.lock().unwrap() = Some((project, seq, at, g));
    }

    pub fn stop(&self) {
        self.shared.generation.fetch_add(1, Ordering::SeqCst);
        self.shared.playing.store(false, Ordering::SeqCst);
        *self.shared.job.lock().unwrap() = None;
    }
}

impl Drop for Playback {
    fn drop(&mut self) {
        self.shared.quit.store(true, Ordering::SeqCst);
    }
}

fn mix_thread(mut mixer: Mixer, mut prod: Producer<(u64, [f32; 2])>, s: Arc<Shared>, meters: Arc<Meters>) {
    let rate = mixer.rate();
    let mut current: Option<(Snapshot, SequenceId, Time, u64)> = None;
    let mut buf = vec![0f32; BLOCK * 2];
    // Samples rendered in the current generation.
    let mut rendered = 0u64;
    // How far ahead of the device to stay: enough to ride out a slow
    // decode, little enough that an edit or a fader is heard promptly.
    let ahead = rate as usize / 4;
    let capacity = prod.buffer().capacity();
    while !s.quit.load(Ordering::Relaxed) {
        // A new job replaces the current one; a new playback starts a new
        // loudness measurement (an edit while playing does not).
        if let Some(job) = s.job.lock().unwrap().take() {
            if current.is_none() {
                mixer.reset_loudness();
            }
            current = Some(job);
            rendered = 0;
        }
        if !s.playing.load(Ordering::Relaxed) {
            current = None;
        }
        let Some((project, seq_id, at, g)) = current.as_mut() else {
            std::thread::sleep(Duration::from_millis(5));
            continue;
        };
        if prod.slots() < BLOCK || capacity - prod.slots() > ahead {
            std::thread::sleep(Duration::from_millis(3));
            continue;
        }
        if let Some(seq) = project.sequence(*seq_id).cloned() {
            mixer.render(project, &seq, *at, &mut buf);
            rendered += BLOCK as u64;
            meters.rendered(*g, rendered, mixer.levels().clone());
        } else {
            buf.fill(0.0);
        }
        for f in buf.as_chunks::<2>().0 {
            let _ = prod.push((*g, *f));
        }
        *at += Time((BLOCK as i128 * ve_time::TICKS_PER_SECOND as i128 / rate as i128) as i64);
    }
}

fn make_callback(mut cons: Consumer<(u64, [f32; 2])>, s: Arc<Shared>, meters: Arc<Meters>, clock: Arc<AudioClock>, channels: usize) -> RenderCallback {
    Box::new(move |out: &mut [f32]| {
        let g = s.generation.load(Ordering::Relaxed);
        let playing = s.playing.load(Ordering::Relaxed);
        let (mut pl, mut pr) = (0f32, 0f32);
        let mut heard = 0u64;
        for frame in out.chunks_mut(channels) {
            let mut sample = [0f32; 2];
            if playing {
                // Skip samples from before the last restart; keep any from
                // a restart newer than this callback's view.
                while let Ok((sg, v)) = cons.peek().copied() {
                    if sg > g {
                        break;
                    }
                    let _ = cons.pop();
                    if sg == g {
                        sample = v;
                        heard += 1;
                        break;
                    }
                }
            } else {
                while cons.pop().is_ok() {}
            }
            pl = pl.max(sample[0].abs());
            pr = pr.max(sample[1].abs());
            match frame.len() {
                1 => frame[0] = (sample[0] + sample[1]) * 0.5,
                _ => {
                    frame[0] = sample[0];
                    frame[1] = sample[1];
                    frame[2..].fill(0.0);
                }
            }
        }
        let frames = (out.len() / channels.max(1)) as u64;
        clock.tick(heard, frames > 0 && heard == frames);
        if meters.heard_gen.load(Ordering::Relaxed) != g {
            meters.heard_gen.store(g, Ordering::Relaxed);
            meters.heard.store(0, Ordering::Release);
        }
        meters.heard.fetch_add(heard, Ordering::Release);
        meters.left.store(pl.to_bits(), Ordering::Relaxed);
        meters.right.store(pr.to_bits(), Ordering::Relaxed);
    })
}
