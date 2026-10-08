use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use ve_model::{AssetId, MediaRef};
use ve_ports::{FrameData, MediaBackend, Storage, VideoDecoder, VideoFrame};
use ve_time::Time;

use crate::Waker;

/// How far ahead of the wanted time a worker decodes.
const AHEAD: Time = Time::from_seconds(1);
/// How much behind it keeps (scrubbing backwards a little stays cached).
const BEHIND: Time = Time::from_ticks(254_016_000_000 / 2);
/// A request this far beyond the decoder seeks instead of decoding through.
const SEEK_GAP: Time = Time::from_seconds(2);

/// The result of asking for a frame.
pub enum Lookup {
    /// The frame shown at the time asked for.
    Exact(Arc<VideoFrame>),
    /// Not decoded yet; the nearest frame there is, to show meanwhile.
    Nearest(Arc<VideoFrame>),
    /// Nothing yet.
    Pending,
    /// The source cannot be decoded.
    Failed(String),
}

impl Lookup {
    pub fn frame(&self) -> Option<&Arc<VideoFrame>> {
        match self {
            Lookup::Exact(f) | Lookup::Nearest(f) => Some(f),
            _ => None,
        }
    }
}

fn frame_bytes(f: &VideoFrame) -> usize {
    match &f.data {
        FrameData::Cpu { planes, .. } => planes.iter().map(|p| p.len()).sum(),
        FrameData::Native(_) => 1 << 20,
    }
}

#[derive(Default)]
struct State {
    want: Option<Time>,
    frames: BTreeMap<Time, Arc<VideoFrame>>,
    bytes: usize,
    error: Option<String>,
    closed: bool,
}

impl State {
    fn covering(&self, t: Time) -> Option<&Arc<VideoFrame>> {
        self.frames.range(..=t).next_back().map(|(_, f)| f).filter(|f| t < f.pts + f.duration)
    }

    fn nearest(&self, t: Time) -> Option<&Arc<VideoFrame>> {
        let before = self.frames.range(..=t).next_back();
        let after = self.frames.range(t..).next();
        match (before, after) {
            (Some(b), Some(a)) => Some(if t - *b.0 < *a.0 - t { b.1 } else { a.1 }),
            (Some(b), None) => Some(b.1),
            (None, Some(a)) => Some(a.1),
            (None, None) => None,
        }
    }

    /// Frames decoded at or after `t`, up to `AHEAD`.
    fn ready_ahead(&self, t: Time) -> Time {
        let mut end = t;
        for (pts, f) in self.frames.range(..=t + AHEAD) {
            if *pts <= end && pts.ticks() + f.duration.ticks() > end.ticks() {
                end = *pts + f.duration;
            }
        }
        end - t
    }

    fn evict(&mut self, budget: usize) {
        let Some(t) = self.want else { return };
        // Behind the window: drop.
        let old: Vec<Time> = self.frames.range(..t - BEHIND).map(|(k, _)| *k).collect();
        for k in old {
            if let Some(f) = self.frames.remove(&k) {
                self.bytes -= frame_bytes(&f);
            }
        }
        // Over budget: drop the farthest from the wanted time.
        while self.bytes > budget && self.frames.len() > 2 {
            let (first, last) = (*self.frames.keys().next().unwrap(), *self.frames.keys().next_back().unwrap());
            let k = if (t - first).ticks().abs() > (last - t).ticks().abs() { first } else { last };
            let f = self.frames.remove(&k).unwrap();
            self.bytes -= frame_bytes(&f);
        }
    }
}

struct Source {
    state: Mutex<State>,
    wake: Condvar,
}

/// Background decode, one worker per source.
pub struct VideoPool {
    storage: Arc<dyn Storage>,
    media: Arc<dyn MediaBackend>,
    sources: Mutex<HashMap<AssetId, Arc<Source>>>,
    /// Bytes of decoded frames each source may hold.
    budget: usize,
    waker: Mutex<Option<Waker>>,
}

impl VideoPool {
    pub fn new(storage: Arc<dyn Storage>, media: Arc<dyn MediaBackend>, budget_per_source: usize) -> Arc<Self> {
        Arc::new(VideoPool { storage, media, sources: Mutex::new(HashMap::new()), budget: budget_per_source, waker: Mutex::new(None) })
    }

    pub fn set_waker(&self, w: Waker) {
        *self.waker.lock().unwrap() = Some(w);
    }

    fn source(self: &Arc<Self>, asset: AssetId, media: &MediaRef) -> Arc<Source> {
        let mut sources = self.sources.lock().unwrap();
        if let Some(s) = sources.get(&asset) {
            return s.clone();
        }
        let src = Arc::new(Source { state: Mutex::new(State::default()), wake: Condvar::new() });
        sources.insert(asset, src.clone());
        let (pool, s, media) = (Arc::downgrade(self), src.clone(), media.clone());
        std::thread::Builder::new()
            .name("ve-decode".into())
            .spawn(move || worker(pool, s, media))
            .expect("decode thread");
        src
    }

