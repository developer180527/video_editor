//! The audio mixer: a sequence's audio at any time, as interleaved stereo.
//!
//! One decoder per audible clip, kept open and positioned; a request that
//! jumps (a seek, a loop) re-seeks it. Volume (`ve.volume` level, in dB) and
//! Panner (`ve.panner` balance) are evaluated per block, so their keyframes
//! work. Muted tracks are silent; when any track is soloed, only soloed
//! tracks play.

use std::collections::{HashMap, VecDeque};
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
        let mut live = Vec::new();
        for track in seq.tracks.iter().filter(|t| t.kind == TrackKind::Audio) {
            if track.muted || (any_solo && !track.solo) {
                continue;
            }
            for clip in track.clips.iter().filter(|c| c.enabled && c.timeline_range().overlaps(range)) {
                live.push(clip.id);
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

        // A stream positioned at src_t.
        let reuse = self.streams.get(&clip.id).is_some_and(|s| {
            let drift = (s.at - src_t).ticks().abs();
            drift < ve_time::TICKS_PER_SECOND / 100 // within 10 ms: continue, don't seek
        });
        if !reuse {
            match self.open(asset, src_t) {
                Ok(s) => {
                    self.streams.insert(clip.id, s);
                }
                Err(e) => {
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
        let clip_t = src_t - clip.source_range.start;
        let gain = db_to_gain(param(clip, "ve.volume", "level", clip_t).unwrap_or(0.0));
        let balance = (param(clip, "ve.panner", "balance", clip_t).unwrap_or(0.0) / 100.0).clamp(-1.0, 1.0) as f32;
        // Constant-power pan.
        let angle = (balance + 1.0) * std::f32::consts::FRAC_PI_4;
        let (gl, gr) = (gain * angle.cos() * std::f32::consts::SQRT_2, gain * angle.sin() * std::f32::consts::SQRT_2);
        let take = n.min(s.buf.len() / 2);
        for i in 0..take {
            let (l, r) = (s.buf.pop_front().unwrap(), s.buf.pop_front().unwrap());
            out[(first + i) * 2] += l * gl;
            out[(first + i) * 2 + 1] += r * gr;
        }
        s.at = src_t + Time((n as i128 * ve_time::TICKS_PER_SECOND as i128 / rate as i128) as i64);
    }

    fn open(&self, asset: &Asset, at: Time) -> Result<Stream, String> {
        let resolved = self.storage.resolve(&asset.media).map_err(|e| e.to_string())?;
        let mut dec = self.media.open_audio(&resolved, self.rate, 2).map_err(|e| e.to_string())?;
        if at > Time::ZERO {
            dec.seek(at).map_err(|e| e.to_string())?;
        }
        Ok(Stream { dec, buf: VecDeque::new(), at, ended: false })
    }
}
