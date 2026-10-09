//! The audio mixer: a sequence's audio at any time, as interleaved stereo.
//!
//! One decoder per audible clip, kept open and positioned; a request that
//! jumps (a seek, a loop) re-seeks it. Volume (`ve.volume` level, in dB) and
//! Panner (`ve.panner` balance) are evaluated per block, so their keyframes
//! work. Muted tracks are silent; when any track is soloed, only soloed
//! tracks play.

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

struct Stream {
    /// The asset the decoder reads (a clip's source can be replaced).
    asset: AssetId,
    dec: Box<dyn AudioDecoder>,
    /// Decoded stereo samples starting at `at` (source time).
    buf: VecDeque<f32>,
    at: Time,
    ended: bool,
}

pub struct Mixer {
    storage: Arc<dyn Storage>,
    media: Arc<dyn MediaBackend>,
    rate: u32,
    streams: HashMap<ClipId, Stream>,
    failed: HashMap<ClipId, String>,
}

fn param(clip: &Clip, effect: &str, name: &str, t: Time) -> Option<f64> {
    let e = clip.effects.iter().find(|e| e.enabled && e.plugin.id == effect)?;
    match e.params.get(name)?.value_at(t) {
        Value::Float(v) => Some(v),
        _ => None,
    }
}

impl Mixer {
    pub fn new(storage: Arc<dyn Storage>, media: Arc<dyn MediaBackend>, rate: u32) -> Self {
        Mixer { storage, media, rate, streams: HashMap::new(), failed: HashMap::new() }
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Sources that could not be decoded, by clip.
    pub fn errors(&self) -> impl Iterator<Item = (&ClipId, &String)> {
        self.failed.iter()
    }

    /// Mix `out.len() / 2` stereo frames of `seq` starting at sequence time `start`.
    pub fn render(&mut self, project: &Project, seq: &Sequence, start: Time, out: &mut [f32]) {
        out.fill(0.0);
        let frames = out.len() / 2;
        let block = Time((frames as i128 * ve_time::TICKS_PER_SECOND as i128 / self.rate as i128) as i64);
        let range = ve_time::TimeRange::new(start, block);
        let any_solo = seq.tracks.iter().any(|t| t.kind == TrackKind::Audio && t.solo);
        let mut live = HashSet::new();
        for track in seq.tracks.iter().filter(|t| t.kind == TrackKind::Audio) {
            if track.muted || (any_solo && !track.solo) {
                continue;
            }
            for clip in track.clips.iter().filter(|c| c.enabled && c.timeline_range().overlaps(range)) {
                live.insert(clip.id);
                self.mix_clip(project, clip, start, frames, out);
            }
        }
        // Close decoders of clips no longer playing.
        self.streams.retain(|id, _| live.contains(id));
    }

    fn mix_clip(&mut self, project: &Project, clip: &Clip, start: Time, frames: usize, out: &mut [f32]) {
        let ClipSource::Asset { asset } = &clip.source else { return };
        let Some(asset) = project.assets.get(asset) else { return };
        if self.failed.contains_key(&clip.id) {
            return;
        }
        let rate = self.rate;
        let to_frames = |t: Time| (t.ticks() as i128 * rate as i128 / ve_time::TICKS_PER_SECOND as i128) as i64;
        // Which output frames this clip covers, and the source time at the first.
        let clip_range = clip.timeline_range();
        let first = to_frames(clip_range.start - start).max(0) as usize;
        let last = (to_frames(clip_range.end() - start).max(0) as usize).min(frames);
        if first >= last {
            return;
        }
        let src_t = clip.source_time(start + Time((first as i128 * ve_time::TICKS_PER_SECOND as i128 / rate as i128) as i64));
        let n = last - first;

        // A stream positioned at src_t: continue the open one when it is
        // already there, seek it when it is elsewhere (a restart, a seek, a
        // loop — opening a file costs far more than seeking it), open one
        // only when there is none.
        let ready = match self.streams.get_mut(&clip.id) {
            Some(s) if s.asset != asset.id => false,
            Some(s) if (s.at - src_t).ticks().abs() < ve_time::TICKS_PER_SECOND / 100 => true, // within 10 ms
            Some(s) => {
                let ok = s.dec.seek(src_t).is_ok();
                if ok {
                    s.buf.clear();
                    s.at = src_t;
                    s.ended = false;
                }
                ok
            }
            None => false,
        };
        if !ready {
            match self.open(asset, src_t) {
                Ok(s) => {
                    self.streams.insert(clip.id, s);
                }
                Err(e) => {
                    self.streams.remove(&clip.id);
                    self.failed.insert(clip.id, e);
                    return;
                }
            }
        }
        let s = self.streams.get_mut(&clip.id).unwrap();
        while s.buf.len() < n * 2 && !s.ended {
            match s.dec.next_block() {
                Ok(Some(b)) => s.buf.extend(b.samples),
                Ok(None) | Err(_) => s.ended = true,
            }
        }
        // Volume and pan at both ends of this span, ramped per sample in
        // between: a stepped gain (one value per block) buzzes audibly on
        // fades. The next span starts where this one ends, so the ramp is
        // continuous across blocks too.
        let clip_t = src_t - clip.source_range.start;
        let span = Time((n as i128 * ve_time::TICKS_PER_SECOND as i128 / rate as i128) as i64);
        let gains = |t: Time| {
            let gain = db_to_gain(param(clip, "ve.volume", "level", t).unwrap_or(0.0));
            let balance = (param(clip, "ve.panner", "balance", t).unwrap_or(0.0) / 100.0).clamp(-1.0, 1.0) as f32;
            // Constant-power pan.
            let angle = (balance + 1.0) * std::f32::consts::FRAC_PI_4;
            (gain * angle.cos() * std::f32::consts::SQRT_2, gain * angle.sin() * std::f32::consts::SQRT_2)
        };
        let ((l0, r0), (l1, r1)) = (gains(clip_t), gains(clip_t + span));
        let take = n.min(s.buf.len() / 2);
        for i in 0..take {
            let x = i as f32 / n as f32;
            let (gl, gr) = (l0 + (l1 - l0) * x, r0 + (r1 - r0) * x);
            let (l, r) = (s.buf.pop_front().unwrap(), s.buf.pop_front().unwrap());
            out[(first + i) * 2] += l * gl;
            out[(first + i) * 2 + 1] += r * gr;
        }
        s.at = src_t + Time((n as i128 * ve_time::TICKS_PER_SECOND as i128 / rate as i128) as i64);
    }

    fn open(&self, asset: &Asset, at: Time) -> Result<Stream, String> {
        let resolved = self.storage.resolve(&asset.media).map_err(|e| e.to_string())?;
        let mut dec = self.media.open_audio(&resolved, self.rate, 2).map_err(|e| e.to_string())?;
        // Always seek, even to zero: the decoder then lines its first sample
        // up with `at` by timestamp, exactly as when starting mid-clip.
        dec.seek(at).map_err(|e| e.to_string())?;
        Ok(Stream { asset: asset.id, dec, buf: VecDeque::new(), at, ended: false })
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
        fn open_audio(&self, _: &Resolved, _: u32, _: u16) -> Result<Box<dyn AudioDecoder>, MediaError> {
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
        let asset = Asset { id: AssetId::new(), name: "a".into(), media: MediaRef("a".into()), info: None };
        let mut params = OrdMap::new();
        params.insert("level".to_string(), volume);
        let plugin = PluginRef { api: PluginApi::Builtin, id: "ve.volume".into(), major_version: 1 };
        let mut track = Track::new(TrackKind::Audio, "A1");
        track.clips.push_back(Arc::new(Clip {
            id: ClipId::new(),
            name: "c".into(),
            source: ClipSource::Asset { asset: asset.id },
            source_range: TimeRange::new(Time::ZERO, Time::from_seconds(60)),
            timeline_start: Time::ZERO,
            enabled: true,
            link: None,
            effects: [Arc::new(Effect { id: EffectId::new(), plugin, enabled: true, params })].into_iter().collect(),
        }));
        p.assets.insert(asset.id, Arc::new(asset));
        let seq = Sequence { id: SequenceId::new(), name: "s".into(), format: SequenceFormat::default(), tracks: [Arc::new(track)].into_iter().collect() };
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
        let asset = Asset { id: AssetId::new(), name: "a".into(), media: MediaRef("a".into()), info: None };
        let mut track = Track::new(TrackKind::Audio, "A1");
        track.clips.push_back(Arc::new(Clip {
            id: ClipId::new(),
            name: "c".into(),
            source: ClipSource::Asset { asset: asset.id },
            source_range: TimeRange::new(Time::ZERO, Time::from_seconds(60)),
            timeline_start: Time::ZERO,
            enabled: true,
            link: None,
            effects: Default::default(),
        }));
        p.assets.insert(asset.id, Arc::new(asset));
        let seq = Sequence { id: SequenceId::new(), name: "s".into(), format: SequenceFormat::default(), tracks: [Arc::new(track)].into_iter().collect() };
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
}
