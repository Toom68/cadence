//! Headless UI workflow tests.
//!
//! These drive the real [`CadenceApp::frame`] with a bare `egui::Context` and
//! synthesised pointer/keyboard input — the same code path `eframe` runs in
//! the desktop app — so drawing, dragging, selection, undo, dirty tracking
//! and dialogs are exercised for real. `CadenceApp::headless` disables I/O:
//! no audio device, no native dialogs, no recovery/autosave writes, and the
//! pack poller stays off, so tests never touch the real user data dir.

#[path = "../../examples/support/perf_ui.rs"]
mod perf_ui;

use std::collections::BTreeSet;

use eframe::Storage as _;
use egui::{pos2, Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect};

use super::dialogs::{ArpDraft, Dialog, NewSongDraft};
use super::files::PendingAction;
use super::sidebar::{
    audition_params, bank_options, gm_family, library_rows, preset_summary, LibraryBank, SideTab,
    QUICK_ADD_FONT_HINT_RECT, QUICK_ADD_FONT_RECT, QUICK_ADD_OPEN,
};
use super::theme::{self, ThemeChoice};
use super::{arranger, ops, CadenceApp, Clipboard, StatusKind};
use crate::font::FontInfo;
use crate::model::{ChordQuality, FontRef, Instrument, KeyMode, Note, Project, SongKey, DRUMS};
use crate::packs::{InstrumentPreset, KitPiece, KitSpec, KitVoice};

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

    /// A click whose press and release land in the *same* frame, like a real
    /// quick click. Rows wrapped in `dnd_drag_source` start their drag on
    /// press (egui treats drag-only widgets as dragged immediately), so a
    /// click split across two frames loses the pending click to the drag.
    fn quick_click(&mut self, pos: Pos2) {
        self.next.events.push(Event::PointerMoved(pos));
        for pressed in [true, false] {
            self.next.events.push(Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            });
        }
        self.step();
    }

    /// Two quick clicks in a row; egui counts the second as a double click.
    fn quick_dclick(&mut self, pos: Pos2) {
        self.quick_click(pos);
        self.quick_click(pos);
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

    /// Mouse-wheel scroll, `delta_y` in points (negative scrolls down).
    fn wheel(&mut self, delta_y: f32) {
        self.next.events.push(Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, delta_y),
            modifiers: Modifiers::NONE,
        });
        self.step();
    }

    /// Types text into the focused widget (e.g. the library search field).
    fn text(&mut self, s: &str) {
        self.next.events.push(Event::Text(s.to_owned()));
        self.step();
    }

    /// Queues one raw egui event and runs a frame — the shape egui-winit
    /// actually delivers for desktop clipboard commands.
    fn event(&mut self, ev: Event) {
        self.next.events.push(ev);
        self.step();
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
        pitch_envelope: None,
        filter_envelope: None,
        filter_cutoff: 8_000.0,
        filter_resonance: 0.0,
        lfo: None,
        bit_depth: 12,
        sample_hold: 2,
        gain: 0.8,
        monophonic: false,
        velocity_to_cutoff: crate::packs::DEFAULT_VELOCITY_TO_CUTOFF,
        spread: 0.0,
        hidden: false,
        audition_note: 55,
        audition_duration: 1.6,
        kit: None,
        sample: None,
    }
}

/// A preset variant with its own id/name/description and audition metadata.
fn preset_with(id: &str, name: &str, desc: &str, note: u8, secs: f32) -> InstrumentPreset {
    let mut p = test_preset();
    p.id = id.into();
    p.name = name.into();
    p.description = desc.into();
    p.audition_note = note;
    p.audition_duration = secs;
    p
}

fn kit_voice() -> KitVoice {
    KitVoice {
        oscillators: vec![Default::default()],
        ..Default::default()
    }
}

/// A three-piece drum-kit preset. Pieces are deliberately NOT in pitch order
/// (Hat@42, Kick@60, Clap@39) so tests can prove the grid follows the pieces
/// array order rather than sorting by pitch — and Kick@60 sits outside the
/// built-in DRUMS table, proving kit rows take arbitrary pitches. `audition_note` stays at the
/// serde default 60 so the kit audition falls back to the first piece.
fn test_kit_preset() -> InstrumentPreset {
    let mut p = preset_with("test-drums.machine", "Machine Kit", "kit", 60, 0.8);
    p.kit = Some(KitSpec {
        pieces: vec![
            KitPiece {
                name: "Hat".into(),
                pitch: 42,
                voice: kit_voice(),
            },
            KitPiece {
                name: "Kick".into(),
                pitch: 60,
                voice: kit_voice(),
            },
            KitPiece {
                name: "Clap".into(),
                pitch: 39,
                voice: kit_voice(),
            },
        ],
    });
    p
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
fn piano_alt_blank_marquee_no_note_no_dirty_and_click_draws() {
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
    ui.drag_mod(a, b, Modifiers::ALT);
    assert_eq!(ui.pattern(idx, 0).notes.len(), before_notes);
    assert_eq!(
        ui.app.sel.notes.len(),
        2,
        "alt+drag on blank grid box-selects notes"
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
fn piano_drag_on_empty_grid_draws_the_note_length() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    ui.app.add_track(Instrument::Piano);
    ui.step();
    let idx = ui.app.project.tracks.len() - 1;
    let id = ui.app.project.tracks[idx].id;
    show_pitch(&mut ui, id, 60);
    let before = ui.app.history.state();

    // Press on step 2 and drag to the middle of step 7: the note reaches
    // past the pointer in whole snap cells (1/16) — six steps long.
    let from = ui.app.roll_cell_center(60, 2.0).unwrap();
    let to = ui.app.roll_cell_center(60, 7.0).unwrap();
    ui.down(from);
    ui.at(pos2(from.x + 12.0, from.y));
    ui.step();
    ui.at(to);
    ui.step();
    ui.up(to);
    ui.step();
    let pat = ui.pattern(idx, 0);
    assert_eq!(pat.notes.len(), 1, "one note drawn");
    let n = &pat.notes[0];
    assert_eq!((n.pitch, n.start, n.duration), (60, 2.0, 6.0));
    assert!(ui.app.sel.notes.contains(&n.id), "drawn note is selected");
    assert_eq!(ui.app.history.undo_label(), Some("Draw note"));
    ui.app.undo();
    assert!(
        ui.pattern(idx, 0).notes.is_empty(),
        "the whole drag is one undo step"
    );
    assert_eq!(ui.app.history.state(), before);

    // Snap 1/8: lengths come in whole 1/8 cells; dragging left of the start
    // keeps one cell.
    ui.app.editor.snap = 2.0;
    let from = ui.app.roll_cell_center(62, 4.0).unwrap();
    let to = ui.app.roll_cell_center(62, 8.0).unwrap();
    ui.drag(from, to);
    let n = ui.pattern(idx, 0).notes[0].clone();
    assert_eq!((n.start, n.duration), (4.0, 6.0), "ceil to 1/8 cells");
    let from = ui.app.roll_cell_center(64, 10.0).unwrap();
    let to = ui.app.roll_cell_center(64, 3.0).unwrap();
    ui.drag(from, to);
    let n = ui
        .pattern(idx, 0)
        .notes
        .iter()
        .find(|n| n.pitch == 64)
        .unwrap()
        .clone();
    assert_eq!((n.start, n.duration), (10.0, 2.0), "at least one cell");

    // A plain click still draws the toolbar's note length.
    ui.app.editor.note_len = 4.0;
    ui.click(ui.app.roll_cell_center(66, 0.0).unwrap());
    let n = ui
        .pattern(idx, 0)
        .notes
        .iter()
        .find(|n| n.pitch == 66)
        .unwrap()
        .clone();
    assert_eq!(n.duration, 4.0);
}

#[test]
fn piano_non_first_note_group_drag_preserves_intervals_and_clamps() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    ui.app.add_track(Instrument::Piano);
    ui.step();
    let idx = ui.app.project.tracks.len() - 1;
    let id = ui.app.project.tracks[idx].id;
    // Centre on pitch 63 so the whole 60–67 range the drag touches stays
    // inside the (now shorter) roll viewport.
    show_pitch(&mut ui, id, 63);
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
fn piano_alt_drag_copies_the_selected_notes() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    ui.app.add_track(Instrument::Piano);
    ui.step();
    let idx = ui.app.project.tracks.len() - 1;
    let id = ui.app.project.tracks[idx].id;
    show_pitch(&mut ui, id, 63);
    {
        let pat = &mut ui.app.project.tracks[idx].patterns[0];
        pat.notes = vec![Note::new(60, 2.0, 2.0, 0.8), Note::new(64, 6.0, 2.0, 0.8)];
    }
    let originals: BTreeSet<u64> = ui.pattern(idx, 0).notes.iter().map(|n| n.id).collect();
    ui.app.sel.notes = originals.clone();
    ui.app.focus = super::Focus::Editor;
    ui.step();
    let before = ui.app.history.state();

    // Alt+click without moving copies nothing.
    let grab = ui.app.roll_cell_center(64, 6.0).unwrap();
    ui.press_mod(grab, PointerButton::Primary, Modifiers::ALT);
    ui.release_mod(grab, PointerButton::Primary, Modifiers::ALT);
    assert_eq!(ui.pattern(idx, 0).notes.len(), 2);
    assert_eq!(ui.app.history.state(), before);

    let to = ui.app.roll_cell_center(67, 10.0).unwrap();
    ui.drag_mod(grab, to, Modifiers::ALT);
    let pat = ui.pattern(idx, 0);
    assert_eq!(pat.notes.len(), 4, "the selection was duplicated");
    for n in pat.notes.iter().filter(|n| originals.contains(&n.id)) {
        assert!(
            (n.pitch, n.start) == (60, 2.0) || (n.pitch, n.start) == (64, 6.0),
            "originals stay put: {n:?}"
        );
    }
    let mut copies: Vec<(u8, f64)> = pat
        .notes
        .iter()
        .filter(|n| !originals.contains(&n.id))
        .map(|n| (n.pitch, n.start))
        .collect();
    copies.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert_eq!(copies, vec![(63, 6.0), (67, 10.0)], "copies moved together");
    assert!(
        ui.app.sel.notes.is_disjoint(&originals) && ui.app.sel.notes.len() == 2,
        "the copies are selected"
    );
    assert_eq!(ui.app.history.undo_label(), Some("Copy notes"));
    ui.app.undo();
    assert_eq!(ui.pattern(idx, 0).notes.len(), 2, "one undo step");
    assert_eq!(ui.app.history.state(), before);
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

    // Shift+←/→ move by a bar (clamped inside the pattern), Alt+←/→ change
    // the length by the snap.
    ui.app.project.tracks[idx].patterns[0].length = 48;
    let note = |ui: &Ui| ui.pattern(idx, 0).notes[0].clone();
    ui.key_mod(Key::ArrowRight, Modifiers::SHIFT);
    assert_eq!(note(&ui).start, 19.0, "shift+right moves a bar");
    ui.key_mod(Key::ArrowLeft, Modifiers::SHIFT);
    assert_eq!(note(&ui).start, 3.0, "shift+left moves back a bar");
    ui.key_mod(Key::ArrowLeft, Modifiers::SHIFT);
    assert_eq!(note(&ui).start, 0.0, "clamped at the pattern start");
    assert_eq!(note(&ui).duration, 2.0);
    ui.key_mod(Key::ArrowRight, Modifiers::ALT);
    assert_eq!(note(&ui).duration, 3.0, "alt+right lengthens by the snap");
    assert_eq!(note(&ui).start, 0.0, "the start stays");
    ui.app.editor.snap = 2.0;
    ui.key_mod(Key::ArrowLeft, Modifiers::ALT);
    assert_eq!(note(&ui).duration, 2.0, "alt+left shortens by the snap");
    ui.key_mod(Key::ArrowLeft, Modifiers::ALT);
    assert_eq!(note(&ui).duration, 2.0, "never below one snap cell");
    assert_eq!(note(&ui).pitch, 73, "no transposition along the way");
}

#[test]
fn digit_keys_pick_the_draw_length_and_set_selected_lengths() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    ui.app.add_track(Instrument::Piano);
    ui.step();
    let idx = ui.app.project.tracks.len() - 1;
    let id = ui.app.project.tracks[idx].id;
    show_pitch(&mut ui, id, 60);
    ui.click(ui.app.roll_cell_center(60, 2.0).unwrap());
    ui.click(ui.app.roll_cell_center(62, 8.0).unwrap());
    assert_eq!(ui.app.focus, super::Focus::Editor);

    for (key, len) in [
        (Key::Num1, 0.5),
        (Key::Num2, 1.0),
        (Key::Num3, 2.0),
        (Key::Num4, 4.0),
        (Key::Num5, 8.0),
        (Key::Num6, 16.0),
    ] {
        ui.key(key);
        assert_eq!(ui.app.editor.note_len, len, "{key:?} picks {len}");
    }
    ui.key(Key::Num3);
    assert_eq!(ui.pattern(idx, 0).notes.len(), 2, "digits draw nothing");

    // Shift+digit sets the selected notes' length — US layouts deliver
    // Shift+1 as logical "!", so the physical key is what counts.
    let durations = |ui: &Ui| -> Vec<f64> {
        ui.pattern(idx, 0)
            .notes
            .iter()
            .map(|n| n.duration)
            .collect()
    };
    ui.app.sel.notes = ui.pattern(idx, 0).notes.iter().map(|n| n.id).collect();
    ui.key_mod(Key::Num4, Modifiers::SHIFT);
    assert_eq!(durations(&ui), vec![4.0, 4.0]);
    assert_eq!(ui.app.history.undo_label(), Some("Note length"));
    assert_eq!(ui.app.editor.note_len, 2.0, "draw length unchanged");
    ui.next.modifiers = Modifiers::SHIFT;
    ui.event(Event::Key {
        key: Key::Exclamationmark,
        physical_key: Some(Key::Num1),
        pressed: true,
        repeat: false,
        modifiers: Modifiers::SHIFT,
    });
    assert_eq!(durations(&ui), vec![0.5, 0.5], "Shift+1 typed as \"!\"");
    ui.key_mod(Key::Num4, Modifiers::SHIFT);
    ui.app.sel.notes = [ui.pattern(idx, 0).notes[1].id].into();
    ui.key_mod(Key::Num6, Modifiers::SHIFT);
    assert_eq!(durations(&ui), vec![4.0, 8.0], "clamped at the pattern end");

    // ⌘2 still switches tabs; digits do nothing once the arranger has focus.
    ui.key_mod(Key::Num2, Modifiers::COMMAND);
    assert_eq!(ui.app.tab, super::Tab::Mixer);
    ui.key_mod(Key::Num1, Modifiers::COMMAND);
    assert_eq!(ui.app.tab, super::Tab::Editor);
    assert_eq!(ui.app.editor.note_len, 2.0);
    ui.app.focus = super::Focus::Arranger;
    ui.key(Key::Num5);
    assert_eq!(ui.app.editor.note_len, 2.0, "arranger focus ignores digits");
}

