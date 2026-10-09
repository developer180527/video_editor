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
    /// Point an asset at other media (relinking a moved or replaced file).
    SetAssetMedia { asset: AssetId, media: MediaRef, info: Option<MediaInfo> },
    /// Replace an asset's renditions (attach or drop proxies).
    SetAssetVariants { asset: AssetId, variants: Vec<MediaVariant> },
    /// Replace the in/out points and markers of a sequence or an asset.
    SetMarks { owner: MarksOwner, marks: Marks },
    /// An audio track's channel layout.
    SetTrackLayout { sequence: SequenceId, track: TrackId, layout: ChannelLayout },
    /// Several commands as one undo step, all or nothing.
    Batch { label: String, commands: Vec<Command> },
}

/// Whose marks a [`Command::SetMarks`] replaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MarksOwner {
    Sequence(SequenceId),
    Asset(AssetId),
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
            Command::SetAssetMedia { .. } => "Relink Media".into(),
            Command::SetAssetVariants { .. } => "Change Proxies".into(),
            Command::SetMarks { .. } => "Change Markers".into(),
            Command::SetTrackLayout { .. } => "Change Track Channels".into(),
            Command::Batch { label, .. } => label.clone(),
        }
    }

    /// Apply to `p`, returning the new project and the inverse command.
    pub fn apply(&self, p: &Project) -> Result<Applied, CommandError> {
        let mut next = p.clone();
        let inverse = self.apply_in_place(&mut next, &mut Locator::default())?;
        validate(&next)?;
        Ok(Applied { project: next, inverse })
    }

    /// Apply without checking the document rules afterwards, for building a
    /// batch step by step: the finished batch is checked when it is applied.
    /// `loc` must describe `p` (or be empty); it is kept describing the result.
    pub(crate) fn apply_unchecked(&self, p: &Project, loc: &mut Locator) -> Result<Project, CommandError> {
        let mut next = p.clone();
        self.apply_in_place(&mut next, loc)?;
        Ok(next)
    }

    fn apply_in_place(&self, p: &mut Project, loc: &mut Locator) -> Result<Command, CommandError> {
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
                loc.clear();
                p.sequences.insert(sequence.id, sequence.clone());
                RemoveSequence { sequence: sequence.id }
            }
            RemoveSequence { sequence } => {
                loc.clear();
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
                loc.clear();
                let s = seq_mut(p, *sequence)?;
                if *index > s.tracks.len() {
                    return Err(CommandError::NotFound("track position"));
                }
                s.tracks.insert(*index, track.clone());
                RemoveTrack { sequence: *sequence, track: track.id }
            }
            RemoveTrack { sequence, track } => {
                loc.clear();
                let s = seq_mut(p, *sequence)?;
                let (i, _) = s.track(*track).ok_or(CommandError::NotFound("track"))?;
                let old = s.tracks.remove(i);
                AddTrack { sequence: *sequence, index: i, track: old }
            }
            AddClip { sequence, track, clip } => {
                check_range(clip.source_range, clip.timeline_start)?;
                let t = track_mut(p, *sequence, *track)?;
                place(t, clip.clone(), None)?;
                loc.set(clip.id, *sequence, *track, clip.timeline_start);
                RemoveClip { clip: clip.id }
            }
            RemoveClip { clip } => {
                let (sid, tid, old) = loc.locate(p, *clip)?;
                let t = track_mut(p, sid, tid)?;
                if t.locked {
                    return Err(CommandError::Locked);
                }
                let i = index_of(t, &old);
                let old = t.clips.remove(i);
                loc.forget(*clip);
                AddClip { sequence: sid, track: tid, clip: old }
            }
            MoveClip { clip, track, start } => {
                let (sid, from, old) = loc.locate(p, *clip)?;
                check_range(old.source_range, *start)?;
                let from_track = track_mut(p, sid, from)?;
                if from_track.locked {
                    return Err(CommandError::Locked);
                }
                let i = index_of(from_track, &old);
                let old_start = old.timeline_start;
                let mut moved = old;
                Arc::make_mut(&mut moved).timeline_start = *start;
                // Along its own track without passing a neighbour (every
                // ripple step): replace in place.
                if *track != from || !replace_at(from_track, i, &moved) {
                    from_track.clips.remove(i);
                    place(track_mut(p, sid, *track)?, moved, Some(*clip))?;
                }
                loc.set(*clip, sid, *track, *start);
                MoveClip { clip: *clip, track: from, start: old_start }
            }
            TrimClip { clip, edge, delta } => {
                let (sid, tid, old) = loc.locate(p, *clip)?;
                let mut c = (*old).clone();
                match edge {
                    Edge::Start => {
                        // Speed-aware: frames (and remap keys) stay on their media.
                        c = old.with_start_advanced(*delta);
                        c.effects = shift_keyframes(&c.effects, *delta);
                    }
                    Edge::End => c.source_range.duration += *delta,
                }
                check_range(c.source_range, c.timeline_start)?;
                check_media(p, &c)?;
                let t = track_mut(p, sid, tid)?;
                if t.locked {
                    return Err(CommandError::Locked);
                }
                if !t.is_free(c.timeline_range(), Some(*clip)) {
                    return Err(CommandError::Overlap);
                }
                let i = index_of(t, &old);
                loc.set(*clip, sid, tid, c.timeline_start);
                t.clips.set(i, Arc::new(c));
                TrimClip { clip: *clip, edge: *edge, delta: -*delta }
            }
            SetEffectParam { clip, effect, param, value } => {
                let c = clip_mut(p, loc, *clip)?;
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
                let (sid, tid, old) = loc.locate(p, clip.id)?;
                check_media(p, clip)?;
                let t = track_mut(p, sid, tid)?;
                if t.locked {
                    return Err(CommandError::Locked);
                }
                let i = index_of(t, &old);
                if !replace_at(t, i, clip) {
                    t.clips.remove(i);
                    place(t, clip.clone(), None)?;
                }
                loc.set(clip.id, sid, tid, clip.timeline_start);
                SetClip { clip: old }
            }
            AddEffect { clip, index, effect } => {
                let c = clip_mut(p, loc, *clip)?;
                if *index > c.effects.len() {
                    return Err(CommandError::NotFound("effect position"));
                }
                c.effects.insert(*index, effect.clone());
                RemoveEffect { clip: *clip, effect: effect.id }
            }
            RemoveEffect { clip, effect } => {
                let c = clip_mut(p, loc, *clip)?;
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
            SetAssetMedia { asset, media, info } => {
                let a = Arc::make_mut(p.assets.get_mut(asset).ok_or(CommandError::NotFound("asset"))?);
                let old_media = std::mem::replace(&mut a.media, media.clone());
                let old_info = std::mem::replace(&mut a.info, info.clone());
                SetAssetMedia { asset: *asset, media: old_media, info: old_info }
            }
            SetAssetVariants { asset, variants } => {
                let a = Arc::make_mut(p.assets.get_mut(asset).ok_or(CommandError::NotFound("asset"))?);
                SetAssetVariants { asset: *asset, variants: std::mem::replace(&mut a.variants, variants.clone()) }
            }
            SetMarks { owner, marks } => {
                let slot = match owner {
                    MarksOwner::Sequence(id) => &mut seq_mut(p, *id)?.marks,
                    MarksOwner::Asset(id) => &mut Arc::make_mut(p.assets.get_mut(id).ok_or(CommandError::NotFound("asset"))?).marks,
                };
                SetMarks { owner: *owner, marks: std::mem::replace(slot, marks.clone()) }
            }
            SetTrackLayout { sequence, track, layout } => {
                let t = track_mut(p, *sequence, *track)?;
                if t.locked {
                    return Err(CommandError::Locked);
                }
                SetTrackLayout { sequence: *sequence, track: *track, layout: std::mem::replace(&mut t.layout, *layout) }
            }
            Batch { label, commands } => {
                // Applied in order on a scratch copy; `p` changes only if all succeed.
                let mut scratch = p.clone();
                let mut inv = Vec::with_capacity(commands.len());
                for c in commands {
                    inv.push(c.apply_in_place(&mut scratch, loc)?);
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

/// How long a clip's source is, when it is known: a file's duration, a
/// nested sequence's. Generators are endless.
fn source_duration(p: &Project, c: &Clip) -> Option<Time> {
    match &c.source {
        ClipSource::Asset { asset, .. } => p.assets.get(asset)?.info.as_ref().map(|i| i.duration),
        ClipSource::Sequence { sequence, .. } => p.sequences.get(sequence).map(|s| s.duration()),
        ClipSource::Generator { .. } => None,
    }
}

/// The media the clip plays — at its speed, or through its remap — lies
/// within its source.
fn check_media(p: &Project, c: &Clip) -> Result<(), CommandError> {
    if let Some(limit) = source_duration(p, c) {
        let used = c.media_extent();
        if used.start < Time::ZERO || used.end() > limit {
            return Err(CommandError::BeyondSource);
        }
    }
    Ok(())
}

fn clips_using(p: &Project, asset: AssetId) -> Option<ClipId> {
    p.sequences.values().flat_map(|s| s.tracks.iter()).flat_map(|t| t.clips.iter()).find_map(|c| match &c.source {
        ClipSource::Asset { asset: a, .. } if *a == asset => Some(c.id),
        _ => None,
    })
}

/// Finds clips by id. Empty, it scans the project; once asked, it builds an
/// index (id → sequence, track, start) that the commands keep current, so a
/// batch of many steps finds each clip in O(log n) rather than by scanning.
#[derive(Default)]
pub(crate) struct Locator(Option<std::collections::HashMap<ClipId, (SequenceId, TrackId, Time)>>);

impl Locator {
    fn locate(&mut self, p: &Project, clip: ClipId) -> Result<(SequenceId, TrackId, Arc<Clip>), CommandError> {
        let map = self.0.get_or_insert_with(|| {
            let mut m = std::collections::HashMap::new();
            for s in p.sequences.values() {
                for t in &s.tracks {
                    for c in &t.clips {
                        m.insert(c.id, (s.id, t.id, c.timeline_start));
                    }
                }
            }
            m
        });
        let &(sid, tid, start) = map.get(&clip).ok_or(CommandError::NotFound("clip"))?;
        let found = p.sequence(sid).and_then(|s| s.track(tid)).and_then(|(_, t)| {
            let i = t.insertion_index(start).checked_sub(1)?;
            t.clips.get(i).filter(|c| c.id == clip).cloned()
        });
        match found {
            Some(c) => Ok((sid, tid, c)),
            None => {
                // Out of step (should not happen): rebuild from the project.
                debug_assert!(false, "locator out of step for {clip}");
                self.0 = None;
                let (s, ti, c) = p.find_clip(clip).ok_or(CommandError::NotFound("clip"))?;
                Ok((s.id, s.tracks[ti].id, c.clone()))
            }
        }
    }

    fn set(&mut self, clip: ClipId, s: SequenceId, t: TrackId, start: Time) {
        if let Some(m) = &mut self.0 {
            m.insert(clip, (s, t, start));
        }
    }

    fn forget(&mut self, clip: ClipId) {
        if let Some(m) = &mut self.0 {
            m.remove(&clip);
        }
    }

    /// Tracks or sequences came or went: start again.
    fn clear(&mut self) {
        self.0 = None;
    }
}

fn seq_mut(p: &mut Project, id: SequenceId) -> Result<&mut Sequence, CommandError> {
    Ok(Arc::make_mut(p.sequences.get_mut(&id).ok_or(CommandError::NotFound("sequence"))?))
}

fn track_mut(p: &mut Project, sid: SequenceId, tid: TrackId) -> Result<&mut Track, CommandError> {
    let s = seq_mut(p, sid)?;
    let i = s.track(tid).ok_or(CommandError::NotFound("track"))?.0;
    Ok(Arc::make_mut(&mut s.tracks[i]))
}

/// Where `clip` (found on `t` by `locate`) sits in `t`: by its start, in
/// O(log n). Starts are unique on a track (clips never overlap).
fn index_of(t: &Track, clip: &Clip) -> usize {
    let i = t.insertion_index(clip.timeline_start) - 1;
    debug_assert_eq!(t.clips[i].id, clip.id);
    i
}

/// The clip with `id`, ready to change. Locked tracks refuse.
fn clip_mut<'p>(p: &'p mut Project, loc: &mut Locator, id: ClipId) -> Result<&'p mut Clip, CommandError> {
    let (sid, tid, old) = loc.locate(p, id)?;
    let t = track_mut(p, sid, tid)?;
    if t.locked {
        return Err(CommandError::Locked);
    }
    let i = index_of(t, &old);
    Ok(Arc::make_mut(&mut t.clips[i]))
}

/// Put `clip` — a new version of the clip at `i` — back at `i`, if it still
/// sorts there and fits. O(log n) and, unlike removing and re-inserting in
/// the middle of the persistent vector, it leaves the vector's tree as it was
/// (repeated middle removals and inserts degrade it). False if the clip must
/// go elsewhere.
fn replace_at(t: &mut Track, i: usize, clip: &Arc<Clip>) -> bool {
    let start = clip.timeline_start;
    let sorted = (i == 0 || t.clips[i - 1].timeline_start < start) && t.clips.get(i + 1).is_none_or(|n| n.timeline_start > start);
    if sorted && t.is_free(clip.timeline_range(), Some(clip.id)) {
        t.clips.set(i, clip.clone());
        return true;
    }
    false
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
