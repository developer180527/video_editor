//! Thumbnails and waveform peaks, made in the background for the UI.
//!
//! One worker with a queue: asking never blocks — it returns what is ready
//! and queues the rest; the waker fires as results land. Thumbnails are
//! small display-encoded RGBA images (for drawing, not for colour work);
//! peaks are the loudest sample in each 1/100 s of an asset's audio.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Condvar, Mutex};

use ve_model::{AssetId, MediaRef};
use ve_ports::{FrameData, MediaBackend, PixelFormat, Storage, VideoFrame};
use ve_time::Time;

use crate::Waker;

/// Thumbnail height in pixels; width follows the source's aspect.
pub const THUMB_H: u32 = 90;
/// Peak buckets per second.
pub const PEAKS_PER_SECOND: u32 = 100;
/// Thumbnails are made on a grid this coarse (seconds), to bound the work.
const GRID: i64 = 1;

pub struct Thumb {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
enum Job {
    Thumb(AssetId, MediaRef, i64),
    Peaks(AssetId, MediaRef),
}

#[derive(Default)]
struct State {
    queue: VecDeque<Job>,
    queued: HashSet<Job>,
    thumbs: HashMap<(AssetId, i64), Arc<Thumb>>,
    peaks: HashMap<AssetId, Arc<Vec<f32>>>,
    failed: HashSet<AssetId>,
    version: u64,
}

pub struct Stills {
    state: Mutex<State>,
    wake: Condvar,
    waker: Mutex<Option<Waker>>,
}

impl Stills {
    pub fn new(storage: Arc<dyn Storage>, media: Arc<dyn MediaBackend>) -> Arc<Self> {
        let s = Arc::new(Stills { state: Mutex::new(State::default()), wake: Condvar::new(), waker: Mutex::new(None) });
        let w = Arc::downgrade(&s);
        std::thread::Builder::new().name("ve-stills".into()).spawn(move || worker(w, storage, media)).expect("stills thread");
        s
    }

    pub fn set_waker(&self, w: Waker) {
        *self.waker.lock().unwrap() = Some(w);
    }

    /// Changes whenever a thumbnail or peaks arrive.
    pub fn version(&self) -> u64 {
        self.state.lock().unwrap().version
    }

    fn ask(&self, st: &mut State, job: Job) {
        if st.queued.insert(job.clone()) {
            st.queue.push_back(job);
            self.wake.notify_one();
        }
    }

    /// The thumbnail nearest `t` (on a 1 s grid) if ready, else queue it.
    /// The key returned identifies the image for texture caching.
    pub fn thumb(&self, asset: AssetId, media: &MediaRef, t: Time) -> Option<((AssetId, i64), Arc<Thumb>)> {
        let slot = t.ticks().div_euclid(ve_time::TICKS_PER_SECOND * GRID);
        let mut st = self.state.lock().unwrap();
        if st.failed.contains(&asset) {
            return None;
        }
        match st.thumbs.get(&(asset, slot)) {
            Some(t) => Some(((asset, slot), t.clone())),
            None => {
                self.ask(&mut st, Job::Thumb(asset, media.clone(), slot));
                None
            }
        }
    }

    /// Peaks for the whole asset if ready, else queue them.
    pub fn peaks(&self, asset: AssetId, media: &MediaRef) -> Option<Arc<Vec<f32>>> {
        let mut st = self.state.lock().unwrap();
        match st.peaks.get(&asset) {
            Some(p) => Some(p.clone()),
            None => {
                if !st.failed.contains(&asset) {
                    self.ask(&mut st, Job::Peaks(asset, media.clone()));
                }
                None
            }
        }
    }

