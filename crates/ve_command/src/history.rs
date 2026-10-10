//! Undo and redo. The history stores inverse commands, not copies of the
//! project, so it costs what the edits cost.

use crate::{Applied, Command, CommandError};
use ve_model::Project;

#[derive(Clone, Debug)]
pub struct HistoryEntry {
    pub label: String,
    /// What to apply to go the other way.
    pub command: Command,
}

#[derive(Default, Debug)]
pub struct History {
    undo: Vec<HistoryEntry>,
    redo: Vec<HistoryEntry>,
    /// Undo depth at the last save, to answer "has unsaved changes".
    saved_at: Option<usize>,
    /// The key of the gesture the top undo entry belongs to, while it may
    /// still grow ([`History::execute_merging`]).
    merging: Option<u64>,
}

impl History {
    /// Apply `cmd` to `p` and record it.
    pub fn execute(&mut self, p: &Project, cmd: Command) -> Result<Project, CommandError> {
        self.merging = None;
        self.record(p, cmd)
    }

    /// Apply `cmd` as part of the gesture `key` (a slider drag): the first
    /// command of a gesture is recorded as usual, and the rest join it, so
    /// undo goes back to before the gesture in one step. Each gesture needs
    /// its own key.
    pub fn execute_merging(&mut self, p: &Project, cmd: Command, key: u64) -> Result<Project, CommandError> {
        if self.merging == Some(key) && !self.undo.is_empty() && self.redo.is_empty() {
            // The first command's inverse already restores the state before
            // the gesture.
            return Ok(cmd.apply(p)?.project);
        }
        let project = self.record(p, cmd)?;
        self.merging = Some(key);
        Ok(project)
    }

    fn record(&mut self, p: &Project, cmd: Command) -> Result<Project, CommandError> {
        let label = cmd.label();
        let Applied { project, inverse } = cmd.apply(p)?;
        self.undo.push(HistoryEntry { label, command: inverse });
        if self.saved_at.is_some_and(|d| d >= self.undo.len()) {
            self.saved_at = None; // the saved state can no longer be reached by redo
        }
        self.redo.clear();
        Ok(project)
    }

    pub fn undo(&mut self, p: &Project) -> Option<Result<Project, CommandError>> {
        self.merging = None;
        let e = self.undo.pop()?;
        Some(e.command.apply(p).map(|a| {
            self.redo.push(HistoryEntry { label: e.label, command: a.inverse });
            a.project
        }))
    }

    pub fn redo(&mut self, p: &Project) -> Option<Result<Project, CommandError>> {
        self.merging = None;
        let e = self.redo.pop()?;
        Some(e.command.apply(p).map(|a| {
            self.undo.push(HistoryEntry { label: e.label, command: a.inverse });
            a.project
        }))
    }

    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|e| e.label.as_str())
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|e| e.label.as_str())
    }

    pub fn mark_saved(&mut self) {
        self.saved_at = Some(self.undo.len());
    }

    pub fn is_dirty(&self) -> bool {
        self.saved_at != Some(self.undo.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TrackState;
    use std::sync::Arc;
    use ve_model::*;

    fn fader(p: &Project, db: f32) -> Command {
        let seq = p.sequences.values().next().unwrap();
        let t = &seq.tracks[0];
        Command::SetTrackState { sequence: seq.id, track: t.id, state: TrackState { volume_db: db, ..TrackState::of(t) } }
    }

    fn volume(p: &Project) -> f32 {
        p.sequences.values().next().unwrap().tracks[0].volume_db
    }

    #[test]
    fn a_gesture_is_one_undo_step() {
        let mut p = Project::new("t");
        let seq = Sequence::new("s", SequenceFormat::default(), [Track::new(TrackKind::Audio, "A1")]);
        p.sequences.insert(seq.id, Arc::new(seq));
        let mut h = History::default();
        // One drag: three moves, one undo step back to the start.
        for db in [-1.0, -3.0, -6.0] {
            p = h.execute_merging(&p, fader(&p, db), 1).unwrap();
        }
        assert_eq!(volume(&p), -6.0);
        // The next drag is a step of its own.
        p = h.execute_merging(&p, fader(&p, -9.0), 2).unwrap();
        p = h.undo(&p).unwrap().unwrap();
        assert_eq!(volume(&p), -6.0);
        p = h.undo(&p).unwrap().unwrap();
        assert_eq!(volume(&p), 0.0, "the whole first drag undone at once");
        assert!(h.undo(&p).is_none());
        // After an undo, the same key starts afresh rather than joining.
        p = h.redo(&p).unwrap().unwrap();
        p = h.execute_merging(&p, fader(&p, -12.0), 1).unwrap();
        p = h.undo(&p).unwrap().unwrap();
        assert_eq!(volume(&p), -6.0);
    }
}
