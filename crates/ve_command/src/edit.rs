//! Editing tools — razor, ripple, roll, slip, insert, overwrite, linked moves.
//!
//! A tool is not a new kind of command: it compiles down to primitive
//! commands in one [`Command::Batch`], so undo, serialization and validation
//! come for free. The [`Builder`] applies each step to a scratch copy as it
//! goes, so a tool can never emit a sequence of steps that passes through an
//! invalid project, and later steps can be computed from earlier results.

use std::collections::HashMap;
use std::sync::Arc;

use ve_model::*;
use ve_time::Time;

use crate::{Command, CommandError, Edge};

/// Builds a batch by applying each step to a scratch project.
pub struct Builder {
    p: Project,
    cmds: Vec<Command>,
}

impl Builder {
    pub fn new(p: &Project) -> Self {
        Builder { p: p.clone(), cmds: Vec::new() }
    }

    pub fn push(&mut self, c: Command) -> Result<(), CommandError> {
        self.p = c.apply(&self.p)?.project;
        self.cmds.push(c);
        Ok(())
    }

    /// The project as it stands after the steps so far.
    pub fn project(&self) -> &Project {
        &self.p
    }

    pub fn finish(self, label: &str) -> Result<Command, CommandError> {
        if self.cmds.is_empty() {
            return Err(CommandError::NotFound("anything to edit"));
        }
        Ok(Command::Batch { label: label.into(), commands: self.cmds })
    }
}

fn find(p: &Project, id: ClipId) -> Result<(SequenceId, TrackId, Arc<Clip>), CommandError> {
    let (s, ti, c) = p.find_clip(id).ok_or(CommandError::NotFound("clip"))?;
    Ok((s.id, s.tracks[ti].id, c.clone()))
}

fn track(p: &Project, seq: SequenceId, track: TrackId) -> Result<&Arc<Track>, CommandError> {
    let s = p.sequence(seq).ok_or(CommandError::NotFound("sequence"))?;
    s.track(track).map(|(_, t)| t).ok_or(CommandError::NotFound("track"))
}

/// `clip` and every clip linked to it, in the same sequence.
pub fn linked(p: &Project, clip: ClipId) -> Vec<ClipId> {
    let Some((seq, _, c)) = p.find_clip(clip) else { return vec![] };
    match c.link {
        None => vec![clip],
        Some(l) => seq.tracks.iter().flat_map(|t| t.clips.iter()).filter(|c| c.link == Some(l)).map(|c| c.id).collect(),
    }
}

fn with_linked(p: &Project, clips: &[ClipId], follow_links: bool) -> Vec<ClipId> {
    let mut out: Vec<ClipId> = Vec::new();
    for &c in clips {
        let group = if follow_links { linked(p, c) } else { vec![c] };
        for g in group {
            if !out.contains(&g) {
                out.push(g);
            }
        }
    }
    out
}

/// Shift every clip on a track that starts at or after `from` by `delta`,
/// in the order that never overlaps (right-to-left when moving right).
fn shift_track(b: &mut Builder, seq: SequenceId, tr: TrackId, from: Time, delta: Time, except: &[ClipId]) -> Result<(), CommandError> {
    if delta == Time::ZERO {
        return Ok(());
    }
    let mut clips: Vec<Arc<Clip>> = track(b.project(), seq, tr)?
        .clips
        .iter()
        .filter(|c| c.timeline_start >= from && !except.contains(&c.id))
        .cloned()
        .collect();
    if delta > Time::ZERO {
        clips.reverse();
    }
    for c in clips {
        b.push(Command::MoveClip { clip: c.id, track: tr, start: c.timeline_start + delta })?;
    }
    Ok(())
}

/// Keyframes are clip-relative: the right half of a split sees them shifted.
fn shift_keyframes(effects: &Vector<Arc<Effect>>, by: Time) -> Vector<Arc<Effect>> {
    effects
        .iter()
        .map(|e| {
            let mut e = (**e).clone();
            e.params = e
                .params
                .iter()
                .map(|(k, p)| {
                    let p = match p {
                        Param::Animated(keys) => Param::Animated(
                            keys.iter().map(|kf| Keyframe { time: kf.time - by, ..kf.clone() }).collect(),
                        ),
                        other => other.clone(),
                    };
                    (k.clone(), p)
                })
                .collect();
            Arc::new(e)
        })
        .collect()
}