#[test]
fn zero_zooms_the_roll_to_fit_the_pattern_or_selection() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    ui.app.add_track(Instrument::Piano);
    ui.step();
    let idx = ui.app.project.tracks.len() - 1;
    let id = ui.app.project.tracks[idx].id;
    show_pitch(&mut ui, id, 60);
    ui.click(ui.app.roll_cell_center(60, 2.0).unwrap());
    {
        let pat = &mut ui.app.project.tracks[idx].patterns[0];
        pat.length = 128;
        pat.notes.push(Note::new(67, 40.0, 4.0, 0.8));
    }
    ui.step();
    let (grid, _, _) = ui.app.editor_test_rects();

    ui.key(Key::Num0);
    ui.step();
    let (x0, x1) = (ui.app.roll_x(0.0).unwrap(), ui.app.roll_x(128.0).unwrap());
    assert!(
        (x0 - grid.left()).abs() < 0.5,
        "pattern start at the left edge"
    );
    assert!(
        (x1 - grid.right()).abs() < 1.0,
        "pattern end at the right edge: {x1} vs {}",
        grid.right()
    );

    // Shift+0 fits the selected notes (steps 2–44, pitches 60–67), from
    // anywhere in the roll.
    ui.app.set_roll_scroll(0.0, 0.0);
    ui.step();
    ui.app.sel.notes = ui.pattern(idx, 0).notes.iter().map(|n| n.id).collect();
    ui.key_mod(Key::Num0, Modifiers::SHIFT);
    ui.step();
    let (a, b) = (ui.app.roll_x(2.0).unwrap(), ui.app.roll_x(44.0).unwrap());
    assert!(
        a > grid.left() && b < grid.right(),
        "selection inside: {a}..{b}"
    );
    assert!(
        b - a > grid.width() * 0.8,
        "selection fills most of the width"
    );
    for pitch in [60, 67] {
        assert!(
            ui.app.roll_cell_center(pitch, 2.0).is_some()
                || ui.app.roll_cell_center(pitch, 40.0).is_some(),
            "pitch {pitch} on screen"
        );
    }
    // With nothing selected Shift+0 fits the pattern.
    ui.app.sel.notes.clear();
    ui.key_mod(Key::Num0, Modifiers::SHIFT);
    ui.step();
    assert!((ui.app.roll_x(128.0).unwrap() - grid.right()).abs() < 1.0);
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
    assert!(
        matches!(ui.app.dialog, Dialog::NewSong(_)),
        "discard moves on to the New Song dialog"
    );
    ui.key(Key::Enter);
    assert!(matches!(ui.app.dialog, Dialog::None));
    assert_eq!(
        ui.app.project.tracks.len(),
        1,
        "Create started a new song (starter drum track)"
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
    // Unique temp dir per run: concurrent test binaries must not race on
    // shared /tmp filenames.
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("roundtrip.cadence");
    crate::persistence::save_project(&ui.app.project, &path).unwrap();
    ui.app.open_path(path.clone());
    ui.step();
    assert_eq!(ui.app.doc.path.as_deref(), Some(path.as_path()));
    assert!(!ui.app.dirty(), "a just-opened song is clean");
}

#[test]
fn midi_export_import_roundtrip() {
    let mut ui = Ui::demo();
    ui.step();
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("roundtrip.mid");
    ui.app.export_midi_file(path.clone());
    ui.step();
    ui.app.import_midi_file(path.clone());
    ui.step();
    assert!(
        !ui.app.project.tracks.is_empty(),
        "MIDI import produced tracks"
    );
    assert!(ui.app.dirty(), "an imported song is unsaved");
}

#[test]
fn wav_export_worker_completes() {
    let mut ui = Ui::demo();
    ui.step();
    // The dir must outlive the async export worker, which writes the file.
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("export.wav");
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
}

#[test]
fn normalized_wav_export_reports_loudness_and_the_toggle_persists() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    assert!(!ui.app.export_normalize, "off unless chosen");
    assert_eq!(ui.app.wav_export_options().normalize_lufs, None);
    ui.app.export_normalize = true;
    assert_eq!(
        ui.app.wav_export_options().normalize_lufs,
        Some(crate::audio::DEFAULT_TARGET_LUFS)
    );
    let mut storage = MemStorage::default();
    eframe::App::save(&mut ui.app, &mut storage);
    assert_eq!(
        storage
            .map
            .get(super::files::NORMALIZE_STORAGE_KEY)
            .map(String::as_str),
        Some("1")
    );
    // A one-bar song with a single note renders quickly.
    let mut p = Project::empty();
    p.bars = 1;
    p.tracks[0].patterns[0].notes = vec![crate::model::Note::new(36, 0.0, 1.0, 1.0)];
    ui.app.project = p;
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("loud.wav");
    ui.app.start_wav_export(path.clone());
    let mut finished = false;
    for _ in 0..3_000 {
        ui.step();
        if ui.app.export.is_none() {
            finished = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(finished, "normalized WAV export finished");
    assert!(path.exists());
    let status = ui.app.status.as_ref().expect("export status");
    assert!(status.text.contains("LUFS"), "{}", status.text);
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

// ----- instrument library: banks, search, audition metadata ---------------

fn test_catalog() -> crate::packs::PackCatalog {
    crate::packs::PackCatalog {
        instruments: vec![
            preset_with("test-a.kick", "Test Kick", "deep kick thump", 36, 0.7),
            preset_with("test-a.snare", "Test Snare", "crack snare", 40, 0.6),
            preset_with("test-b.pad", "Test Pad", "ambient wash", 55, 3.0),
        ],
        errors: vec![],
    }
}

#[test]
fn library_bank_and_search_filters() {
    let mut ui = Ui::demo();
    ui.step();
    let catalog = test_catalog();
    ui.app.catalog = catalog.clone();
    let state = ui.app.history.state();
    let tracks = ui.app.project.tracks.len();

    // All banks: every built-in plus every preset.
    let rows = library_rows(&LibraryBank::All, "", &catalog);
    assert_eq!(rows.builtins.len(), Instrument::ALL.len());
    assert_eq!(rows.presets.len(), 3);

    // A single pack bank shows only its own presets.
    let rows = library_rows(&LibraryBank::Pack("test-a".into()), "", &catalog);
    assert!(rows.builtins.is_empty());
    assert_eq!(rows.presets.len(), 2);
    let rows = library_rows(&LibraryBank::BuiltIn, "", &catalog);
    assert_eq!(rows.builtins.len(), Instrument::ALL.len());
    assert!(rows.presets.is_empty());

    // Search hits the name…
    let rows = library_rows(&LibraryBank::All, "snare", &catalog);
    assert_eq!(rows.presets.len(), 1);
    assert_eq!(rows.presets[0].name, "Test Snare");
    // …the description…
    let rows = library_rows(&LibraryBank::All, "ambient", &catalog);
    assert_eq!(rows.presets.len(), 1);
    assert_eq!(rows.presets[0].id, "test-b.pad");
    // …and the bank name: "test b" appears only in the pack label
    // ("Test B test-b"), never in the preset id or text.
    let rows = library_rows(&LibraryBank::All, "test b", &catalog);
    assert_eq!(rows.presets.len(), 1);
    assert_eq!(rows.presets[0].id, "test-b.pad");

    // Bank chooser reports sound counts per bank.
    let options = bank_options(&catalog);
    assert_eq!(options[0], (LibraryBank::All, Instrument::ALL.len() + 3));
    assert_eq!(options[1], (LibraryBank::BuiltIn, Instrument::ALL.len()));
    assert!(options.contains(&(LibraryBank::Pack("test-a".into()), 2)));
    assert!(options.contains(&(LibraryBank::Pack("test-b".into()), 1)));

    // Choosing a bank / typing a filter is view state only: the project is
    // untouched and the song stays clean.
    ui.app.library_bank = LibraryBank::Pack("test-a".into());
    ui.app.library_filter = "kick".into();
    ui.step();
    assert_eq!(ui.app.project.tracks.len(), tracks);
    assert_eq!(ui.app.history.state(), state);
    assert!(!ui.app.dirty());
}

#[test]
fn library_bank_repair_after_pack_disappears() {
    let mut ui = Ui::demo();
    ui.step();
    ui.app.catalog = test_catalog();
    ui.app.library_bank = LibraryBank::Pack("test-a".into());
    ui.app.repair_library_bank();
    assert_eq!(ui.app.library_bank, LibraryBank::Pack("test-a".into()));

    // The pack is gone from the catalog (e.g. after a reload): fall back to All.
    ui.app.catalog = crate::packs::PackCatalog::default();
    ui.app.repair_library_bank();
    assert_eq!(ui.app.library_bank, LibraryBank::All);

    // All and Built-in never need repair.
    ui.app.library_bank = LibraryBank::BuiltIn;
    ui.app.repair_library_bank();
    assert_eq!(ui.app.library_bank, LibraryBank::BuiltIn);
}

#[test]
fn library_reset_clears_search_and_bank() {
    let mut ui = Ui::demo();
    ui.step();
    ui.app.library_filter = "kick".into();
    ui.app.library_bank = LibraryBank::BuiltIn;
    ui.step();
    let bank_rect = ui.app.library_rects.bank;
    assert!(bank_rect.width() > 20.0);
    // Reset is the right-most widget of the bank row.
    ui.click(Pos2::new(bank_rect.right() - 28.0, bank_rect.center().y));
    ui.step();
    assert_eq!(ui.app.library_filter, "");
    assert_eq!(ui.app.library_bank, LibraryBank::All);
}

#[test]
fn library_controls_stay_above_scrolling_list() {
    // Enough presets that the instrument list must scroll.
    let presets: Vec<InstrumentPreset> = (0..40)
        .map(|i| preset_with(&format!("big.pad-{i}"), &format!("Pad {i}"), "pad", 60, 1.0))
        .collect();
    let mut ui = Ui::demo();
    ui.step();
    ui.app.catalog = crate::packs::PackCatalog {
        instruments: presets,
        errors: vec![],
    };
    ui.step();

    let search = ui.app.library_rects.search;
    let bank = ui.app.library_rects.bank;
    let list = ui.app.library_rects.list;
    assert!(search.width() > 20.0 && bank.width() > 20.0);
    assert!(list.height() > 40.0);
    assert!(
        list.top() >= bank.bottom(),
        "the list starts below the bank controls"
    );

    // Wheel-scroll inside the list; the controls must not move.
    ui.at(list.center());
    ui.step();
    for _ in 0..8 {
        ui.wheel(-240.0);
    }
    let scrolled = ui.app.library_rects.scroll;
    assert!(scrolled.y > 1.0, "the list scrolled down: {scrolled:?}");
    assert_eq!(ui.app.library_rects.search, search);
    assert_eq!(ui.app.library_rects.bank, bank);

    // The controls are still live: typing filters the list.
    ui.click(search.center());
    ui.text("pad");
    ui.step();
    assert_eq!(ui.app.library_filter, "pad");
}

#[test]
fn preset_audition_uses_pack_metadata() {
    // audition_duration is f32 in the preset; compare with a small epsilon.
    fn assert_params(actual: (u8, f64), pitch: u8, secs: f64) {
        assert_eq!(actual.0, pitch);
        assert!((actual.1 - secs).abs() < 1e-6, "{actual:?}");
    }

    // A pack preset auditions at its declared note and duration.
    let preset = preset_with("test-a.kick", "Kick", "", 36, 0.7);
    let mut track = crate::model::create_track(Instrument::Lead);
    track.preset = Some(preset);
    assert_params(audition_params(&track, 60), 36, 0.7);

    // A preset serialized without the fields (older packs) gets the
    // documented defaults: MIDI 60 and 0.8 s.
    let legacy: InstrumentPreset =
        serde_json::from_str(r#"{"id":"old.x","name":"X","oscillators":[{}]}"#)
            .expect("preset without audition metadata still parses");
    let mut track = crate::model::create_track(Instrument::Lead);
    track.preset = Some(legacy);
    assert_params(audition_params(&track, 60), 60, 0.8);

    // Built-ins keep the caller's fallback pitch and 0.7 s preview.
    let track = crate::model::create_track(Instrument::Bass);
    assert_params(audition_params(&track, 36), 36, 0.7);
}

#[test]
fn new_preset_track_centers_editor_on_audition_note() {
    let mut ui = Ui::demo();
    ui.step();
    let preset = preset_with("test-a.kick", "Kick", "", 33, 0.7);
    ui.app.add_preset_track(&preset);
    ui.step();

    // The empty pattern homes on the preset's audition note (A#1), not the
    // Lead default of C5 — and neighbouring registers are on screen.
    assert!(
        ui.app.roll_cell_center(33, 1.0).is_some(),
        "audition note is visible"
    );
    assert!(
        ui.app.roll_cell_center(36, 1.0).is_some(),
        "a few semitones up is visible too"
    );
    assert!(
        ui.app.roll_cell_center(72, 1.0).is_none(),
        "the generic Lead default pitch is not what the editor centered on"
    );
}

#[test]
fn preset_track_with_notes_still_centers_on_note_content() {
    let mut ui = Ui::demo();
    ui.step();
    let preset = preset_with("test-a.pad", "Pad", "", 33, 3.0);
    ui.app.add_preset_track(&preset);
    ui.step();
    let tid = ui.app.project.tracks.last().unwrap().id;

    // Drop a high note into the (previously empty) pattern, then re-enter
    // the track so the editor re-homes: content wins over audition metadata.
    ui.app.project.tracks.last_mut().unwrap().patterns[0]
        .notes
        .push(Note::new(96, 0.0, 4.0, 0.8));
    let other = ui.app.project.tracks[0].id;
    ui.app.select_track(other);
    ui.step();
    ui.app.select_track(tid);
    ui.step();

    assert!(
        ui.app.roll_cell_center(96, 1.0).is_some(),
        "the written note is centered"
    );
    assert!(
        ui.app.roll_cell_center(33, 1.0).is_none(),
        "audition metadata no longer drives the scroll once notes exist"
    );
}

#[test]
fn kit_preset_track_gets_the_drum_grid() {
    let mut ui = Ui::demo();
    ui.step();
    ui.app.add_preset_track(&test_kit_preset());
    ui.step();

    let t = ui.app.project.tracks.last().unwrap();
    assert_eq!(t.instrument, Instrument::Drums, "kits become drum tracks");
    assert_eq!(t.preset.as_ref().unwrap().id, "test-drums.machine");
    assert!(t.shows_drum_grid());
    // Rows follow the pieces array order, not pitch order. Kick sits at
    // pitch 60 — outside the built-in DRUMS table — proving kit rows are
    // arbitrary pitches, not the eight synthesized drums.
    assert_eq!(t.drum_rows(), vec![(42, "Hat"), (60, "Kick"), (39, "Clap")]);
    assert_eq!(t.patterns[0].name, "Beat 1", "drum-grid naming");
    let status = ui.app.status.as_ref().expect("add status");
    assert!(
        status.text.contains("drum grid"),
        "status mentions the drum grid: {}",
        status.text
    );
}

#[test]
fn kit_drum_grid_has_one_row_per_piece_and_paints_piece_pitches() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    ui.app.add_preset_track(&test_kit_preset());
    ui.step();
    let idx = ui.app.project.tracks.len() - 1;

    // Three pieces → three rows; a fourth row index is off the grid.
    for row in 0..3 {
        assert!(
            ui.app.drum_cell_center(row, 0).is_some(),
            "row {row} on screen"
        );
    }
    assert!(ui.app.drum_cell_center(3, 0).is_none(), "no fourth row");

    // Clicks on rows 0/1/2 write the piece pitches in pieces order:
    // row 0 = Hat@42, row 1 = Kick@60 (off the built-in drum table),
    // row 2 = Clap@39.
    for (row, step) in [(0usize, 0u32), (1, 4), (2, 8)] {
        let pos = ui.app.drum_cell_center(row, step).expect("cell");
        ui.click(pos);
        ui.step();
    }
    let pat = ui.pattern(idx, 0);
    for (pitch, step) in [(42u8, 0u32), (60, 4), (39, 8)] {
        assert!(
            ops::drum_hit(pat, pitch, step).is_some(),
            "piece pitch {pitch} painted at step {step}"
        );
    }
    assert_eq!(pat.notes.len(), 3, "exactly the three painted hits");
    assert!(
        pat.notes.iter().all(|n| [39, 42, 60].contains(&n.pitch)),
        "no note lands on a pitch outside the kit rows"
    );
}

#[test]
fn kit_preset_summary_and_audition_pitch() {
    let preset = test_kit_preset();
    assert_eq!(preset_summary(&preset), "kit · 3 pieces");
    // Melodic presets keep the oscillator summary.
    assert!(preset_summary(&test_preset()).contains("sine"));

    // audition_note at the serde default (60) falls back to the first piece.
    let track = crate::model::Track::from_preset(Instrument::Drums, preset.clone());
    let (pitch, secs) = audition_params(&track, 60);
    assert_eq!(pitch, 42);
    assert!((secs - 0.8).abs() < 1e-6, "{secs}");

    // An explicit audition_note wins over the piece fallback.
    let mut explicit = preset.clone();
    explicit.audition_note = 48;
    let track = crate::model::Track::from_preset(Instrument::Drums, explicit);
    assert_eq!(audition_params(&track, 60).0, 48);
}

#[test]
fn melodic_preset_on_a_drums_track_keeps_the_piano_roll() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    // Swap the built-in drums track's sound for a melodic preset before the
    // first frame so the editor homes on the preset's audition note (55).
    ui.app.project.tracks[0].preset = Some(test_preset());
    let id = ui.app.project.tracks[0].id;
    ui.app.select_track(id);
    ui.step();

    let t = &ui.app.project.tracks[0];
    assert!(!t.shows_drum_grid(), "melodic preset → piano roll");
    assert!(ui.app.drum_cell_center(0, 0).is_none());
    assert!(
        ui.app.roll_cell_center(55, 1.0).is_some(),
        "the piano roll is what rendered, homed on the audition note"
    );
}

#[test]
fn kit_preset_on_a_melodic_track_keeps_the_piano_roll() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.app.add_track(Instrument::Lead);
    // Apply the kit preset before the first frame: the editor must home on a
    // real piece pitch (first piece, Hat@42) even though the roll stays.
    ui.app.project.tracks.last_mut().unwrap().preset = Some(test_kit_preset());
    ui.step();

    let t = ui.app.project.tracks.last().unwrap();
    assert_eq!(t.instrument, Instrument::Lead);
    assert!(!t.shows_drum_grid(), "kit preset on Lead → piano roll");
    assert_eq!(t.drum_rows(), vec![(42, "Hat"), (60, "Kick"), (39, "Clap")]);
    assert!(ui.app.drum_cell_center(0, 0).is_none());
    assert!(
        ui.app.roll_cell_center(42, 1.0).is_some(),
        "piano roll homed on the first kit piece pitch"
    );
}

