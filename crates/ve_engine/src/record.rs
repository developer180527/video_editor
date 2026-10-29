//! Voice-over recording: the microphone into a WAV file next to the user's
//! projects (Documents/Recordings), written as it records, so a long take
//! never sits in memory. When it stops, the file is imported and laid on the
//! armed track where recording began (see `Engine::import_placed`).
//!
//! The input callback runs on the real-time thread: it only copies into a
//! buffer reserved up front (taken with `try_lock`, so it never waits), and
//! a writer thread swaps that buffer out every few milliseconds.

use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ve_model::TrackId;
use ve_ports::{AudioConfig, CaptureStream, Location, Platform, Resolved};
use ve_time::Time;

/// What the input thread shares with the writer and the UI.
struct Shared {
    /// Samples captured since the writer last took them (interleaved).
    pending: Mutex<Vec<f32>>,
    /// Loudest sample since the UI last looked (f32 bits).
    peak: AtomicU32,
    /// Frames captured in all.
    frames: AtomicU64,
    /// Frames the input dropped because the writer held the buffer or it
    /// was full (should stay 0).
    dropped: AtomicU64,
}

/// Room for this much audio between two writer passes.
const SLACK_SECONDS: usize = 4;
const WRITE_EVERY: Duration = Duration::from_millis(20);

pub struct Recording {
    pub track: TrackId,
    /// Sequence time the take starts at.
    pub start: Time,
    pub path: PathBuf,
    config: AudioConfig,
    shared: Arc<Shared>,
    stream: Option<Box<dyn CaptureStream>>,
    stop: Arc<AtomicBool>,
    writer: Option<std::thread::JoinHandle<Result<(), String>>>,
    _access: Resolved,
}

/// The canonical 44-byte header of a 32-bit float WAV holding `frames`.
fn wav_header(config: AudioConfig, frames: u64) -> Vec<u8> {
    let ch = config.channels as u32;
    let data = (frames * ch as u64 * 4).min(u32::MAX as u64 - 36) as u32;
    let mut h = Vec::with_capacity(44);
    h.extend_from_slice(b"RIFF");
    h.extend_from_slice(&(36 + data).to_le_bytes());
    h.extend_from_slice(b"WAVEfmt ");
    h.extend_from_slice(&16u32.to_le_bytes());
    h.extend_from_slice(&3u16.to_le_bytes()); // IEEE float
    h.extend_from_slice(&(ch as u16).to_le_bytes());
    h.extend_from_slice(&config.sample_rate.to_le_bytes());
    h.extend_from_slice(&(config.sample_rate * ch * 4).to_le_bytes());
    h.extend_from_slice(&((ch * 4) as u16).to_le_bytes());
    h.extend_from_slice(&32u16.to_le_bytes());
    h.extend_from_slice(b"data");
    h.extend_from_slice(&data.to_le_bytes());
    h
}