/// Split `clip` at sequence time `at`. Returns the new right-hand clip's id.
/// `new_link` maps the clip's link group to the group its right half joins.
fn split(b: &mut Builder, clip: ClipId, at: Time, new_link: &mut HashMap<LinkId, LinkId>) -> Result<Option<ClipId>, CommandError> {
    let (seq, tr, c) = find(b.project(), clip)?;
    let r = c.timeline_range();
    if at <= r.start || at >= r.end() {
        return Ok(None);
    }
    let offset = at - c.timeline_start;
    let mut left = (*c).clone();
    left.source_range.duration = offset;
    let mut right = (*c).clone();
    right.id = ClipId::new();
    right.timeline_start = at;
    right.source_range.start += offset;
    right.source_range.duration -= offset;
    right.link = c.link.map(|l| *new_link.entry(l).or_insert_with(LinkId::new));
    right.effects = shift_keyframes(&c.effects, offset);
    let id = right.id;
    b.push(Command::SetClip { clip: Arc::new(left) })?;
    b.push(Command::AddClip { sequence: seq, track: tr, clip: Arc::new(right) })?;
    Ok(Some(id))
}

/// Razor: split each of `clips` (and, with `follow_links`, their partners)
/// at `at`. Split halves of linked clips stay linked to each other.
pub fn razor(p: &Project, clips: &[ClipId], at: Time, follow_links: bool) -> Result<Command, CommandError> {
    let mut b = Builder::new(p);
    let mut links = HashMap::new();
    for c in with_linked(p, clips, follow_links) {
        split(&mut b, c, at, &mut links)?;
    }
    b.finish("Razor")
}

/// Add Edit (Cmd+K): split whatever lies under `at` on `tracks` (all tracks
/// when empty).
pub fn add_edit(p: &Project, seq: SequenceId, tracks: &[TrackId], at: Time) -> Result<Command, CommandError> {
    let s = p.sequence(seq).ok_or(CommandError::NotFound("sequence"))?;
    let under: Vec<ClipId> = s
        .tracks
        .iter()
        .filter(|t| tracks.is_empty() || tracks.contains(&t.id))
        .filter_map(|t| t.clip_at(at).map(|c| c.id))
        .collect();
    razor(p, &under, at, false)
}

/// Delete and close the gap each clip leaves on its own track.
pub fn ripple_delete(p: &Project, clips: &[ClipId], follow_links: bool) -> Result<Command, CommandError> {
    let targets = with_linked(p, clips, follow_links);
    let mut b = Builder::new(p);
    // Per track: the removed ranges, so each remaining clip moves left by the
    // total length removed before it.
    let mut by_track: HashMap<(SequenceId, TrackId), Vec<ve_time::TimeRange>> = HashMap::new();
    for &c in &targets {
        let (s, t, clip) = find(p, c)?;
        by_track.entry((s, t)).or_default().push(clip.timeline_range());
        b.push(Command::RemoveClip { clip: c })?;
    }
    for ((s, t), mut removed) in by_track {
        removed.sort_by_key(|r| r.start);
        let first = removed[0].start;
        let rest: Vec<Arc<Clip>> = track(b.project(), s, t)?.clips.iter().filter(|c| c.timeline_start >= first).cloned().collect();
        for c in rest {
            let gap = removed.iter().filter(|r| r.end() <= c.timeline_start).fold(Time::ZERO, |a, r| a + r.duration);
            b.push(Command::MoveClip { clip: c.id, track: t, start: c.timeline_start - gap })?;
        }
    }
    b.finish("Ripple Delete")
}

/// Ripple trim: move an edge and push or pull everything after it on the
/// same track, so no gap opens and nothing is overwritten. Trimming the start
/// edge keeps the clip where it is and changes which frames it shows.
pub fn ripple_trim(p: &Project, clip: ClipId, edge: Edge, delta: Time, follow_links: bool) -> Result<Command, CommandError> {
    let mut b = Builder::new(p);
    for c in with_linked(p, &[clip], follow_links) {
        let (s, t, old) = find(b.project(), c)?;
        let end = old.timeline_range().end();
        let mut new = (*old).clone();
        // How far the clip's end moves.
        let growth = match edge {
            Edge::End => {
                new.source_range.duration += delta;
                delta
            }
            Edge::Start => {
                new.source_range.start += delta;
                new.source_range.duration -= delta;
                -delta
            }
        };
        if growth > Time::ZERO {
            shift_track(&mut b, s, t, end, growth, &[c])?;
            b.push(Command::SetClip { clip: Arc::new(new) })?;
        } else {
            b.push(Command::SetClip { clip: Arc::new(new) })?;
            shift_track(&mut b, s, t, end, growth, &[c])?;
        }
    }
    b.finish("Ripple Trim")
}

/// Roll edit: move the cut between two adjacent clips; the total stays the same.
pub fn roll(p: &Project, left: ClipId, right: ClipId, delta: Time) -> Result<Command, CommandError> {
    let (_, lt, l) = find(p, left)?;
    let (_, rt, r) = find(p, right)?;
    if lt != rt || l.timeline_range().end() != r.timeline_start {
        return Err(CommandError::NotFound("adjacent clips"));
    }
    let mut b = Builder::new(p);
    let shrink_right = Command::TrimClip { clip: right, edge: Edge::Start, delta };
    let grow_left = Command::TrimClip { clip: left, edge: Edge::End, delta };
    if delta > Time::ZERO {
        b.push(shrink_right)?;
        b.push(grow_left)?;
    } else {
        b.push(grow_left)?;
        b.push(shrink_right)?;
    }
    b.finish("Roll Edit")
}

