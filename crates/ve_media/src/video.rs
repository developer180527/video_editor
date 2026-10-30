use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use ve_model::MediaRef;
use ve_ports::{FrameData, MediaBackend, Resolved, Storage, VideoDecoder, VideoFrame};
use ve_time::Time;

use crate::Waker;

/// How far ahead of the wanted time a worker decodes.
const AHEAD: Time = Time::from_seconds(1);
/// How much behind it keeps: about a key-frame interval, so scrubbing
/// backwards finds the frames a backward seek decoded on its way (the
/// budget still decides; the oldest go first).
const BEHIND: Time = Time::from_seconds(5);
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
    /// The last ask got a stand-in (a nearby frame) or nothing: the asker is
    /// waiting, so the frame it wants arriving is news worth a wake.
    waiting: bool,
    /// The cache was emptied under the decoder: it starts again from the
    /// wanted time rather than decoding on from where it had got to.
    restart: bool,
    /// The wanted time last moved back (a drag backwards): keep what is
    /// behind it, not ahead.
    backward: bool,
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
    /// Lane mode (see `VideoPool::set_lanes`): the source cut into spans.
    spans: Option<Spans>,
    /// When a blocking ask last had to wait for its frame: decoding is what
    /// the consumer waits on, so more lanes help.
    starved: Option<std::time::Instant>,
}

/// A source cut at its seek points into spans that separate decoder
/// sessions ("lanes") decode side by side.
#[derive(Default)]
struct Spans {
    /// Span `k` is `[starts[k], starts[k + 1])`; the last runs to the end.
    starts: Vec<Time>,
    /// Being decoded.
    taken: HashSet<usize>,
    /// Decoded. Its frames are dropped once passed; a span the wanted time
    /// comes back to is decoded again from there (see `State::pick`).
    done: HashSet<usize>,
    /// Where a span was last decoded again from: not twice from the same
    /// place, so a frame that cannot be made fails the wait instead of
    /// being decoded for ever.
    redone: HashMap<usize, Time>,
}

impl Spans {
    fn of(&self, t: Time) -> usize {
        self.starts.partition_point(|s| *s <= t).saturating_sub(1)
    }

    fn end(&self, k: usize) -> Time {
        self.starts.get(k + 1).copied().unwrap_or(Time::MAX)
    }
}

impl State {
    fn covering(&self, t: Time) -> Option<&Arc<VideoFrame>> {
        let (k, f) = self.frames.range(..=t).next_back()?;
        let end = (*k).max(f.pts) + f.duration;
        if t < end {
            return Some(f);
        }
        // Frames whose stored lengths are rounded (to the stream's time
        // base, in decode order) leave slivers between them: a frame shows
        // until the next one starts when that is within a sliver of its end.
        let next = self.frames.range((std::ops::Bound::Excluded(t), std::ops::Bound::Unbounded)).next()?;
        (*next.0 <= end + sliver(f)).then_some(f)
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
        let from = self.frames.range(..=t).next_back().map_or(t, |(k, _)| *k);
        for (pts, f) in self.frames.range(from..) {
            if end >= t + AHEAD || *pts > end + sliver(f) {
                break;
            }
            if pts.ticks() + f.duration.ticks() > end.ticks() {
                end = *pts + f.duration;
            }
        }
        (end - t).min(AHEAD)
    }

    /// The upcoming start, while its hint is fresh.
    fn upcoming(&self) -> Option<Time> {
        self.upcoming.filter(|(_, at)| at.elapsed() < HINT_LIFE).map(|(u, _)| u)
    }

    /// Frames kept for an upcoming clip.
    fn protected(&self, k: Time) -> bool {
        self.upcoming().is_some_and(|u| k >= u && k < u + PREFETCH)
    }

