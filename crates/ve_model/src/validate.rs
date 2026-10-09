//! The invariants every project satisfies. Commands keep them; loading a file
//! checks them; tests assert them after every edit.

use crate::{ClipId, ClipSource, Param, Project, TrackId, TrackKind};
use thiserror::Error;

#[derive(Debug, Error, PartialEq)]
pub enum ModelError {
    #[error("clip {0} has a non-positive duration")]
    EmptyClip(ClipId),
    #[error("clips {0} and {1} overlap on track {2}")]
    Overlap(ClipId, ClipId, TrackId),
    #[error("clips on track {0} are out of order")]
    Unsorted(TrackId),
    #[error("clip {0} refers to a missing asset")]
    MissingAsset(ClipId),
    #[error("clip {0} starts before zero")]
    NegativeStart(ClipId),
    #[error("an animated parameter on clip {0} has no keys or unsorted keys")]
    BadKeyframes(ClipId),
    #[error("video tracks must come before audio tracks")]
    TrackOrder,
    #[error("the active sequence does not exist")]
    MissingActiveSequence,
    #[error("clip {0} plays an audio stream its media does not have")]
    MissingAudioStream(ClipId),
}

pub fn validate(p: &Project) -> Result<(), ModelError> {
    if let Some(id) = p.active_sequence {
        if !p.sequences.contains_key(&id) {
            return Err(ModelError::MissingActiveSequence);
        }
    }
    for seq in p.sequences.values() {
        let mut seen_audio = false;
        for t in &seq.tracks {
            match t.kind {
                TrackKind::Audio => seen_audio = true,
                TrackKind::Video if seen_audio => return Err(ModelError::TrackOrder),
                TrackKind::Video => {}
            }
            for (i, c) in t.clips.iter().enumerate() {
                if c.source_range.is_empty() {
                    return Err(ModelError::EmptyClip(c.id));
                }
                if c.timeline_start.ticks() < 0 {
                    return Err(ModelError::NegativeStart(c.id));
                }
                if let ClipSource::Asset { asset, audio_stream } = &c.source {
                    let Some(a) = p.assets.get(asset) else {
                        return Err(ModelError::MissingAsset(c.id));
                    };
                    // Only known streams can be checked (unprobed media has no info).
                    let streams = a.info.as_ref().map_or(0, |i| i.audio.len());
                    if t.kind == TrackKind::Audio && streams > 0 && *audio_stream as usize >= streams {
                        return Err(ModelError::MissingAudioStream(c.id));
                    }
                }
                for e in &c.effects {
                    for param in e.params.values() {
                        if let Param::Animated(k) = param {
                            if k.is_empty() || k.windows(2).any(|w| w[0].time >= w[1].time) {
                                return Err(ModelError::BadKeyframes(c.id));
                            }
                        }
                    }
                }
                if let Some(next) = t.clips.get(i + 1) {
                    if next.timeline_start < c.timeline_start {
                        return Err(ModelError::Unsorted(t.id));
                    }
                    if c.timeline_range().overlaps(next.timeline_range()) {
                        return Err(ModelError::Overlap(c.id, next.id, t.id));
                    }
                }
            }
        }
    }
    Ok(())
}
