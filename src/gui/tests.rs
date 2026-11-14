//! Headless UI workflow tests.
//!
//! These drive the real [`CadenceApp::frame`] with a bare `egui::Context` and
//! synthesised pointer/keyboard input — the same code path `eframe` runs in
//! the desktop app — so drawing, dragging, selection, undo, dirty tracking
//! and dialogs are exercised for real. `CadenceApp::headless` disables I/O:
//! no audio device, no native dialogs, no recovery/autosave writes, and the
//! pack poller stays off, so tests never touch the real user data dir.

use std::collections::BTreeSet;

use egui::{pos2, Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect};

use super::dialogs::Dialog;
use super::files::PendingAction;
use super::{arranger, ops, CadenceApp, StatusKind};
use crate::model::{Instrument, Note, Project, DRUMS};
use crate::packs::InstrumentPreset;

/// Pitch-row height used by the piano roll (mirrors `editor.rs` `ROW_H`).
const ROW_H: f32 = 16.0;

fn base_input() -> RawInput {
    RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1360.0, 860.0))),
        focused: true,
        ..Default::default()
    }
}

/// Runs `CadenceApp::frame` against a bare egui context with queued input.
struct Ui {
    ctx: egui::Context,
    app: CadenceApp,
    next: RawInput,
    time: f64,
}

impl Ui {
    fn new(app: CadenceApp) -> Self {
        Self {
            ctx: egui::Context::default(),
            app,
            next: base_input(),
            time: 0.0,
        }
    }

    fn demo() -> Self {
        Self::new(CadenceApp::headless(Project::demo()))
    }

    /// One full frame: queued input goes in, `frame()` runs, input resets.
    fn step(&mut self) {
        self.next.time = Some(self.time);
        let input = std::mem::replace(&mut self.next, base_input());
        let _ = self.ctx.run(input, |ctx| self.app.frame(ctx));
        self.time += 1.0 / 60.0;
    }

    fn steps(&mut self, n: usize) {
        for _ in 0..n {
            self.step();
        }
    }

    fn at(&mut self, pos: Pos2) {
        self.next.events.push(Event::PointerMoved(pos));
    }

    fn press(&mut self, pos: Pos2, button: PointerButton) {
        self.at(pos);
        self.next.events.push(Event::PointerButton {
            pos,
            button,
            pressed: true,
            modifiers: Modifiers::NONE,
        });
        self.step();
    }

    fn release(&mut self, pos: Pos2, button: PointerButton) {
        self.at(pos);
        self.next.events.push(Event::PointerButton {
            pos,
            button,
            pressed: false,
            modifiers: Modifiers::NONE,
        });
        self.step();
    }

    fn press_mod(&mut self, pos: Pos2, button: PointerButton, modifiers: Modifiers) {
        self.next.modifiers = modifiers;
        self.at(pos);
        self.next.events.push(Event::PointerButton {
            pos,
            button,
            pressed: true,
            modifiers,
        });
        self.step();
    }

    fn release_mod(&mut self, pos: Pos2, button: PointerButton, modifiers: Modifiers) {
        self.next.modifiers = modifiers;
        self.at(pos);
        self.next.events.push(Event::PointerButton {
            pos,
            button,
            pressed: false,
            modifiers,
        });
        self.step();
    }

    fn down(&mut self, pos: Pos2) {
        self.press(pos, PointerButton::Primary);
    }

    fn up(&mut self, pos: Pos2) {
        self.release(pos, PointerButton::Primary);
    }

    fn click(&mut self, pos: Pos2) {
        self.down(pos);
        self.up(pos);
    }

    /// Two quick primary clicks; egui counts the second as a double click.
    fn dclick(&mut self, pos: Pos2) {
        self.click(pos);
        self.click(pos);
    }

    fn drag(&mut self, from: Pos2, to: Pos2) {
        self.down(from);
        self.at(to);
        self.step();
        self.up(to);
    }

    fn drag_mod(&mut self, from: Pos2, to: Pos2, modifiers: Modifiers) {
        self.press_mod(from, PointerButton::Primary, modifiers);
        self.next.modifiers = modifiers;
        self.at(to);
        self.step();
        self.release_mod(to, PointerButton::Primary, modifiers);
    }

    fn drag_out_release(&mut self, from: Pos2, through: Pos2, release: Pos2) {
        self.down(from);
        self.at(through);
        self.step();
        self.at(release);
        self.step();
        self.up(release);
    }

    fn key_mod(&mut self, key: Key, modifiers: Modifiers) {
        self.next.modifiers = modifiers;
        for pressed in [true, false] {
            self.next.events.push(Event::Key {
                key,
                physical_key: Some(key),
                pressed,
                repeat: false,
                modifiers,
            });
        }
        self.step();
    }

    fn key(&mut self, key: Key) {
        self.key_mod(key, Modifiers::NONE);
    }

    fn pattern(&self, track_idx: usize, pattern_idx: usize) -> &crate::model::Pattern {
        &self.app.project.tracks[track_idx].patterns[pattern_idx]
    }
}

/// Selects `track` and scrolls the piano roll so `pitch` is vertically centred.
fn show_pitch(ui: &mut Ui, track_id: u64, pitch: u8) {
    ui.app.select_track(track_id);
    ui.step();
    let (grid, _, _) = ui.app.editor_test_rects();
    let row = (127u32 - pitch as u32) as f32;
    let scroll = (row * ROW_H - grid.height() * 0.5).max(0.0);
    ui.app.set_roll_scroll(0.0, scroll);
    ui.step();
}

fn test_preset() -> InstrumentPreset {
    InstrumentPreset {
        id: "test-pack.glass".into(),
        name: "Glass Bell".into(),
        description: "headless test preset".into(),
        oscillators: vec![Default::default()],
        envelope: Default::default(),
        filter_cutoff: 8_000.0,
        bit_depth: 12,
        sample_hold: 2,
        gain: 0.8,
        monophonic: false,
    }
}

