//! The invariants every project satisfies. Commands keep them; loading a file
//! checks them; tests assert them after every edit.

use crate::{ClipId, ClipSource, Marks, Param, Project, Retime, SequenceId, TrackId, TrackKind};
use ve_time::Time;
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
    #[error("clip {0} has a speed of zero or unsorted remap keys")]
    BadRetime(ClipId),
    #[error("a transition on clip {0} is empty, negative or longer than the clip")]
    BadTransition(ClipId),
    #[error("clip {0} refers to a missing sequence")]
    MissingSequence(ClipId),
    #[error("sequence {0} contains itself")]
    NestingCycle(SequenceId),
    #[error("clip {0} shows a multicam angle its sequence does not have")]
    MissingAngle(ClipId),
    #[error("clip {0} picks audio channels its stream does not have (or one twice)")]
    BadChannels(ClipId),
    #[error("in point after out point, or markers out of order")]
    BadMarks,
    #[error("clip id {0} is used twice")]
    DuplicateClip(ClipId),
}

fn check_marks(m: &Marks) -> Result<(), ModelError> {
    let ordered = m.markers.iter().zip(m.markers.iter().skip(1)).all(|(a, b)| a.time <= b.time);
    let in_out = match (m.in_point, m.out_point) {
        (Some(a), Some(b)) => a <= b,
        _ => true,
    };
    if ordered && in_out && m.markers.iter().all(|k| k.duration >= ve_time::Time::ZERO) {
        Ok(())
    } else {
        Err(ModelError::BadMarks)
    }
}

/// No sequence may contain itself, however deeply.
fn check_nesting(p: &Project) -> Result<(), ModelError> {
    use std::collections::HashMap;
    let nested = |s: SequenceId| -> Vec<SequenceId> {
        p.sequences.get(&s).map_or(vec![], |seq| {
            seq.tracks
                .iter()
                .flat_map(|t| t.clips.iter())
                .filter_map(|c| match c.source {
                    ClipSource::Sequence { sequence, .. } => Some(sequence),
                    _ => None,
                })
                .collect()
        })
    };
    // 0 unvisited, 1 on the current path, 2 done.
    let mut state: HashMap<SequenceId, u8> = HashMap::new();
    fn visit(s: SequenceId, nested: &dyn Fn(SequenceId) -> Vec<SequenceId>, state: &mut HashMap<SequenceId, u8>) -> Result<(), ModelError> {
        match state.get(&s) {
            Some(1) => return Err(ModelError::NestingCycle(s)),
            Some(2) => return Ok(()),
            _ => {}
        }
        state.insert(s, 1);
        for n in nested(s) {
            visit(n, nested, state)?;
        }
        state.insert(s, 2);
        Ok(())
    }
    for s in p.sequences.keys() {
        visit(*s, &nested, &mut state)?;
    }
    Ok(())
}

pub fn validate(p: &Project) -> Result<(), ModelError> {
    if let Some(id) = p.active_sequence {
        if !p.sequences.contains_key(&id) {
            return Err(ModelError::MissingActiveSequence);
        }
    }
    check_nesting(p)?;
    // Clip ids are unique across the project: commands find clips by id.
    let mut ids = std::collections::HashSet::new();
    for c in p.sequences.values().flat_map(|s| s.tracks.iter()).flat_map(|t| t.clips.iter()) {
        if !ids.insert(c.id) {
            return Err(ModelError::DuplicateClip(c.id));
        }
    }
    for a in p.assets.values() {
        check_marks(&a.marks)?;
    }
    for seq in p.sequences.values() {
        check_marks(&seq.marks)?;
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
                match &c.source {
                    ClipSource::Asset { asset, audio_stream } => {
                        let Some(a) = p.assets.get(asset) else {
                            return Err(ModelError::MissingAsset(c.id));
                        };
                        // Only known streams can be checked (unprobed media has no info).
                        let stream = a.info.as_ref().and_then(|i| i.audio.get(*audio_stream as usize));
                        let streams = a.info.as_ref().map_or(0, |i| i.audio.len());
                        if t.kind == TrackKind::Audio && streams > 0 && stream.is_none() {
                            return Err(ModelError::MissingAudioStream(c.id));
                        }
                        if let Some(st) = stream {
                            let mut seen = std::collections::HashSet::new();
                            if c.channels.iter().any(|ch| *ch >= st.channels || !seen.insert(*ch)) {
                                return Err(ModelError::BadChannels(c.id));
                            }
                        }
                    }
                    ClipSource::Sequence { sequence, angle } => {
                        let Some(n) = p.sequences.get(sequence) else {
                            return Err(ModelError::MissingSequence(c.id));
                        };
                        let video = n.tracks.iter().filter(|t| t.kind == TrackKind::Video).count();
                        if angle.is_some_and(|a| a as usize >= video) {
                            return Err(ModelError::MissingAngle(c.id));
                        }
                    }
                    ClipSource::Generator { .. } => {}
                }
                match &c.retime {
                    Retime::Speed(r) if r.num == 0 => return Err(ModelError::BadRetime(c.id)),
                    Retime::Remap(k) if k.is_empty() || k.windows(2).any(|w| w[0].time >= w[1].time) => return Err(ModelError::BadRetime(c.id)),
                    _ => {}
                }
                let dur = c.source_range.duration;
                let edge = |tr: &Option<std::sync::Arc<crate::Transition>>, inside: fn(&crate::Transition) -> ve_time::Time| {
                    tr.as_ref().map_or(Ok(ve_time::Time::ZERO), |tr| {
                        let keys_ok = tr.params.values().all(|p| !matches!(p, Param::Animated(k) if k.is_empty() || k.windows(2).any(|w| w[0].time >= w[1].time)));
                        let ok = tr.before >= ve_time::Time::ZERO && tr.after >= ve_time::Time::ZERO && tr.duration() > ve_time::Time::ZERO && inside(tr) <= dur && keys_ok;
                        if ok {
                            Ok(inside(tr))
                        } else {
                            Err(ModelError::BadTransition(c.id))
                        }
                    })
                };
                // The parts of each transition inside the clip must fit in it.
                let (a, b) = (edge(&c.transition_in, |t| t.after)?, edge(&c.transition_out, |t| t.before)?);
                if a + b > dur {
                    return Err(ModelError::BadTransition(c.id));
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
                    // Transition regions never overlap: a cut's reach back
                    // stays clear of the previous clip's own way in, and a
                    // fade out stays clear of the next clip's fade in.
                    let adjacent = next.timeline_start == c.timeline_range().end();
                    let next_head = next.transition_in.as_ref().map_or(next.timeline_start, |tr| next.timeline_start - tr.before);
                    let clear_from = if adjacent {
                        c.timeline_start + c.transition_in.as_ref().map_or(Time::ZERO, |tr| tr.after)
                    } else {
                        c.timeline_range().end() + c.transition_out.as_ref().map_or(Time::ZERO, |tr| tr.after)
                    };
                    if next_head < clear_from {
                        return Err(ModelError::BadTransition(next.id));
                    }
                }
            }
        }
    }
    Ok(())
}
