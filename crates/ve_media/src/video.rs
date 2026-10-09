use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use ve_model::{AssetId, MediaRef};
use ve_ports::{FrameData, MediaBackend, Resolved, Storage, VideoDecoder, VideoFrame};
use ve_time::Time;

use crate::Waker;

/// How far ahead of the wanted time a worker decodes.
const AHEAD: Time = Time::from_seconds(1);
/// How much behind it keeps (scrubbing backwards a little stays cached).
const BEHIND: Time = Time::from_ticks(254_016_000_000 / 2);
/// A request this far beyond the decoder seeks instead of decoding through.
const SEEK_GAP: Time = Time::from_seconds(2);
/// How much of an upcoming clip is decoded before the playhead gets there.
const PREFETCH: Time = Time::from_ticks(254_016_000_000 / 2);
/// A prefetch hint not renewed for this long is forgotten (playback stopped,
/// the playhead moved).
const HINT_LIFE: Duration = Duration::from_secs(2);
/// A source nobody has asked for in this long closes its decoder (its
/// frames stay cached). Decoders hold threads, memory and, on Apple, a
/// limited number of hardware sessions.
const IDLE: Duration = if cfg!(test) { Duration::from_millis(300) } else { Duration::from_secs(30) };

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
        FrameData::Cpu { .. } | FrameData::Shared(_) => f.data.cpu().map_or(0, |c| c.planes.iter().map(|p| p.len()).sum()),
        // Held by the hardware: count what the same picture would take in
        // memory (8- or 10-bit 4:2:0), so the budget still means something.
        FrameData::Native(_) => f.width as usize * f.height as usize * 3 / 2 * if f.format == ve_ports::PixelFormat::P010 { 2 } else { 1 },
    }
}

#[derive(Default)]
struct State {
    want: Option<Time>,
    frames: BTreeMap<Time, Arc<VideoFrame>>,
    bytes: usize,
    /// Size of the last frame decoded: what one more frame will cost.
    frame_bytes: usize,
    /// Where the stream ended, once the decoder has run off its end (until
    /// the next seek). Times past it show the last frame.
    end: Option<Time>,
    /// Where a clip of this source will start soon, and when that was said:
    /// decoded when the wanted time needs nothing, and kept from eviction.
    upcoming: Option<(Time, std::time::Instant)>,
    /// When a frame was last asked for.
    asked: Option<std::time::Instant>,
    error: Option<String>,
    closed: bool,
}