/// Slip: show different frames through the same window on the timeline.
pub fn slip(p: &Project, clip: ClipId, delta: Time, follow_links: bool) -> Result<Command, CommandError> {
    let mut b = Builder::new(p);
    for c in with_linked(p, &[clip], follow_links) {
        let (_, _, old) = find(p, c)?;
        let mut new = (*old).clone();
        new.source_range.start += delta;
        b.push(Command::SetClip { clip: Arc::new(new) })?;
    }
    b.finish("Slip")
}

/// Move a clip by dragging it: to `start` on `track`, its linked partners by
/// the same amount on their own tracks.
pub fn move_clip(p: &Project, clip: ClipId, to_track: TrackId, start: Time, follow_links: bool) -> Result<Command, CommandError> {
    let (_, _, c) = find(p, clip)?;
    let delta = start - c.timeline_start;
    let group = with_linked(p, &[clip], follow_links);
    let mut b = Builder::new(p);
    // Lift them all, then put them all down: partners never collide mid-move.
    let mut placed = Vec::new();
    for &g in &group {
        let (s, t, gc) = find(p, g)?;
        let mut moved = (*gc).clone();
        moved.timeline_start = gc.timeline_start + delta;
        placed.push((s, if g == clip { to_track } else { t }, Arc::new(moved)));
        b.push(Command::RemoveClip { clip: g })?;
    }
    for (s, t, c) in placed {
        b.push(Command::AddClip { sequence: s, track: t, clip: c })?;
    }
    b.finish("Move")
}

/// Insert edit: open a gap at `at` on each item's track (splitting a clip in
/// the way) and put the item there.
pub fn insert(p: &Project, seq: SequenceId, at: Time, items: &[(TrackId, Arc<Clip>)]) -> Result<Command, CommandError> {
    let mut b = Builder::new(p);
    let mut links = HashMap::new();
    for (t, clip) in items {
        if let Some(c) = track(b.project(), seq, *t)?.clip_at(at).cloned() {
            split(&mut b, c.id, at, &mut links)?;
        }
        shift_track(&mut b, seq, *t, at, clip.source_range.duration, &[])?;
        let mut c = (**clip).clone();
        c.timeline_start = at;
        b.push(Command::AddClip { sequence: seq, track: *t, clip: Arc::new(c) })?;
    }
    b.finish("Insert")
}

/// Overwrite edit: replace whatever is under `[at, at + duration)` on each
/// item's track with the item.
pub fn overwrite(p: &Project, seq: SequenceId, at: Time, items: &[(TrackId, Arc<Clip>)]) -> Result<Command, CommandError> {
    let mut b = Builder::new(p);
    let mut links = HashMap::new();
    for (t, clip) in items {
        let end = at + clip.source_range.duration;
        for cut in [at, end] {
            if let Some(c) = track(b.project(), seq, *t)?.clip_at(cut).cloned() {
                split(&mut b, c.id, cut, &mut links)?;
            }
        }
        let covered: Vec<ClipId> = track(b.project(), seq, *t)?
            .clips
            .iter()
            .filter(|c| c.timeline_start >= at && c.timeline_range().end() <= end)
            .map(|c| c.id)
            .collect();
        for c in covered {
            b.push(Command::RemoveClip { clip: c })?;
        }
        let mut c = (**clip).clone();
        c.timeline_start = at;
        b.push(Command::AddClip { sequence: seq, track: *t, clip: Arc::new(c) })?;
    }
    b.finish("Overwrite")
}

/// Delete without closing the gap.
pub fn lift(p: &Project, clips: &[ClipId], follow_links: bool) -> Result<Command, CommandError> {
    let mut b = Builder::new(p);
    for c in with_linked(p, clips, follow_links) {
        b.push(Command::RemoveClip { clip: c })?;
    }
    b.finish("Delete")
}

/// Edges worth snapping to: clip starts and ends on every track (except the
/// clips being dragged), the playhead, and zero.
pub fn snap_points(seq: &Sequence, except: &[ClipId], playhead: Time) -> Vec<Time> {
    let mut v = vec![Time::ZERO, playhead];
    for t in &seq.tracks {
        for c in &t.clips {
            if !except.contains(&c.id) {
                v.push(c.timeline_start);
                v.push(c.timeline_range().end());
            }
        }
    }
    v.sort();
    v.dedup();
    v
}

#[cfg(test)]
mod tests;
