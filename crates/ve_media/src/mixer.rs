//! The audio mixer: a sequence's audio at any time, as interleaved stereo.
//!
//! Each audible clip reads from a [`Source`] — a decoder behind a buffered
//! window it can read at any sample position, or for a nested sequence a
//! mixer of its own. At normal speed a clip reads its media sample for
//! sample, continuing exactly where the last block ended; at other speeds,
//! backwards or through a time remap it resamples (linear interpolation:
//! varispeed, pitch follows speed).
//!
//! A clip's chosen source channels are mixed to its track's layout (mono or
//! stereo), then Volume (`ve.volume` level, dB) and Panner (`ve.panner`
//! balance) place it in the stereo master; gains ramp per sample. Inside a
//! transition the clips going out and coming in cross with equal power,
//! playing into their media handles. Muted tracks are silent; when any track
//! is soloed, only soloed tracks play.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use ve_model::*;
use ve_ports::{AudioDecoder, MediaBackend, Storage};
use ve_time::Time;

pub fn db_to_gain(db: f64) -> f32 {
    if db <= -96.0 {
        0.0
    } else {
        10f64.powf(db / 20.0) as f32
    }
}

/// Where an audio clip's samples come from.
enum Source {
    /// One audio stream of a file, decoded to `channels`.
    File { asset: AssetId, stream: usize, window: Window },
    /// A nested sequence's own mix (stereo), from a mixer of its own.
    Nested { sequence: SequenceId, mixer: Box<Mixer> },
}

impl Source {
    fn channels(&self) -> usize {
        match self {
            Source::File { window, .. } => window.channels,
            Source::Nested { .. } => 2,
        }
    }
}

/// A decoder behind a window of decoded samples, readable at any sample
/// position: reading on continues, a jump seeks.
struct Window {
    dec: Box<dyn AudioDecoder>,
    channels: usize,
    /// Interleaved frames from sample `start` on.
    buf: VecDeque<f32>,
    start: i64,
    ended: bool,
}

/// How far ahead a read may be before seeking beats decoding through.
const SEEK_AHEAD_SECONDS: i64 = 1;
/// How much already-played audio a window keeps (reverse play, small jumps).
const KEEP_BEHIND_SECONDS: i64 = 2;

impl Window {
    fn end(&self) -> i64 {
        self.start + (self.buf.len() / self.channels) as i64
    }

    /// Frames `at..at + n` into `out` (interleaved), silence where the media
    /// has none (before its start, past its end).
    fn read(&mut self, at: i64, n: usize, rate: u32, out: &mut Vec<f32>) {
        let ch = self.channels;
        let rate = rate as i64;
        let behind = at < self.start;
        let far = at > self.end() + SEEK_AHEAD_SECONDS * rate;
        if behind || far {
            // Backwards (reverse play, a jump back): land a second early, so
            // the reads that follow find their samples already decoded.
            let target = if behind { (at - rate).max(0) } else { at.max(0) };
            if self.dec.seek(Time((target as i128 * ve_time::TICKS_PER_SECOND as i128 / rate as i128) as i64)).is_ok() {
                self.buf.clear();
                self.start = target;
                self.ended = false;
            }
        }
        while self.end() < at + n as i64 && !self.ended {
            match self.dec.next_block() {
                Ok(Some(b)) => self.buf.extend(b.samples),
                Ok(None) | Err(_) => self.ended = true,
            }
        }
        let keep_from = at - KEEP_BEHIND_SECONDS * rate;
        if self.start < keep_from {
            let drop = ((keep_from - self.start) as usize * ch).min(self.buf.len());
            self.buf.drain(..drop);
            self.start += (drop / ch) as i64;
        }
        out.clear();
        out.resize(n * ch, 0.0);
        let have = (self.buf.len() / ch) as i64;
        for k in 0..n {
            let i = at + k as i64 - self.start;
            if (0..have).contains(&i) {
                for c in 0..ch {
                    out[k * ch + c] = self.buf[i as usize * ch + c];
                }
            }
        }
    }
}

/// A clip's source and where its last read ended, for continuity.
struct Stream {
    source: Source,
    next: Option<i64>,
}

pub struct Mixer {
    storage: Arc<dyn Storage>,
    media: Arc<dyn MediaBackend>,
    rate: u32,
    streams: HashMap<ClipId, Stream>,
    failed: HashMap<ClipId, String>,
    scratch: Vec<f32>,
}