impl State {
    fn covering(&self, t: Time) -> Option<&Arc<VideoFrame>> {
        self.frames.range(..=t).next_back().filter(|(k, f)| t < (**k).max(f.pts) + f.duration).map(|(_, f)| f)
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

    /// The frame to show at `t` past the end of the stream: the last one.
    fn past_end(&self, t: Time) -> Option<&Arc<VideoFrame>> {
        self.end.filter(|e| t >= *e).and_then(|_| self.frames.values().next_back())
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

    /// The upcoming start, while its hint is fresh.
    fn upcoming(&self) -> Option<Time> {
        self.upcoming.filter(|(_, at)| at.elapsed() < HINT_LIFE).map(|(u, _)| u)
    }

    /// Frames kept for an upcoming clip.
    fn protected(&self, k: Time) -> bool {
        self.upcoming().is_some_and(|u| k >= u && k < u + PREFETCH)
    }

    /// Bytes of frames still wanted: `[t, t + AHEAD]` and the upcoming window.
    fn wanted_bytes(&self, t: Time) -> usize {
        self.frames
            .iter()
            .filter(|(k, f)| self.protected(**k) || (**k <= t + AHEAD && f.pts + f.duration > t))
            .map(|(_, f)| frame_bytes(f))
            .sum()
    }

    /// Whether decoding one more frame would overflow the budget with frames
    /// that are still wanted. Without this check a budget smaller than the
    /// look-ahead window makes the worker decode and immediately evict,
    /// forever.
    fn ahead_full(&self, t: Time, budget: usize) -> bool {
        self.wanted_bytes(t) + self.frame_bytes > budget
    }

    fn remove(&mut self, k: Time) {
        if let Some(f) = self.frames.remove(&k) {
            self.bytes -= frame_bytes(&f);
        }
    }

    fn evict(&mut self, budget: usize) {
        let Some(t) = self.want else { return };
        // Behind the window: drop (unless kept for an upcoming clip).
        let old: Vec<Time> = self.frames.range(..t - BEHIND).map(|(k, _)| *k).filter(|k| !self.protected(*k)).collect();
        for k in old {
            self.remove(k);
        }
        // Over budget: frames already shown go first (oldest first), then
        // the farthest ahead. The frame showing at `t` and frames kept for
        // an upcoming clip are never dropped.
        while self.bytes > budget {
            let behind = self.frames.iter().find(|(k, f)| !self.protected(**k) && f.pts + f.duration <= t).map(|(k, _)| *k);
            let victim = behind.or_else(|| self.frames.iter().rev().find(|(k, f)| **k > t && f.pts > t && !self.protected(**k)).map(|(k, _)| *k));
            match victim {
                Some(k) => self.remove(k),
                None => break,
            }
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
    /// Ask decoders to leave hardware frames in GPU memory: set by a
    /// consumer whose compositor can import them. Applies to decoders opened
    /// from then on.
    gpu_frames: std::sync::atomic::AtomicBool,
}

impl VideoPool {
    pub fn new(storage: Arc<dyn Storage>, media: Arc<dyn MediaBackend>, budget_per_source: usize) -> Arc<Self> {
        Arc::new(VideoPool {
            storage,
            media,
            sources: Mutex::new(HashMap::new()),
            budget: budget_per_source,
            waker: Mutex::new(None),
            gpu_frames: Default::default(),
        })
    }

    pub fn set_waker(&self, w: Waker) {
        *self.waker.lock().unwrap() = Some(w);
    }

    /// Whoever draws this pool's frames can import frames left in GPU
    /// memory (see `MediaBackend::open_video_for_gpu`).
    pub fn set_gpu_frames(&self, on: bool) {
        self.gpu_frames.store(on, std::sync::atomic::Ordering::Relaxed);
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
        st.asked = Some(std::time::Instant::now());
        if let Some(e) = &st.error {
            return Lookup::Failed(e.clone());
        }
        if st.want != Some(t) {
            st.want = Some(t);
            src.wake.notify_all();
        }
        if st.upcoming().is_some_and(|u| t >= u) {
            st.upcoming = None; // arrived
        }
        match st.covering(t).or_else(|| st.past_end(t)) {
            Some(f) => Lookup::Exact(f.clone()),
            None => st.nearest(t).map_or(Lookup::Pending, |f| Lookup::Nearest(f.clone())),
        }
    }

    /// A clip of `asset` will start showing source time `t` soon: decode its
    /// first frames ahead of time, without disturbing the frames wanted now.
    /// Renew the hint while it holds; it lapses after a couple of seconds.
    pub fn prefetch(self: &Arc<Self>, asset: AssetId, media: &MediaRef, t: Time) {
        let src = self.source(asset, media);
        let mut st = src.state.lock().unwrap();
        let fresh = st.upcoming() != Some(t);
        st.upcoming = Some((t, std::time::Instant::now()));
        st.asked = st.upcoming.map(|(_, at)| at);
        if fresh {
            src.wake.notify_all();
        }
    }

    /// The exact frame at `t`, waiting for it (export).
    pub fn frame_blocking(self: &Arc<Self>, asset: AssetId, media: &MediaRef, t: Time, timeout: Duration) -> Result<Arc<VideoFrame>, String> {
        let src = self.source(asset, media);
        let deadline = std::time::Instant::now() + timeout;
        let mut st = src.state.lock().unwrap();
        st.want = Some(t);
        st.asked = Some(std::time::Instant::now());
        src.wake.notify_all();
        loop {
            if let Some(e) = &st.error {
                return Err(e.clone());
            }
            if let Some(f) = st.covering(t).or_else(|| st.past_end(t)) {
                return Ok(f.clone());
            }
            let now = std::time::Instant::now();
            if now >= deadline {
                return Err(format!("timed out waiting for the frame at {:.3} s", t.as_seconds_f64()));
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
            st.end = None;
        }
    }

    fn wake_ui(&self) {
        if let Some(w) = self.waker.lock().unwrap().as_ref() {
            w();
        }
    }
}

impl Drop for VideoPool {
    /// Workers wait on their source, not on the pool: tell them to stop.
    fn drop(&mut self) {
        for s in self.sources.get_mut().unwrap().values() {
            s.state.lock().unwrap().closed = true;
            s.wake.notify_all();
        }
    }
}

fn worker(pool: std::sync::Weak<VideoPool>, src: Arc<Source>, media: MediaRef) {
    let fail = |msg: String| {
        src.state.lock().unwrap().error = Some(msg);
        src.wake.notify_all();
    };
    let Some(p) = pool.upgrade() else { return };
    let (storage, backend, budget) = (p.storage.clone(), p.media.clone(), p.budget);
    let gpu = pool.clone();
    drop(p);
    // Opened when there is work, closed when idle. The `Resolved` keeps the
    // file reachable (a security scope on iPadOS) while the decoder reads.
    let mut dec: Option<(Resolved, Box<dyn VideoDecoder>)> = None;
    // Where the decoder will continue from (the end of its last frame).
    let mut pos: Option<Time> = None;
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
                    let missing = st.covering(t).is_none() && st.past_end(t).is_none();
                    let seek = missing && must_seek(t, pos);
                    if seek {
                        st.end = None;
                    }
                    let more = st.end.is_none() && st.ready_ahead(t) < AHEAD && !st.ahead_full(t, budget);
                    if seek || (missing && st.end.is_none()) || more {
                        break (t, seek);
                    }
                    // Nothing needed now: get the next clip's head ready.
                    if let Some(u) = st.upcoming() {
                        let needed = st.ready_ahead(u) < PREFETCH && st.past_end(u).is_none() && !st.ahead_full(t, budget);
                        if needed && !(st.end.is_some() && pos.is_some_and(|p| u >= p)) {
                            let seek = st.covering(u).is_none() && must_seek(u, pos);
                            if seek {
                                st.end = None;
                            }
                            break (u, seek);
                        }
                    }
                }
                // Hints lapse on their own: wake to notice.
                if st.upcoming.is_some() && st.upcoming().is_none() {
                    st.upcoming = None;
                }
                // Nobody has asked for a while: close the decoder.
                if dec.is_some() && st.asked.is_none_or(|a| a.elapsed() >= IDLE) {
                    dec = None;
                    pos = None;
                }
                st = match (&st.upcoming, &dec) {
                    (Some(_), _) => src.wake.wait_timeout(st, HINT_LIFE).unwrap().0,
                    (None, Some(_)) => src.wake.wait_timeout(st, IDLE).unwrap().0,
                    (None, None) => src.wake.wait(st).unwrap(),
                };
            }
        };
        if dec.is_none() {
            let for_gpu = gpu.upgrade().is_some_and(|p| p.gpu_frames.load(std::sync::atomic::Ordering::Relaxed));
            let opened = storage.resolve(&media).map_err(|e| e.to_string()).and_then(|r| {
                let d = if for_gpu { backend.open_video_for_gpu(&r) } else { backend.open_video(&r) };
                d.map(|d| (r, d)).map_err(|e| e.to_string())
            });
            match opened {
                Ok(d) => dec = Some(d),
                Err(e) => return fail(e),
            }
        }
        let (_, d) = dec.as_mut().unwrap();
        // A freshly opened decoder is at its start: position it.
        if seek || pos.is_none() {
            if let Err(e) = d.seek(target) {
                return fail(e.to_string());
            }
            pos = None;
        }
        let seek = seek || pos.is_none();
        match d.next_frame() {
            Ok(Some(f)) => {
                pos = Some(f.pts + f.duration);
                // The first frame after a seek starts after the target only
                // when nothing covers it (the head of the stream): it is
                // shown from the target on.
                let key = if seek && f.pts > target { target } else { f.pts };
                let f = Arc::new(f);
                let fresh;
                {
                    let mut st = src.state.lock().unwrap();
                    fresh = st.want.is_some_and(|t| key <= t && t < f.pts + f.duration);
                    st.frame_bytes = frame_bytes(&f);
                    st.bytes += frame_bytes(&f);
                    if let Some(old) = st.frames.insert(key, f) {
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
                // Nothing at all after a seek: the end is the target, and the
                // decoder is not asked again until something else is wanted.
                let end = pos.unwrap_or(target);
                pos = Some(end);
                src.state.lock().unwrap().end = Some(end);
                src.wake.notify_all();
                if let Some(p) = pool.upgrade() {
                    p.wake_ui();
                }
            }
            Err(e) => return fail(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use ve_ports::*;
    use ve_time::Rate;

    const RATE: Rate = Rate::FPS_25;
    const FRAME: usize = 1000;

    /// A stream of `count` frames of `FRAME` bytes from `first` (a frame
    /// index), counting every frame decoded.
    struct Fake {
        first: i64,
        count: i64,
        decoded: Arc<AtomicUsize>,
        alive: Arc<AtomicUsize>,
    }

    struct FakeDec {
        alive: Arc<AtomicUsize>,
        first: i64,
        next: i64,
        end: i64,
        decoded: Arc<AtomicUsize>,
    }

    impl Drop for FakeDec {
        fn drop(&mut self) {
            self.alive.fetch_sub(1, Ordering::SeqCst);
        }
    }

    impl VideoDecoder for FakeDec {
        fn seek(&mut self, t: Time) -> Result<(), MediaError> {
            // Like the real decoder: past the end, the last frame.
            self.next = t.to_frame(RATE).clamp(self.first, self.end - 1);
            Ok(())
        }
        fn next_frame(&mut self) -> Result<Option<VideoFrame>, MediaError> {
            if self.next >= self.end {
                return Ok(None);
            }
            self.decoded.fetch_add(1, Ordering::SeqCst);
            let pts = RATE.frame_to_time(self.next);
            self.next += 1;
            Ok(Some(VideoFrame {
                pts,
                duration: RATE.frame_duration(),
                width: 1,
                height: 1,
                format: PixelFormat::Nv12,
                color: ColorTags::default(),
                data: FrameData::Cpu { planes: vec![vec![0; FRAME]], strides: vec![1] },
            }))
        }
    }

    impl MediaBackend for Fake {
        fn name(&self) -> &str {
            "fake"
        }
        fn probe(&self, _: &Resolved) -> Result<ve_model::MediaInfo, MediaError> {
            unimplemented!()
        }
        fn open_video(&self, _: &Resolved) -> Result<Box<dyn VideoDecoder>, MediaError> {
            self.alive.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(FakeDec { alive: self.alive.clone(), first: self.first, next: self.first, end: self.first + self.count, decoded: self.decoded.clone() }))
        }
        fn open_audio(&self, _: &Resolved, _: usize, _: u32, _: u16) -> Result<Box<dyn AudioDecoder>, MediaError> {
            unimplemented!()
        }
        fn open_encoder(&self, _: &Resolved, _: &EncoderSettings) -> Result<Box<dyn Encoder>, MediaError> {
            unimplemented!()
        }
    }

    use crate::fakes::AnyFile;

    fn pool(first: i64, count: i64, budget_frames: usize) -> (Arc<VideoPool>, Arc<AtomicUsize>) {
        let decoded = Arc::new(AtomicUsize::new(0));
        let media = Arc::new(Fake { first, count, decoded: decoded.clone(), alive: Arc::new(AtomicUsize::new(0)) });
        (VideoPool::new(Arc::new(AnyFile), media, budget_frames * FRAME), decoded)
    }

    const WAIT: Duration = Duration::from_secs(5);

    #[test]
    fn a_small_budget_does_not_decode_the_whole_file() {
        // Room for 10 frames; the look-ahead wants 25 (1 s at 25 fps).
        let (pool, decoded) = pool(0, 10_000, 10);
        let (a, m) = (AssetId::new(), MediaRef("x".into()));
        pool.frame_blocking(a, &m, Time::ZERO, WAIT).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        let n = decoded.load(Ordering::SeqCst);
        assert!(n <= 12, "decoded {n} frames into a 10-frame budget");
        // Playing on still finds every frame.
        for i in 1..60 {
            let f = pool.frame_blocking(a, &m, RATE.frame_to_time(i), WAIT).unwrap();
            assert_eq!(f.pts, RATE.frame_to_time(i));
        }
        let n = decoded.load(Ordering::SeqCst);
        assert!(n <= 75, "decoded {n} frames to play 60");
    }

    #[test]
    fn a_large_budget_decodes_a_second_ahead() {
        let (pool, decoded) = pool(0, 10_000, 1000);
        let (a, m) = (AssetId::new(), MediaRef("x".into()));
        pool.frame_blocking(a, &m, Time::ZERO, WAIT).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        // Count before asking for frame 24: asking moves the look-ahead
        // window, and a fast machine starts on the next second at once.
        let n = decoded.load(Ordering::SeqCst);
        assert!(n <= 30, "decoded {n} frames for a one-second look-ahead");
        assert!(matches!(pool.frame(a, &m, RATE.frame_to_time(24)), Lookup::Exact(_)), "frame 24 is within the second ahead");
    }

    #[test]
    fn before_the_first_frame_shows_the_first_frame() {
        let (pool, _) = pool(3, 50, 100);
        let f = pool.frame_blocking(AssetId::new(), &MediaRef("x".into()), Time::ZERO, WAIT).unwrap();
        assert_eq!(f.pts, RATE.frame_to_time(3));
    }

    #[test]
    fn past_the_end_holds_the_last_frame_without_waiting() {
        let (pool, _) = pool(0, 50, 100);
        let start = std::time::Instant::now();
        let f = pool.frame_blocking(AssetId::new(), &MediaRef("x".into()), RATE.frame_to_time(60), WAIT).unwrap();
        assert_eq!(f.pts, RATE.frame_to_time(49));
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn the_next_clip_is_ready_before_the_cut() {
        // Playing at 2 s; a clip of the same source starting at 30 s is next.
        let (pool, _) = pool(0, 10_000, 200);
        let (a, m) = (AssetId::new(), MediaRef("x".into()));
        let now = Time::from_seconds(2);
        pool.frame_blocking(a, &m, now, WAIT).unwrap();
        pool.prefetch(a, &m, Time::from_seconds(30));
        std::thread::sleep(Duration::from_millis(300));
        // The playing clip kept its own look-ahead…
        assert!(matches!(pool.frame(a, &m, now + RATE.frame_to_time(10)), Lookup::Exact(_)));
        // …and at the cut the incoming frame is already there, without
        // waiting. (Asked last: asking moves the wanted time to the cut.)
        assert!(matches!(pool.frame(a, &m, Time::from_seconds(30)), Lookup::Exact(_)), "next clip's head was decoded");
    }

    #[test]
    fn idle_sources_close_their_decoders() {
        let alive = Arc::new(AtomicUsize::new(0));
        let media = Arc::new(Fake { first: 0, count: 10_000, decoded: Arc::new(AtomicUsize::new(0)), alive: alive.clone() });
        let pool = VideoPool::new(Arc::new(AnyFile), media, 1000 * FRAME);
        let (a, m) = (AssetId::new(), MediaRef("x".into()));
        pool.frame_blocking(a, &m, Time::ZERO, WAIT).unwrap();
        assert_eq!(alive.load(Ordering::SeqCst), 1);
        std::thread::sleep(IDLE * 3);
        assert_eq!(alive.load(Ordering::SeqCst), 0, "closed when idle");
        // Cached frames are still there; new ones reopen it.
        assert!(matches!(pool.frame(a, &m, RATE.frame_to_time(3)), Lookup::Exact(_)));
        let f = pool.frame_blocking(a, &m, Time::from_seconds(100), WAIT).unwrap();
        assert_eq!(f.pts, Time::from_seconds(100));
        assert_eq!(alive.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_dropped_pool_stops_its_workers() {
        let (pool, decoded) = pool(0, 10_000, 1000);
        let (a, m) = (AssetId::new(), MediaRef("x".into()));
        pool.frame_blocking(a, &m, Time::ZERO, WAIT).unwrap();
        let src = Arc::downgrade(pool.sources.lock().unwrap().get(&a).unwrap());
        drop(pool);
        std::thread::sleep(Duration::from_millis(200));
        assert!(src.upgrade().is_none(), "worker still holds its source");
        let _ = decoded;
    }
}