#[test]
fn renders_empty_and_demo_songs() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.steps(3);
    assert_eq!(
        ui.app.project.tracks.len(),
        1,
        "empty project ships a starter drum track"
    );

    let mut ui = Ui::demo();
    ui.steps(3);
    assert!(
        ui.app.project.tracks.len() > 1,
        "demo has a full arrangement"
    );
}

#[test]
fn add_track_marks_dirty_and_undoes() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    assert!(!ui.app.dirty(), "fresh headless project starts clean");
    ui.app.add_track(Instrument::Drums);
    ui.step();
    assert_eq!(
        ui.app.project.tracks.len(),
        2,
        "starter drum track plus the new one"
    );
    assert!(ui.app.dirty());
    ui.app.undo();
    ui.step();
    assert_eq!(ui.app.project.tracks.len(), 1);
    ui.app.redo();
    ui.step();
    assert_eq!(ui.app.project.tracks.len(), 2);
}

#[test]
fn drum_grid_click_paints_and_erases() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    // Project::empty() already contains a drum track with an empty 16-step pattern.
    let idx = 0;
    let id = ui.app.project.tracks[idx].id;
    ui.app.select_track(id);
    ui.step();

    let pos = ui.app.drum_cell_center(0, 4).expect("drum cell on screen");
    let pitch = DRUMS[0].0;
    assert!(ops::drum_hit(ui.pattern(idx, 0), pitch, 4).is_none());
    ui.click(pos);
    ui.step();
    assert!(
        ops::drum_hit(ui.pattern(idx, 0), pitch, 4).is_some(),
        "primary click paints a hit"
    );
    ui.click(pos);
    ui.step();
    assert!(
        ops::drum_hit(ui.pattern(idx, 0), pitch, 4).is_none(),
        "primary click on a hit erases it"
    );
    ui.app.undo();
    assert!(
        ops::drum_hit(ui.pattern(idx, 0), pitch, 4).is_some(),
        "undo restores the hit"
    );
}

#[test]
fn drum_drag_paints_a_row_and_shift_click_cycles_velocity() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    let idx = 0;
    let id = ui.app.project.tracks[idx].id;
    ui.app.select_track(id);
    ui.step();

    let pitch = DRUMS[1].0;
    // Drag across row 1: cells under the press and the drop get painted.
    let from = ui.app.drum_cell_center(1, 0).expect("cell");
    let to = ui.app.drum_cell_center(1, 8).expect("cell");
    ui.drag(from, to);
    assert!(ops::drum_hit(ui.pattern(idx, 0), pitch, 0).is_some());
    assert!(ops::drum_hit(ui.pattern(idx, 0), pitch, 8).is_some());

    // Shift+click on a hit cycles its velocity through the accent levels.
    let before = ops::drum_hit(ui.pattern(idx, 0), pitch, 0)
        .unwrap()
        .velocity;
    let cell = ui.app.drum_cell_center(1, 0).unwrap();
    self::click_with_mods(&mut ui, cell, Modifiers::SHIFT);
    let after = ops::drum_hit(ui.pattern(idx, 0), pitch, 0)
        .unwrap()
        .velocity;
    assert_ne!(before, after, "shift+click changed the velocity");
}

#[test]
fn piano_roll_draw_move_and_resize() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    ui.app.add_track(Instrument::Piano);
    ui.step();
    let idx = ui.app.project.tracks.len() - 1;
    let id = ui.app.project.tracks[idx].id;
    show_pitch(&mut ui, id, 61);

    // Draw a note at pitch 60, step 2 (snap 1.0, default length 2).
    let pos = ui.app.roll_cell_center(60, 2.0).expect("cell on screen");
    ui.click(pos);
    ui.step();
    let note_id = *ui
        .app
        .sel
        .notes
        .iter()
        .next()
        .expect("drawn note is selected");
    let pat = ui.pattern(idx, 0);
    let note = pat
        .notes
        .iter()
        .find(|n| n.id == note_id)
        .expect("drawn note");
    assert_eq!(note.pitch, 60);
    assert_eq!(note.start, 2.0);
    assert_eq!(note.duration, 2.0);

    // Drag the body to pitch 62, step 6.
    let dest = ui.app.roll_cell_center(62, 6.0).expect("cell on screen");
    ui.drag(pos, dest);
    ui.step();
    let pat = ui.pattern(idx, 0);
    let note = pat
        .notes
        .iter()
        .find(|n| n.id == note_id)
        .expect("note survives the drag");
    assert_eq!(note.pitch, 62, "drag transposed");
    assert_eq!(note.start, 6.0, "drag moved to the new step");

    // Resize: grab the right edge (x of start+duration) and pull two steps.
    let pat = ui.pattern(idx, 0);
    let note = pat.notes.iter().find(|n| n.id == note_id).unwrap();
    let edge_x = ui.app.roll_x(note.start + note.duration).expect("edge x") - 2.0;
    let edge = pos2(
        edge_x,
        ui.app
            .roll_cell_center(note.pitch, note.start + 0.5)
            .unwrap()
            .y,
    );
    let target_x = ui.app.roll_x(note.start + note.duration + 2.0).unwrap();
    ui.drag(edge, pos2(target_x, edge.y));
    ui.step();
    let pat = ui.pattern(idx, 0);
    let note = pat.notes.iter().find(|n| n.id == note_id).unwrap();
    assert_eq!(note.start, 6.0, "resize does not move the start");
    assert_eq!(note.duration, 4.0, "resize extended the note two steps");
}