fn param(clip: &Clip, effect: &str, name: &str, t: Time) -> Option<f64> {
    let e = clip.effects.iter().find(|e| e.enabled && e.plugin.id == effect)?;
    match e.params.get(name)?.value_at(t) {
        Value::Float(v) => Some(v),
        _ => None,
    }
}

/// Equal-power gain of clip `id` on `track` at `t` from transitions: 1
/// outside them, rising (incoming) or falling (outgoing) inside, 0 where
/// the clip does not play.
fn transition_gain(track: &Track, id: ClipId, t: Time) -> f32 {
    use std::f64::consts::FRAC_PI_2;
    track.active_at(t).iter().find(|a| a.clip.id == id).map_or(0.0, |a| match a.transition {
        None => 1.0,
        Some((_, p, Role::Incoming)) => (p * FRAC_PI_2).sin() as f32,
        Some((_, p, Role::Outgoing)) => (p * FRAC_PI_2).cos() as f32,
    })
}

impl Mixer {
    pub fn new(storage: Arc<dyn Storage>, media: Arc<dyn MediaBackend>, rate: u32) -> Self {
        Mixer { storage, media, rate, streams: HashMap::new(), failed: HashMap::new(), scratch: Vec::new() }
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Sources that could not be decoded, by clip.
    pub fn errors(&self) -> impl Iterator<Item = (&ClipId, &String)> {
        self.failed.iter()
    }

    fn ticks(&self, frames: i64) -> Time {
        Time((frames as i128 * ve_time::TICKS_PER_SECOND as i128 / self.rate as i128) as i64)
    }

    /// Mix `out.len() / 2` stereo frames of `seq` starting at sequence time `start`.
    pub fn render(&mut self, project: &Project, seq: &Sequence, start: Time, out: &mut [f32]) {
        out.fill(0.0);
        let frames = out.len() / 2;
        let range = ve_time::TimeRange::new(start, self.ticks(frames as i64));
        let any_solo = seq.tracks.iter().any(|t| t.kind == TrackKind::Audio && t.solo);
        let mut live = HashSet::new();
        for track in seq.tracks.iter().filter(|t| t.kind == TrackKind::Audio) {
            if track.muted || (any_solo && !track.solo) {
                continue;
            }
            for (i, clip) in track.clips.iter().enumerate() {
                if clip.enabled && track.reach(i).overlaps(range) {
                    live.insert(clip.id);
                    self.mix_clip(project, track, i, start, frames, out);
                }
            }
        }
        // Close decoders of clips no longer playing.
        self.streams.retain(|id, _| live.contains(id));
    }

    fn mix_clip(&mut self, project: &Project, track: &Track, index: usize, start: Time, frames: usize, out: &mut [f32]) {
        let clip = &track.clips[index];
        if self.failed.contains_key(&clip.id) {
            return;
        }
        let rate = self.rate;
        // Output frame k is at `start + k / rate`; the clip plays the ones
        // with reach.start <= time < reach.end (so both bounds round up).
        let ceil_frames = |t: Time| {
            let x = t.ticks() as i128 * rate as i128;
            let d = ve_time::TICKS_PER_SECOND as i128;
            (x.div_euclid(d) + (x.rem_euclid(d) != 0) as i128) as i64
        };
        let reach = track.reach(index);
        let first = ceil_frames(reach.start - start).clamp(0, frames as i64) as usize;
        let last = ceil_frames(reach.end() - start).clamp(0, frames as i64) as usize;
        if first >= last {
            return;
        }
        let n = last - first;
        let time_of = |k: usize| start + Time(((first + k) as i128 * ve_time::TICKS_PER_SECOND as i128 / rate as i128) as i64);
        let clip_time = |k: usize| time_of(k) - clip.timeline_start;

        // The source, (re)opened when the clip's source or channels change.
        let channels = self.decode_channels(project, track, clip);
        let current = self.streams.get(&clip.id).is_some_and(|s| match (&s.source, &clip.source) {
            (Source::File { asset, stream, window }, ClipSource::Asset { asset: a, audio_stream }) => {
                asset == a && *stream == *audio_stream as usize && window.channels == channels
            }
            (Source::Nested { sequence, .. }, ClipSource::Sequence { sequence: q, .. }) => sequence == q,
            _ => false,
        });
        if !current {
            match self.open(project, clip, channels) {
                Ok(source) => {
                    self.streams.insert(clip.id, Stream { source, next: None });
                }
                Err(e) => {
                    self.streams.remove(&clip.id);
                    self.failed.insert(clip.id, e);
                    return;
                }
            }
        }

        // The source frames for output frames first..last.
        let pos = |k: usize| clip.media_time(clip_time(k)).ticks() as f64 * rate as f64 / ve_time::TICKS_PER_SECOND as f64;
        let stream = self.streams.get_mut(&clip.id).unwrap();
        let ch = stream.source.channels();
        let mut frames_in = std::mem::take(&mut self.scratch);
        if clip.retime.is_normal() {
            // Sample for sample, continuing exactly where the last block
            // ended (rounding never adds or drops a sample).
            let mut at = pos(0).round() as i64;
            if stream.next.is_some_and(|e| (e - at).abs() <= 1) {
                at = stream.next.unwrap();
            }
            read(&mut stream.source, project, at, n, rate, &mut frames_in);
            stream.next = Some(at + n as i64);
        } else {
            // Resampled: read the span the positions cover, interpolate.
            let p: Vec<f64> = (0..n).map(pos).collect();
            let lo = p.iter().cloned().fold(f64::INFINITY, f64::min).floor() as i64;
            let hi = p.iter().cloned().fold(f64::NEG_INFINITY, f64::max).ceil() as i64 + 2;
            let mut span = Vec::new();
            read(&mut stream.source, project, lo, (hi - lo) as usize, rate, &mut span);
            frames_in.clear();
            frames_in.reserve(n * ch);
            for x in &p {
                let at = x - lo as f64;
                let (i, f) = (at.floor() as usize, (at - at.floor()) as f32);
                for c in 0..ch {
                    let a = span[i * ch + c];
                    let b = span[(i + 1) * ch + c];
                    frames_in.push(a + (b - a) * f);
                }
            }
            stream.next = None;
        }

        // Gains at both ends of the span (volume, pan, transitions), ramped
        // per sample: stepped gains buzz audibly on fades.
        let mono = track.layout == ChannelLayout::Mono;
        let gains = |k: usize| {
            let t = clip_time(k);
            let gain = db_to_gain(param(clip, "ve.volume", "level", t).unwrap_or(0.0)) * transition_gain(track, clip.id, time_of(k));
            let balance = (param(clip, "ve.panner", "balance", t).unwrap_or(0.0) / 100.0).clamp(-1.0, 1.0) as f32;
            // Constant-power pan: a mono track sits at -3 dB each side when
            // centred; a stereo track's balance keeps unity at centre.
            let angle = (balance + 1.0) * std::f32::consts::FRAC_PI_4;
            let law = if mono { 1.0 } else { std::f32::consts::SQRT_2 };
            (gain * angle.cos() * law, gain * angle.sin() * law)
        };
        let ((l0, r0), (l1, r1)) = (gains(0), gains(n - 1));
        let picked: Vec<usize> = if clip.channels.is_empty() { (0..ch).collect() } else { clip.channels.iter().map(|c| *c as usize).filter(|c| *c < ch).collect() };
        if picked.is_empty() {
            self.scratch = frames_in;
            return;
        }
        for k in 0..n {
            let x = k as f32 / n as f32;
            let (gl, gr) = (l0 + (l1 - l0) * x, r0 + (r1 - r0) * x);
            let frame = &frames_in[k * ch..k * ch + ch];
            // The clip's channels, as the track's layout.
            let (l, r) = if mono {
                let m = picked.iter().map(|&c| frame[c]).sum::<f32>() / picked.len() as f32;
                (m, m)
            } else if picked.len() == 1 {
                (frame[picked[0]], frame[picked[0]])
            } else {
                (frame[picked[0]], frame[picked[1]])
            };
            out[(first + k) * 2] += l * gl;
            out[(first + k) * 2 + 1] += r * gr;
        }
        self.scratch = frames_in;
    }

    /// Channels to decode a clip's stream to: its own count when the clip
    /// picks channels from it, else the track's (downmixed by layout).
    fn decode_channels(&self, project: &Project, track: &Track, clip: &Clip) -> usize {
        match &clip.source {
            ClipSource::Asset { asset, audio_stream } if !clip.channels.is_empty() => project
                .assets
                .get(asset)
                .and_then(|a| a.info.as_ref())
                .and_then(|i| i.audio.get(*audio_stream as usize))
                .map_or(2, |s| s.channels.max(1) as usize),
            _ => track.layout.channels() as usize,
        }
    }

    fn open(&self, project: &Project, clip: &Clip, channels: usize) -> Result<Source, String> {
        match &clip.source {
            ClipSource::Asset { asset, audio_stream } => {
                let a = project.assets.get(asset).ok_or("missing asset")?;
                let resolved = self.storage.resolve(&a.media).map_err(|e| e.to_string())?;
                let dec = self.media.open_audio(&resolved, *audio_stream as usize, self.rate, channels as u16).map_err(|e| e.to_string())?;
                // Positioned by the first read (a seek, even to zero, lines the
                // first sample up with its time by timestamp).
                Ok(Source::File {
                    asset: *asset,
                    stream: *audio_stream as usize,
                    window: Window { dec, channels, buf: VecDeque::new(), start: i64::MAX / 2, ended: false },
                })
            }
            ClipSource::Sequence { sequence, .. } => {
                Ok(Source::Nested { sequence: *sequence, mixer: Box::new(Mixer::new(self.storage.clone(), self.media.clone(), self.rate)) })
            }
            ClipSource::Generator { .. } => Err("generators make no sound".into()),
        }
    }
}

/// Frames `at..at + n` of `source` into `out`, interleaved.
fn read(source: &mut Source, project: &Project, at: i64, n: usize, rate: u32, out: &mut Vec<f32>) {
    match source {
        Source::File { window, .. } => window.read(at, n, rate, out),
        Source::Nested { sequence, mixer } => {
            out.clear();
            out.resize(n * 2, 0.0);
            if let Some(seq) = project.sequences.get(sequence).cloned() {
                let t = Time((at as i128 * ve_time::TICKS_PER_SECOND as i128 / rate as i128) as i64);
                mixer.render(project, &seq, t, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use ve_ports::*;
    use ve_time::TimeRange;

    /// Endless audio of `1.0`s, counting opens and seeks.
    #[derive(Default)]
    struct Counting {
        opens: Arc<AtomicUsize>,
        seeks: Arc<AtomicUsize>,
    }

    struct Dec {
        seeks: Arc<AtomicUsize>,
    }

    impl AudioDecoder for Dec {
        fn seek(&mut self, _: Time) -> Result<(), MediaError> {
            self.seeks.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn next_block(&mut self) -> Result<Option<AudioBlock>, MediaError> {
            Ok(Some(AudioBlock { pts: Time::ZERO, sample_rate: 48_000, channels: 2, samples: vec![1.0; 2048] }))
        }
    }

    impl MediaBackend for Counting {
        fn name(&self) -> &str {
            "counting"
        }
        fn probe(&self, _: &Resolved) -> Result<MediaInfo, MediaError> {
            unimplemented!()
        }
        fn open_video(&self, _: &Resolved) -> Result<Box<dyn VideoDecoder>, MediaError> {
            unimplemented!()
        }
        fn open_audio(&self, _: &Resolved, _: usize, _: u32, _: u16) -> Result<Box<dyn AudioDecoder>, MediaError> {
            self.opens.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(Dec { seeks: self.seeks.clone() }))
        }
        fn open_encoder(&self, _: &Resolved, _: &EncoderSettings) -> Result<Box<dyn Encoder>, MediaError> {
            unimplemented!()
        }
    }

    /// A one-clip sequence of `Counting`'s constant 1.0 audio, with `volume`
    /// as the clip's level parameter.
    fn one_clip(volume: Param) -> (Project, Sequence) {
        let mut p = Project::new("t");
        let asset = Asset { id: AssetId::new(), name: "a".into(), media: MediaRef("a".into()), info: None, variants: Vec::new(), marks: Default::default() };
        let mut params = OrdMap::new();
        params.insert("level".to_string(), volume);
        let plugin = PluginRef { api: PluginApi::Builtin, id: "ve.volume".into(), major_version: 1 };
        let mut track = Track::new(TrackKind::Audio, "A1");
        track.clips.push_back(Arc::new(Clip {
            id: ClipId::new(),
            name: "c".into(),
            source: ClipSource::Asset { asset: asset.id, audio_stream: 0 },
            source_range: TimeRange::new(Time::ZERO, Time::from_seconds(60)),
            timeline_start: Time::ZERO,
            enabled: true,
            link: None,
            effects: [Arc::new(Effect { id: EffectId::new(), plugin, enabled: true, params })].into_iter().collect(),
            retime: Default::default(),
            transition_in: None,
            transition_out: None,
            channels: Vec::new(),
        }));
        p.assets.insert(asset.id, Arc::new(asset));
        let seq = Sequence { id: SequenceId::new(), name: "s".into(), format: SequenceFormat::default(), tracks: [Arc::new(track)].into_iter().collect(), marks: Default::default() };
        (p, seq)
    }

    #[test]
    fn keyframed_volume_ramps_smoothly_within_and_across_blocks() {
        // A fade from silence to full over 0.1 s (about five blocks).
        let key = |t: Time, db: f64| Keyframe { time: t, value: Value::Float(db), interp: Interp::Linear };
        let (p, seq) = one_clip(Param::Animated(vec![key(Time::ZERO, -60.0), key(Time::from_ticks(ve_time::TICKS_PER_SECOND / 10), 0.0)]));
        let mut mixer = Mixer::new(Arc::new(crate::fakes::AnyFile), Arc::new(Counting::default()), 48_000);
        let mut heard = Vec::new();
        let mut out = vec![0f32; 1024 * 2];
        for b in 0..6 {
            mixer.render(&p, &seq, Time::from_ticks(b * 1024 * ve_time::TICKS_PER_SECOND / 48_000), &mut out);
            heard.extend(out.iter().step_by(2).copied());
        }
        // The level rises sample by sample: no step bigger than the steepest
        // part of the curve needs (a stepped gain jumps ~7% per block here).
        let worst = heard.windows(2).map(|w| w[1] - w[0]).fold(0f32, f32::max);
        assert!(worst < 0.002, "largest step between samples: {worst}");
        assert!(heard.windows(2).all(|w| w[1] >= w[0] - 1e-6), "fade never dips");
        assert!((heard.last().unwrap() - 1.0).abs() < 1e-3, "reaches full level");
    }

    #[test]
    fn restarts_seek_open_decoders_instead_of_reopening() {
        let media = Arc::new(Counting::default());
        let (opens, seeks) = (media.opens.clone(), media.seeks.clone());
        let mut mixer = Mixer::new(Arc::new(crate::fakes::AnyFile), media, 48_000);
        let mut p = Project::new("t");
        let asset = Asset { id: AssetId::new(), name: "a".into(), media: MediaRef("a".into()), info: None, variants: Vec::new(), marks: Default::default() };
        let mut track = Track::new(TrackKind::Audio, "A1");
        track.clips.push_back(Arc::new(Clip {
            id: ClipId::new(),
            name: "c".into(),
            source: ClipSource::Asset { asset: asset.id, audio_stream: 0 },
            source_range: TimeRange::new(Time::ZERO, Time::from_seconds(60)),
            timeline_start: Time::ZERO,
            enabled: true,
            link: None,
            effects: Default::default(),
            retime: Default::default(),
            transition_in: None,
            transition_out: None,
            channels: Vec::new(),
        }));
        p.assets.insert(asset.id, Arc::new(asset));
        let seq = Sequence { id: SequenceId::new(), name: "s".into(), format: SequenceFormat::default(), tracks: [Arc::new(track)].into_iter().collect(), marks: Default::default() };
        let mut out = vec![0f32; 1024 * 2];
        let block = Time::from_ticks(1024 * ve_time::TICKS_PER_SECOND / 48_000);
        // Play on: one open, one seek (to the start), continuous after that.
        for i in 0..10 {
            mixer.render(&p, &seq, Time::from_ticks(block.ticks() * i), &mut out);
        }
        assert_eq!((opens.load(Ordering::SeqCst), seeks.load(Ordering::SeqCst)), (1, 1));
        assert!(out.iter().all(|s| (*s - 1.0).abs() < 1e-5), "sound throughout");
        // A restart elsewhere (a seek, an edit while playing): a seek, no open.
        mixer.render(&p, &seq, Time::from_seconds(30), &mut out);
        mixer.render(&p, &seq, Time::from_seconds(2), &mut out);
        assert_eq!((opens.load(Ordering::SeqCst), seeks.load(Ordering::SeqCst)), (1, 3));
    }

    /// A seekable decoder whose frames say where they are: channel c of
    /// sample i is `i + c / 4` ("ramp"), or a constant ("const:<v>").
    struct Signal {
        value: Option<f32>,
        channels: usize,
        pos: i64,
    }

    impl AudioDecoder for Signal {
        fn seek(&mut self, t: Time) -> Result<(), MediaError> {
            self.pos = (t.ticks() as i128 * 48_000 / ve_time::TICKS_PER_SECOND as i128) as i64;
            Ok(())
        }
        fn next_block(&mut self) -> Result<Option<AudioBlock>, MediaError> {
            let samples = (0..1024i64)
                .flat_map(|k| (0..self.channels).map(move |c| (k, c)))
                .map(|(k, c)| self.value.unwrap_or((self.pos + k) as f32 + c as f32 / 4.0))
                .collect();
            self.pos += 1024;
            Ok(Some(AudioBlock { pts: Time::ZERO, sample_rate: 48_000, channels: self.channels as u16, samples }))
        }
    }

    /// Storage that resolves a ref to a "path" naming the signal.
    struct Named;
    impl Storage for Named {
        fn make_ref(&self, p: &str) -> Result<MediaRef, StorageError> {
            Ok(MediaRef(p.into()))
        }
        fn resolve(&self, r: &MediaRef) -> Result<Resolved, StorageError> {
            Ok(Resolved { path: Some(r.0.clone().into()), guard: Box::new(()) })
        }
        fn resolve_new(&self, r: &MediaRef) -> Result<Resolved, StorageError> {
            self.resolve(r)
        }
        fn open_read(&self, r: &MediaRef) -> Result<Box<dyn ReadSeek>, StorageError> {
            Err(StorageError::NotFound(r.0.clone()))
        }
        fn open_write(&self, r: &MediaRef) -> Result<Box<dyn std::io::Write + Send>, StorageError> {
            Err(StorageError::NotFound(r.0.clone()))
        }
        fn location(&self, _: Location, n: &str) -> Result<MediaRef, StorageError> {
            Ok(MediaRef(n.into()))
        }
        fn display_name(&self, r: &MediaRef) -> String {
            r.0.clone()
        }
    }

    struct SignalMedia;
    impl MediaBackend for SignalMedia {
        fn name(&self) -> &str {
            "signals"
        }
        fn probe(&self, _: &Resolved) -> Result<MediaInfo, MediaError> {
            unimplemented!()
        }
        fn open_video(&self, _: &Resolved) -> Result<Box<dyn VideoDecoder>, MediaError> {
            unimplemented!()
        }
        fn open_audio(&self, r: &Resolved, _: usize, _: u32, channels: u16) -> Result<Box<dyn AudioDecoder>, MediaError> {
            let name = r.path.as_ref().unwrap().to_string_lossy().into_owned();
            let value = name.strip_prefix("const:").map(|v| v.parse().unwrap());
            Ok(Box::new(Signal { value, channels: channels as usize, pos: 0 }))
        }
        fn open_encoder(&self, _: &Resolved, _: &EncoderSettings) -> Result<Box<dyn Encoder>, MediaError> {
            unimplemented!()
        }
    }

    fn signal_asset(p: &mut Project, name: &str, channels: u16) -> AssetId {
        let stream = AudioStreamInfo { sample_rate: 48_000, channels, codec: "pcm".into(), layout: String::new() };
        let a = Asset::new(name, MediaRef(name.into()), Some(MediaInfo { duration: Time::from_seconds(100), video: None, audio: vec![stream] }));
        let id = a.id;
        p.assets.insert(id, Arc::new(a));
        id
    }

    fn audio_clip(asset: AssetId, start_s: i64, src_s: i64, len_s: i64) -> Clip {
        Clip {
            id: ClipId::new(),
            name: "c".into(),
            source: ClipSource::Asset { asset, audio_stream: 0 },
            source_range: TimeRange::new(Time::from_seconds(src_s), Time::from_seconds(len_s)),
            timeline_start: Time::from_seconds(start_s),
            enabled: true,
            link: None,
            effects: Default::default(),
            retime: Default::default(),
            transition_in: None,
            transition_out: None,
            channels: Vec::new(),
        }
    }

    /// Render `n` frames of a one-track sequence at `at_s` seconds.
    fn hear(p: &Project, track: Track, at: Time, n: usize) -> Vec<[f32; 2]> {
        let seq = Sequence::new("s", SequenceFormat::default(), [track]);
        let mut mixer = Mixer::new(Arc::new(Named), Arc::new(SignalMedia), 48_000);
        let mut out = vec![0f32; n * 2];
        mixer.render(p, &seq, at, &mut out);
        out.chunks(2).map(|f| [f[0], f[1]]).collect()
    }

    fn track_with(clips: Vec<Clip>) -> Track {
        let mut t = Track::new(TrackKind::Audio, "A1");
        t.clips = clips.into_iter().map(Arc::new).collect();
        t
    }

    #[test]
    fn speed_and_reverse_resample_the_media() {
        let mut p = Project::new("t");
        let a = signal_asset(&mut p, "ramp", 2);
        // Double speed from media 10 s: output frame k plays media sample 480000 + 2k.
        let mut c = audio_clip(a, 0, 10, 5);
        c.retime = Retime::Speed(Ratio::new(2, 1));
        let out = hear(&p, track_with(vec![c.clone()]), Time::ZERO, 256);
        for k in [0usize, 1, 100, 255] {
            assert!((out[k][0] - (480_000.0 + 2.0 * k as f32)).abs() < 0.5, "frame {k}: {}", out[k][0]);
        }
        // Backwards: the media runs down.
        c.retime = Retime::Speed(Ratio::new(-1, 1));
        let out = hear(&p, track_with(vec![c]), Time::ZERO, 256);
        assert!((out[0][0] - 480_000.0).abs() < 0.5 && (out[100][0] - 479_900.0).abs() < 0.5, "{} {}", out[0][0], out[100][0]);
    }

    #[test]
    fn a_mono_track_plays_the_picked_channel_at_minus_3_db() {
        let mut p = Project::new("t");
        let a = signal_asset(&mut p, "ramp", 2);
        let mut c = audio_clip(a, 0, 0, 5);
        c.channels = vec![1]; // the right channel of the stereo stream
        let mut t = track_with(vec![c]);
        t.layout = ChannelLayout::Mono;
        let out = hear(&p, t, Time::ZERO, 16);
        let half = std::f32::consts::FRAC_1_SQRT_2;
        assert!((out[10][0] - (10.25 * half)).abs() < 1e-3 && (out[10][1] - out[10][0]).abs() < 1e-6, "{:?}", out[10]);
    }

    #[test]
    fn cross_dissolves_mix_with_equal_power() {
        let mut p = Project::new("t");
        let (one, half) = (signal_asset(&mut p, "const:1.0", 2), signal_asset(&mut p, "const:0.5", 2));
        let a = audio_clip(one, 0, 10, 10);
        let mut b = audio_clip(half, 10, 10, 10);
        b.transition_in = Some(Arc::new(Transition {
            id: EffectId::new(),
            plugin: PluginRef { api: PluginApi::Builtin, id: "ve.crossfade".into(), major_version: 1 },
            before: Time::from_seconds(1),
            after: Time::from_seconds(1),
            params: OrdMap::new(),
        }));
        let t = track_with(vec![a, b]);
        let at = |sec: f64| hear(&p, t.clone(), Time::from_seconds_f64(sec), 1)[0][0];
        let mid = std::f32::consts::FRAC_1_SQRT_2 * 1.5;
        assert!((at(5.0) - 1.0).abs() < 1e-3, "before: the first clip alone");
        assert!((at(10.0) - mid).abs() < 2e-3, "at the cut both at -3 dB: {}", at(10.0));
        assert!((at(9.0) - 1.0).abs() < 1e-3 && (at(11.0) - 0.5).abs() < 1e-3, "{} {}", at(9.0), at(11.0));
        assert!((at(15.0) - 0.5).abs() < 1e-3);
    }

    #[test]
    fn nested_sequences_play_their_mix() {
        let mut p = Project::new("t");
        let one = signal_asset(&mut p, "const:0.25", 2);
        let inner = Sequence::new("inner", SequenceFormat::default(), [track_with(vec![audio_clip(one, 2, 0, 4)])]);
        let inner_id = inner.id;
        p.sequences.insert(inner_id, Arc::new(inner));
        let mut outer = audio_clip(one, 0, 0, 10);
        outer.source = ClipSource::Sequence { sequence: inner_id, angle: None };
        let t = track_with(vec![outer]);
        assert!(hear(&p, t.clone(), Time::from_seconds(1), 1)[0][0].abs() < 1e-6, "inner silence");
        assert!((hear(&p, t, Time::from_seconds(3), 1)[0][0] - 0.25).abs() < 1e-6, "inner clip");
    }
}