// ----- new song + song key ----------------------------------------------------

#[test]
fn new_song_opens_dialog_instead_of_replacing() {
    let mut ui = Ui::demo();
    ui.step();
    let demo = ui.app.project.clone();
    ui.app.request(PendingAction::New);
    assert!(
        matches!(&ui.app.dialog, Dialog::NewSong(d) if *d == NewSongDraft::default()),
        "clean song goes straight to the New Song dialog"
    );
    ui.steps(2);
    assert!(matches!(ui.app.dialog, Dialog::NewSong(_)), "stays open");
    assert_eq!(ui.app.project, demo, "nothing replaced yet");
}

#[test]
fn new_song_create_applies_name_and_key() {
    let mut ui = Ui::demo();
    ui.step();
    ui.app.request(PendingAction::New);
    ui.app.dialog = Dialog::NewSong(NewSongDraft {
        name: "  Moonlit  ".into(),
        key: SongKey::new(9, KeyMode::Minor),
    });
    ui.step();
    ui.key(Key::Enter);
    assert!(matches!(ui.app.dialog, Dialog::None));
    assert_eq!(ui.app.project.name, "Moonlit");
    assert_eq!(
        ui.app.project.key,
        SongKey {
            root: 9,
            mode: KeyMode::Minor
        }
    );
    assert_eq!(ui.app.project.tracks.len(), 1);
    assert!(!ui.app.dirty());
    let status = ui.app.status.as_ref().unwrap();
    assert!(status.text.contains("A minor"), "{}", status.text);

    // A blank name falls back to the default.
    ui.app.create_new_song("   ", SongKey::default());
    assert_eq!(ui.app.project.name, Project::empty().name);
    assert_eq!(ui.app.project.key, SongKey::default());
}

#[test]
fn new_song_cancel_keeps_the_current_song() {
    let mut ui = Ui::demo();
    ui.step();
    let demo = ui.app.project.clone();
    ui.app.request(PendingAction::New);
    ui.step();
    ui.key(Key::Escape);
    assert!(matches!(ui.app.dialog, Dialog::None));
    assert_eq!(ui.app.project, demo);
}

#[test]
fn dirty_new_song_asks_to_save_then_opens_new_song_dialog() {
    let mut ui = Ui::demo();
    ui.step();
    ui.app.add_track(Instrument::Lead);
    ui.step();
    ui.app.request(PendingAction::New);
    assert!(matches!(ui.app.dialog, Dialog::Unsaved(PendingAction::New)));
    // "Don't Save" path: the unsaved dialog runs the pending action, which
    // must leave the New Song dialog open rather than being overwritten.
    ui.app.dialog = Dialog::None;
    ui.app.run_pending_action(PendingAction::New);
    assert!(matches!(ui.app.dialog, Dialog::NewSong(_)));
    ui.steps(2);
    assert!(matches!(ui.app.dialog, Dialog::NewSong(_)));
    assert!(ui.app.dirty(), "old song untouched until Create");
    ui.key(Key::Enter);
    assert!(!ui.app.dirty());
    assert_eq!(ui.app.project.tracks.len(), 1);
}

// ----- arpeggiator -----------------------------------------------------------

/// Demo with the "Afterglow keys" piano track selected.
fn arp_ui() -> (Ui, u64, u64) {
    let mut ui = Ui::demo();
    ui.step();
    let track = ui
        .app
        .project
        .tracks
        .iter()
        .find(|t| t.instrument == Instrument::Piano)
        .unwrap();
    let (tid, pid) = (track.id, track.patterns[0].id);
    ui.app.select_track(tid);
    ui.step();
    (ui, tid, pid)
}

#[test]
fn arpeggiator_needs_a_melodic_track_with_a_pattern() {
    let (mut ui, tid, _) = arp_ui();
    assert!(ui.app.can_arpeggiate());
    let drums = ui.app.project.tracks[0].id;
    assert!(ui.app.project.tracks[0].shows_drum_grid());
    ui.app.select_track(drums);
    ui.step();
    assert!(!ui.app.can_arpeggiate(), "drum grid track");
    ui.app.open_arpeggiator();
    assert!(matches!(ui.app.dialog, Dialog::None));

    ui.app.select_track(tid);
    ui.app.edit("Clear patterns", None, |p| {
        let t = ops::track_mut(p, tid).unwrap();
        t.clips.clear();
        t.patterns.clear();
    });
    ui.step();
    assert!(!ui.app.can_arpeggiate(), "no pattern");
    assert_eq!(ui.app.arpeggiate(&ops::ArpSpec::default()), None);
}

#[test]
fn arpeggiator_defaults_follow_song_key_and_instrument() {
    let (mut ui, _, _) = arp_ui();
    ui.app.open_arpeggiator();
    let Dialog::Arp(draft) = &ui.app.dialog else {
        panic!("arp dialog open");
    };
    // Demo is C minor: the i chord is C minor, rooted at C4 for a piano.
    let spec = draft.spec(ui.app.project.key);
    assert_eq!(spec.chord_root, 60);
    assert_eq!(spec.quality, ChordQuality::Minor);
    assert_eq!(spec.octaves, 2);
    assert_eq!(spec.direction, ops::ArpDirection::Up);
    assert_eq!(spec.rate_steps, 2);
    assert_eq!(spec.velocity, 0.8);
    assert!(spec.accent && spec.replace);
    ui.steps(3);
    assert!(matches!(ui.app.dialog, Dialog::Arp(_)), "dialog stays open");
    ui.key(Key::Escape);
    assert!(matches!(ui.app.dialog, Dialog::None));

    // Degree V of C minor is G minor (natural minor v).
    let mut draft = ArpDraft::new(ui.app.project.key, 36);
    draft.degree = Some(4);
    draft.quality = ui.app.project.key.degree_chord_quality(4);
    let spec = draft.spec(ui.app.project.key);
    assert_eq!(spec.chord_root, 43, "G2 for a bass-range instrument");
    assert_eq!(spec.quality, ChordQuality::Minor);
}

#[test]
fn arpeggiate_writes_current_pattern_and_undoes() {
    let (mut ui, tid, pid) = arp_ui();
    let before = ui.app.project.clone();
    let pattern_len = ops::pattern(ops::track(&before, tid).unwrap(), pid)
        .unwrap()
        .length;
    let spec = ops::ArpSpec {
        chord_root: 60,
        quality: ChordQuality::Major7,
        octaves: 1,
        rate_steps: 4,
        ..ops::ArpSpec::default()
    };
    assert_eq!(ui.app.arpeggiate(&spec), Some(pattern_len as usize / 4));
    ui.step();
    let pat = ops::pattern(ops::track(&ui.app.project, tid).unwrap(), pid).unwrap();
    let pitches: Vec<u8> = pat.notes.iter().take(5).map(|n| n.pitch).collect();
    assert_eq!(pitches, [60, 64, 67, 71, 60]);
    assert!(ui.app.dirty());
    let status = ui.app.status.as_ref().unwrap();
    assert!(
        status.text.starts_with("Arpeggio: Cmaj7"),
        "{}",
        status.text
    );
    ui.app.project.validate().unwrap();

    ui.app.undo();
    ui.step();
    assert_eq!(ui.app.project, before, "one undo restores the pattern");
}