    /// Bytes of frames still wanted: `[t, t + AHEAD]` (or, moving back,
    /// `[t - BEHIND, t]`) and the upcoming window.
    fn wanted_bytes(&self, t: Time) -> usize {
        let (from, to) = if self.backward { (t - BEHIND, t) } else { (t, t + AHEAD) };
        self.frames
            .iter()
            .filter(|(k, f)| self.protected(**k) || (**k <= to && f.pts + f.duration > from))
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

    /// Lane mode: drop frames already passed (in the direction of travel)
    /// until `bytes` fits, oldest first. Never what is ahead: that is the
    /// lanes' work, wanted next.
    fn evict_behind(&mut self, budget: usize) {
        let Some(t) = self.want else { return };
        while self.bytes > budget {
            let behind = if self.backward {
                self.frames.iter().rev().find(|(k, _)| **k > t && !self.protected(**k)).map(|(k, _)| *k)
            } else {
                self.frames.iter().find(|(k, f)| (**k).max(f.pts) + f.duration <= t && !self.protected(**k)).map(|(k, _)| *k)
            };
            match behind {
                Some(k) => self.remove(k),
                None => break,
            }
        }
    }

    /// Lane mode: the next span for an idle lane, and where to start it:
    /// the wanted time's span first, then the spans after it (before it,
    /// moving back) up to `window`, then the next clip's.
    fn pick(&self, window: usize) -> Option<(usize, Time)> {
        let sp = self.spans.as_ref()?;
        let t = self.want?;
        let cur = sp.of(t);
        let free = |k: usize| !sp.taken.contains(&k) && !sp.done.contains(&k) && self.end.is_none_or(|e| sp.starts[k] < e);
        // The wanted span: whole if never decoded; decoded once already
        // (come back to, its frames since dropped), from the wanted time.
        let missing = self.covering(t).is_none() && self.past_end(t).is_none();
        if missing && !sp.taken.contains(&cur) {
            if !sp.done.contains(&cur) {
                return Some((cur, sp.starts[cur]));
            }
            if sp.redone.get(&cur) != Some(&t) {
                return Some((cur, t));
            }
        }
        // Side by side only while the consumer waits on decoding; otherwise
        // one session at a time, going on into the next span. When the
        // consumer does not wait (a decoder faster than the encoder), more
        // sessions only take the media engine from the encoder: measured
        // slower.
        let starved = self.starved.is_some_and(|at| at.elapsed() < STARVED);
        if !starved && !sp.taken.is_empty() {
            return None;
        }
        let window = if starved { window } else { 1 };
        let last = sp.starts.len() - 1;
        let ahead: Vec<usize> = if self.backward { (cur.saturating_sub(window)..cur).rev().collect() } else { (cur + 1..=(cur + window).min(last)).collect() };
        if let Some(k) = ahead.into_iter().find(|k| free(*k)) {
            return Some((k, sp.starts[k]));
        }
        let k = sp.of(self.upcoming()?);
        free(k).then(|| (k, sp.starts[k]))
    }

    /// Lane mode: whether a lane decoding span `k`, at `pos`, makes frames
    /// wanted now (so it goes on even over budget).
    fn needed_soon(&self, k: usize, pos: Time) -> bool {
        let (Some(sp), Some(t)) = (&self.spans, self.want) else { return true };
        sp.of(t) == k && (self.backward || pos <= t + AHEAD)
    }

    fn evict(&mut self, budget: usize) {
        let Some(t) = self.want else { return };
        // Behind the window: drop (unless kept for an upcoming clip).
        let old: Vec<Time> = self.frames.range(..t - BEHIND).map(|(k, _)| *k).filter(|k| !self.protected(*k)).collect();
        for k in old {
            self.remove(k);
        }
        // Over budget. Moving forwards: frames already shown go first
        // (oldest first), then the farthest ahead. Moving backwards (a
        // drag back): the far end of the look-ahead first, then the frames
        // farthest behind — the ones just behind `t` are the very ones a
        // backward drag wants next. The frame showing at `t` and frames
        // kept for an upcoming clip are never dropped.
        while self.bytes > budget {
            let shown = self.covering(t).map(|f| f.pts);
            let showing = |k: &Time, f: &Arc<VideoFrame>| (*k <= t && t < (*k).max(f.pts) + f.duration) || shown == Some(f.pts);
            let ahead = || self.frames.iter().rev().find(|(k, f)| **k > t && !showing(k, f) && !self.protected(**k)).map(|(k, _)| *k);
            let behind = || self.frames.iter().find(|(k, f)| !self.protected(**k) && f.pts + f.duration <= t && !showing(k, f)).map(|(k, _)| *k);
            let victim = if self.backward { ahead().or_else(behind) } else { behind().or_else(ahead) };
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
    /// By media file: an asset's original and its proxy are separate
    /// sources; two assets of one file share one.
    sources: Mutex<HashMap<MediaRef, Arc<Source>>>,
    /// Bytes of decoded frames each source may hold.
    budget: usize,
    waker: Mutex<Option<Waker>>,
    /// Ask decoders to leave hardware frames in GPU memory: set by a
    /// consumer whose compositor can import them. Applies to decoders opened
    /// from then on.
    gpu_frames: std::sync::atomic::AtomicBool,
    /// Decoder sessions per source (see `set_lanes`), and the bytes each
    /// source may hold then.
    lanes: std::sync::atomic::AtomicUsize,
    lane_budget: std::sync::atomic::AtomicUsize,
}

/// Lane mode: the shortest span. Each span costs its lane a few frames
/// decoded twice (the reorder delay at its start); a second keeps that
/// small.
const MIN_SPAN: Time = Time::from_seconds(1);
/// Lane mode: how long after the consumer last waited for a frame more
/// lanes keep taking spans ahead.
const STARVED: Duration = Duration::from_secs(1);

impl VideoPool {
    pub fn new(storage: Arc<dyn Storage>, media: Arc<dyn MediaBackend>, budget_per_source: usize) -> Arc<Self> {
        Arc::new(VideoPool {
            storage,
            media,
            sources: Mutex::new(HashMap::new()),
            budget: budget_per_source,
            waker: Mutex::new(None),
            gpu_frames: Default::default(),
            lanes: std::sync::atomic::AtomicUsize::new(1),
            lane_budget: Default::default(),
        })
    }

    /// Decode each source with `lanes` sessions side by side, for a
    /// consumer that reads through sources in order as fast as it can
    /// (export). A hardware decoder is limited by the time each frame takes,
    /// not by the hardware, so separate sessions decoding separate spans
    /// multiply its speed; they decode ahead into `budget` bytes per source,
    /// so the spans must fit in it to run side by side. Sources whose seek
    /// points are not known decode with one session. Applies to sources
    /// opened from then on.
    pub fn set_lanes(&self, lanes: usize, budget: usize) {
        self.lanes.store(lanes.max(1), std::sync::atomic::Ordering::Relaxed);
        self.lane_budget.store(budget, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn set_waker(&self, w: Waker) {
        *self.waker.lock().unwrap() = Some(w);
    }

    /// Whoever draws this pool's frames can import frames left in GPU
    /// memory (see `MediaBackend::open_video_for_gpu`).
    pub fn set_gpu_frames(&self, on: bool) {
        self.gpu_frames.store(on, std::sync::atomic::Ordering::Relaxed);
    }

    fn source(self: &Arc<Self>, media: &MediaRef) -> Arc<Source> {
        let mut sources = self.sources.lock().unwrap();
        if let Some(s) = sources.get(media) {
            return s.clone();
        }
        let src = Arc::new(Source { state: Mutex::new(State::default()), wake: Condvar::new() });
        sources.insert(media.clone(), src.clone());
        let (pool, s, media) = (Arc::downgrade(self), src.clone(), media.clone());
        let lanes = self.lanes.load(std::sync::atomic::Ordering::Relaxed);
        std::thread::Builder::new()
            .name("ve-decode".into())
            .spawn(move || if lanes > 1 { start_lanes(pool, s, media, lanes) } else { worker(pool, s, media) })
            .expect("decode thread");
        src
    }

    /// The frame of `asset` shown at source time `t`. Never blocks; asks the
    /// worker to decode around `t`.
    pub fn frame(self: &Arc<Self>, media: &MediaRef, t: Time) -> Lookup {
        let src = self.source(media);
        let mut st = src.state.lock().unwrap();
        st.asked = Some(std::time::Instant::now());
        if let Some(e) = &st.error {
            return Lookup::Failed(e.clone());
        }
        if st.want != Some(t) {
            if let Some(prev) = st.want {
                st.backward = t < prev;
            }
            st.want = Some(t);
            src.wake.notify_all();
        }
        if st.upcoming().is_some_and(|u| t >= u) {
            st.upcoming = None; // arrived
        }
        let found = match st.covering(t).or_else(|| st.past_end(t)) {
            Some(f) => Lookup::Exact(f.clone()),
            None => st.nearest(t).map_or(Lookup::Pending, |f| Lookup::Nearest(f.clone())),
        };
        st.waiting = !matches!(found, Lookup::Exact(_));
        found
    }

    /// A clip of `asset` will start showing source time `t` soon: decode its
    /// first frames ahead of time, without disturbing the frames wanted now.
    /// Renew the hint while it holds; it lapses after a couple of seconds.
    pub fn prefetch(self: &Arc<Self>, media: &MediaRef, t: Time) {
        let src = self.source(media);
        let mut st = src.state.lock().unwrap();
        let fresh = st.upcoming() != Some(t);
        st.upcoming = Some((t, std::time::Instant::now()));
        st.asked = st.upcoming.map(|(_, at)| at);
        if fresh {
            src.wake.notify_all();
        }
    }

    /// The exact frame at `t`, waiting for it (export).
    pub fn frame_blocking(self: &Arc<Self>, media: &MediaRef, t: Time, timeout: Duration) -> Result<Arc<VideoFrame>, String> {
        let src = self.source(media);
        let deadline = std::time::Instant::now() + timeout;
        let mut st = src.state.lock().unwrap();
        if let Some(prev) = st.want.filter(|p| *p != t) {
            st.backward = t < prev;
        }
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
            if !st.frames.is_empty() {
                st.starved = Some(std::time::Instant::now());
            }
            let now = std::time::Instant::now();
            if now >= deadline {
                return Err(format!("timed out waiting for the frame at {:.3} s", t.as_seconds_f64()));
            }
            st = src.wake.wait_timeout(st, deadline - now).unwrap().0;
        }
    }

    /// Stop decoding `asset` and free its frames (it left the project).
    pub fn close(&self, media: &MediaRef) {
        if let Some(s) = self.sources.lock().unwrap().remove(media) {
            s.state.lock().unwrap().closed = true;
            s.wake.notify_all();
        }
    }

    /// Let go of cached frames (memory warning): all but the one each
    /// source is showing now, and wake the decoders, so anything still
    /// wanted is decoded again. (Without the wake, a paused monitor — whose
    /// wanted time no longer changes — asked for a frame nobody would
    /// decode, and stayed blank.)
    pub fn purge(&self) {
        for s in self.sources.lock().unwrap().values() {
            let mut st = s.state.lock().unwrap();
            // Under the key it was stored with (a seek can file a frame
            // under its target), so it still covers the wanted time.
            let keep = st.want.and_then(|t| {
                st.frames.range(..=t).next_back().filter(|(k, f)| t < (**k).max(f.pts) + f.duration).map(|(k, f)| (*k, f.clone()))
            });
            st.frames.clear();
            st.bytes = 0;
            st.end = None;
            st.restart = true;
            if let Some(sp) = &mut st.spans {
                sp.done.clear();
                sp.redone.clear();
            }
            if let Some((k, f)) = keep {
                st.bytes = frame_bytes(&f);
                st.frames.insert(k, f);
            }
            drop(st);
            s.wake.notify_all();
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

/// The most two neighbouring frames may be apart and still count as
/// back to back: a quarter of a frame.
fn sliver(f: &VideoFrame) -> Time {
    Time::from_ticks(f.duration.ticks() / 4)
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
    // The last place a seek went to: a seek to before it is a step back.
    let mut last_seek: Option<Time> = None;
    // A wanted time the decoder cannot reach by decoding on: before it, or far ahead.
    let must_seek = |t: Time, pos: Option<Time>| pos.is_none_or(|p| t < p || t > p + SEEK_GAP);
    loop {
        let (target, seek) = {
            let mut st = src.state.lock().unwrap();
            loop {
                if st.closed {
                    return;
                }
                if std::mem::take(&mut st.restart) {
                    pos = None;
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
            // Going backwards: keep the frames decoded on the way to the
            // target, which are the ones the next steps back will want.
            d.keep_lead_in(last_seek.is_some_and(|l| target < l));
            last_seek = Some(target);
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
                    // Only someone who was shown a stand-in needs waking:
                    // playback that keeps up asks for each frame on time.
                    fresh = st.waiting && st.want.is_some_and(|t| key <= t && t < f.pts + f.duration);
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

/// Open `media`'s decoder as the pool asks (frames left in GPU memory or not).
fn open_decoder(pool: &std::sync::Weak<VideoPool>, storage: &dyn Storage, backend: &dyn MediaBackend, media: &MediaRef) -> Result<(Resolved, Box<dyn VideoDecoder>), String> {
    let for_gpu = pool.upgrade().is_some_and(|p| p.gpu_frames.load(std::sync::atomic::Ordering::Relaxed));
    let r = storage.resolve(media).map_err(|e| e.to_string())?;
    let d = if for_gpu { backend.open_video_for_gpu(&r) } else { backend.open_video(&r) };
    d.map(|d| (r, d)).map_err(|e| e.to_string())
}

/// Lane mode's first thread: open the source, cut it into spans at its seek
/// points and start the other lanes — or, with no seek points (or just one
/// span), decode it the usual way.
fn start_lanes(pool: std::sync::Weak<VideoPool>, src: Arc<Source>, media: MediaRef, lanes: usize) {
    let Some(p) = pool.upgrade() else { return };
    let (storage, backend) = (p.storage.clone(), p.media.clone());
    drop(p);
    let mut dec = match open_decoder(&pool, &*storage, &*backend, &media) {
        Ok(d) => d,
        Err(e) => {
            src.state.lock().unwrap().error = Some(e);
            src.wake.notify_all();
            return;
        }
    };
    let mut starts: Vec<Time> = Vec::new();
    for t in dec.1.seek_points().unwrap_or_default() {
        if starts.last().is_none_or(|l| t >= *l + MIN_SPAN) {
            starts.push(t);
        }
    }
    if starts.len() < 2 {
        drop(dec);
        return worker(pool, src, media);
    }
    src.state.lock().unwrap().spans = Some(Spans { starts, ..Default::default() });
    for _ in 1..lanes {
        let (pool, src, media) = (pool.clone(), src.clone(), media.clone());
        std::thread::Builder::new().name("ve-decode".into()).spawn(move || lane(pool, src, media, None, lanes)).expect("decode thread");
    }
    dec.1.keep_lead_in(false);
    lane(pool, src, media, Some(dec), lanes)
}

/// One lane: take the next span the wanted time needs, decode it whole
/// (sleeping while the source's frames fill its budget, unless they are
/// wanted now), and again, until the pool closes the source.
fn lane(pool: std::sync::Weak<VideoPool>, src: Arc<Source>, media: MediaRef, mut dec: Option<(Resolved, Box<dyn VideoDecoder>)>, lanes: usize) {
    let Some(p) = pool.upgrade() else { return };
    let (storage, backend) = (p.storage.clone(), p.media.clone());
    let budget = p.lane_budget.load(std::sync::atomic::Ordering::Relaxed);
    drop(p);
    let first_lane = dec.is_some();
    // The span this lane last finished and the frame it read past its end
    // (the next span's first): taking the next span, it goes on from there
    // without seeking.
    let mut carry: Option<(usize, VideoFrame)> = None;
    loop {
        let (k, from, to, repair) = {
            let mut st = src.state.lock().unwrap();
            loop {
                if st.closed {
                    return;
                }
                if let Some((k, from)) = st.pick(lanes * 2) {
                    let sp = st.spans.as_mut().unwrap();
                    sp.taken.insert(k);
                    let repair = from != sp.starts[k];
                    if repair {
                        sp.redone.insert(k, from);
                    }
                    break (k, from, sp.end(k), repair);
                }
                st = src.wake.wait(st).unwrap();
            }
        };
        let untake = |st: &mut State| {
            if let Some(sp) = &mut st.spans {
                sp.taken.remove(&k);
            }
            src.wake.notify_all();
        };
        if dec.is_none() {
            match open_decoder(&pool, &*storage, &*backend, &media) {
                Ok(d) => dec = Some(d),
                // The first lane's decoder opened, so this is a limit on
                // sessions (or memory): the others do without this lane.
                Err(_) if !first_lane => return untake(&mut src.state.lock().unwrap()),
                Err(e) => {
                    let mut st = src.state.lock().unwrap();
                    st.error = Some(e);
                    return untake(&mut st);
                }
            }
        }
        let (_, d) = dec.as_mut().unwrap();
        let mut next = match carry.take() {
            Some((j, f)) if j + 1 == k && !repair => Some(f),
            _ => {
                if let Err(e) = d.seek(from) {
                    let mut st = src.state.lock().unwrap();
                    st.error = Some(e.to_string());
                    return untake(&mut st);
                }
                None
            }
        };
        let sought = next.is_none();
        let mut first = true;
        let mut pos = from;
        let ended = loop {
            // Room for one more frame, or wanted now.
            {
                let mut st = src.state.lock().unwrap();
                loop {
                    if st.closed {
                        return;
                    }
                    if st.bytes + st.frame_bytes <= budget {
                        break;
                    }
                    let keep = budget.saturating_sub(st.frame_bytes);
                    st.evict_behind(keep);
                    if st.bytes + st.frame_bytes <= budget || st.needed_soon(k, pos) {
                        break;
                    }
                    st = src.wake.wait(st).unwrap();
                }
            }
            let f = match next.take() {
                Some(f) => f,
                None => match d.next_frame() {
                    Ok(Some(f)) => f,
                    Ok(None) => break Ok(true),
                    Err(e) => break Err(e.to_string()),
                },
            };
            if f.pts >= to {
                carry = Some((k, f));
                break Ok(false);
            }
            pos = f.pts + f.duration;
            // The first frame after a seek starts after the target only at
            // the head of the stream: it is shown from the target on.
            let key = if sought && first && f.pts > from { from } else { f.pts };
            first = false;
            let f = Arc::new(f);
            let fresh;
            {
                let mut st = src.state.lock().unwrap();
                fresh = st.waiting && st.want.is_some_and(|t| key <= t && t < f.pts + f.duration);
                st.frame_bytes = frame_bytes(&f);
                st.bytes += frame_bytes(&f);
                if let Some(old) = st.frames.insert(key, f) {
                    st.bytes -= frame_bytes(&old);
                }
            }
            src.wake.notify_all();
            if fresh {
                if let Some(p) = pool.upgrade() {
                    p.wake_ui();
                }
            }
        };
        let mut st = src.state.lock().unwrap();
        match ended {
            Ok(at_end) => {
                if at_end {
                    st.end = Some(pos);
                }
                if let Some(sp) = &mut st.spans {
                    sp.done.insert(k);
                }
            }
            Err(e) => st.error = Some(e),
        }
        untake(&mut st);
        drop(st);
        if let Some(p) = pool.upgrade() {
            p.wake_ui();
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
        /// Ticks each frame's stored length falls short of the next frame.
        trim: i64,
        /// A key frame every this many frames (seek points), if any: a seek
        /// decodes from the one before its target.
        keys: Option<i64>,
        /// Time each frame takes, and how many are being decoded at once
        /// (now, most).
        cost: Duration,
        busy: Arc<(AtomicUsize, AtomicUsize)>,
    }

    struct FakeDec {
        keys: Option<i64>,
        cost: Duration,
        busy: Arc<(AtomicUsize, AtomicUsize)>,
        alive: Arc<AtomicUsize>,
        first: i64,
        next: i64,
        end: i64,
        decoded: Arc<AtomicUsize>,
        trim: i64,
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
            // From the key frame before: the frames on the way are decoded too.
            if let Some(k) = self.keys {
                self.decoded.fetch_add(((self.next - self.first) % k) as usize, Ordering::SeqCst);
            }
            Ok(())
        }
        fn seek_points(&mut self) -> Option<Vec<Time>> {
            let k = self.keys?;
            Some((0..).map(|i| RATE.frame_to_time(i * k)).take_while(|t| t.to_frame(RATE) < self.end).collect())
        }
        fn next_frame(&mut self) -> Result<Option<VideoFrame>, MediaError> {
            if self.next >= self.end {
                return Ok(None);
            }
            let (now, most) = &*self.busy;
            most.fetch_max(now.fetch_add(1, Ordering::SeqCst) + 1, Ordering::SeqCst);
            std::thread::sleep(self.cost);
            now.fetch_sub(1, Ordering::SeqCst);
            self.decoded.fetch_add(1, Ordering::SeqCst);
            let pts = RATE.frame_to_time(self.next);
            self.next += 1;
            Ok(Some(VideoFrame {
                pts,
                duration: RATE.frame_duration() - Time::from_ticks(self.trim),
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
            Ok(Box::new(FakeDec {
                alive: self.alive.clone(),
                first: self.first,
                next: self.first,
                end: self.first + self.count,
                decoded: self.decoded.clone(),
                trim: self.trim,
                keys: self.keys,
                cost: self.cost,
                busy: self.busy.clone(),
            }))
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
        pool_trimmed(first, count, budget_frames, 0)
    }

    fn pool_trimmed(first: i64, count: i64, budget_frames: usize, trim: i64) -> (Arc<VideoPool>, Arc<AtomicUsize>) {
        let decoded = Arc::new(AtomicUsize::new(0));
        let media = Arc::new(Fake { first, count, decoded: decoded.clone(), alive: Arc::new(AtomicUsize::new(0)), trim, keys: None, cost: Duration::ZERO, busy: Default::default() });
        (VideoPool::new(Arc::new(AnyFile), media, budget_frames * FRAME), decoded)
    }

    const WAIT: Duration = Duration::from_secs(5);

    #[test]
    fn a_small_budget_does_not_decode_the_whole_file() {
        // Room for 10 frames; the look-ahead wants 25 (1 s at 25 fps).
        let (pool, decoded) = pool(0, 10_000, 10);
        let m = MediaRef("x".into());
        pool.frame_blocking(&m, Time::ZERO, WAIT).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        let n = decoded.load(Ordering::SeqCst);
        assert!(n <= 12, "decoded {n} frames into a 10-frame budget");
        // Playing on still finds every frame.
        for i in 1..60 {
            let f = pool.frame_blocking(&m, RATE.frame_to_time(i), WAIT).unwrap();
            assert_eq!(f.pts, RATE.frame_to_time(i));
        }
        // 60 shown, the 10-frame look-ahead, and a few more the decoder may
        // start while the test thread races it (the old bound of 75 failed
        // ~1 run in 8 on a busy machine). Decoding the file would be 10 000.
        let n = decoded.load(Ordering::SeqCst);
        assert!(n <= 85, "decoded {n} frames to play 60");
    }

    #[test]
    fn a_large_budget_decodes_a_second_ahead() {
        let (pool, decoded) = pool(0, 10_000, 1000);
        let m = MediaRef("x".into());
        pool.frame_blocking(&m, Time::ZERO, WAIT).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        // Count before asking for frame 24: asking moves the look-ahead
        // window, and a fast machine starts on the next second at once.
        let n = decoded.load(Ordering::SeqCst);
        assert!(n <= 30, "decoded {n} frames for a one-second look-ahead");
        assert!(matches!(pool.frame(&m, RATE.frame_to_time(24)), Lookup::Exact(_)), "frame 24 is within the second ahead");
    }

    /// A memory warning while paused: the frame on show survives, and a
    /// paused monitor asking the same time again is served — the purge
    /// left the decoders awake, not waiting for a new time.
    #[test]
    fn a_memory_purge_keeps_a_paused_monitor_showing() {
        let (pool, _) = pool(0, 10_000, 100);
        let m = MediaRef("x".into());
        let t = RATE.frame_to_time(10);
        pool.frame_blocking(&m, t, WAIT).unwrap();
        pool.purge();
        assert!(matches!(pool.frame(&m, t), Lookup::Exact(_)), "the frame on show is kept");
        // The look-ahead after it is gone, and comes back without the wanted
        // time changing (as when paused).
        let ahead = RATE.frame_to_time(12);
        let start = std::time::Instant::now();
        while !matches!(pool.frame(&m, t), Lookup::Exact(_)) || !pool.sources.lock().unwrap().values().any(|s| s.state.lock().unwrap().covering(ahead).is_some()) {
            assert!(start.elapsed() < WAIT, "nothing decoded again after the purge");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// A file whose frames are stored a little shorter than the distance
    /// between them (lengths rounded to a 1/90000 s time base, as in real
    /// movies): a time in the sliver between two frames is the earlier
    /// frame's. It used to be "not decoded yet", and the decoder ran on
    /// through the rest of the file looking for it, stalling playback.
    #[test]
    fn a_sliver_between_frames_is_the_earlier_frame() {
        let (pool, decoded) = pool_trimmed(0, 10_000, 100, 1000);
        let m = MediaRef("x".into());
        pool.frame_blocking(&m, Time::ZERO, WAIT).unwrap();
        let gap = RATE.frame_to_time(5) - Time::from_ticks(500);
        let f = pool.frame_blocking(&m, gap, WAIT).unwrap();
        assert_eq!(f.pts, RATE.frame_to_time(4));
        std::thread::sleep(Duration::from_millis(300));
        let n = decoded.load(Ordering::SeqCst);
        assert!(n <= 60, "decoded {n} frames: it ran on past the look-ahead");
    }

    #[test]
    fn before_the_first_frame_shows_the_first_frame() {
        let (pool, _) = pool(3, 50, 100);
        let f = pool.frame_blocking(&MediaRef("x".into()), Time::ZERO, WAIT).unwrap();
        assert_eq!(f.pts, RATE.frame_to_time(3));
    }

    #[test]
    fn past_the_end_holds_the_last_frame_without_waiting() {
        let (pool, _) = pool(0, 50, 100);
        let start = std::time::Instant::now();
        let f = pool.frame_blocking(&MediaRef("x".into()), RATE.frame_to_time(60), WAIT).unwrap();
        assert_eq!(f.pts, RATE.frame_to_time(49));
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn the_next_clip_is_ready_before_the_cut() {
        // Playing at 2 s; a clip of the same source starting at 30 s is next.
        let (pool, _) = pool(0, 10_000, 200);
        let m = MediaRef("x".into());
        let now = Time::from_seconds(2);
        pool.frame_blocking(&m, now, WAIT).unwrap();
        pool.prefetch(&m, Time::from_seconds(30));
        std::thread::sleep(Duration::from_millis(300));
        // The playing clip kept its own look-ahead…
        assert!(matches!(pool.frame(&m, now + RATE.frame_to_time(10)), Lookup::Exact(_)));
        // …and at the cut the incoming frame is already there, without
        // waiting. (Asked last: asking moves the wanted time to the cut.)
        assert!(matches!(pool.frame(&m, Time::from_seconds(30)), Lookup::Exact(_)), "next clip's head was decoded");
    }

    #[test]
    fn idle_sources_close_their_decoders() {
        let alive = Arc::new(AtomicUsize::new(0));
        let media = Arc::new(Fake { first: 0, count: 10_000, decoded: Arc::new(AtomicUsize::new(0)), alive: alive.clone(), trim: 0, keys: None, cost: Duration::ZERO, busy: Default::default() });
        let pool = VideoPool::new(Arc::new(AnyFile), media, 1000 * FRAME);
        let m = MediaRef("x".into());
        pool.frame_blocking(&m, Time::ZERO, WAIT).unwrap();
        assert_eq!(alive.load(Ordering::SeqCst), 1);
        std::thread::sleep(IDLE * 3);
        assert_eq!(alive.load(Ordering::SeqCst), 0, "closed when idle");
        // Cached frames are still there; new ones reopen it.
        assert!(matches!(pool.frame(&m, RATE.frame_to_time(3)), Lookup::Exact(_)));
        let f = pool.frame_blocking(&m, Time::from_seconds(100), WAIT).unwrap();
        assert_eq!(f.pts, Time::from_seconds(100));
        assert_eq!(alive.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_dropped_pool_stops_its_workers() {
        let (pool, decoded) = pool(0, 10_000, 1000);
        let m = MediaRef("x".into());
        pool.frame_blocking(&m, Time::ZERO, WAIT).unwrap();
        let src = Arc::downgrade(pool.sources.lock().unwrap().get(&m).unwrap());
        drop(pool);
        std::thread::sleep(Duration::from_millis(200));
        assert!(src.upgrade().is_none(), "worker still holds its source");
        let _ = decoded;
    }

    /// Export's lane mode: a 40 s source with a key frame each second,
    /// read through frame by frame. Every frame comes, in order; three
    /// sessions decode at once; seeks land on key frames (little decoded
    /// twice); the frames held stay near the budget.
    #[test]
    fn lanes_decode_spans_side_by_side() {
        let (decoded, busy) = (Arc::new(AtomicUsize::new(0)), Arc::new((AtomicUsize::new(0), AtomicUsize::new(0))));
        let media = Arc::new(Fake { first: 0, count: 1000, decoded: decoded.clone(), alive: Default::default(), trim: 0, keys: Some(25), cost: Duration::from_micros(300), busy: busy.clone() });
        let pool = VideoPool::new(Arc::new(AnyFile), media, 1000 * FRAME);
        let budget = 100;
        pool.set_lanes(3, budget * FRAME);
        let m = MediaRef("x".into());
        let mut most_held = 0;
        for i in 0..1000 {
            let f = pool.frame_blocking(&m, RATE.frame_to_time(i), WAIT).unwrap();
            assert_eq!(f.pts, RATE.frame_to_time(i));
            let src = pool.sources.lock().unwrap().get(&m).unwrap().clone();
            most_held = most_held.max(src.state.lock().unwrap().bytes / FRAME);
        }
        let most = busy.1.load(Ordering::SeqCst);
        let n = decoded.load(Ordering::SeqCst);
        eprintln!("{most} sessions at once, {n} frames decoded, {most_held} held at most");
        assert!(most >= 2, "only {most} session decoded at a time");
        assert!(n <= 1000 + 3 * 40, "decoded {n} frames to read 1000");
        // The budget, plus what the span being read may take beyond it
        // (wanted now: a second's look-ahead), plus a frame per lane.
        assert!(most_held <= budget + 25 + 3, "held {most_held} frames");
    }

    /// Without seek points (no index), lane mode is the one decoder.
    #[test]
    fn lanes_need_seek_points() {
        let busy = Arc::new((AtomicUsize::new(0), AtomicUsize::new(0)));
        let alive = Arc::new(AtomicUsize::new(0));
        let media = Arc::new(Fake { first: 0, count: 200, decoded: Default::default(), alive: alive.clone(), trim: 0, keys: None, cost: Duration::from_micros(200), busy: busy.clone() });
        let pool = VideoPool::new(Arc::new(AnyFile), media, 1000 * FRAME);
        pool.set_lanes(3, 100 * FRAME);
        let m = MediaRef("x".into());
        for i in 0..200 {
            assert_eq!(pool.frame_blocking(&m, RATE.frame_to_time(i), WAIT).unwrap().pts, RATE.frame_to_time(i));
        }
        assert_eq!(busy.1.load(Ordering::SeqCst), 1);
        assert_eq!(alive.load(Ordering::SeqCst), 1);
    }

    /// Read backwards (a reversed clip), lane mode still finds every frame,
    /// each span decoded about once.
    #[test]
    fn lanes_read_backwards() {
        let decoded = Arc::new(AtomicUsize::new(0));
        let media = Arc::new(Fake { first: 0, count: 300, decoded: decoded.clone(), alive: Default::default(), trim: 0, keys: Some(25), cost: Duration::ZERO, busy: Default::default() });
        let pool = VideoPool::new(Arc::new(AnyFile), media, 1000 * FRAME);
        pool.set_lanes(3, 100 * FRAME);
        let m = MediaRef("x".into());
        for i in (0..300).rev() {
            assert_eq!(pool.frame_blocking(&m, RATE.frame_to_time(i), WAIT).unwrap().pts, RATE.frame_to_time(i));
        }
        let n = decoded.load(Ordering::SeqCst);
        eprintln!("{n} frames decoded to read 300 backwards");
        assert!(n <= 330, "decoded {n} frames to read 300 backwards");
    }
}