#[test]
fn marquee_selects_notes_and_delete_removes_them() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    ui.app.add_track(Instrument::Piano);
    ui.step();
    let idx = ui.app.project.tracks.len() - 1;
    let id = ui.app.project.tracks[idx].id;
    show_pitch(&mut ui, id, 61);

    // Draw two notes on different pitches near the roll's home octave.
    for (pitch, step) in [(60u8, 0.0f64), (62, 4.0)] {
        let pos = ui.app.roll_cell_center(pitch, step).expect("cell visible");
        ui.click(pos);
        ui.step();
    }
    assert_eq!(ui.pattern(idx, 0).notes.len(), 2);

    // Shift-drag a box from the grid's top-left corner over both notes.
    let (grid, _, _) = ui.app.editor_test_rects();
    let a = ui.app.roll_cell_center(60, 0.0).unwrap();
    let b = ui.app.roll_cell_center(62, 4.0).unwrap();
    let tl = pos2(grid.left() + 2.0, grid.top() + 2.0);
    let br = pos2(b.x + 8.0, a.y + 10.0);
    ui.next.modifiers = Modifiers::SHIFT;
    ui.down(tl);
    ui.next.modifiers = Modifiers::SHIFT;
    ui.at(br);
    ui.step();
    ui.next.modifiers = Modifiers::SHIFT;
    ui.up(br);
    assert_eq!(ui.app.sel.notes.len(), 2, "marquee selected both notes");

    // Delete removes them; undo restores both and the selection stays valid.
    ui.key(Key::Delete);
    assert!(ui.pattern(idx, 0).notes.is_empty());
    ui.app.undo();
    assert_eq!(ui.pattern(idx, 0).notes.len(), 2);
}

#[test]
fn piano_plain_blank_marquee_no_note_no_dirty_and_click_draws() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    ui.app.add_track(Instrument::Piano);
    ui.step();
    let idx = ui.app.project.tracks.len() - 1;
    let id = ui.app.project.tracks[idx].id;
    show_pitch(&mut ui, id, 62);

    for (pitch, step) in [(60u8, 2.0), (62, 5.0)] {
        ui.click(ui.app.roll_cell_center(pitch, step).expect("cell"));
        ui.step();
    }
    ui.app.doc.saved_state = Some(ui.app.history.state());
    assert!(!ui.app.dirty());
    let before_notes = ui.pattern(idx, 0).notes.len();
    let before_state = ui.app.history.state();
    let a = ui.app.roll_cell_center(64, 0.0).unwrap();
    let b = ui.app.roll_cell_center(59, 7.0).unwrap();
    ui.drag(a, b);
    assert_eq!(ui.pattern(idx, 0).notes.len(), before_notes);
    assert_eq!(
        ui.app.sel.notes.len(),
        2,
        "plain blank drag box-selects notes"
    );
    assert_eq!(
        ui.app.history.state(),
        before_state,
        "selection is not an edit"
    );
    assert!(!ui.app.dirty());

    ui.click(ui.app.roll_cell_center(65, 8.0).unwrap());
    ui.step();
    assert_eq!(ui.pattern(idx, 0).notes.len(), before_notes + 1);
    assert!(ui.app.dirty(), "ordinary click still draws a note");
}

#[test]
fn piano_non_first_note_group_drag_preserves_intervals_and_clamps() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    ui.app.add_track(Instrument::Piano);
    ui.step();
    let idx = ui.app.project.tracks.len() - 1;
    let id = ui.app.project.tracks[idx].id;
    show_pitch(&mut ui, id, 60);
    {
        let pat = &mut ui.app.project.tracks[idx].patterns[0];
        pat.notes = vec![Note::new(60, 2.0, 2.0, 0.8), Note::new(64, 6.0, 2.0, 0.8)];
    }
    let ids: BTreeSet<u64> = ui.pattern(idx, 0).notes.iter().map(|n| n.id).collect();
    ui.app.sel.notes = ids;
    ui.app.focus = super::Focus::Editor;
    ui.step();

    let second_id = ui.pattern(idx, 0).notes[1].id;
    let from = ui.app.roll_cell_center(64, 6.0).unwrap();
    let to = ui.app.roll_cell_center(67, 10.0).unwrap();
    ui.drag(from, to);
    let pat = ui.pattern(idx, 0);
    let first = pat.notes.iter().find(|n| n.pitch == 63).unwrap();
    let second = pat.notes.iter().find(|n| n.id == second_id).unwrap();
    assert_eq!((first.start, second.start), (6.0, 10.0));
    assert_eq!(second.pitch, 67);
    assert_eq!(second.start - first.start, 4.0);
    assert_eq!(second.pitch as i32 - first.pitch as i32, 4);

    let from = ui.app.roll_cell_center(67, 10.0).unwrap();
    let to = pos2(from.x + 1_000.0, from.y);
    ui.drag_out_release(from, to, to);
    let pat = ui.pattern(idx, 0);
    let first = pat.notes.iter().find(|n| n.pitch == 63).unwrap();
    let second = pat.notes.iter().find(|n| n.id == second_id).unwrap();
    assert_eq!(
        (first.start, second.start),
        (11.0, 15.0),
        "common time delta clamps at the pattern end"
    );
    assert_eq!(second.start - first.start, 4.0);
}

#[test]
fn velocity_lane_sets_velocity_of_selected_notes() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    ui.app.add_track(Instrument::Piano);
    ui.step();
    let idx = ui.app.project.tracks.len() - 1;
    let id = ui.app.project.tracks[idx].id;
    show_pitch(&mut ui, id, 60);
    let pos = ui.app.roll_cell_center(60, 2.0).expect("cell");
    ui.click(pos);
    ui.step();
    let nid = *ui.app.sel.notes.iter().next().expect("drawn note selected");

    let (_grid, vel, _ruler) = ui.app.editor_test_rects();
    assert!(
        vel.width() > 10.0 && vel.height() > 8.0,
        "velocity lane laid out"
    );
    let x = ui.app.roll_x(2.5).unwrap();
    let y = vel.top() + vel.height() * 0.25;
    ui.click(pos2(x, y));
    let v = ui
        .pattern(idx, 0)
        .notes
        .iter()
        .find(|n| n.id == nid)
        .unwrap()
        .velocity;
    assert!(
        (v - 0.75).abs() < 0.12,
        "click at 25% from the top sets ~0.75 velocity, got {v}"
    );
}