#[test]
fn arp_key_change_persists_and_drives_the_chord() {
    let (mut ui, tid, pid) = arp_ui();
    // The dialog's key picker goes through set_song_key: an undoable edit.
    ui.app.set_song_key(SongKey::new(2, KeyMode::Major));
    assert!(ui.app.dirty());
    let status = ui.app.status.as_ref().unwrap();
    assert!(status.text.contains("D major"), "{}", status.text);
    ui.app.open_arpeggiator();
    ui.step();
    let Dialog::Arp(draft) = &ui.app.dialog else {
        panic!("arp dialog open");
    };
    let spec = draft.spec(ui.app.project.key);
    assert_eq!(spec.chord_root, 62, "I of D major");
    assert_eq!(spec.quality, ChordQuality::Major);
    ui.app.arpeggiate(&spec);
    let pat = ops::pattern(ops::track(&ui.app.project, tid).unwrap(), pid).unwrap();
    assert_eq!(pat.notes[0].pitch, 62);
    assert_eq!(pat.notes[1].pitch, 66);
    assert_eq!(ui.app.project.key, SongKey::new(2, KeyMode::Major));
    ui.app.undo();
    ui.app.undo();
    assert_eq!(ui.app.project.key, SongKey::new(0, KeyMode::Minor));
}

// ----- UI themes -------------------------------------------------------------

/// In-memory `eframe::Storage` for theme persistence tests — no user-data IO.
#[derive(Default)]
struct MemStorage {
    map: std::collections::HashMap<String, String>,
}

impl eframe::Storage for MemStorage {
    fn get_string(&self, key: &str) -> Option<String> {
        self.map.get(key).cloned()
    }
    fn set_string(&mut self, key: &str, value: String) {
        self.map.insert(key.to_owned(), value);
    }
    fn flush(&mut self) {}
}

#[test]
fn theme_defaults_to_graphite() {
    assert_eq!(ThemeChoice::default(), ThemeChoice::Graphite);
    assert_eq!(ThemeChoice::from_id(""), ThemeChoice::Graphite);
    assert_eq!(ThemeChoice::from_id("bogus"), ThemeChoice::Graphite);
    assert_eq!(ThemeChoice::from_storage(None), ThemeChoice::Graphite);
    let mut ui = Ui::demo();
    ui.step();
    assert_eq!(ui.app.theme, ThemeChoice::Graphite);
    assert_eq!(ui.app.palette(), ThemeChoice::Graphite.palette());
}

#[test]
fn theme_switch_changes_palette_not_the_song() {
    let mut ui = Ui::demo();
    ui.step();
    let before = ui.app.project.clone();
    let saved = ui.app.history.state();
    let was_dirty = ui.app.dirty();
    for choice in [
        ThemeChoice::Midnight,
        ThemeChoice::Paper,
        ThemeChoice::Graphite,
    ] {
        ui.app.set_theme(&ui.ctx, choice);
        ui.step();
        assert_eq!(ui.app.theme, choice);
        assert_eq!(ui.app.palette(), choice.palette());
        assert_eq!(
            ui.app.project, before,
            "theme switch must not mutate the song"
        );
        assert_eq!(ui.app.history.state(), saved, "no undo step recorded");
        assert_eq!(ui.app.dirty(), was_dirty, "dirty flag untouched");
    }
    // Re-selecting the current theme is a no-op.
    ui.app.set_theme(&ui.ctx, ThemeChoice::Paper);
    assert_eq!(ui.app.theme, ThemeChoice::Paper);
}

#[test]
fn theme_storage_round_trip_and_fallback() {
    // Missing key → default.
    let storage = MemStorage::default();
    assert_eq!(
        ThemeChoice::from_storage(Some(&storage)),
        ThemeChoice::Graphite
    );
    // Every known id parses back to its theme.
    for choice in ThemeChoice::ALL {
        assert_eq!(ThemeChoice::from_id(choice.id()), choice);
    }
    // Unknown / malformed values fall back to Graphite.
    for bad in ["neon", "MIDNIGHT", " paper ", "0", "light"] {
        let mut storage = MemStorage::default();
        storage.set_string(theme::STORAGE_KEY, bad.to_owned());
        assert_eq!(
            ThemeChoice::from_storage(Some(&storage)),
            ThemeChoice::Graphite,
            "{bad:?} should fall back"
        );
    }
    // App::save writes the id; from_storage reads it back.
    let mut ui = Ui::demo();
    ui.step();
    ui.app.set_theme(&ui.ctx, ThemeChoice::Midnight);
    let mut storage = MemStorage::default();
    eframe::App::save(&mut ui.app, &mut storage);
    assert_eq!(
        storage.map.get(theme::STORAGE_KEY).map(String::as_str),
        Some("midnight")
    );
    assert_eq!(
        ThemeChoice::from_storage(Some(&storage)),
        ThemeChoice::Midnight
    );
}

/// Perceived luminance used by the contrast checks below.
fn luma(c: egui::Color32) -> f32 {
    0.2126 * f32::from(c.r()) + 0.7152 * f32::from(c.g()) + 0.0722 * f32::from(c.b())
}

#[test]
fn theme_palettes_keep_text_and_grids_readable() {
    for choice in ThemeChoice::ALL {
        let pal = choice.palette();
        let body = (luma(pal.text) - luma(pal.bg)).abs();
        assert!(
            body > 150.0,
            "{choice:?}: body text must contrast with the background ({body:.0})"
        );
        let faint = (luma(pal.text_faint) - luma(pal.canvas)).abs();
        assert!(
            faint > 55.0,
            "{choice:?}: faint text must stay legible on the canvas ({faint:.0})"
        );
        // Accent fills (play button, primary buttons) need a readable ink.
        let ink = theme::ink_on(pal.accent);
        let ink_contrast = (luma(ink) - luma(pal.accent)).abs();
        assert!(
            ink_contrast > 120.0,
            "{choice:?}: ink_on(accent) must contrast ({ink_contrast:.0})"
        );
        // Grid lines must be visible against the canvas but not glare.
        let grid = (luma(pal.grid_beat) - luma(pal.canvas)).abs();
        assert!(
            grid > 8.0,
            "{choice:?}: beat grid barely visible ({grid:.0})"
        );
    }
    // ink_on picks dark ink on light fills and light ink on dark fills.
    assert!(luma(theme::ink_on(egui::Color32::WHITE)) < 60.0);
    assert!(luma(theme::ink_on(egui::Color32::BLACK)) > 190.0);
}

// ----- soundfonts --------------------------------------------------------------

#[test]
fn font_pick_assigns_a_soundfont_and_moves_to_the_piano_roll() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    let id = ui.app.project.tracks[0].id;
    assert!(ui.app.project.tracks[0].shows_drum_grid());
    ui.app.select_track(id);
    ui.step();
    // Headless has no fonts installed; assigning still records the reference
    // (the picker lists it as "not installed" and playback falls back).
    assert!(ui.app.fonts.fonts.is_empty());
    let track = ui.app.project.tracks[0].clone();
    ui.app
        .apply_font_pick(&track, Some(FontRef::new("GeneralUser", 40)));
    ui.step();
    let t = &ui.app.project.tracks[0];
    assert_eq!(t.font, Some(FontRef::new("GeneralUser", 40)));
    assert_eq!(
        t.instrument,
        Instrument::Lead,
        "font tracks get the piano roll"
    );
    assert!(!t.shows_drum_grid());
    assert_eq!(
        t.name, "GeneralUser",
        "font tracks are named after the font"
    );
    assert_eq!(ops::instrument_label(t), "GeneralUser · Violin");
    assert!(ui.app.dirty());
    // Undo restores the drum track.
    ui.app.undo();
    ui.step();
    assert!(ui.app.project.tracks[0].font.is_none());
    assert!(ui.app.project.tracks[0].shows_drum_grid());
}

#[test]
fn font_clear_and_sound_pick_restore_the_embedded_sound() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    ui.app.add_preset_track(&test_preset());
    ui.step();
    let idx = ui.app.project.tracks.len() - 1;
    ui.app.select_track(ui.app.project.tracks[idx].id);
    ui.step();

    // Assign a font on top of the preset track: the preset stays as the
    // missing-font fallback and the track is renamed after the font.
    let track = ui.app.project.tracks[idx].clone();
    ui.app
        .apply_font_pick(&track, Some(FontRef::new("GeneralUser", 0)));
    ui.step();
    {
        let t = &ui.app.project.tracks[idx];
        assert!(t.font.is_some());
        assert_eq!(
            t.preset.as_ref().unwrap().id,
            "test-pack.glass",
            "the preset stays as the missing-font fallback"
        );
        assert_eq!(t.name, "GeneralUser");
        assert_eq!(
            ops::instrument_label(t),
            "GeneralUser · Acoustic Grand Piano"
        );
    }

    // Clearing the font brings the preset sound (and its name) back.
    let track = ui.app.project.tracks[idx].clone();
    ui.app.apply_font_pick(&track, None);
    ui.step();
    let t = &ui.app.project.tracks[idx];
    assert!(t.font.is_none());
    assert_eq!(t.name, "Glass Bell");
    assert_eq!(ops::instrument_label(t), "Glass Bell");

    // An explicit built-in pick clears font AND preset — the choice the user
    // made is the sound that plays.
    let track = ui.app.project.tracks[idx].clone();
    ui.app
        .apply_font_pick(&track, Some(FontRef::new("GeneralUser", 0)));
    ui.step();
    let track = ui.app.project.tracks[idx].clone();
    ui.app.apply_sound_pick(&track, Instrument::Piano, None);
    ui.step();
    let t = &ui.app.project.tracks[idx];
    assert!(t.font.is_none() && t.preset.is_none());
    assert_eq!(t.instrument, Instrument::Piano);
    assert_eq!(t.name, "Electric piano");
}

#[test]
fn font_audition_params_and_gm_family_names() {
    // Font tracks preview at the caller's pitch for a fixed 0.8 s, ignoring
    // preset audition metadata — SoundFonts carry none.
    let mut track = crate::model::create_track(Instrument::Lead);
    track.font = Some(FontRef::new("GeneralUser", 40));
    track.preset = Some(preset_with("x.y", "X", "", 33, 3.0));
    let (pitch, secs) = audition_params(&track, 60);
    assert_eq!(pitch, 60);
    assert!((secs - 0.8).abs() < 1e-6, "{secs}");

    // The 128 GM programs group into 16 families of 8.
    assert_eq!(gm_family(0), "Piano");
    assert_eq!(gm_family(7), "Piano");
    assert_eq!(gm_family(8), "Chromatic Percussion");
    assert_eq!(gm_family(40), "Strings");
    assert_eq!(gm_family(56), "Brass");
    assert_eq!(gm_family(80), "Synth Lead");
    assert_eq!(gm_family(120), "Sound FX");
    assert_eq!(gm_family(127), "Sound FX");
    assert_eq!(
        gm_family(200),
        "Sound FX",
        "out-of-range clamps like gm_program_name"
    );
}

#[test]
fn sound_picker_renders_the_font_section() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    // Fabricate catalog metadata (no parsed SoundFont needed — the picker
    // only reads FontInfo) plus a load error, then render the Track tab.
    ui.app.fonts.fonts.push(FontInfo {
        name: "GeneralUser".into(),
        key: crate::font::font_key("GeneralUser"),
        presets: 128,
    });
    ui.app.fonts.errors.push("bad.sf2: not a soundfont".into());
    ui.app.select_track(ui.app.project.tracks[0].id);
    ui.app.side_tab = SideTab::Track;
    ui.step();

    // With a font assigned the program combo renders too.
    let track = ui.app.project.tracks[0].clone();
    ui.app
        .apply_font_pick(&track, Some(FontRef::new("GeneralUser", 40)));
    ui.step();
    let t = &ui.app.project.tracks[0];
    assert_eq!(t.font.as_ref().unwrap().program, 40);
    assert_eq!(ops::instrument_label(t), "GeneralUser · Violin");
}

// ----- soundfont library bank and quick-add ----------------------------------

#[test]
fn soundfont_bank_lists_programs_and_double_click_adds_a_track() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    ui.app.fonts.fonts.push(FontInfo {
        name: "GeneralUser".into(),
        key: crate::font::font_key("GeneralUser"),
        presets: 128,
    });
    ui.app.library_bank = LibraryBank::Font;
    ui.step();

    // The GM catalog renders: 16 families of 8, first row the grand piano.
    let row = ui.app.library_rects.first_font_row;
    assert!(row.width() > 20.0, "first program row rendered: {row:?}");
    // Double-click adds, like every library row.
    ui.quick_dclick(row.center());
    assert_eq!(ui.app.project.tracks.len(), 2);
    let t = &ui.app.project.tracks[1];
    assert_eq!(t.font, Some(FontRef::new("GeneralUser", 0)));
    assert_eq!(t.name, "GeneralUser");
    assert!(!t.shows_drum_grid(), "font tracks use the piano roll");
    assert_eq!(
        ops::instrument_label(t),
        "GeneralUser · Acoustic Grand Piano"
    );
    assert!(ui.app.dirty());
    // The bank view is chrome only: filtering was not part of the edit.
    ui.app.undo();
    assert_eq!(ui.app.project.tracks.len(), 1);
    assert_eq!(ui.app.library_bank, LibraryBank::Font);
}

#[test]
fn soundfont_bank_search_filters_programs() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    ui.app.fonts.fonts.push(FontInfo {
        name: "GeneralUser".into(),
        key: crate::font::font_key("GeneralUser"),
        presets: 128,
    });
    ui.app.library_bank = LibraryBank::Font;
    ui.app.library_filter = "trumpet".into();
    ui.step();
    // Search hits the program name; the bank still renders its first row.
    let row = ui.app.library_rects.first_font_row;
    assert!(row.width() > 20.0, "filtered program row rendered: {row:?}");
    ui.app.library_filter = "no such program".into();
    ui.step();
    assert_eq!(
        ui.app.library_rects.first_font_row,
        Rect::NOTHING,
        "no rows when nothing matches"
    );
}