impl Recording {
    /// Open the microphone and start writing a new take for `track` at `start`.
    pub fn start(platform: &Platform, track: TrackId, start: Time) -> Result<Recording, String> {
        let input = platform.audio_in.clone();
        let config = input.preferred().ok_or("No microphone is available.")?;
        // A fresh name: Voice-over 1.wav, 2, …
        let (media, access, path) = (1..10_000)
            .find_map(|n| {
                let media = platform.storage.location(Location::Documents, &format!("Recordings/Voice-over {n}.wav")).ok()?;
                let access = platform.storage.resolve_new(&media).ok()?;
                let path = access.path.clone()?;
                (!path.exists()).then_some((media, access, path))
            })
            .ok_or("Could not make a file for the recording.")?;
        let _ = media;
        let mut file = std::fs::File::create(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        file.write_all(&wav_header(config, 0)).map_err(|e| e.to_string())?;

        let room = config.sample_rate as usize * config.channels as usize * SLACK_SECONDS;
        let shared = Arc::new(Shared {
            pending: Mutex::new(Vec::with_capacity(room)),
            peak: AtomicU32::new(0),
            frames: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
        });
        let channels = config.channels.max(1) as u64;
        let cb = shared.clone();
        let capture = Box::new(move |buf: &[f32]| {
            let peak = buf.iter().fold(0f32, |m, x| m.max(x.abs()));
            // Non-negative floats order as their bits do.
            cb.peak.fetch_max(peak.to_bits(), Ordering::Relaxed);
            let frames = buf.len() as u64 / channels;
            match cb.pending.try_lock() {
                Ok(mut p) if p.len() + buf.len() <= p.capacity() => {
                    p.extend_from_slice(buf);
                    cb.frames.fetch_add(frames, Ordering::Relaxed);
                }
                _ => {
                    cb.dropped.fetch_add(frames, Ordering::Relaxed);
                }
            }
        });

        let stop = Arc::new(AtomicBool::new(false));
        let (w_shared, w_stop) = (shared.clone(), stop.clone());
        let writer = std::thread::Builder::new()
            .name("ve-record".into())
            .spawn(move || -> Result<(), String> {
                let mut spare: Vec<f32> = Vec::with_capacity(room);
                let mut bytes: Vec<u8> = Vec::with_capacity(room * 4);
                let mut written = 0u64;
                loop {
                    let last = w_stop.load(Ordering::SeqCst);
                    {
                        let mut p = w_shared.pending.lock().unwrap();
                        std::mem::swap(&mut *p, &mut spare);
                    }
                    bytes.clear();
                    for s in &spare {
                        bytes.extend_from_slice(&s.to_le_bytes());
                    }
                    file.write_all(&bytes).map_err(|e| e.to_string())?;
                    written += spare.len() as u64 / channels;
                    spare.clear();
                    if last {
                        break;
                    }
                    std::thread::sleep(WRITE_EVERY);
                }
                // The real sizes, now they are known.
                file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
                file.write_all(&wav_header(config, written)).map_err(|e| e.to_string())?;
                file.sync_all().map_err(|e| e.to_string())
            })
            .map_err(|e| e.to_string())?;

        let stream = match input.open(capture) {
            Ok(s) => s,
            Err(e) => {
                stop.store(true, Ordering::SeqCst);
                let _ = writer.join();
                let _ = std::fs::remove_file(&path);
                return Err(format!("The microphone could not be opened: {e}"));
            }
        };
        Ok(Recording { track, start, path, config: stream.config(), shared, stream: Some(stream), stop, writer: Some(writer), _access: access })
    }

    /// How long the take is so far.
    pub fn elapsed(&self) -> Time {
        let frames = self.shared.frames.load(Ordering::Relaxed);
        Time::from_ticks((frames as i128 * ve_time::TICKS_PER_SECOND as i128 / self.config.sample_rate.max(1) as i128) as i64)
    }

    /// The loudest sample since the last call (for the record meter).
    pub fn take_peak(&self) -> f32 {
        f32::from_bits(self.shared.peak.swap(0, Ordering::Relaxed))
    }

    /// Stop: close the microphone, finish the file. Returns its path, or
    /// `None` when nothing was captured (the empty file is removed).
    pub fn finish(mut self) -> Result<Option<PathBuf>, String> {
        self.stream = None; // the input stops
        self.stop.store(true, Ordering::SeqCst);
        if let Some(w) = self.writer.take() {
            w.join().map_err(|_| "the recording writer failed".to_string())??;
        }
        let dropped = self.shared.dropped.load(Ordering::Relaxed);
        if dropped > 0 {
            eprintln!("voice-over: {dropped} frames dropped");
        }
        if self.shared.frames.load(Ordering::Relaxed) == 0 {
            let _ = std::fs::remove_file(&self.path);
            return Ok(None);
        }
        Ok(Some(self.path.clone()))
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        // Abandoned without `finish` (the app closing): still a valid file.
        self.stream = None;
        self.stop.store(true, Ordering::SeqCst);
        if let Some(w) = self.writer.take() {
            let _ = w.join();
        }
    }
}