#[test]
fn arrow_keys_nudge_notes() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    ui.app.add_track(Instrument::Piano);
    ui.step();
    let idx = ui.app.project.tracks.len() - 1;
    let id = ui.app.project.tracks[idx].id;
    show_pitch(&mut ui, id, 60);
    let pos = ui.app.roll_cell_center(60, 2.0).expect("cell");
    ui.click(pos);
    ui.step();
    let nid = *ui.app.sel.notes.iter().next().expect("drawn note selected");

    ui.key(Key::ArrowRight);
    assert_eq!(
        ui.pattern(idx, 0)
            .notes
            .iter()
            .find(|n| n.id == nid)
            .unwrap()
            .start,
        3.0,
        "right arrow nudges by the grid snap"
    );
    ui.key(Key::ArrowUp);
    assert_eq!(
        ui.pattern(idx, 0)
            .notes
            .iter()
            .find(|n| n.id == nid)
            .unwrap()
            .pitch,
        61,
        "up arrow transposes a semitone"
    );
    ui.key_mod(Key::ArrowUp, Modifiers::SHIFT);
    assert_eq!(
        ui.pattern(idx, 0)
            .notes
            .iter()
            .find(|n| n.id == nid)
            .unwrap()
            .pitch,
        73,
        "shift+arrow transposes by an octave"
    );
}

#[test]
fn arranger_clip_drag_moves_within_bounds() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    let grid = ui.app.arranger.grid;
    assert!(grid.width() > 50.0, "arranger laid out");
    let pps = ui.app.arranger.px_per_step;
    // Demo track 0 clip 0 starts at 0; grab its middle at step 8.
    let clip = ui.app.project.tracks[0].clips[0].clone();
    let from = pos2(
        grid.left() + (clip.start as f32 + 8.0) * pps,
        grid.top() + arranger::LANE_H * 0.5,
    );
    ui.click(from);
    assert_eq!(ui.app.sel.clip, Some(clip.id), "click selects the clip");

    let to = pos2(from.x + 16.0 * pps, from.y);
    ui.drag(from, to);
    ui.step();
    let clip = ui.app.project.tracks[0]
        .clips
        .iter()
        .find(|c| c.id == clip.id)
        .unwrap();
    assert_eq!(clip.start, 16.0, "clip snapped to the bar grid");
}

#[test]
fn arranger_marquee_group_drag_delete_undo_and_empty_click() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    ui.app.add_track(Instrument::Piano);
    ui.step();
    ui.app.arranger.snap = arranger::Snap::Step;
    let t0 = ui.app.project.tracks[0].id;
    let t1 = ui.app.project.tracks[1].id;
    let c0 = ui.app.project.tracks[0].clips[0].id;
    let c1 = ui.app.project.tracks[1].clips[0].id;
    ui.app.project.tracks[0].clips[0].start = 4.0;
    ui.app.project.tracks[1].clips[0].start = 20.0;
    ui.step();
    let grid = ui.app.arranger.grid;
    let pps = ui.app.arranger.px_per_step;
    let lane_y = |lane: usize| grid.top() + arranger::LANE_H * (lane as f32 + 0.5);
    let at = |step: f64, lane: usize| pos2(grid.left() + step as f32 * pps, lane_y(lane));

    ui.drag(at(48.0, 1), at(2.0, 0));
    assert_eq!(ui.app.selected_clip_ids(), BTreeSet::from([c0, c1]));

    let mut extra = None;
    ui.app.edit("test clip", None, |p| {
        extra = ops::add_clip(p, t0, p.tracks[0].patterns[0].id, 60.0)
    });
    let c2 = extra.unwrap();
    ui.step();
    ui.drag_mod(at(78.0, 0), at(58.0, 0), Modifiers::SHIFT);
    assert_eq!(ui.app.selected_clip_ids(), BTreeSet::from([c0, c1, c2]));

    let from = at(28.0, 1);
    let to = at(40.0, 1);
    ui.drag(from, to);
    assert_eq!(
        ui.app.project.tracks[0]
            .clips
            .iter()
            .find(|c| c.id == c0)
            .unwrap()
            .start,
        16.0
    );
    assert_eq!(
        ui.app.project.tracks[1]
            .clips
            .iter()
            .find(|c| c.id == c1)
            .unwrap()
            .start,
        32.0
    );
    assert_eq!(
        ui.app.project.tracks[0]
            .clips
            .iter()
            .find(|c| c.id == c2)
            .unwrap()
            .start,
        72.0
    );

    let from = at(16.0, 0);
    let to = at(-40.0, 0);
    ui.drag_out_release(from, to, to);
    assert_eq!(
        ui.app.project.tracks[0]
            .clips
            .iter()
            .find(|c| c.id == c0)
            .unwrap()
            .start,
        0.0
    );
    assert_eq!(
        ui.app.project.tracks[1]
            .clips
            .iter()
            .find(|c| c.id == c1)
            .unwrap()
            .start,
        16.0
    );
    assert_eq!(
        ui.app.project.tracks[0]
            .clips
            .iter()
            .find(|c| c.id == c2)
            .unwrap()
            .start,
        56.0
    );

    ui.key(Key::Delete);
    assert!(ui.app.project.tracks[0]
        .clips
        .iter()
        .all(|c| c.id != c0 && c.id != c2));
    assert!(ui.app.project.tracks[1].clips.iter().all(|c| c.id != c1));
    assert!(ui.app.selected_clip_ids().is_empty());
    ui.app.undo();
    ui.step();
    assert!(ui.app.project.tracks[0].clips.iter().any(|c| c.id == c0));
    assert!(ui.app.project.tracks[1].clips.iter().any(|c| c.id == c1));

    ui.click(at(100.0, 0));
    assert!(
        ui.app.selected_clip_ids().is_empty(),
        "empty click clears arranger selection"
    );
    ui.app.sel.track = Some(t1);
    ui.app.focus = super::Focus::Arranger;
    ui.key_mod(Key::A, Modifiers::COMMAND);
    assert_eq!(
        ui.app.selected_clip_ids().len(),
        3,
        "Cmd/Ctrl+A selects arranger clips when arranger has focus"
    );
}