#[test]
fn soundfont_bank_without_fonts_shows_the_add_hint() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    ui.app.library_bank = LibraryBank::Font;
    ui.step();
    assert!(ui.app.fonts.fonts.is_empty());
    let hint = ui.app.library_rects.font_hint;
    assert!(hint.width() > 20.0, "hint button rendered: {hint:?}");
    ui.click(hint.center());
    // Headless has no native file manager: the click describes the folder
    // instead, proving the no-font path stays graceful.
    assert!(matches!(
        &ui.app.status,
        Some(super::StatusLine { kind: StatusKind::Info, text })
            if text.contains("SoundFonts folder")
    ));
    // The hint adds no track.
    assert_eq!(ui.app.project.tracks.len(), 1);
    assert!(!ui.app.dirty());
}

#[test]
fn quick_add_soundfont_row_adds_a_font_track() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    ui.app.fonts.fonts.push(FontInfo {
        name: "GeneralUser".into(),
        key: crate::font::font_key("GeneralUser"),
        presets: 128,
    });
    ui.app.side_tab = SideTab::Track;
    ui.ctx
        .data_mut(|d| d.insert_temp(egui::Id::new(QUICK_ADD_OPEN), true));
    ui.step();
    let row = ui
        .ctx
        .data(|d| d.get_temp::<Rect>(egui::Id::new(QUICK_ADD_FONT_RECT)))
        .expect("quick-add SoundFont row rendered");
    ui.quick_click(row.center());
    ui.step();
    assert_eq!(ui.app.project.tracks.len(), 2);
    let t = &ui.app.project.tracks[1];
    assert_eq!(t.font, Some(FontRef::new("GeneralUser", 0)));
    assert_eq!(t.name, "GeneralUser");
    assert!(!t.shows_drum_grid());
    assert!(ui.app.dirty());
    ui.app.undo();
    assert_eq!(ui.app.project.tracks.len(), 1);
}

#[test]
fn quick_add_without_fonts_shows_a_slim_add_hint() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    assert!(ui.app.fonts.fonts.is_empty());
    ui.app.side_tab = SideTab::Track;
    ui.ctx
        .data_mut(|d| d.insert_temp(egui::Id::new(QUICK_ADD_OPEN), true));
    ui.step();
    let row = ui
        .ctx
        .data(|d| d.get_temp::<Rect>(egui::Id::new(QUICK_ADD_FONT_HINT_RECT)))
        .expect("quick-add hint row rendered");
    ui.click(row.center());
    assert!(matches!(
        &ui.app.status,
        Some(super::StatusLine { kind: StatusKind::Info, text })
            if text.contains("SoundFonts folder")
    ));
    assert_eq!(ui.app.project.tracks.len(), 1, "the hint adds no track");
    assert!(!ui.app.dirty());
}

// ----- clipboard (copy / paste / cut) -------------------------------------

/// Replaces track `track_idx`'s first pattern with `notes` as
/// `(pitch, start, duration, velocity)` and selects them all.
fn plant_selected_notes(ui: &mut Ui, track_idx: usize, notes: &[(u8, f64, f64, f32)]) -> Vec<u64> {
    let pat = &mut ui.app.project.tracks[track_idx].patterns[0];
    pat.notes = notes
        .iter()
        .map(|&(pitch, start, duration, velocity)| Note::new(pitch, start, duration, velocity))
        .collect();
    let ids: Vec<u64> = pat.notes.iter().map(|n| n.id).collect();
    ui.app.sel.notes = ids.iter().copied().collect();
    ui.app.focus = super::Focus::Editor;
    ids
}

#[test]
fn clipboard_notes_copy_paste_at_playhead_and_undo() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    ui.app.add_track(Instrument::Piano);
    ui.step();
    let idx = ui.app.project.tracks.len() - 1;
    let orig = plant_selected_notes(&mut ui, idx, &[(60, 2.0, 1.0, 0.8), (64, 6.0, 2.0, 0.5)]);

    // Playhead at song step 20 wraps into the 16-step pattern at step 4;
    // the selection's left edge (step 2) becomes the paste origin.
    ui.app.transport.position = 20.0;
    ui.key_mod(Key::C, Modifiers::COMMAND);
    assert!(
        matches!(ui.app.clipboard, Clipboard::Notes(_)),
        "notes landed on the clipboard"
    );
    ui.key_mod(Key::V, Modifiers::COMMAND);

    let pat = ui.pattern(idx, 0);
    assert_eq!(pat.notes.len(), 4, "paste merges with existing notes");
    let pasted: Vec<_> = pat
        .notes
        .iter()
        .filter(|n| !orig.contains(&n.id))
        .map(|n| (n.pitch, n.start, n.duration, n.velocity))
        .collect();
    assert_eq!(
        pasted,
        vec![(60, 4.0, 1.0, 0.8), (64, 8.0, 2.0, 0.5)],
        "relative layout preserved at the wrapped playhead"
    );
    let sel: BTreeSet<u64> = pat
        .notes
        .iter()
        .filter(|n| !orig.contains(&n.id))
        .map(|n| n.id)
        .collect();
    assert_eq!(ui.app.sel.notes, sel, "pasted notes become the selection");

    ui.app.undo();
    assert_eq!(ui.pattern(idx, 0).notes.len(), 2, "undo removes the paste");
    ui.app.redo();
    assert_eq!(ui.pattern(idx, 0).notes.len(), 4, "redo restores the paste");
}

#[test]
fn clipboard_notes_paste_works_on_the_drum_grid() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    // Track 0 of an empty project is the built-in drums track — the drum
    // grid. Drum hits are ordinary notes, so the same clipboard applies.
    let tid = ui.app.project.tracks[0].id;
    ui.app.select_track(tid);
    ui.app.focus = super::Focus::Editor;
    let orig = plant_selected_notes(&mut ui, 0, &[(36, 0.0, 1.0, 1.0), (38, 4.0, 1.0, 0.6)]);

    ui.app.transport.position = 8.0;
    ui.key_mod(Key::C, Modifiers::COMMAND);
    ui.key_mod(Key::V, Modifiers::COMMAND);

    let pat = ui.pattern(0, 0);
    assert_eq!(pat.notes.len(), 4);
    let pasted: Vec<_> = pat
        .notes
        .iter()
        .filter(|n| !orig.contains(&n.id))
        .map(|n| (n.pitch, n.start))
        .collect();
    assert_eq!(pasted, vec![(36, 8.0), (38, 12.0)]);
}

#[test]
fn clipboard_copy_prefers_focused_domain_over_stale_selection() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    ui.app.add_track(Instrument::Piano);
    ui.step();
    let idx = ui.app.project.tracks.len() - 1;
    plant_selected_notes(&mut ui, idx, &[(60, 0.0, 1.0, 0.8)]);
    // A clip selected earlier in the arranger stays in sel.clips — editor
    // note selection never clears it. The focused domain must still win,
    // or ⌘C in the piano roll silently copies a clip instead.
    let clip_id = ui.app.project.tracks[idx].clips[0].id;
    ui.app.sel.clips.insert(clip_id);

    ui.key_mod(Key::C, Modifiers::COMMAND);
    assert!(
        matches!(ui.app.clipboard, Clipboard::Notes(_)),
        "editor-focused note copy beats the stale clip selection"
    );

    // Back in the arranger with both selections stale-ish, clips win again.
    ui.app.focus = super::Focus::Arranger;
    ui.key_mod(Key::C, Modifiers::COMMAND);
    assert!(
        matches!(ui.app.clipboard, Clipboard::Clips(..)),
        "arranger-focused copy prefers clips"
    );
}

#[test]
fn clipboard_clips_copy_paste_lands_on_same_tracks_at_playhead() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    ui.app.add_track(Instrument::Piano);
    ui.step();
    // One clip per track, both starting at step 0.
    let c0 = ui.app.project.tracks[0].clips[0].id;
    let c1 = ui.app.project.tracks[1].clips[0].id;
    ui.app
        .set_clip_selection(BTreeSet::from([c0, c1]), Some(c0));
    ui.app.focus = super::Focus::Arranger;
    // A selected note as well — clips still win the copy.
    let nid = {
        let pat = &mut ui.app.project.tracks[1].patterns[0];
        pat.notes.push(Note::new(60, 0.0, 1.0, 0.5));
        pat.notes[0].id
    };
    ui.app.sel.notes.insert(nid);

    ui.app.transport.position = 24.0;
    ui.key_mod(Key::C, Modifiers::COMMAND);
    assert!(
        matches!(ui.app.clipboard, Clipboard::Clips(..)),
        "clips beat notes on the clipboard"
    );
    ui.key_mod(Key::V, Modifiers::COMMAND);

    for (idx, orig) in [(0usize, c0), (1usize, c1)] {
        let track = &ui.app.project.tracks[idx];
        assert_eq!(track.clips.len(), 2, "track {idx} gained a pasted clip");
        let source = track.clips.iter().find(|c| c.id == orig).unwrap();
        let pasted = track.clips.iter().find(|c| c.id != orig).unwrap();
        assert_eq!(pasted.start, 24.0, "group's left edge on the playhead");
        assert_eq!(
            pasted.pattern_id, source.pattern_id,
            "clips keep referencing their pattern"
        );
        assert_eq!(pasted.loops, source.loops);
    }
    let sel = ui.app.selected_clip_ids();
    assert_eq!(sel.len(), 2, "the two pasted clips are selected");
    assert!(sel.is_disjoint(&BTreeSet::from([c0, c1])));

    ui.app.undo();
    assert!(
        ui.app.project.tracks.iter().all(|t| t.clips.len() == 1),
        "one undo removes both pasted clips"
    );
}

#[test]
fn clipboard_track_copy_paste_appends_an_independent_track() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    let src = ui.app.project.tracks[0].id;
    {
        let t = &mut ui.app.project.tracks[0];
        t.name = "Kit".into();
        t.volume = 0.4;
        t.pan = -0.3;
        t.mute = true;
        t.patterns[0].notes.push(Note::new(36, 0.0, 1.0, 1.0));
    }
    let src_pattern = ui.app.project.tracks[0].patterns[0].id;
    let src_note = ui.app.project.tracks[0].patterns[0].notes[0].id;
    ui.app.select_track(src);

    ui.key_mod(Key::C, Modifiers::COMMAND);
    assert!(matches!(ui.app.clipboard, Clipboard::Track(_)));
    ui.key_mod(Key::V, Modifiers::COMMAND);

    assert_eq!(ui.app.project.tracks.len(), 2);
    let copy = &ui.app.project.tracks[1];
    assert_eq!(
        ui.app.sel.track,
        Some(copy.id),
        "pasted track gets selected"
    );
    assert_eq!(copy.name, "Kit 2", "name uniquified like a new track");
    assert_ne!(copy.id, src);
    assert_ne!(
        copy.patterns[0].id, src_pattern,
        "patterns are not shared with the source"
    );
    assert_ne!(copy.patterns[0].notes[0].id, src_note);
    assert_eq!(
        copy.clips[0].pattern_id, copy.patterns[0].id,
        "clip remapped onto the copy's pattern"
    );
    assert!(
        (copy.volume - 0.4).abs() < 1e-6 && copy.pan == -0.3 && copy.mute,
        "mixer state carries over"
    );
    assert!(ui.app.project.validate().is_ok());

    // Repeated paste stacks another independent copy.
    ui.key_mod(Key::V, Modifiers::COMMAND);
    assert_eq!(ui.app.project.tracks.len(), 3);
    assert_eq!(ui.app.project.tracks[2].name, "Kit 3");

    ui.app.undo();
    ui.app.undo();
    assert_eq!(ui.app.project.tracks.len(), 1, "undo removes both pastes");
}

#[test]
fn clipboard_cut_notes_round_trips_through_paste() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    ui.app.add_track(Instrument::Piano);
    ui.step();
    let idx = ui.app.project.tracks.len() - 1;
    plant_selected_notes(&mut ui, idx, &[(60, 2.0, 1.0, 0.8), (64, 6.0, 2.0, 0.5)]);

    ui.key_mod(Key::X, Modifiers::COMMAND);
    assert!(ui.pattern(idx, 0).notes.is_empty(), "cut removes the notes");
    assert!(ui.app.sel.notes.is_empty());
    assert!(matches!(ui.app.clipboard, Clipboard::Notes(_)));

    ui.app.transport.position = 12.0;
    ui.key_mod(Key::V, Modifiers::COMMAND);
    let pat = ui.pattern(idx, 0);
    assert_eq!(pat.notes.len(), 2, "paste restores the cut notes");
    // Origin 12, relative starts 0 and 4 → 12 and 16→clamped 15.
    let mut starts: Vec<f64> = pat.notes.iter().map(|n| n.start).collect();
    starts.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert_eq!(starts, vec![12.0, 15.0], "pasted at the playhead");

    ui.app.undo();
    assert!(
        ui.pattern(idx, 0).notes.is_empty(),
        "undo removes the paste"
    );
    ui.app.undo();
    assert_eq!(
        ui.pattern(idx, 0).notes.len(),
        2,
        "second undo restores the cut — cut is one undo step"
    );
}