    /// The frame of `asset` shown at source time `t`. Never blocks; asks the
    /// worker to decode around `t`.
    pub fn frame(self: &Arc<Self>, asset: AssetId, media: &MediaRef, t: Time) -> Lookup {
        let src = self.source(asset, media);
        let mut st = src.state.lock().unwrap();
        if let Some(e) = &st.error {
            return Lookup::Failed(e.clone());
        }
        if st.want != Some(t) {
            st.want = Some(t);
            src.wake.notify_all();
        }
        match st.covering(t) {
            Some(f) => Lookup::Exact(f.clone()),
            None => st.nearest(t).map_or(Lookup::Pending, |f| Lookup::Nearest(f.clone())),
        }
    }

    /// The exact frame at `t`, waiting for it (export).
    pub fn frame_blocking(self: &Arc<Self>, asset: AssetId, media: &MediaRef, t: Time, timeout: Duration) -> Result<Arc<VideoFrame>, String> {
        let src = self.source(asset, media);
        let deadline = std::time::Instant::now() + timeout;
        let mut st = src.state.lock().unwrap();
        st.want = Some(t);
        src.wake.notify_all();
        loop {
            if let Some(e) = &st.error {
                return Err(e.clone());
            }
            if let Some(f) = st.covering(t) {
                return Ok(f.clone());
            }
            let now = std::time::Instant::now();
            if now >= deadline {
                // Past the end of the media: hold the last frame.
                return st.nearest(t).cloned().ok_or_else(|| "timed out waiting for a frame".into());
            }
            st = src.wake.wait_timeout(st, deadline - now).unwrap().0;
        }
    }

    /// Stop decoding `asset` and free its frames (it left the project).
    pub fn close(&self, asset: AssetId) {
        if let Some(s) = self.sources.lock().unwrap().remove(&asset) {
            s.state.lock().unwrap().closed = true;
            s.wake.notify_all();
        }
    }

    /// Drop every cached frame (memory warning).
    pub fn purge(&self) {
        for s in self.sources.lock().unwrap().values() {
            let mut st = s.state.lock().unwrap();
            st.frames.clear();
            st.bytes = 0;
        }
    }

    fn wake_ui(&self) {
        if let Some(w) = self.waker.lock().unwrap().as_ref() {
            w();
        }
    }
}

fn worker(pool: std::sync::Weak<VideoPool>, src: Arc<Source>, media: MediaRef) {
    let fail = |msg: String| {
        src.state.lock().unwrap().error = Some(msg);
        src.wake.notify_all();
    };
    let Some(p) = pool.upgrade() else { return };
    let resolved = match p.storage.resolve(&media) {
        Ok(r) => r,
        Err(e) => return fail(e.to_string()),
    };
    let mut dec: Box<dyn VideoDecoder> = match p.media.open_video(&resolved) {
        Ok(d) => d,
        Err(e) => return fail(e.to_string()),
    };
    let budget = p.budget;
    drop(p);
    // Where the decoder will continue from (the end of its last frame).
    let mut pos: Option<Time> = None;
    let mut at_end = false;
    // A wanted time the decoder cannot reach by decoding on: before it, or far ahead.
    let must_seek = |t: Time, pos: Option<Time>| pos.is_none_or(|p| t < p || t > p + SEEK_GAP);
    loop {
        let (target, seek) = {
            let mut st = src.state.lock().unwrap();
            loop {
                if st.closed {
                    return;
                }
                if let Some(t) = st.want {
                    let seek = st.covering(t).is_none() && must_seek(t, pos);
                    if seek {
                        at_end = false;
                    }
                    if seek || (!at_end && st.ready_ahead(t) < AHEAD) {
                        break (t, seek);
                    }
                }
                st = src.wake.wait(st).unwrap();
            }
        };
        if seek {
            if let Err(e) = dec.seek(target) {
                return fail(e.to_string());
            }
        }
        match dec.next_frame() {
            Ok(Some(f)) => {
                pos = Some(f.pts + f.duration);
                let f = Arc::new(f);
                let fresh;
                {
                    let mut st = src.state.lock().unwrap();
                    fresh = st.want.is_some_and(|t| f.pts <= t && t < f.pts + f.duration);
                    st.bytes += frame_bytes(&f);
                    if let Some(old) = st.frames.insert(f.pts, f) {
                        st.bytes -= frame_bytes(&old);
                    }
                    st.evict(budget);
                }
                src.wake.notify_all();
                if fresh {
                    if let Some(p) = pool.upgrade() {
                        p.wake_ui();
                    }
                }
            }
            Ok(None) => {
                at_end = true;
                src.wake.notify_all();
            }
            Err(e) => return fail(e.to_string()),
        }
    }
}
