//! Every change to a project is a [`Command`].
//!
//! A command is plain data (serializable), so the same path serves the UI,
//! scripts, add-ons over IPC, autosave journals and, later, collaboration.
//! [`Command::apply`] takes a project and returns a **new** project plus the
//! command that undoes it; the input is never touched. A command either
//! applies completely and leaves a valid project, or fails and changes
//! nothing.

mod history;
pub mod edit;

pub use history::{History, HistoryEntry};

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use thiserror::Error;
use ve_model::*;
use ve_time::{Time, TimeRange};

#[derive(Debug, Error, PartialEq)]
pub enum CommandError {
    #[error("no such {0}")]
    NotFound(&'static str),
    #[error("the destination overlaps another clip")]
    Overlap,
    #[error("the track is locked")]
    Locked,
    #[error("a clip cannot go to a {0:?} track")]
    WrongTrackKind(TrackKind),
    #[error("a clip must be at least one tick long and start at or after zero")]
    BadRange,
    #[error("trimmed past the end of the source media")]
    BeyondSource,
    #[error("the asset is still used by clip {0}")]
    InUse(ClipId),
    #[error("the result breaks a document rule: {0}")]
    Invalid(#[from] ModelError),
}

/// A track's switches.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackState {
    pub enabled: bool,
    pub locked: bool,
    pub muted: bool,
    pub solo: bool,
}

impl TrackState {
    pub fn of(t: &Track) -> Self {
        TrackState { enabled: t.enabled, locked: t.locked, muted: t.muted, solo: t.solo }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Edge {
    Start,
    End,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Command {
    Rename { name: String },
    AddAsset { asset: Arc<Asset> },
    RemoveAsset { asset: AssetId },
    SetAssetInfo { asset: AssetId, info: Option<MediaInfo> },
    AddSequence { sequence: Arc<Sequence> },
    RemoveSequence { sequence: SequenceId },
    SetActiveSequence { sequence: Option<SequenceId> },
    AddTrack { sequence: SequenceId, index: usize, track: Arc<Track> },
    RemoveTrack { sequence: SequenceId, track: TrackId },
    /// Places a clip where the track is empty. (Insert and overwrite edits are
    /// built from this plus moves and trims, in `Batch`es.)
    AddClip { sequence: SequenceId, track: TrackId, clip: Arc<Clip> },
    RemoveClip { clip: ClipId },
    MoveClip { clip: ClipId, track: TrackId, start: Time },
    /// Moves one edge by `delta`: the start edge also slides the source in
    /// point, so the frames that stay keep their place on the timeline.
    TrimClip { clip: ClipId, edge: Edge, delta: Time },
    SetEffectParam { clip: ClipId, effect: EffectId, param: String, value: Option<Param> },
    /// Replace a clip with a new version of itself (same id, same track):
    /// slips, splits, renames, enabling.
    SetClip { clip: Arc<Clip> },
    AddEffect { clip: ClipId, index: usize, effect: Arc<Effect> },
    RemoveEffect { clip: ClipId, effect: EffectId },
    SetTrackState { sequence: SequenceId, track: TrackId, state: TrackState },
    /// Several commands as one undo step, all or nothing.
    Batch { label: String, commands: Vec<Command> },
}

/// The result of applying a command.
#[derive(Debug)]
pub struct Applied {
    pub project: Project,
    pub inverse: Command,
}

impl Command {
    /// A short label for the Edit menu ("Undo Move Clip").
    pub fn label(&self) -> String {
        match self {
            Command::Rename { .. } => "Rename Project".into(),
            Command::AddAsset { .. } => "Import".into(),
            Command::RemoveAsset { .. } => "Remove Media".into(),
            Command::SetAssetInfo { .. } => "Update Media Info".into(),
            Command::AddSequence { .. } => "New Sequence".into(),
            Command::RemoveSequence { .. } => "Delete Sequence".into(),
            Command::SetActiveSequence { .. } => "Open Sequence".into(),
            Command::AddTrack { .. } => "Add Track".into(),
            Command::RemoveTrack { .. } => "Delete Track".into(),
            Command::AddClip { .. } => "Add Clip".into(),
            Command::RemoveClip { .. } => "Delete Clip".into(),
            Command::MoveClip { .. } => "Move Clip".into(),
            Command::TrimClip { .. } => "Trim".into(),
            Command::SetEffectParam { param, .. } => format!("Change {param}"),
            Command::SetClip { .. } => "Change Clip".into(),
            Command::AddEffect { .. } => "Add Effect".into(),
            Command::RemoveEffect { .. } => "Remove Effect".into(),
            Command::SetTrackState { .. } => "Change Track".into(),
            Command::Batch { label, .. } => label.clone(),
        }
    }

    /// Apply to `p`, returning the new project and the inverse command.
    pub fn apply(&self, p: &Project) -> Result<Applied, CommandError> {
        let mut next = p.clone();
        let inverse = self.apply_in_place(&mut next)?;
        validate(&next)?;
        Ok(Applied { project: next, inverse })
    }

    fn apply_in_place(&self, p: &mut Project) -> Result<Command, CommandError> {
        use Command::*;
        Ok(match self {
            Rename { name } => Rename { name: std::mem::replace(&mut p.name, name.clone()) },
            AddAsset { asset } => {
                p.assets.insert(asset.id, asset.clone());
                RemoveAsset { asset: asset.id }
            }
            RemoveAsset { asset } => {
                if let Some(c) = clips_using(p, *asset) {
                    return Err(CommandError::InUse(c));
                }
                let old = p.assets.remove(asset).ok_or(CommandError::NotFound("asset"))?;
                AddAsset { asset: old }
            }
            SetAssetInfo { asset, info } => {
                let a = Arc::make_mut(p.assets.get_mut(asset).ok_or(CommandError::NotFound("asset"))?);
                SetAssetInfo { asset: *asset, info: std::mem::replace(&mut a.info, info.clone()) }
            }
            AddSequence { sequence } => {
                p.sequences.insert(sequence.id, sequence.clone());
                RemoveSequence { sequence: sequence.id }
            }
            RemoveSequence { sequence } => {
                let old = p.sequences.remove(sequence).ok_or(CommandError::NotFound("sequence"))?;
                let mut inv = vec![AddSequence { sequence: old }];
                if p.active_sequence == Some(*sequence) {
                    p.active_sequence = None;
                    inv.push(SetActiveSequence { sequence: Some(*sequence) });
                }
                batch("Delete Sequence", inv)
            }
            SetActiveSequence { sequence } => {
                if let Some(id) = sequence {
                    p.sequences.get(id).ok_or(CommandError::NotFound("sequence"))?;
                }
                SetActiveSequence { sequence: std::mem::replace(&mut p.active_sequence, *sequence) }
            }
            AddTrack { sequence, index, track } => {
                let s = seq_mut(p, *sequence)?;
                if *index > s.tracks.len() {
                    return Err(CommandError::NotFound("track position"));
                }
                s.tracks.insert(*index, track.clone());
                RemoveTrack { sequence: *sequence, track: track.id }
            }
            RemoveTrack { sequence, track } => {
                let s = seq_mut(p, *sequence)?;
                let (i, _) = s.track(*track).ok_or(CommandError::NotFound("track"))?;
                let old = s.tracks.remove(i);
                AddTrack { sequence: *sequence, index: i, track: old }
            }
            AddClip { sequence, track, clip } => {
                check_range(clip.source_range, clip.timeline_start)?;
                let t = track_mut(p, *sequence, *track)?;
                place(t, clip.clone(), None)?;
                RemoveClip { clip: clip.id }
            }
            RemoveClip { clip } => {
                let (sid, tid, _) = locate(p, *clip)?;
                let t = track_mut(p, sid, tid)?;
                if t.locked {
                    return Err(CommandError::Locked);
                }
                let i = t.clips.iter().position(|c| c.id == *clip).unwrap();
                let old = t.clips.remove(i);
                AddClip { sequence: sid, track: tid, clip: old }
            }
            MoveClip { clip, track, start } => {
                let (sid, from, old) = locate(p, *clip)?;
                check_range(old.source_range, *start)?;
                let from_track = track_mut(p, sid, from)?;
                if from_track.locked {
                    return Err(CommandError::Locked);
                }
                let i = from_track.clips.iter().position(|c| c.id == *clip).unwrap();
                let mut moved = from_track.clips.remove(i);
                let old_start = moved.timeline_start;
                Arc::make_mut(&mut moved).timeline_start = *start;
                place(track_mut(p, sid, *track)?, moved, Some(*clip))?;
                MoveClip { clip: *clip, track: from, start: old_start }
            }
            TrimClip { clip, edge, delta } => {
                let (sid, tid, old) = locate(p, *clip)?;
                let mut c = (*old).clone();
                match edge {
                    Edge::Start => {
                        c.timeline_start += *delta;
                        c.source_range.start += *delta;
                        c.source_range.duration -= *delta;
                        c.effects = shift_keyframes(&c.effects, *delta);
                    }
                    Edge::End => c.source_range.duration += *delta,
                }
                check_range(c.source_range, c.timeline_start)?;
                if let Some(limit) = source_duration(p, &c) {
                    if c.source_range.start < Time::ZERO || c.source_range.end() > limit {
                        return Err(CommandError::BeyondSource);
                    }
                }
                let t = track_mut(p, sid, tid)?;
                if t.locked {
                    return Err(CommandError::Locked);
                }
                if !t.is_free(c.timeline_range(), Some(*clip)) {
                    return Err(CommandError::Overlap);
                }
                let i = t.clips.iter().position(|x| x.id == *clip).unwrap();
                t.clips.set(i, Arc::new(c));
                TrimClip { clip: *clip, edge: *edge, delta: -*delta }
            }
            SetEffectParam { clip, effect, param, value } => {
                let c = clip_mut(p, *clip)?;
                let e = c.effects.iter_mut().find(|e| e.id == *effect).ok_or(CommandError::NotFound("effect"))?;
                let e = Arc::make_mut(e);
                let old = match value {
                    Some(v) => e.params.insert(param.clone(), v.clone()),
                    None => e.params.remove(param),
                };
                SetEffectParam { clip: *clip, effect: *effect, param: param.clone(), value: old }
            }
            SetClip { clip } => {
                check_range(clip.source_range, clip.timeline_start)?;
                let (sid, tid, old) = locate(p, clip.id)?;
                if let Some(limit) = source_duration(p, clip) {
                    if clip.source_range.start < Time::ZERO || clip.source_range.end() > limit {
                        return Err(CommandError::BeyondSource);
                    }
                }
                let t = track_mut(p, sid, tid)?;
                let i = t.clips.iter().position(|c| c.id == clip.id).unwrap();
                t.clips.remove(i);
                place(t, clip.clone(), None)?;
                SetClip { clip: old }
            }
            AddEffect { clip, index, effect } => {
                let c = clip_mut(p, *clip)?;
                if *index > c.effects.len() {
                    return Err(CommandError::NotFound("effect position"));
                }
                c.effects.insert(*index, effect.clone());
                RemoveEffect { clip: *clip, effect: effect.id }
            }
            RemoveEffect { clip, effect } => {
                let c = clip_mut(p, *clip)?;
                let i = c.effects.iter().position(|e| e.id == *effect).ok_or(CommandError::NotFound("effect"))?;
                let old = c.effects.remove(i);
                AddEffect { clip: *clip, index: i, effect: old }
            }
            SetTrackState { sequence, track, state } => {
                let t = track_mut(p, *sequence, *track)?;
                let old = TrackState::of(t);
                t.enabled = state.enabled;
                t.locked = state.locked;
                t.muted = state.muted;
                t.solo = state.solo;
                SetTrackState { sequence: *sequence, track: *track, state: old }
            }
            Batch { label, commands } => {
                // Applied in order on a scratch copy; `p` changes only if all succeed.
                let mut scratch = p.clone();
                let mut inv = Vec::with_capacity(commands.len());
                for c in commands {
                    inv.push(c.apply_in_place(&mut scratch)?);
                }
                *p = scratch;
                inv.reverse();
                Batch { label: label.clone(), commands: inv }
            }
        })
    }
}

/// Keyframe times are relative to the clip's start. When the start moves
/// through the source by `by` (a head trim, a split's right half), keys move
/// the other way, so each stays on the same source frame — a fade keyed on a
/// shot stays on that shot however its head is trimmed. Keys that end up
/// before the start are kept (and come back if the trim is undone).
pub(crate) fn shift_keyframes(effects: &ve_model::Vector<Arc<Effect>>, by: Time) -> ve_model::Vector<Arc<Effect>> {
    let animated = |e: &Effect| e.params.values().any(|p| matches!(p, Param::Animated(_)));
    effects
        .iter()
        .map(|e| {
            if by == Time::ZERO || !animated(e) {
                return e.clone();
            }
            let mut e = (**e).clone();
            e.params = e
                .params
                .iter()
                .map(|(k, p)| {
                    let p = match p {
                        Param::Animated(keys) => Param::Animated(keys.iter().map(|kf| Keyframe { time: kf.time - by, ..kf.clone() }).collect()),
                        other => other.clone(),
                    };
                    (k.clone(), p)
                })
                .collect();
            Arc::new(e)
        })
        .collect()
}

fn batch(label: &str, mut v: Vec<Command>) -> Command {
    if v.len() == 1 {
        v.pop().unwrap()
    } else {
        Command::Batch { label: label.into(), commands: v }
    }
}

fn check_range(source: TimeRange, start: Time) -> Result<(), CommandError> {
    if source.is_empty() || start < Time::ZERO {
        return Err(CommandError::BadRange);
    }
    Ok(())
}

fn source_duration(p: &Project, c: &Clip) -> Option<Time> {
    match &c.source {
        ClipSource::Asset { asset } => p.assets.get(asset)?.info.as_ref().map(|i| i.duration),
        ClipSource::Generator { .. } => None,
    }
}

fn clips_using(p: &Project, asset: AssetId) -> Option<ClipId> {
    p.sequences.values().flat_map(|s| s.tracks.iter()).flat_map(|t| t.clips.iter()).find_map(|c| match &c.source {
        ClipSource::Asset { asset: a } if *a == asset => Some(c.id),
        _ => None,
    })
}

fn locate(p: &Project, clip: ClipId) -> Result<(SequenceId, TrackId, Arc<Clip>), CommandError> {
    let (s, ti, c) = p.find_clip(clip).ok_or(CommandError::NotFound("clip"))?;
    Ok((s.id, s.tracks[ti].id, c.clone()))
}

fn seq_mut(p: &mut Project, id: SequenceId) -> Result<&mut Sequence, CommandError> {
    Ok(Arc::make_mut(p.sequences.get_mut(&id).ok_or(CommandError::NotFound("sequence"))?))
}

fn track_mut(p: &mut Project, sid: SequenceId, tid: TrackId) -> Result<&mut Track, CommandError> {
    let s = seq_mut(p, sid)?;
    let i = s.track(tid).ok_or(CommandError::NotFound("track"))?.0;
    Ok(Arc::make_mut(&mut s.tracks[i]))
}

/// The clip with `id`, ready to change. Locked tracks refuse.
fn clip_mut(p: &mut Project, id: ClipId) -> Result<&mut Clip, CommandError> {
    let (sid, tid, _) = locate(p, id)?;
    let t = track_mut(p, sid, tid)?;
    if t.locked {
        return Err(CommandError::Locked);
    }
    let i = t.clips.iter().position(|c| c.id == id).unwrap();
    Ok(Arc::make_mut(&mut t.clips[i]))
}

/// Put `clip` on `t` at its `timeline_start`, keeping the track sorted.
fn place(t: &mut Track, clip: Arc<Clip>, except: Option<ClipId>) -> Result<(), CommandError> {
    if t.locked {
        return Err(CommandError::Locked);
    }
    if !t.is_free(clip.timeline_range(), except) {
        return Err(CommandError::Overlap);
    }
    let i = t.insertion_index(clip.timeline_start);
    t.clips.insert(i, clip);
    Ok(())
}

#[cfg(test)]
mod tests;