#[test]
fn arranger_alt_group_copy_preserves_metadata_and_cap_failure_is_atomic() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    ui.app.add_track(Instrument::Piano);
    ui.step();
    ui.app.arranger.snap = arranger::Snap::Step;
    let c0 = ui.app.project.tracks[0].clips[0].id;
    let c1 = ui.app.project.tracks[1].clips[0].id;
    let p0 = ui.app.project.tracks[0].clips[0].pattern_id;
    let p1 = ui.app.project.tracks[1].clips[0].pattern_id;
    ui.app.project.tracks[0].clips[0].start = 4.0;
    ui.app.project.tracks[0].clips[0].loops = 3;
    ui.app.project.tracks[0].clips[0].fade_in = 2.0;
    ui.app.project.tracks[0].clips[0].fade_out = 1.0;
    ui.app.project.tracks[1].clips[0].start = 20.0;
    ui.app.project.tracks[1].clips[0].loops = 2;
    ui.app.project.tracks[1].clips[0].fade_in = 1.0;
    ui.app.project.tracks[1].clips[0].fade_out = 3.0;
    ui.app
        .set_clip_selection(BTreeSet::from([c0, c1]), Some(c1));
    ui.app.focus = super::Focus::Arranger;
    ui.step();
    let grid = ui.app.arranger.grid;
    let pps = ui.app.arranger.px_per_step;
    let lane_y = |lane: usize| grid.top() + arranger::LANE_H * (lane as f32 + 0.5);
    let at = |step: f64, lane: usize| pos2(grid.left() + step as f32 * pps, lane_y(lane));

    ui.drag_mod(at(28.0, 1), at(36.0, 1), Modifiers::ALT);
    let new_ids = ui.app.selected_clip_ids();
    assert_eq!(new_ids.len(), 2);
    assert!(new_ids.is_disjoint(&BTreeSet::from([c0, c1])));
    assert_eq!(
        ui.app.project.tracks[0]
            .clips
            .iter()
            .find(|c| c.id == c0)
            .unwrap()
            .start,
        4.0
    );
    assert_eq!(
        ui.app.project.tracks[1]
            .clips
            .iter()
            .find(|c| c.id == c1)
            .unwrap()
            .start,
        20.0
    );
    let copy0 = ui.app.project.tracks[0]
        .clips
        .iter()
        .find(|c| c.id != c0)
        .unwrap();
    let copy1 = ui.app.project.tracks[1]
        .clips
        .iter()
        .find(|c| c.id != c1)
        .unwrap();
    assert_eq!(
        (
            copy0.pattern_id,
            copy0.start,
            copy0.loops,
            copy0.fade_in,
            copy0.fade_out
        ),
        (p0, 12.0, 3, 2.0, 1.0)
    );
    assert_eq!(
        (
            copy1.pattern_id,
            copy1.start,
            copy1.loops,
            copy1.fade_in,
            copy1.fade_out
        ),
        (p1, 28.0, 2, 1.0, 3.0)
    );

    let before_counts = (
        ui.app.project.tracks[0].clips.len(),
        ui.app.project.tracks[1].clips.len(),
    );
    let originals = vec![(
        ui.app.project.tracks[0].id,
        ui.app.project.tracks[0].clips[0].clone(),
    )];
    ui.app.edit("fill cap", None, |p| {
        let tid = p.tracks[0].id;
        let pid = p.tracks[0].patterns[0].id;
        while p.tracks[0].clips.len() < ops::MAX_CLIPS_PER_TRACK {
            let start = p.tracks[0].clips.len() as f64 % 120.0;
            ops::add_clip(p, tid, pid, start);
        }
    });
    let full_count = ui.app.project.tracks[0].clips.len();
    let failed = ops::copy_clip_group(&mut ui.app.project, &originals, 4.0);
    assert!(failed.is_none());
    assert_eq!(
        ui.app.project.tracks[0].clips.len(),
        full_count,
        "failed group copy adds nothing to full track"
    );
    assert_eq!(
        ui.app.project.tracks[1].clips.len(),
        before_counts.1,
        "failed group copy leaves other tracks untouched"
    );
}

#[test]
fn arranger_shift_click_toggles_members_and_delete_hits_only_remaining() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    ui.app.add_track(Instrument::Piano);
    ui.step();
    ui.app.arranger.snap = arranger::Snap::Step;
    let c0 = ui.app.project.tracks[0].clips[0].id;
    let c1 = ui.app.project.tracks[1].clips[0].id;
    let grid = ui.app.arranger.grid;
    let pps = ui.app.arranger.px_per_step;
    let lane_y = |lane: usize| grid.top() + arranger::LANE_H * (lane as f32 + 0.5);
    let at = |step: f64, lane: usize| pos2(grid.left() + step as f32 * pps, lane_y(lane));

    // Plain click on an unselected clip selects only it.
    ui.click(at(8.0, 0));
    assert_eq!(ui.app.selected_clip_ids(), BTreeSet::from([c0]));

    // Shift-click adds the second clip; the clicked clip becomes primary.
    click_with_mods(&mut ui, at(8.0, 1), Modifiers::SHIFT);
    assert_eq!(ui.app.selected_clip_ids(), BTreeSet::from([c0, c1]));
    assert_eq!(ui.app.sel.clip, Some(c1));

    // Shift-click the primary removes it; primary falls back to a member.
    click_with_mods(&mut ui, at(8.0, 1), Modifiers::SHIFT);
    assert_eq!(ui.app.selected_clip_ids(), BTreeSet::from([c0]));
    assert_eq!(ui.app.sel.clips, BTreeSet::from([c0]));
    assert_eq!(ui.app.sel.clip, Some(c0));

    // Shift-click the last clip empties the selection entirely.
    click_with_mods(&mut ui, at(8.0, 0), Modifiers::SHIFT);
    assert!(ui.app.selected_clip_ids().is_empty());
    assert!(ui.app.sel.clips.is_empty());
    assert!(ui.app.sel.clip.is_none());

    // Box-select across both lanes, then Shift-click away the non-primary.
    ui.drag(at(48.0, 1), at(2.0, 0));
    assert_eq!(ui.app.selected_clip_ids(), BTreeSet::from([c0, c1]));
    let primary = ui.app.sel.clip.expect("marquee sets a primary");
    let other = if primary == c0 { c1 } else { c0 };
    let other_lane = if other == c0 { 0 } else { 1 };
    click_with_mods(&mut ui, at(8.0, other_lane), Modifiers::SHIFT);
    assert_eq!(ui.app.selected_clip_ids(), BTreeSet::from([primary]));
    assert_eq!(ui.app.sel.clip, Some(primary));

    // Delete removes only the remaining selected clip.
    ui.key(Key::Delete);
    assert!(ui
        .app
        .project
        .tracks
        .iter()
        .all(|t| t.clips.iter().all(|c| c.id != primary)));
    assert!(ui
        .app
        .project
        .tracks
        .iter()
        .any(|t| t.clips.iter().any(|c| c.id == other)));
    ui.app.undo();
    ui.step();
    assert!(ui
        .app
        .project
        .tracks
        .iter()
        .any(|t| t.clips.iter().any(|c| c.id == primary)));

    // Re-select both, drop one again, and a group drag moves only the rest.
    ui.drag(at(48.0, 1), at(2.0, 0));
    assert_eq!(ui.app.selected_clip_ids(), BTreeSet::from([c0, c1]));
    let primary = ui.app.sel.clip.expect("marquee sets a primary");
    let other = if primary == c0 { c1 } else { c0 };
    let other_lane = if other == c0 { 0 } else { 1 };
    let primary_lane = if primary == c0 { 0 } else { 1 };
    click_with_mods(&mut ui, at(8.0, other_lane), Modifiers::SHIFT);
    assert_eq!(ui.app.selected_clip_ids(), BTreeSet::from([primary]));
    ui.drag(at(8.0, primary_lane), at(24.0, primary_lane));
    let start_of = |id: u64| {
        ui.app
            .project
            .tracks
            .iter()
            .flat_map(|t| &t.clips)
            .find(|c| c.id == id)
            .unwrap()
            .start
    };
    assert_eq!(start_of(primary), 16.0, "remaining clip moved");
    assert_eq!(start_of(other), 0.0, "deselected clip stayed put");
}