#[test]
fn clipboard_cut_clips_and_track() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    ui.app.add_track(Instrument::Piano);
    ui.step();
    // Cut the drums track's only clip, then paste it at step 16.
    let c0 = ui.app.project.tracks[0].clips[0].id;
    ui.app.set_clip_selection(BTreeSet::from([c0]), Some(c0));
    ui.app.focus = super::Focus::Arranger;
    ui.key_mod(Key::X, Modifiers::COMMAND);
    assert!(
        ui.app.project.tracks[0].clips.is_empty(),
        "cut removes the clip"
    );
    assert!(matches!(ui.app.clipboard, Clipboard::Clips(..)));
    ui.app.transport.position = 16.0;
    ui.key_mod(Key::V, Modifiers::COMMAND);
    let track = &ui.app.project.tracks[0];
    assert_eq!(track.clips.len(), 1);
    assert_eq!(track.clips[0].start, 16.0, "pasted clip lands at playhead");

    // Now cut the whole drums track (index 0): only the piano track remains,
    // and the paste appends the copy at the end rather than restoring index 0.
    ui.app.clear_selection();
    let drums = ui.app.project.tracks[0].id;
    ui.app.select_track(drums);
    ui.key_mod(Key::X, Modifiers::COMMAND);
    assert_eq!(ui.app.project.tracks.len(), 1, "cut removes the track");
    assert_eq!(ui.app.project.tracks[0].instrument, Instrument::Piano);
    assert!(matches!(ui.app.clipboard, Clipboard::Track(_)));
    ui.key_mod(Key::V, Modifiers::COMMAND);
    assert_eq!(ui.app.project.tracks.len(), 2);
    let pasted = &ui.app.project.tracks[1];
    assert_eq!(pasted.instrument, Instrument::Drums);
    assert_ne!(pasted.id, drums, "pasted track gets a fresh id");
    assert_eq!(ui.app.sel.track, Some(pasted.id));
}

#[test]
fn clipboard_empty_states_do_not_mutate_the_project() {
    // A song with no tracks at all: nothing is copyable or cuttable.
    let mut p = Project::empty();
    p.tracks.clear();
    let mut ui = Ui::new(CadenceApp::headless(p));
    ui.step();
    assert!(ui.app.sel.track.is_none());
    let state = ui.app.history.state();

    ui.key_mod(Key::C, Modifiers::COMMAND);
    let status = ui.app.status.as_ref().unwrap().text.clone();
    assert!(status.contains("Nothing to copy"), "{status}");
    ui.key_mod(Key::V, Modifiers::COMMAND);
    let status = ui.app.status.as_ref().unwrap().text.clone();
    assert!(status.contains("Nothing to paste"), "{status}");
    ui.key_mod(Key::X, Modifiers::COMMAND);
    let status = ui.app.status.as_ref().unwrap().text.clone();
    assert!(status.contains("Nothing to cut"), "{status}");
    assert_eq!(ui.app.history.state(), state, "no undo entries recorded");
    assert!(!ui.app.dirty());

    // A notes clipboard with no selected track warns instead of mutating.
    ui.app.clipboard = Clipboard::Notes(vec![ops::NoteSnap {
        pitch: 60,
        start: 0.0,
        duration: 1.0,
        velocity: 0.8,
    }]);
    ui.key_mod(Key::V, Modifiers::COMMAND);
    let status = ui.app.status.as_ref().unwrap().text.clone();
    assert!(status.contains("Select a track"), "{status}");
    assert_eq!(ui.app.history.state(), state);
    assert!(!ui.app.dirty());
}

#[test]
fn drag_after_text_focus_still_allows_clipboard_shortcuts() {
    // Regression: `wants_keyboard_input()` stays true while a TextEdit holds
    // focus, and the default `SurrenderFocusOn::Clicks` only releases it on a
    // full click — so drag gestures (marquee-selecting notes, scrubbing the
    // playhead) left stale focus that silently killed every editing shortcut.
    // `frame()` now configures `SurrenderFocusOn::Presses`.
    let mut ui = Ui::demo();
    ui.steps(3);

    // Focus the library search field the way a click on it does.
    let search = ui.app.library_rects.search;
    assert!(search.width() > 20.0, "search field rendered: {search:?}");
    ui.click(search.center());
    ui.steps(2);
    assert!(
        ui.ctx.memory(|m| m.focused().is_some()),
        "the search field holds keyboard focus"
    );

    // A press-drag outside the field must hand focus straight back — the
    // press half of a marquee selection is enough, no click required.
    ui.drag(pos2(420.0, 420.0), pos2(520.0, 460.0));
    ui.step();
    assert!(
        ui.ctx.memory(|m| m.focused().is_none()),
        "pointer-down outside the field surrendered focus"
    );

    // With focus cleared the clipboard shortcuts work again — nothing is
    // selected, so ⌘C takes the selected-track fallback.
    ui.key_mod(Key::C, Modifiers::COMMAND);
    assert!(
        !matches!(ui.app.clipboard, Clipboard::None),
        "⌘C copies once stale focus is gone"
    );
}

#[test]
fn clipboard_dedicated_events_drive_copy_paste_cut() {
    // egui-winit translates real ⌘C/⌘V/⌘X presses into Event::Copy/Paste/Cut
    // and drops the key event — the path the desktop app actually takes.
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    ui.app.add_track(Instrument::Piano);
    ui.step();
    let idx = ui.app.project.tracks.len() - 1;
    plant_selected_notes(&mut ui, idx, &[(60, 2.0, 1.0, 0.8)]);

    ui.event(Event::Copy);
    assert!(
        matches!(ui.app.clipboard, Clipboard::Notes(_)),
        "Event::Copy captured the notes"
    );

    ui.app.transport.position = 8.0;
    ui.event(Event::Paste("text on the OS clipboard".to_owned()));
    assert_eq!(
        ui.pattern(idx, 0).notes.len(),
        2,
        "Event::Paste pasted the notes (OS text ignored)"
    );

    ui.event(Event::Cut);
    assert_eq!(
        ui.pattern(idx, 0).notes.len(),
        1,
        "Event::Cut removed the selected (pasted) note"
    );
    assert!(
        matches!(ui.app.clipboard, Clipboard::Notes(_)),
        "cut leaves the notes on the clipboard"
    );
}

#[test]
fn clipboard_v_release_covers_empty_os_clipboard() {
    // ⌘V against an empty/non-text OS clipboard produces no Event::Paste at
    // all — the V-release key event is the fallback signal. A release paired
    // with a real Paste event must not paste a second time.
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    ui.app.add_track(Instrument::Piano);
    ui.step();
    let idx = ui.app.project.tracks.len() - 1;
    plant_selected_notes(&mut ui, idx, &[(60, 2.0, 1.0, 0.8)]);
    ui.event(Event::Copy);
    assert!(matches!(ui.app.clipboard, Clipboard::Notes(_)));

    let v_release = || Event::Key {
        key: Key::V,
        physical_key: None,
        pressed: false,
        repeat: false,
        modifiers: Modifiers::COMMAND,
    };

    // Empty-OS-clipboard path: the release alone pastes once.
    ui.app.transport.position = 8.0;
    ui.event(v_release());
    assert_eq!(
        ui.pattern(idx, 0).notes.len(),
        2,
        "V-release pasted when no Paste event existed"
    );

    // Paired-release path: the Paste event already pasted and armed the
    // guard, so the following release is swallowed.
    ui.app.transport.position = 12.0;
    ui.event(Event::Paste("os text".to_owned()));
    assert_eq!(ui.pattern(idx, 0).notes.len(), 3, "Paste event pasted");
    ui.event(v_release());
    assert_eq!(
        ui.pattern(idx, 0).notes.len(),
        3,
        "the release paired with Paste did not paste again"
    );
}

#[test]
fn clipboard_paste_notes_at_pointer_over_editor() {
    // With the pointer over the piano-roll grid, ⌘V lands the notes at the
    // pointer's step — not the playhead — and selects them all.
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    ui.app.add_track(Instrument::Piano);
    ui.step();
    let idx = ui.app.project.tracks.len() - 1;
    let track_id = ui.app.project.tracks[idx].id;
    show_pitch(&mut ui, track_id, 100);
    let orig = plant_selected_notes(&mut ui, idx, &[(60, 2.0, 1.0, 0.8)]);
    ui.key_mod(Key::C, Modifiers::COMMAND);
    assert!(matches!(ui.app.clipboard, Clipboard::Notes(_)));

    // Playhead sits at step 0; the pointer hovers step 12's cell instead.
    ui.app.transport.position = 0.0;
    let center = ui
        .app
        .roll_cell_center(100, 12.0)
        .expect("a grid point inside step 12");
    ui.at(center);
    ui.step();
    ui.key_mod(Key::V, Modifiers::COMMAND);

    let pat = ui.pattern(idx, 0);
    let pasted: Vec<_> = pat.notes.iter().filter(|n| !orig.contains(&n.id)).collect();
    assert_eq!(pasted.len(), 1, "one note pasted");
    assert_eq!(
        pasted[0].start, 12.0,
        "pasted at the pointer's step, not the playhead"
    );
    assert_eq!(
        pasted[0].pitch, 100,
        "the selection transposes so its nearest note lands on the pointer's pitch row"
    );
    assert!(
        ui.app.sel.notes.contains(&pasted[0].id),
        "pasted notes become the selection"
    );

    // Pointer off the grid (mixer tab) falls back to the playhead.
    ui.app.tab = super::Tab::Mixer;
    ui.app.transport.position = 4.0;
    ui.key_mod(Key::V, Modifiers::COMMAND);
    let pat = ui.pattern(idx, 0);
    let starts: Vec<f64> = pat.notes.iter().map(|n| n.start).collect();
    assert!(
        starts.contains(&4.0),
        "playhead fallback pasted at step 4: {starts:?}"
    );
}

// ----- computer keyboard performance & chord strip ---------------------------

use super::editor::{
    drum_keyboard_pitches, held_keyboard_keys, kb_key_index, key_to_pitch, scale_pitch_classes,
};

#[test]
fn keyboard_mapping_is_chromatic_and_octave_aware() {
    // FL-Studio two-row layout, in chromatic order: whites Z X C V B N M =
    // C..B on the bottom row, blacks S D G H J = C# D# F# G# A# in the gaps.
    let order = [
        Key::Z,
        Key::S,
        Key::X,
        Key::D,
        Key::C,
        Key::V,
        Key::G,
        Key::B,
        Key::H,
        Key::N,
        Key::J,
        Key::M,
    ];
    for (i, key) in order.iter().enumerate() {
        assert_eq!(kb_key_index(*key), Some(i), "{key:?} chromatic index");
        assert_eq!(
            key_to_pitch(*key, 4),
            Some(60 + i as u8),
            "{key:?} in octave 4"
        );
    }
    // Octave shifts move the whole row; unmapped keys — including L (loop),
    // Q/W (octave controls) and letter E/F/I/K/O/P/R/T/U/Y/A — produce nothing.
    assert_eq!(key_to_pitch(Key::Z, 2), Some(36), "C2 for bass");
    assert_eq!(key_to_pitch(Key::M, 5), Some(83));
    for key in [
        Key::A,
        Key::E,
        Key::F,
        Key::I,
        Key::K,
        Key::L,
        Key::O,
        Key::P,
        Key::Q,
        Key::R,
        Key::T,
        Key::U,
        Key::W,
        Key::Y,
    ] {
        assert_eq!(key_to_pitch(key, 4), None, "{key:?} must stay unmapped");
    }
    // Out-of-range octaves degrade per-key instead of wrapping.
    assert_eq!(key_to_pitch(Key::Z, 9), Some(120));
    assert_eq!(key_to_pitch(Key::M, 9), None, "B9 = 131 > 127");
    assert_eq!(key_to_pitch(Key::Z, -2), None);
    assert_eq!(key_to_pitch(Key::M, -1), Some(11));
    // Every mapped key lands on a distinct pitch class.
    let pcs: BTreeSet<u8> = order
        .iter()
        .map(|k| key_to_pitch(*k, 4).unwrap() % 12)
        .collect();
    assert_eq!(pcs.len(), 12);
}

#[test]
fn drum_keyboard_maps_sorted_piece_pitches() {
    // Built-in kit: the eight DRUMS rows sorted ascending, capped at 12 keys.
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    let pitches = drum_keyboard_pitches(&ui.app.project.tracks[0]);
    assert_eq!(pitches, vec![36, 38, 39, 42, 45, 46, 49, 51]);
    // Key order is chromatic, so Z→lowest piece, S→next, …
    assert_eq!(key_to_pitch(Key::Z, 4), Some(60)); // sanity: melodic path unaffected
                                                   // A kit preset's pieces sort by pitch regardless of row order, and the
                                                   // mapping caps at the 12 available keys.
    let mut preset = test_kit_preset();
    preset.kit.as_mut().unwrap().pieces.push(KitPiece {
        name: "Rim".into(),
        pitch: 70,
        voice: kit_voice(),
    });
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    ui.app.add_preset_track(&preset);
    ui.step();
    let kit_track = ui.app.project.tracks.last().unwrap();
    let pitches = drum_keyboard_pitches(kit_track);
    assert_eq!(
        pitches,
        vec![39, 42, 60, 70],
        "sorted, deduped piece pitches"
    );
    assert!(pitches.len() <= 12);
}

