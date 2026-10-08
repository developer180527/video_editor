//! Real-time playback: a `Mixer` running ahead of the audio device.
//!
//! The mixer thread renders blocks from the play position into a lock-free
//! ring; the device's callback only pops samples and measures levels — no
//! locks, no allocation, no decoding on the real-time thread. Restarting
//! (play from elsewhere, an edit) bumps a generation number; the callback
//! drops samples of an old generation.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rtrb::{Consumer, Producer, RingBuffer};
use ve_model::{SequenceId, Snapshot};
use ve_ports::RenderCallback;
use ve_time::Time;

use crate::Mixer;

const BLOCK: usize = 1024;

/// Peak levels of the last callback, per channel, as linear amplitude.
#[derive(Default)]
pub struct Meters {
    left: AtomicU32,
    right: AtomicU32,
}

impl Meters {
    pub fn peaks(&self) -> [f32; 2] {
        [f32::from_bits(self.left.load(Ordering::Relaxed)), f32::from_bits(self.right.load(Ordering::Relaxed))]
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
        let s = shared.clone();
        std::thread::Builder::new().name("ve-mixer".into()).spawn(move || mix_thread(mixer, prod, s)).expect("mixer thread");
        let callback = make_callback(cons, shared.clone(), meters.clone(), channels as usize);
        (Playback { shared, meters, rate }, callback)
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Play `seq` of `project` from `at` (restarts if already playing).
    pub fn play(&self, project: Snapshot, seq: SequenceId, at: Time) {
        let g = self.shared.generation.fetch_add(1, Ordering::SeqCst) + 1;
        *self.shared.job.lock().unwrap() = Some((project, seq, at, g));
        self.shared.playing.store(true, Ordering::SeqCst);
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

fn mix_thread(mut mixer: Mixer, mut prod: Producer<(u64, [f32; 2])>, s: Arc<Shared>) {
    let rate = mixer.rate();
    let mut current: Option<(Snapshot, SequenceId, Time, u64)> = None;
    let mut buf = vec![0f32; BLOCK * 2];
    while !s.quit.load(Ordering::Relaxed) {
        // A new job replaces the current one.
        if let Some(job) = s.job.lock().unwrap().take() {
            current = Some(job);
        }
        if !s.playing.load(Ordering::Relaxed) {
            current = None;
        }
        let Some((project, seq_id, at, g)) = current.as_mut() else {
            std::thread::sleep(Duration::from_millis(5));
            continue;
        };
        if prod.slots() < BLOCK {
            std::thread::sleep(Duration::from_millis(3));
            continue;
        }
        if let Some(seq) = project.sequence(*seq_id).cloned() {
            mixer.render(project, &seq, *at, &mut buf);
        } else {
            buf.fill(0.0);
        }
        for f in buf.as_chunks::<2>().0 {
            let _ = prod.push((*g, *f));
        }
        *at += Time((BLOCK as i128 * ve_time::TICKS_PER_SECOND as i128 / rate as i128) as i64);
    }
}

fn make_callback(mut cons: Consumer<(u64, [f32; 2])>, s: Arc<Shared>, meters: Arc<Meters>, channels: usize) -> RenderCallback {
    Box::new(move |out: &mut [f32]| {
        let g = s.generation.load(Ordering::Relaxed);
        let playing = s.playing.load(Ordering::Relaxed);
        let (mut pl, mut pr) = (0f32, 0f32);
        for frame in out.chunks_mut(channels) {
            let mut sample = [0f32; 2];
            if playing {
                // Skip samples from before the last restart.
                while let Ok((sg, v)) = cons.peek().copied() {
                    let _ = cons.pop();
                    if sg == g {
                        sample = v;
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
        meters.left.store(pl.to_bits(), Ordering::Relaxed);
        meters.right.store(pr.to_bits(), Ordering::Relaxed);
    })
}