#[test]
fn arranger_group_drag_multi_frame_move_is_one_undo() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    ui.app.add_track(Instrument::Piano);
    ui.step();
    ui.app.arranger.snap = arranger::Snap::Step;
    let c0 = ui.app.project.tracks[0].clips[0].id;
    let c1 = ui.app.project.tracks[1].clips[0].id;
    ui.app
        .set_clip_selection(BTreeSet::from([c0, c1]), Some(c1));
    ui.app.focus = super::Focus::Arranger;
    ui.step();
    let grid = ui.app.arranger.grid;
    let pps = ui.app.arranger.px_per_step;
    let lane_y = |lane: usize| grid.top() + arranger::LANE_H * (lane as f32 + 0.5);
    let at = |step: f64, lane: usize| pos2(grid.left() + step as f32 * pps, lane_y(lane));
    let original = ui.app.project.clone();

    ui.down(at(8.0, 1));
    ui.at(at(20.0, 1));
    ui.step();
    ui.at(at(28.0, 1));
    ui.step();
    ui.up(at(28.0, 1));

    let start_of = |id: u64| {
        ui.app
            .project
            .tracks
            .iter()
            .flat_map(|t| &t.clips)
            .find(|c| c.id == id)
            .unwrap()
            .start
    };
    assert_eq!(start_of(c0), 20.0);
    assert_eq!(start_of(c1), 20.0);
    let moved = ui.app.project.clone();
    ui.app.undo();
    ui.step();
    assert_eq!(ui.app.project, original, "one undo restores the song");
    ui.app.redo();
    ui.step();
    assert_eq!(ui.app.project, moved, "redo restores the final drag");
}

#[test]
fn arranger_alt_group_drag_multi_frame_is_one_undo() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    ui.app.add_track(Instrument::Piano);
    ui.step();
    ui.app.arranger.snap = arranger::Snap::Step;
    let c0 = ui.app.project.tracks[0].clips[0].id;
    let c1 = ui.app.project.tracks[1].clips[0].id;
    let p0 = ui.app.project.tracks[0].clips[0].pattern_id;
    let p1 = ui.app.project.tracks[1].clips[0].pattern_id;
    ui.app.project.tracks[0].clips[0].loops = 3;
    ui.app.project.tracks[0].clips[0].fade_in = 2.0;
    ui.app.project.tracks[0].clips[0].fade_out = 1.0;
    ui.app.project.tracks[1].clips[0].loops = 2;
    ui.app.project.tracks[1].clips[0].fade_in = 1.0;
    ui.app.project.tracks[1].clips[0].fade_out = 3.0;
    ui.app
        .set_clip_selection(BTreeSet::from([c0, c1]), Some(c1));
    ui.app.focus = super::Focus::Arranger;
    ui.step();
    let grid = ui.app.arranger.grid;
    let pps = ui.app.arranger.px_per_step;
    let lane_y = |lane: usize| grid.top() + arranger::LANE_H * (lane as f32 + 0.5);
    let at = |step: f64, lane: usize| pos2(grid.left() + step as f32 * pps, lane_y(lane));
    let original = ui.app.project.clone();

    // Alt-drag with two motion frames: the first creates the copies (fresh
    // ids), the second keeps moving them — still one undo gesture.
    ui.press_mod(at(8.0, 1), PointerButton::Primary, Modifiers::ALT);
    ui.next.modifiers = Modifiers::ALT;
    ui.at(at(20.0, 1));
    ui.step();
    ui.next.modifiers = Modifiers::ALT;
    ui.at(at(28.0, 1));
    ui.step();
    ui.release_mod(at(28.0, 1), PointerButton::Primary, Modifiers::ALT);

    let clip_of = |id: u64| {
        ui.app
            .project
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter())
            .find(|c| c.id == id)
            .unwrap()
            .clone()
    };
    assert_eq!(clip_of(c0).start, 0.0, "originals stay put");
    assert_eq!(clip_of(c1).start, 0.0, "originals stay put");
    assert_eq!(ui.app.project.tracks[0].clips.len(), 2);
    assert_eq!(ui.app.project.tracks[1].clips.len(), 2);
    let copies = ui.app.selected_clip_ids();
    assert_eq!(copies.len(), 2);
    assert!(copies.is_disjoint(&BTreeSet::from([c0, c1])));
    let copy0 = ui
        .app
        .project
        .tracks
        .iter()
        .flat_map(|t| &t.clips)
        .find(|c| copies.contains(&c.id) && c.pattern_id == p0)
        .unwrap();
    let copy1 = ui
        .app
        .project
        .tracks
        .iter()
        .flat_map(|t| &t.clips)
        .find(|c| copies.contains(&c.id) && c.pattern_id == p1)
        .unwrap();
    for (copy, want_loops, want_fi, want_fo) in [(copy0, 3, 2.0, 1.0), (copy1, 2, 1.0, 3.0)] {
        assert_eq!(
            (copy.start, copy.loops, copy.fade_in, copy.fade_out),
            (20.0, want_loops, want_fi, want_fo),
            "copy lands at the final offset with metadata preserved"
        );
    }

    let copied = ui.app.project.clone();
    ui.app.undo();
    ui.step();
    assert_eq!(
        ui.app.project, original,
        "one undo removes every copy and restores the whole project"
    );
    ui.app.redo();
    ui.step();
    assert_eq!(ui.app.project, copied, "redo restores the final copies");
}