#[test]
fn keyboard_held_keys_track_press_release_and_modifiers() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    ui.app.add_track(Instrument::Piano);
    ui.step();
    let tid = ui.app.project.tracks[1].id;
    ui.app.select_track(tid);
    ui.step();
    assert_eq!(ui.app.editor.kb_octave, 4, "piano homes on C4");

    // Hold Z (index 0 = C): the held set reports it until release.
    ui.next.modifiers = Modifiers::NONE;
    ui.next.events.push(Event::Key {
        key: Key::Z,
        physical_key: Some(Key::Z),
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    });
    ui.step();
    assert!(held_keyboard_keys(&ui.ctx).contains(&0));

    // Alt+X is a shortcut chord, not a note — nothing counts while Alt is down.
    ui.next.modifiers = Modifiers::ALT;
    ui.next.events.push(Event::Key {
        key: Key::X,
        physical_key: Some(Key::X),
        pressed: true,
        repeat: false,
        modifiers: Modifiers::ALT,
    });
    ui.step();
    assert!(
        held_keyboard_keys(&ui.ctx).is_empty(),
        "Alt-held presses are shortcuts, not notes"
    );
    // Modifiers released: both held keys count again.
    ui.next.modifiers = Modifiers::NONE;
    ui.step();
    let held = held_keyboard_keys(&ui.ctx);
    assert!(held.contains(&0) && held.contains(&2), "held: {held:?}");

    // Releases clear the set.
    for key in [Key::Z, Key::X] {
        ui.next.events.push(Event::Key {
            key,
            physical_key: Some(key),
            pressed: false,
            repeat: false,
            modifiers: Modifiers::NONE,
        });
    }
    ui.step();
    assert!(held_keyboard_keys(&ui.ctx).is_empty());

    // Q/W shift the keyboard octave; it clamps inside the MIDI range.
    ui.key(Key::Q);
    assert_eq!(ui.app.editor.kb_octave, 3);
    ui.key(Key::W);
    ui.key(Key::W);
    assert_eq!(ui.app.editor.kb_octave, 5);
    for _ in 0..10 {
        ui.key(Key::W);
    }
    assert_eq!(ui.app.editor.kb_octave, 8, "octave clamps at the top");
}

#[test]
fn keyboard_play_auditions_without_touching_the_project() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    ui.app.add_track(Instrument::Piano);
    ui.step();
    let tid = ui.app.project.tracks[1].id;
    ui.app.select_track(tid);
    ui.step();
    let state = ui.app.history.state();
    let was_dirty = ui.app.dirty();
    for key in [
        Key::Z,
        Key::S,
        Key::X,
        Key::D,
        Key::C,
        Key::V,
        Key::G,
        Key::B,
        Key::H,
        Key::N,
        Key::J,
        Key::M,
    ] {
        ui.key(key);
    }
    ui.step();
    assert_eq!(ui.app.history.state(), state, "auditioning is not an edit");
    assert_eq!(ui.app.dirty(), was_dirty, "audition adds no edits");
    assert!(ui.pattern(1, 0).notes.is_empty());
    // No selected track: presses are safely ignored.
    ui.app.sel.track = None;
    ui.key(Key::Z);
    ui.step();
}

#[test]
fn chord_strip_stamps_diatonic_chord_block_undoable() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    ui.app.add_track(Instrument::Piano);
    ui.step();
    let idx = ui.app.project.tracks.len() - 1;
    let tid = ui.app.project.tracks[idx].id;
    ui.app.select_track(tid);
    ui.step();

    // I chord of the default C-major key = C major triad at the piano's
    // home octave (C4 area), half the 16-step pattern long.
    ui.app.stamp_diatonic_chord(0, &ui.ctx);
    ui.step();
    let pat = ui.pattern(idx, 0);
    let mut pitches: Vec<u8> = pat.notes.iter().map(|n| n.pitch).collect();
    pitches.sort_unstable();
    assert_eq!(pitches, vec![60, 64, 67], "C4 E4 G4 block chord");
    assert!(
        pat.notes
            .iter()
            .all(|n| n.start == 0.0 && n.duration == 8.0),
        "stamped at the playhead, half-pattern duration"
    );
    assert_eq!(ui.app.sel.notes.len(), 3, "stamped notes selected");
    assert_eq!(ui.app.focus, super::Focus::Editor);
    assert!(
        ui.app
            .status
            .as_ref()
            .is_some_and(|s| s.text.contains("Stamped")),
        "status reports the stamped chord: {:?}",
        ui.app.status
    );

    ui.app.undo();
    assert!(ui.pattern(idx, 0).notes.is_empty(), "one undo removes it");
    ui.app.redo();
    assert_eq!(ui.pattern(idx, 0).notes.len(), 3, "redo restores it");

    // ii chord of C major = D minor: D4 F4 A4.
    ui.app.stamp_diatonic_chord(1, &ui.ctx);
    let sel_pitches: Vec<u8> = ui
        .pattern(idx, 0)
        .notes
        .iter()
        .filter(|n| ui.app.sel.notes.contains(&n.id))
        .map(|n| n.pitch)
        .collect();
    let mut sel_pitches = sel_pitches;
    sel_pitches.sort_unstable();
    assert_eq!(sel_pitches, vec![62, 65, 69], "D4 F4 A4 = ii in C");
}

#[test]
fn stamp_progression_fills_the_pattern() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    ui.app.add_track(Instrument::Piano);
    ui.step();
    let idx = ui.app.project.tracks.len() - 1;
    ui.app.select_track(ui.app.project.tracks[idx].id);
    ui.step();
    let bar = ops::STEPS_PER_BAR;
    assert_eq!(ui.pattern(idx, 0).length, bar, "default one-bar pattern");
    let n_chords = ops::progression_specs(ui.app.project.key, &ops::PROGRESSIONS[0], None).len();
    assert!(n_chords >= 2, "fixture preset has multiple chords");

    // One-bar pattern: the progression subdivides evenly, one slot per chord.
    ui.app.stamp_progression(&ops::PROGRESSIONS[0], &ui.ctx);
    ui.step();
    let pat = ui.pattern(idx, 0);
    assert_eq!(pat.length, bar, "the pattern keeps its length");
    let slot = f64::from(bar) / n_chords as f64;
    let mut starts: Vec<f64> = pat.notes.iter().map(|n| n.start).collect();
    starts.sort_by(f64::total_cmp);
    starts.dedup();
    assert_eq!(starts.len(), n_chords, "one distinct start per chord");
    for (i, at) in starts.iter().enumerate() {
        assert_eq!(*at, i as f64 * slot, "chord {i} gets an even share");
    }
    ui.app.undo();
    assert!(ui.pattern(idx, 0).notes.is_empty(), "one undo removes it");

    // Eight-bar pattern: the loop repeats at one chord per bar.
    ui.app.project.tracks[idx].patterns[0].length = bar * 8;
    ui.app.stamp_progression(&ops::PROGRESSIONS[0], &ui.ctx);
    ui.step();
    let pat = ui.pattern(idx, 0);
    let mut starts: Vec<f64> = pat.notes.iter().map(|n| n.start).collect();
    starts.sort_by(f64::total_cmp);
    starts.dedup();
    assert_eq!(starts.len(), 8, "the loop covers every bar");
    for (i, at) in starts.iter().enumerate() {
        assert_eq!(
            *at,
            i as f64 * f64::from(bar),
            "bar {i} gets a chord — the loop repeats"
        );
    }
}

#[test]
fn chord_strip_strum_rolls_notes_in_order() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    ui.app.add_track(Instrument::Piano);
    ui.step();
    let idx = ui.app.project.tracks.len() - 1;
    ui.app.select_track(ui.app.project.tracks[idx].id);
    ui.step();

    ui.app.editor.chord_spacing = 1.0;
    ui.app.editor.chord_direction = ops::StrumDirection::Down;
    ui.app.stamp_diatonic_chord(0, &ui.ctx);
    ui.step();
    let mut got: Vec<(u8, f64)> = ui
        .pattern(idx, 0)
        .notes
        .iter()
        .map(|n| (n.pitch, n.start))
        .collect();
    got.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    assert_eq!(
        got,
        vec![(67, 0.0), (64, 1.0), (60, 2.0)],
        "Down strum: highest first, one step apart"
    );

    // Alt direction alternates outside-in: low, high, next-low.
    ui.app.undo();
    ui.app.editor.chord_direction = ops::StrumDirection::Alt;
    ui.app.stamp_diatonic_chord(0, &ui.ctx);
    let mut got: Vec<(u8, f64)> = ui
        .pattern(idx, 0)
        .notes
        .iter()
        .map(|n| (n.pitch, n.start))
        .collect();
    got.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    assert_eq!(
        got,
        vec![(60, 0.0), (67, 1.0), (64, 2.0)],
        "Alt strum: low, high, next-low"
    );
}

#[test]
fn chord_strip_quality_override_changes_tones() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    ui.app.add_track(Instrument::Piano);
    ui.step();
    let idx = ui.app.project.tracks.len() - 1;
    ui.app.select_track(ui.app.project.tracks[idx].id);
    ui.step();

    ui.app.editor.chord_quality = Some(ChordQuality::Minor7);
    ui.app.stamp_diatonic_chord(0, &ui.ctx);
    let mut pitches: Vec<u8> = ui.pattern(idx, 0).notes.iter().map(|n| n.pitch).collect();
    pitches.sort_unstable();
    assert_eq!(
        pitches,
        vec![60, 63, 67, 70],
        "C minor7 = C Eb G Bb overrides the diatonic major triad"
    );
}

#[test]
fn chord_strip_rejects_drum_tracks() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    // Track 0 of an empty project is the built-in drums track.
    let tid = ui.app.project.tracks[0].id;
    ui.app.select_track(tid);
    ui.step();
    ui.app.stamp_diatonic_chord(0, &ui.ctx);
    ui.step();
    assert!(
        ui.pattern(0, 0).notes.is_empty(),
        "no chord tones on the drum grid"
    );
    let status = ui.app.status.as_ref().expect("a warning status");
    assert!(
        status.text.contains("melodic"),
        "warning explains why: {}",
        status.text
    );
}

#[test]
fn scale_highlight_toggles_and_marks_scale_pitch_classes() {
    // C major = the white keys.
    assert_eq!(
        scale_pitch_classes(SongKey::new(0, KeyMode::Major)),
        [true, false, true, false, true, true, false, true, false, true, false, true]
    );
    // A natural minor shares C major's collection.
    assert_eq!(
        scale_pitch_classes(SongKey::new(9, KeyMode::Minor)),
        scale_pitch_classes(SongKey::new(0, KeyMode::Major))
    );
    // D major adds F# and C#.
    let d = scale_pitch_classes(SongKey::new(2, KeyMode::Major));
    assert!(d[1] && d[6] && !d[3], "C# and F# in, D# out");

    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    assert!(!ui.app.editor.scale_highlight);
    ui.app.toggle_scale_highlight();
    assert!(ui.app.editor.scale_highlight);
    ui.app.toggle_scale_highlight();
    assert!(!ui.app.editor.scale_highlight);
    // Frames with the tint on exercise the row-highlight paint path.
    ui.app.editor.scale_highlight = true;
    ui.steps(2);
}

// ----- MIDI recording ------------------------------------------------------

/// Adds and selects a piano track — the melodic lane recording tests play
/// into — and returns its track index.
fn plant_record_track(ui: &mut Ui) -> usize {
    ui.app.add_track(Instrument::Piano);
    ui.step();
    let idx = ui.app.project.tracks.len() - 1;
    ui.app.select_track(ui.app.project.tracks[idx].id);
    ui.app.focus = super::Focus::Editor;
    ui.step();
    idx
}

fn press_key(ui: &mut Ui, key: Key, pressed: bool) {
    ui.event(Event::Key {
        key,
        physical_key: Some(key),
        pressed,
        repeat: false,
        modifiers: Modifiers::NONE,
    });
}

#[test]
fn record_arm_toggles_with_r_and_reports_status() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    assert!(!ui.app.transport.recording);
    // Disarmed, playing a key inserts nothing.
    ui.key(Key::Z);
    assert!(
        ui.pattern(0, 0).notes.is_empty(),
        "keys only record when armed"
    );
    ui.key(Key::R);
    assert!(ui.app.transport.recording, "R arms recording");
    let status = ui
        .app
        .status
        .as_ref()
        .expect("arming sets a status")
        .text
        .clone();
    assert!(status.contains("Recording"), "status: {status}");
    ui.key(Key::R);
    assert!(!ui.app.transport.recording, "R disarms recording");
}

#[test]
fn record_helpers_snap_starts_durations_and_find_the_covering_pattern() {
    assert_eq!(super::editor::record_start(10.3, 1.0), 10.0);
    assert_eq!(super::editor::record_start(10.9, 0.5), 10.5);
    assert_eq!(super::editor::record_duration(10.0, 12.7, 1.0), 3.0);
    assert_eq!(
        super::editor::record_duration(4.0, 4.0, 1.0),
        1.0,
        "a tap still lasts one snap cell"
    );
    assert_eq!(
        super::editor::record_duration(30.0, 2.0, 1.0),
        1.0,
        "a loop wrap reads as one cell"
    );
    // A one-bar clip at step 0 looped twice covers 0..32; a position inside
    // the second repetition folds back into the pattern.
    let mut track = crate::model::create_track(Instrument::Lead);
    track.clips[0].loops = 2;
    let pid = track.clips[0].pattern_id;
    assert_eq!(
        super::editor::covering_pattern(&track, 20.0),
        Some((pid, 4.0))
    );
    assert_eq!(
        super::editor::covering_pattern(&track, 16.0),
        Some((pid, 0.0))
    );
    assert_eq!(super::editor::covering_pattern(&track, 32.0), None);
}