    pub fn purge(&self) {
        let mut st = self.state.lock().unwrap();
        st.thumbs.clear();
    }
}

/// YCbCr (limited, BT.709) → display RGB, nearest-neighbour downscale.
fn to_thumb(f: &VideoFrame) -> Option<Thumb> {
    let FrameData::Cpu { planes, strides } = &f.data else { return None };
    let h = THUMB_H.min(f.height);
    let w = ((f.width as u64 * h as u64) / f.height.max(1) as u64).max(1) as u32;
    let deep = f.format == PixelFormat::P010;
    let sample = |plane: &[u8], stride: usize, x: usize, y: usize, comp: usize| -> f32 {
        if deep {
            let i = y * stride + x * 4 + comp * 2;
            u16::from_le_bytes([plane[i], plane[i + 1]]) as f32 / 65535.0
        } else {
            plane[y * stride + x * 2 + comp] as f32 / 255.0
        }
    };
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for ty in 0..h {
        let sy = (ty as u64 * f.height as u64 / h as u64) as usize;
        for tx in 0..w {
            let sx = (tx as u64 * f.width as u64 / w as u64) as usize;
            let y = if deep {
                let i = sy * strides[0] + sx * 2;
                u16::from_le_bytes([planes[0][i], planes[0][i + 1]]) as f32 / 65535.0
            } else {
                planes[0][sy * strides[0] + sx] as f32 / 255.0
            };
            let (cb, cr) = (sample(&planes[1], strides[1], sx / 2, sy / 2, 0) - 0.5, sample(&planes[1], strides[1], sx / 2, sy / 2, 1) - 0.5);
            let y = (y - 16.0 / 255.0) * (255.0 / 219.0);
            let (cb, cr) = (cb * (255.0 / 224.0), cr * (255.0 / 224.0));
            let r = y + 1.5748 * cr;
            let g = y - 0.1873 * cb - 0.4681 * cr;
            let b = y + 1.8556 * cb;
            rgba.extend([r, g, b].map(|c| (c.clamp(0.0, 1.0) * 255.0) as u8));
            rgba.push(255);
        }
    }
    Some(Thumb { width: w, height: h, rgba })
}

fn worker(s: std::sync::Weak<Stills>, storage: Arc<dyn Storage>, media: Arc<dyn MediaBackend>) {
    loop {
        let Some(stills) = s.upgrade() else { return };
        let job = {
            let mut st = stills.state.lock().unwrap();
            loop {
                if let Some(j) = st.queue.pop_front() {
                    break j;
                }
                // Wait with a timeout so a dropped pool ends the thread.
                st = stills.wake.wait_timeout(st, std::time::Duration::from_millis(500)).unwrap().0;
                if std::sync::Arc::strong_count(&stills) == 1 {
                    return;
                }
            }
        };
        drop(stills);
        let result: Result<(), AssetId> = match &job {
            Job::Thumb(asset, m, slot) => {
                let made = storage
                    .resolve(m)
                    .ok()
                    .and_then(|r| media.open_video(&r).ok())
                    .and_then(|mut d| {
                        let t = Time::from_seconds(slot * GRID);
                        if *slot > 0 {
                            d.seek(t).ok()?;
                        }
                        d.next_frame().ok().flatten()
                    })
                    .and_then(|f| to_thumb(&f));
                match made {
                    Some(t) => {
                        if let Some(stills) = s.upgrade() {
                            let mut st = stills.state.lock().unwrap();
                            st.thumbs.insert((*asset, *slot), Arc::new(t));
                            st.version += 1;
                        }
                        Ok(())
                    }
                    None => Err(*asset),
                }
            }
            Job::Peaks(asset, m) => {
                let rate = 8_000u32;
                let per = (rate / PEAKS_PER_SECOND) as usize;
                let peaks = storage.resolve(m).ok().and_then(|r| media.open_audio(&r, rate, 1).ok()).map(|mut d| {
                    let mut out = Vec::new();
                    let (mut acc, mut n) = (0f32, 0usize);
                    while let Ok(Some(b)) = d.next_block() {
                        for v in b.samples {
                            acc = acc.max(v.abs());
                            n += 1;
                            if n == per {
                                out.push(acc);
                                acc = 0.0;
                                n = 0;
                            }
                        }
                    }
                    out
                });
                match peaks {
                    Some(p) => {
                        if let Some(stills) = s.upgrade() {
                            let mut st = stills.state.lock().unwrap();
                            st.peaks.insert(*asset, Arc::new(p));
                            st.version += 1;
                        }
                        Ok(())
                    }
                    None => Err(*asset),
                }
            }
        };
        if let Some(stills) = s.upgrade() {
            if let Err(asset) = result {
                // Audio-only files have no picture, video-only no sound: not errors.
                if matches!(job, Job::Thumb(..)) {
                    stills.state.lock().unwrap().failed.insert(asset);
                }
            }
            stills.state.lock().unwrap().queued.remove(&job);
            if let Some(w) = stills.waker.lock().unwrap().as_ref() {
                w();
            }
        }
    }
}