#[test]
fn arranger_double_click_on_empty_lane_places_a_clip() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    ui.app.add_track(Instrument::Piano);
    ui.step();
    let grid = ui.app.arranger.grid;
    let pps = ui.app.arranger.px_per_step;
    // Lane 0 has a clip at bar 1 (start 0, length 16); double-click at bar 2.
    let pos = pos2(
        grid.left() + 16.5 * pps,
        grid.top() + arranger::LANE_H * 0.5,
    );
    ui.dclick(pos);
    let track = &ui.app.project.tracks[0];
    assert!(
        track.clips.iter().any(|c| c.start == 16.0),
        "double-click places a clip snapped to the bar"
    );
    assert!(ui.app.project.bars >= 2, "song has room for the new clip");
}

#[test]
fn clip_loops_extend_span_and_undo() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    let track = &ui.app.project.tracks[0];
    let (tid, cid) = (track.id, track.clips[0].id);
    assert_eq!(track.clips[0].loops, 1, "fresh clip plays once");

    ui.app.set_clip_loops(tid, cid, 4);
    ui.step();
    let (loops, len) = {
        let track = &ui.app.project.tracks[0];
        let clip = track.clips.iter().find(|c| c.id == cid).unwrap();
        (clip.loops, ops::clip_len(track, clip))
    };
    assert_eq!(loops, 4);
    assert_eq!(
        len, 64.0,
        "clip_len is the looped span (16-step pattern × 4)"
    );
    assert!(
        ui.app.project.bars as f64 * 16.0 >= 64.0,
        "song still covers the whole clip"
    );

    ui.app.undo();
    ui.step();
    let (loops, len) = {
        let track = &ui.app.project.tracks[0];
        let clip = track.clips.iter().find(|c| c.id == cid).unwrap();
        (clip.loops, ops::clip_len(track, clip))
    };
    assert_eq!(loops, 1, "undo restores the single play-through");
    assert_eq!(len, 16.0);
}

#[test]
fn clip_fade_corner_drag() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    ui.app.arranger.snap = arranger::Snap::Step;
    let grid = ui.app.arranger.grid;
    let pps = ui.app.arranger.px_per_step;
    let clip = ui.app.project.tracks[0].clips[0].clone();
    assert_eq!(clip.start, 0.0);
    // Lane top band centre: clip rect starts at lane_top+4, band is 15 px tall.
    let lane_top = grid.top();
    let band_y = lane_top + 11.0;
    // Left fade zone: within 14 px of the clip's left edge, inside the band.
    let from = pos2(grid.left() + 6.0, band_y);
    ui.down(from);
    assert_eq!(
        ui.app.sel.clip,
        Some(clip.id),
        "pressing a fade handle selects the clip"
    );
    // Drag six steps right: fade-in should become ~6 steps.
    let to = pos2(grid.left() + 6.0 * pps, band_y);
    ui.at(to);
    ui.step();
    ui.up(to);
    ui.step();
    let (fade_in, fade_out) = {
        let clip = ui.app.project.tracks[0]
            .clips
            .iter()
            .find(|c| c.id == clip.id)
            .unwrap();
        (clip.fade_in, clip.fade_out)
    };
    assert!(
        (fade_in - 6.0).abs() < 0.6,
        "fade-in follows the pointer, got {fade_in}"
    );
    assert_eq!(fade_out, 0.0, "the other fade is untouched");

    ui.app.undo();
    ui.step();
    let fade_in = ui.app.project.tracks[0]
        .clips
        .iter()
        .find(|c| c.id == clip.id)
        .unwrap()
        .fade_in;
    assert_eq!(fade_in, 0.0, "undo clears the fade");
}

#[test]
fn clip_loop_edge_drag() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    ui.app.arranger.snap = arranger::Snap::Step;
    let grid = ui.app.arranger.grid;
    let pps = ui.app.arranger.px_per_step;
    let clip = ui.app.project.tracks[0].clips[0].clone();
    assert_eq!(clip.loops, 1);
    // Pattern is 16 steps, so the clip's right edge sits at 16*pps.
    let x1 = grid.left() + 16.0 * pps;
    // Just inside the right edge, below the 15 px top band.
    let from = pos2(x1 - 3.0, grid.top() + arranger::LANE_H * 0.5);
    ui.down(from);
    assert_eq!(ui.app.sel.clip, Some(clip.id));
    // Drag one full pattern length to the right: loops should go 1 -> 2.
    let to = pos2(x1 - 3.0 + 16.0 * pps, from.y);
    ui.at(to);
    ui.step();
    ui.up(to);
    ui.step();
    let (loops, len) = {
        let track = &ui.app.project.tracks[0];
        let clip = track.clips.iter().find(|c| c.id == clip.id).unwrap();
        (clip.loops, ops::clip_len(track, clip))
    };
    assert_eq!(loops, 2, "one pattern length of travel adds a loop");
    assert_eq!(len, 32.0);

    ui.app.undo();
    ui.step();
    let loops = ui.app.project.tracks[0]
        .clips
        .iter()
        .find(|c| c.id == clip.id)
        .unwrap()
        .loops;
    assert_eq!(loops, 1, "undo restores the loop count");
}

