//! Snapshot-based undo/redo with edit coalescing and saved-state tracking.
//!
//! Every distinct project state gets a monotonically increasing id. The
//! document remembers the id it was last saved at, so "dirty" is simply
//! `history.state() != saved_state`, which stays correct across undo/redo.

use crate::model::Project;

const LIMIT: usize = 200;

#[derive(Debug, Clone)]
struct Snapshot {
    project: Project,
    state: u64,
    label: String,
}

#[derive(Debug)]
pub struct History {
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    group: Option<String>,
    state: u64,
    next_state: u64,
}

impl Default for History {
    fn default() -> Self {
        Self::new()
    }
}

impl History {
    pub fn new() -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            group: None,
            state: 1,
            next_state: 2,
        }
    }

    /// Id of the current project state.
    pub fn state(&self) -> u64 {
        self.state
    }

    /// Forget all history and start a fresh state id (new/open project).
    pub fn reset(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.group = None;
        self.bump();
    }

    fn bump(&mut self) {
        self.state = self.next_state;
        self.next_state += 1;
    }

    /// Records that the project changed from `before` to its new state.
    ///
    /// Consecutive edits that share the same `group` key (a drag, a slider
    /// gesture, typing in a field) collapse into a single undo step until
    /// [`History::seal`] is called or a different edit happens.
    pub fn record(&mut self, before: Project, label: &str, group: Option<&str>) {
        let coalesce = group.is_some() && self.group.as_deref() == group && !self.undo.is_empty();
        if !coalesce {
            self.undo.push(Snapshot {
                project: before,
                state: self.state,
                label: label.to_owned(),
            });
            if self.undo.len() > LIMIT {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
        self.group = group.map(str::to_owned);
        self.bump();
    }

    /// Ends the current coalescing gesture so the next edit is a new step.
    pub fn seal(&mut self) {
        self.group = None;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|s| s.label.as_str())
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|s| s.label.as_str())
    }

    /// Restores the previous state into `current`. Returns the undone label.
    pub fn undo(&mut self, current: &mut Project) -> Option<String> {
        let snap = self.undo.pop()?;
        self.group = None;
        let label = snap.label.clone();
        let project = std::mem::replace(current, snap.project);
        self.redo.push(Snapshot {
            project,
            state: self.state,
            label: label.clone(),
        });
        self.state = snap.state;
        Some(label)
    }

    /// Re-applies the most recently undone state. Returns its label.
    pub fn redo(&mut self, current: &mut Project) -> Option<String> {
        let snap = self.redo.pop()?;
        self.group = None;
        let label = snap.label.clone();
        let project = std::mem::replace(current, snap.project);
        self.undo.push(Snapshot {
            project,
            state: self.state,
            label: label.clone(),
        });
        self.state = snap.state;
        Some(label)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(name: &str) -> Project {
        let mut p = Project::empty();
        p.name = name.to_owned();
        p
    }

    #[test]
    fn undo_redo_round_trip_restores_states_and_ids() {
        let mut h = History::new();
        let mut p = named("a");
        let s0 = h.state();
        let before = p.clone();
        p.name = "b".into();
        h.record(before, "Rename", None);
        let s1 = h.state();
        assert_ne!(s0, s1);
        assert_eq!(h.undo_label(), Some("Rename"));
        assert_eq!(h.undo(&mut p).as_deref(), Some("Rename"));
        assert_eq!(p.name, "a");
        assert_eq!(h.state(), s0);
        assert!(h.can_redo());
        h.redo(&mut p);
        assert_eq!(p.name, "b");
        assert_eq!(h.state(), s1);
        assert!(!h.can_redo());
    }

    #[test]
    fn grouped_edits_coalesce_until_sealed() {
        let mut h = History::new();
        let mut p = named("x");
        for i in 0..5 {
            let before = p.clone();
            p.bpm = 100.0 + i as f64;
            h.record(before, "Tempo", Some("bpm"));
        }
        h.seal();
        let before = p.clone();
        p.bpm = 150.0;
        h.record(before, "Tempo", Some("bpm"));
        h.undo(&mut p);
        assert_eq!(p.bpm, 104.0);
        h.undo(&mut p);
        assert_eq!(p.bpm, Project::empty().bpm);
        assert!(!h.can_undo());
    }

    #[test]
    fn different_groups_do_not_coalesce_and_new_edit_clears_redo() {
        let mut h = History::new();
        let mut p = named("x");
        let b = p.clone();
        p.bpm = 90.0;
        h.record(b, "Tempo", Some("a"));
        let b = p.clone();
        p.swing = 0.2;
        h.record(b, "Swing", Some("b"));
        h.undo(&mut p);
        assert_eq!(p.swing, Project::empty().swing);
        let b = p.clone();
        p.name = "z".into();
        h.record(b, "Rename", None);
        assert!(!h.can_redo());
        assert_eq!(h.undo_label(), Some("Rename"));
    }

    #[test]
    fn saved_state_marker_tracks_dirty_through_undo() {
        let mut h = History::new();
        let mut p = named("x");
        let b = p.clone();
        p.bpm = 99.0;
        h.record(b, "Tempo", None);
        let saved = h.state();
        let b = p.clone();
        p.bpm = 98.0;
        h.record(b, "Tempo", None);
        assert_ne!(h.state(), saved);
        h.undo(&mut p);
        assert_eq!(h.state(), saved, "undo back to the saved state is clean");
        h.redo(&mut p);
        assert_ne!(h.state(), saved);
        h.reset();
        assert!(!h.can_undo() && !h.can_redo());
        assert_ne!(h.state(), saved);
    }

    #[test]
    fn history_is_bounded() {
        let mut h = History::new();
        let mut p = named("x");
        for i in 0..(LIMIT + 50) {
            let b = p.clone();
            p.bpm = 60.0 + i as f64 * 0.1;
            h.record(b, "Tempo", None);
        }
        let mut n = 0;
        while h.undo(&mut p).is_some() {
            n += 1;
        }
        assert_eq!(n, LIMIT);
    }
}