#[test]
fn step_record_inserts_at_the_playhead_and_advances_on_release() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    let idx = plant_record_track(&mut ui);
    ui.app.transport.position = 4.0;
    ui.app.toggle_record();
    // Hold Z (= C at the keyboard's home octave)…
    press_key(&mut ui, Key::Z, true);
    let pat = ui.pattern(idx, 0);
    assert_eq!(pat.notes.len(), 1, "press inserts while held");
    assert_eq!(
        pat.notes[0].pitch,
        super::editor::key_to_pitch(Key::Z, ui.app.editor.kb_octave).unwrap()
    );
    assert_eq!(pat.notes[0].start, 4.0, "inserted at the playhead");
    assert_eq!(ui.app.transport.position, 4.0, "held: the playhead waits");
    // …then release: the playhead advances by one Snap cell.
    press_key(&mut ui, Key::Z, false);
    assert_eq!(ui.app.transport.position, 5.0, "release advances one snap");
    assert_eq!(
        ui.pattern(idx, 0).notes[0].duration,
        1.0,
        "one snap cell long"
    );
    // The insertion is an ordinary undo step.
    ui.app.undo();
    assert!(ui.pattern(idx, 0).notes.is_empty(), "undo removes the note");
}

#[test]
fn live_record_quantizes_to_the_snap_grid() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    let idx = plant_record_track(&mut ui);
    ui.app.toggle_record();
    // The transport is engine-driven during playback; headless tests set it
    // directly — the insertion code only reads `transport.position`.
    ui.app.transport.playing = true;
    ui.app.transport.position = 10.3;
    let id = ui.app.record_note_on(64).expect("note inserted");
    let note = ui
        .pattern(idx, 0)
        .notes
        .iter()
        .find(|n| n.id == id)
        .unwrap();
    assert_eq!(note.start, 10.0, "the start snaps down onto the grid");
    assert_eq!(
        note.velocity, ui.app.editor.velocity,
        "recorded notes use the editor velocity"
    );
    let status = ui.app.status.as_ref().unwrap().text.clone();
    assert!(status.contains("Recorded 1 note"), "status: {status}");
    ui.app.transport.position = 12.7;
    ui.app.record_note_off(64);
    let note = ui
        .pattern(idx, 0)
        .notes
        .iter()
        .find(|n| n.id == id)
        .unwrap();
    assert_eq!(
        note.duration, 3.0,
        "duration = ceil((release − start) / snap)"
    );
    assert_eq!(
        ui.app.transport.position, 12.7,
        "live release must not move the playhead"
    );
    // With Snap at 1/8 (2 steps) everything rounds to whole eighth notes.
    ui.app.editor.snap = 2.0;
    ui.app.transport.position = 21.0;
    let id2 = ui.app.record_note_on(65).expect("second note inserted");
    ui.app.transport.position = 26.5;
    ui.app.record_note_off(65);
    let note2 = ui
        .pattern(idx, 0)
        .notes
        .iter()
        .find(|n| n.id == id2)
        .unwrap();
    assert_eq!(
        note2.start, 4.0,
        "step 20 folds into the 16-step pattern at 4"
    );
    assert_eq!(note2.duration, 8.0, "6.5 steps rounds up to 4 eighth notes");
}

#[test]
fn record_extends_the_current_clip_to_cover_the_playhead() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    let idx = plant_record_track(&mut ui);
    ui.app.toggle_record();
    // The one-bar clip ends at step 16; recording at step 20 grows its loop
    // count so the pattern covers the playhead.
    assert_eq!(ui.app.project.tracks[idx].clips[0].loops, 1);
    ui.app.transport.position = 20.0;
    let id = ui.app.record_note_on(62).expect("note inserted");
    let track = &ui.app.project.tracks[idx];
    assert_eq!(track.clips[0].loops, 2, "the clip extends to cover step 20");
    let pat = ops::pattern(track, track.clips[0].pattern_id).unwrap();
    let note = pat.notes.iter().find(|n| n.id == id).unwrap();
    assert_eq!(
        note.start, 4.0,
        "step 20 folds into the 16-step pattern at 4"
    );
    // The extension and the note belong to one recording take.
    ui.app.undo();
    let track = &ui.app.project.tracks[idx];
    assert_eq!(track.clips[0].loops, 1);
    assert!(ops::pattern(track, track.clips[0].pattern_id)
        .unwrap()
        .notes
        .is_empty());
}

#[test]
fn record_skips_audio_lanes_and_uncovered_playheads() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    ui.app.add_audio_track();
    ui.step();
    let idx = ui.app.project.tracks.len() - 1;
    ui.app.select_track(ui.app.project.tracks[idx].id);
    ui.app.toggle_record();
    assert!(
        ui.app.record_note_on(60).is_none(),
        "audio lanes hold no patterns"
    );
    let status = ui.app.status.as_ref().unwrap().text.clone();
    assert!(status.contains("MIDI tracks"), "status: {status}");

    // A pattern clip moved past the playhead cannot be extended backwards.
    let idx2 = plant_record_track(&mut ui);
    let track = ui.app.project.tracks[idx2].clone();
    let (tid, clip_id) = (track.id, track.clips[0].id);
    ui.app.edit("Move clip", None, |p| {
        let _ = ops::move_clip(p, tid, clip_id, 100.0);
    });
    ui.app.transport.position = 5.0;
    assert!(
        ui.app.record_note_on(60).is_none(),
        "clip starts after the playhead"
    );
    let status = ui.app.status.as_ref().unwrap().text.clone();
    assert!(status.contains("no clip covers"), "status: {status}");
}

#[test]
fn record_release_ends_the_note_after_an_octave_change() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    let idx = plant_record_track(&mut ui);
    ui.app.transport.position = 4.0;
    ui.app.toggle_record();
    let octave = ui.app.editor.kb_octave;
    press_key(&mut ui, Key::Z, true);
    ui.key(Key::W);
    assert_eq!(ui.app.editor.kb_octave, octave + 1, "W shifts the octave");
    press_key(&mut ui, Key::Z, false);
    assert!(
        ui.app.editor.record_pending.is_empty(),
        "the release ends the note that Z started"
    );
    assert_eq!(ui.app.transport.position, 5.0, "release advances one snap");
    assert_eq!(
        ui.pattern(idx, 0).notes[0].pitch,
        super::editor::key_to_pitch(Key::Z, octave).unwrap()
    );
}

#[test]
fn record_release_is_processed_with_a_modifier_held() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    plant_record_track(&mut ui);
    ui.app.transport.position = 4.0;
    ui.app.toggle_record();
    press_key(&mut ui, Key::Z, true);
    assert_eq!(ui.app.editor.record_pending.len(), 1);
    ui.event(Event::Key {
        key: Key::Z,
        physical_key: Some(Key::Z),
        pressed: false,
        repeat: false,
        modifiers: Modifiers::COMMAND,
    });
    assert!(ui.app.editor.record_pending.is_empty());
    assert_eq!(ui.app.transport.position, 5.0);
}

#[test]
fn stopping_or_disarming_finalizes_held_recorded_notes() {
    for disarm in [false, true] {
        let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
        ui.step();
        let idx = plant_record_track(&mut ui);
        ui.app.toggle_record();
        ui.app.transport.playing = true;
        ui.app.transport.position = 2.3;
        let id = ui.app.record_note_on(64).expect("note inserted");
        ui.app.transport.position = 6.5;
        if disarm {
            ui.app.toggle_record();
        } else {
            ui.app.stop();
        }
        assert!(ui.app.editor.record_pending.is_empty(), "disarm: {disarm}");
        let note = ui
            .pattern(idx, 0)
            .notes
            .iter()
            .find(|n| n.id == id)
            .unwrap();
        assert_eq!(note.duration, 5.0, "held from 2 to 6.5 (disarm: {disarm})");
    }
}

#[test]
fn one_recording_take_is_one_undo_step() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    let idx = plant_record_track(&mut ui);
    let before = ui.app.project.clone();
    ui.app.toggle_record();
    ui.app.transport.playing = true;
    for (i, pitch) in [60u8, 62, 64].into_iter().enumerate() {
        ui.app.transport.position = 4.0 + 8.0 * i as f64;
        ui.app.record_note_on(pitch).expect("note inserted");
        ui.app.transport.position += 1.0;
        ui.app.record_note_off(pitch);
        // A click anywhere between notes must not split the take.
        ui.click(egui::pos2(5.0, 5.0));
    }
    ui.app.stop();
    let track = &ui.app.project.tracks[idx];
    assert_eq!(track.clips[0].loops, 2, "the take extended the clip");
    assert_eq!(ui.pattern(idx, 0).notes.len(), 3);
    ui.app.undo();
    assert_eq!(ui.app.project, before, "one undo removes the whole take");
    ui.app.redo();
    assert_eq!(ui.pattern(idx, 0).notes.len(), 3);

    // Still armed after Stop: the next take is a separate step.
    assert!(ui.app.transport.recording);
    ui.app.transport.position = 1.0;
    ui.app.record_note_on(65).expect("note inserted");
    ui.app.record_note_off(65);
    ui.app.toggle_record();
    ui.app.undo();
    assert_eq!(ui.pattern(idx, 0).notes.len(), 3);
}

#[test]
fn count_in_preference_toggles_and_round_trips_through_storage() {
    let storage = MemStorage::default();
    assert!(!super::transport::count_in_from_storage(Some(&storage)));
    assert!(!super::transport::count_in_from_storage(None));
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    assert!(!ui.app.transport.count_in, "off by default");
    ui.app.toggle_count_in();
    assert!(ui.app.transport.count_in);
    let mut storage = MemStorage::default();
    eframe::App::save(&mut ui.app, &mut storage);
    assert!(super::transport::count_in_from_storage(Some(&storage)));
    ui.app.toggle_count_in();
    eframe::App::save(&mut ui.app, &mut storage);
    assert!(!super::transport::count_in_from_storage(Some(&storage)));
}

#[test]
fn notes_played_during_the_count_in_are_not_recorded() {
    let mut ui = Ui::new(CadenceApp::headless(Project::empty()));
    ui.step();
    let idx = plant_record_track(&mut ui);
    ui.app.toggle_record();
    ui.app.transport.playing = true;
    ui.app.transport.count_in_left = 8.0;
    assert!(ui.app.record_note_on(60).is_none());
    assert!(ui.pattern(idx, 0).notes.is_empty());
    assert!(ui.app.editor.record_pending.is_empty());
    ui.app.transport.count_in_left = 0.0;
    assert!(
        ui.app.record_note_on(60).is_some(),
        "recording after the bar"
    );
}

/// Regression probe for the library/quick-add click fix: a click interact
/// *inside* `dnd_drag_source` can never fire (egui drops click hits under a
/// drag-only top widget), but a click interact registered *after* the drag
/// source — on the same rect — coexists with it: clicks land on the late
/// interact while drags still belong to the drag source.
#[test]
fn scratch_dnd_row_click_probe() {
    let ctx = egui::Context::default();
    let mut inner_clicked = false;
    let mut outer_clicked = false;
    let mut run = |ctx: &egui::Context| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let dnd_id = ui.id().with("probe");
            let dnd = ui.dnd_drag_source(dnd_id, 1usize, |ui| {
                let (_, resp) =
                    ui.allocate_exact_size(egui::vec2(200.0, 30.0), egui::Sense::click());
                inner_clicked |= resp.clicked();
            });
            outer_clicked |= ui
                .interact(
                    dnd.response.rect,
                    dnd_id.with("click"),
                    egui::Sense::click(),
                )
                .clicked();
        });
    };
    let mut step = |events: Vec<Event>| {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1360.0, 860.0))),
            focused: true,
            events,
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| run(ctx));
    };
    step(vec![Event::PointerMoved(pos2(100.0, 20.0))]);
    step(vec![
        Event::PointerMoved(pos2(100.0, 20.0)),
        Event::PointerButton {
            pos: pos2(100.0, 20.0),
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        },
    ]);
    step(vec![
        Event::PointerMoved(pos2(100.0, 20.0)),
        Event::PointerButton {
            pos: pos2(100.0, 20.0),
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        },
    ]);
    step(vec![]);
    assert!(!inner_clicked, "the body's click interact stays shadowed");
    assert!(
        outer_clicked,
        "the post-drag-source interact receives the click"
    );
}

/// egui paints "🔥 First/Second use of widget ID …" over widgets that share an
/// id (debug builds only — the clash itself is real in every build and mixes
/// up hover, press and tooltip state). No screen may produce one.
#[test]
fn no_screen_paints_widget_id_clash_warnings() {
    let mut ui = Ui::demo();
    ui.ctx.options_mut(|o| o.warn_on_id_clash = true);
    let tracks: Vec<u64> = ui.app.project.tracks.iter().map(|t| t.id).collect();
    let mut clashes = BTreeSet::new();
    for tab in [super::Tab::Editor, super::Tab::Mixer, super::Tab::Keys] {
        for side in [SideTab::Library, SideTab::Track] {
            for &track in &tracks {
                ui.app.tab = tab;
                ui.app.side_tab = side;
                ui.app.sel.track = Some(track);
                // Toolbars settle their measured layout over a few frames.
                ui.steps(3);
                ui.next.time = Some(ui.time);
                let input = std::mem::replace(&mut ui.next, base_input());
                let out = ui.ctx.run(input, |ctx| ui.app.frame(ctx));
                for clipped in &out.shapes {
                    if let egui::Shape::Text(text) = &clipped.shape {
                        let s = text.galley.text();
                        if s.contains(" use of ") && s.contains(" ID ") {
                            clashes.insert(format!("{tab:?}/{side:?}/track {track}: {s}"));
                        }
                    }
                }
            }
        }
    }
    assert!(clashes.is_empty(), "id clashes:\n{clashes:#?}");
}

#[test]
fn roll_notes_light_up_only_while_the_playhead_is_inside_them() {
    use super::editor::note_is_live;
    let n = Note::new(60, 4.0, 2.5, 0.8);
    assert!(!note_is_live(&n, None), "stopped: nothing lights");
    assert!(!note_is_live(&n, Some(3.99)));
    assert!(note_is_live(&n, Some(4.0)), "lights on its start");
    assert!(note_is_live(&n, Some(6.49)));
    assert!(!note_is_live(&n, Some(6.5)), "off again at its end");
}