#[test]
fn unsaved_guard_blocks_then_discards() {
    let mut ui = Ui::demo();
    ui.step();
    ui.app.add_track(Instrument::Lead);
    ui.step();
    ui.app.request(PendingAction::New);
    assert!(
        matches!(ui.app.dialog, Dialog::Unsaved(PendingAction::New)),
        "dirty song asks first"
    );
    ui.app.dialog_escape();
    assert!(matches!(ui.app.dialog, Dialog::None));
    ui.app.request(PendingAction::New);
    ui.app.run_pending_action(PendingAction::New);
    assert_eq!(
        ui.app.project.tracks.len(),
        1,
        "discard started a new song (starter drum track)"
    );
    assert!(!ui.app.dirty());
}

#[test]
fn escape_closes_help_dialog() {
    let mut ui = Ui::demo();
    ui.step();
    ui.app.dialog = Dialog::Help;
    ui.step();
    assert!(matches!(ui.app.dialog, Dialog::Help));
    ui.key(Key::Escape);
    assert!(
        matches!(ui.app.dialog, Dialog::None),
        "Escape dismissed help"
    );
}

#[test]
fn space_warns_without_audio_but_keeps_editing() {
    let mut ui = Ui::demo();
    ui.step();
    ui.key(Key::Space);
    assert!(!ui.app.transport.playing, "no engine => no playback");
    let status = ui.app.status.as_ref().expect("offline warning shown");
    assert!(matches!(status.kind, StatusKind::Warning));
}

#[test]
fn preset_track_is_embedded_and_portable() {
    let mut ui = Ui::demo();
    let before = ui.app.project.tracks.len();
    ui.app.add_preset_track(&test_preset());
    ui.step();
    assert_eq!(ui.app.project.tracks.len(), before + 1);
    let t = ui.app.project.tracks.last().unwrap();
    assert_eq!(t.instrument, Instrument::Lead);
    assert_eq!(t.name, "Glass Bell");
    assert_eq!(t.sound_name(), "Glass Bell");
    assert_eq!(t.preset.as_ref().unwrap().id, "test-pack.glass");
    assert!(
        ui.app.sel.track == Some(t.id),
        "new preset track is selected"
    );

    // The preset travels inside the song: a JSON round-trip keeps it.
    let json = crate::persistence::project_to_json(&ui.app.project).unwrap();
    let back = crate::persistence::project_from_json(&json).unwrap();
    let t = back.tracks.last().unwrap();
    assert_eq!(t.preset.as_ref().unwrap().id, "test-pack.glass");
    assert_eq!(t.sound_name(), "Glass Bell");
}

#[test]
fn save_and_open_roundtrip() {
    let mut ui = Ui::demo();
    ui.step();
    let dir = std::env::temp_dir();
    let path = dir.join("cadence-gui-roundtrip.cadence");
    crate::persistence::save_project(&ui.app.project, &path).unwrap();
    ui.app.open_path(path.clone());
    ui.step();
    assert_eq!(ui.app.doc.path.as_deref(), Some(path.as_path()));
    assert!(!ui.app.dirty(), "a just-opened song is clean");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn midi_export_import_roundtrip() {
    let mut ui = Ui::demo();
    ui.step();
    let path = std::env::temp_dir().join("cadence-gui-roundtrip.mid");
    ui.app.export_midi_file(path.clone());
    ui.step();
    ui.app.import_midi_file(path.clone());
    ui.step();
    assert!(
        !ui.app.project.tracks.is_empty(),
        "MIDI import produced tracks"
    );
    assert!(ui.app.dirty(), "an imported song is unsaved");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn wav_export_worker_completes() {
    let mut ui = Ui::demo();
    ui.step();
    let path = std::env::temp_dir().join("cadence-gui-export.wav");
    ui.app.start_wav_export(path.clone());
    assert!(ui.app.export.is_some());
    // The worker renders offline; poll frames until the job reports back.
    let mut finished = false;
    for _ in 0..3_000 {
        ui.step();
        if ui.app.export.is_none() {
            finished = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(finished, "WAV export finished");
    let meta = std::fs::metadata(&path).expect("wav written");
    assert!(meta.len() > 44, "wav has audio data");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn clip_delete_and_undo_restores_selection_state() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    let clip = ui.app.project.tracks[0].clips[0].clone();
    ui.app.sel.clip = Some(clip.id);
    ui.app.sel.track = Some(ui.app.project.tracks[0].id);
    let n_clips = ui.app.project.tracks[0].clips.len();
    ui.key(Key::Delete);
    assert_eq!(ui.app.project.tracks[0].clips.len(), n_clips - 1);
    assert!(
        ui.app.sel.clip.is_none(),
        "deleted clip leaves the selection"
    );
    ui.app.undo();
    assert_eq!(ui.app.project.tracks[0].clips.len(), n_clips);
    ui.step();
    // Selection repair keeps only ids that still exist.
    let sel_clip = ui.app.sel.clip;
    if let Some(id) = sel_clip {
        assert!(ui
            .app
            .project
            .tracks
            .iter()
            .flat_map(|t| &t.clips)
            .any(|c| c.id == id));
    }
}

/// Test-only click helper with held modifiers (drum accent painting).
fn click_with_mods(ui: &mut Ui, pos: Pos2, modifiers: Modifiers) {
    ui.next.modifiers = modifiers;
    ui.at(pos);
    ui.next.events.push(Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed: true,
        modifiers,
    });
    ui.step();
    ui.next.modifiers = modifiers;
    ui.next.events.push(Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed: false,
        modifiers,
    });
    ui.step();
}
