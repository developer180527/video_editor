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
}

impl History {
    /// Apply `cmd` to `p` and record it.
    pub fn execute(&mut self, p: &Project, cmd: Command) -> Result<Project, CommandError> {
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
        let e = self.undo.pop()?;
        Some(e.command.apply(p).map(|a| {
            self.redo.push(HistoryEntry { label: e.label, command: a.inverse });
            a.project
        }))
    }

    pub fn redo(&mut self, p: &Project) -> Option<Result<Project, CommandError>> {
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
