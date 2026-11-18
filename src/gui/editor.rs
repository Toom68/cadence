//! Pattern editor: drum step sequencer and melodic piano roll.
//! Both are custom-painted canvases with their own scroll/zoom, note
//! draw/drag/resize/box-select, a velocity lane and drum accent painting.

use std::collections::{BTreeSet, HashMap};

use egui::{
    pos2, vec2, Align, Align2, Color32, Context, CornerRadius, CursorIcon, Event, FontId, Key,
    Layout, Margin, PointerButton, Pos2, Rect, RichText, Sense, Shape, Stroke, StrokeKind, Ui,
};

use super::arranger::snap_floor;
use super::theme;
use super::{ops, sidebar, CadenceApp, StatusKind};
use crate::model::{next_id, note_name, ChordQuality, Note, Pattern, SongKey, Track, NOTE_NAMES};

const KEYS_W: f32 = 66.0;
const DRUM_LABEL_W: f32 = 132.0;
const RULER_H: f32 = 24.0;
const VEL_H: f32 = 42.0;
const ROW_H: f32 = 16.0;
const RESIZE_ZONE: f32 = 7.0;
const MIN_PPS: f32 = 8.0;
const MAX_PPS: f32 = 64.0;
const FIRST_PITCH: u8 = 0;
const LAST_PITCH: u8 = 127;
const PITCHES: usize = (LAST_PITCH - FIRST_PITCH + 1) as usize;
/// Octave bounds for computer-keyboard audition: octave `o` covers MIDI
/// `(o + 1) * 12 ..= (o + 1) * 12 + 11`, so 0..=8 keeps the whole row inside
/// 0..=127 while still reaching C0 for bass.
const KB_OCTAVE_MIN: i32 = 0;
const KB_OCTAVE_MAX: i32 = 8;

/// Computer-key press/release pairs as `(key, shift held)`.
type KeyEvents = Vec<(Key, bool)>;

/// Jitter the Humanize button applies: ±10% velocity and ±0.25 steps (a
/// 1/64 note) of timing — enough to loosen a rigid line without scattering
/// it off the grid.
const HUMANIZE_VELOCITY: f32 = 0.10;
const HUMANIZE_TIMING: f64 = 0.25;
/// Velocity-scale chips next to Humanize in both editor toolbars.
const VEL_CHIPS: [(&str, f32); 2] = [("-10%", 0.9), ("+10%", 1.1)];

/// The 12 mapped computer keys by chromatic index (0 = C … 11 = B) — the
/// FL-Studio two-row layout: white keys `Z X C V B N M` on the bottom row and
/// black keys `S D G H J` filling the gaps of the row above. `L` stays free
/// for the loop shortcut; `Q`/`W` shift the octave instead of playing.
const KB_KEYS: [Key; 12] = [
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
/// White-key pitch classes per octave (C D E F G A B) for the Keys tab.
const KB_WHITE_PCS: [u8; 7] = [0, 2, 4, 5, 7, 9, 11];
/// `(black pitch class, index of the white key it follows)` — C# after C,
/// D# after D, F# after F, G# after G, A# after A.
const KB_BLACKS: [(u8, usize); 5] = [(1, 0), (3, 1), (6, 3), (8, 4), (10, 5)];

/// Chromatic index (0 = C … 11 = B) of a computer key in the two-row
/// performance layout, or `None` for unmapped keys.
pub(super) fn kb_key_index(key: Key) -> Option<usize> {
    KB_KEYS.iter().position(|&k| k == key)
}

/// MIDI pitch of `key` in `octave` (octave numbering like
/// [`ops::chord_notes`]: C4 = MIDI 60). `None` for unmapped keys and for
/// pitches outside 0..=127 — high/low octaves simply fall off the end of the
/// keyboard instead of wrapping.
pub(super) fn key_to_pitch(key: Key, octave: i32) -> Option<u8> {
    let pc = kb_key_index(key)? as i32;
    u8::try_from((octave + 1) * 12 + pc)
        .ok()
        .filter(|&p| p <= ops::MAX_PITCH)
}

/// Pitch a computer key plays: the track's drum rows on a drum grid,
/// otherwise the two-row keyboard mapping at `octave` (±12 with Shift).
fn keyboard_pitch(
    key: Key,
    shift: bool,
    octave: i32,
    drum_pitches: &Option<Vec<u8>>,
) -> Option<u8> {
    match drum_pitches {
        Some(pitches) => kb_key_index(key).and_then(|i| pitches.get(i).copied()),
        None => key_to_pitch(key, octave).and_then(|p| {
            let p = if shift { p.saturating_add(12) } else { p };
            (p <= ops::MAX_PITCH).then_some(p)
        }),
    }
}

/// The pattern whose clip covers song step `pos` on `track`, plus the
/// pattern-relative step it maps to (loops fold the step back into the
/// pattern). `None` when no clip covers the position.
pub(super) fn covering_pattern(track: &Track, pos: f64) -> Option<(u64, f64)> {
    track.clips.iter().find_map(|clip| {
        let plen = ops::pattern(track, clip.pattern_id)?.length.max(1) as f64;
        let span = clip.span(plen);
        if pos >= clip.start - 1e-9 && pos < clip.start + span {
            Some((clip.pattern_id, (pos - clip.start) % plen))
        } else {
            None
        }
    })
}

/// Recorded note start: the song step floored onto the snap grid.
pub(super) fn record_start(pos: f64, snap: f64) -> f64 {
    snap_floor(pos.max(0.0), snap)
}

/// Recorded note duration: the release step minus the snapped start,
/// rounded up to whole snap cells — at least one cell, so a tap still
/// sounds. Non-finite input (release before start, loop wrap) reads as one
/// cell.
pub(super) fn record_duration(start_step: f64, release_step: f64, snap: f64) -> f64 {
    let dur = ((release_step - start_step) / snap).ceil() * snap;
    if dur.is_finite() {
        dur.max(snap)
    } else {
        snap
    }
}

/// Drum piece pitches the computer keyboard maps onto: the track's drum rows
/// sorted ascending, deduplicated and capped at the 12 mapped keys.
pub(super) fn drum_keyboard_pitches(track: &Track) -> Vec<u8> {
    let mut pitches: Vec<u8> = track.drum_rows().iter().map(|(p, _)| *p).collect();
    pitches.sort_unstable();
    pitches.dedup();
    pitches.truncate(KB_KEYS.len());
    pitches
}

/// Chromatic indices (0 = C … 11 = B) of the computer keys currently held —
/// for the on-screen keyboard strip and drum-row highlights. Cmd/Ctrl/Alt
/// held presses don't count (they're shortcuts); Shift still counts since it
/// only transposes the pitch.
pub(super) fn held_keyboard_keys(ctx: &Context) -> BTreeSet<usize> {
    let mut out = BTreeSet::new();
    let (keys_down, modifiers) = ctx.input(|i| (i.keys_down.clone(), i.modifiers));
    if modifiers.command || modifiers.ctrl || modifiers.alt {
        return out;
    }
    for key in keys_down {
        if let Some(index) = kb_key_index(key) {
            out.insert(index);
        }
    }
    out
}

/// Pitch classes (0 = C … 11 = B) belonging to `key`'s scale — its seven
/// diatonic degrees — for the scale-highlight row tint.
pub(super) fn scale_pitch_classes(key: SongKey) -> [bool; 12] {
    key.pitch_classes()
}

/// Octave the computer keyboard and chord strip centre on for `track`: the
/// octave holding its audition pitch — the same centring the arpeggiator
/// dialog uses (`ArpDraft::new` derives its octave the same way).
fn kb_home_octave(track: &Track) -> i32 {
    let pitch = sidebar::audition_params(track, track.instrument.default_pitch()).0;
    ((i32::from(pitch) / 12) - 1).clamp(KB_OCTAVE_MIN, KB_OCTAVE_MAX)
}

/// Label for the strum-spacing combo, matching its item text.
fn chord_spacing_label(spacing: f64) -> &'static str {
    match spacing {
        s if s <= 0.0 => "Block",
        s if (s - 0.25).abs() < 1e-6 => "1/64",
        s if (s - 0.5).abs() < 1e-6 => "1/32",
        _ => "1/16",
    }
}

/// Labels for the chord-strip inversion selector — rotations of the lowest
/// chord tone up an octave, so three covers any chord the strip can build.
const CHORD_INVERSION_LABELS: [&str; 4] = ["Root", "1st", "2nd", "3rd"];

/// Label for the slash-bass combo: "Off", the chord "Root" or the chosen
/// scale degree's numeral (the degree's pitch class comes from `key`).
fn chord_bass_label(bass: ops::ChordBass, key: SongKey) -> String {
    match bass {
        ops::ChordBass::Off => "Off".to_string(),
        ops::ChordBass::Root => "Root".to_string(),
        ops::ChordBass::Degree(degree) => key.degree_numeral(degree),
    }
}

/// The three chord-strip palettes: the key's diatonic chords, borrowed
/// chords / secondary dominants and one-click progression presets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum ChordStripTab {
    #[default]
    Diatonic,
    Borrowed,
    Progressions,
}

impl ChordStripTab {
    pub(super) const ALL: [ChordStripTab; 3] = [
        ChordStripTab::Diatonic,
        ChordStripTab::Borrowed,
        ChordStripTab::Progressions,
    ];

    pub(super) fn label(self) -> &'static str {
        match self {
            ChordStripTab::Diatonic => "Diatonic",
            ChordStripTab::Borrowed => "Borrowed",
            ChordStripTab::Progressions => "Progressions",
        }
    }
}

#[derive(Debug, Clone)]
struct NoteDrag {
    /// Snapshots of the dragged notes when the gesture began.
    originals: Vec<Note>,
    /// Steps the pointer has moved so far, snapped.
    dstep: f64,
    dpitch: i32,
    mode: DragMode,
    anchor: Note,
    grab_step_offset: f64,
    grab_pitch: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DragMode {
    Move,
    Resize,
}

#[derive(Debug, Clone)]
struct NoteMarquee {
    start: Pos2,
    current: Pos2,
    base: BTreeSet<u64>,
    additive: bool,
}

#[derive(Debug, Clone, Copy)]
struct PendingNote {
    pos: Pos2,
    pitch: u8,
    start: f64,
    draw_on_click: bool,
}

/// One held recorded note (MIDI recording): enough to finalize its duration
/// on release and, in step recording, advance the playhead.
#[derive(Debug, Clone, Copy)]
pub(super) struct PendingRecord {
    pub track_id: u64,
    pub pattern_id: u64,
    pub note_id: u64,
    /// Snapped song-space start step the release step is measured against.
    pub start_step: f64,
    /// Recorded while the transport was stopped: release advances the
    /// playhead by one Snap cell instead of only setting the duration.
    pub step_record: bool,
}

/// Last-frame editor geometry. `grid` feeds scrollbar logic; the rest is
/// written each frame so `#[cfg(test)]` helpers can aim clicks at real layout.
#[derive(Debug)]
#[allow(dead_code)]
struct EditorRects {
    grid: Rect,
    vel: Rect,
    keys: Rect,
    ruler: Rect,
    /// Computer-keyboard strip under the piano roll (melodic editor only).
    kb: Rect,
    /// Drum label column (drum editor only).
    labels: Rect,
}

impl Default for EditorRects {
    fn default() -> Self {
        Self {
            grid: Rect::NOTHING,
            vel: Rect::NOTHING,
            keys: Rect::NOTHING,
            ruler: Rect::NOTHING,
            kb: Rect::NOTHING,
            labels: Rect::NOTHING,
        }
    }
}

/// What a click under the pointer hit.
#[derive(Clone, Copy, Debug)]
struct NoteHit {
    index: usize,
    zone: HitZone,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HitZone {
    Body,
    Resize,
}

#[derive(Debug)]
pub(super) struct EditorView {
    /// Default length for new notes, in steps.
    pub note_len: f64,
    /// Velocity applied to newly drawn notes and new drum hits.
    pub velocity: f32,
    /// Move/resize snap in steps.
    pub snap: f64,
    pub roll_pps: f32,
    roll_scroll_x: f32,
    roll_scroll_y: f32,
    pub drum_pps: f32,
    drum_scroll_x: f32,
    last_track: Option<u64>,
    drag: Option<NoteDrag>,
    draw: Option<u64>,
    pending_note: Option<PendingNote>,
    marquee: Option<NoteMarquee>,
    right_erase: bool,
    drum_paint: Option<bool>,
    vel_drag: Option<Vec<u64>>,
    /// Computer-keyboard octave: the octave of the Z key's C (C4 = 4),
    /// re-centred on each track's home octave.
    pub kb_octave: i32,
    /// Chord-strip disclosure and stamping options — session view state,
    /// never part of the song.
    pub chord_open: bool,
    /// Which chord-strip palette is showing: diatonic chords, borrowed
    /// chords or progression presets.
    pub chord_tab: ChordStripTab,
    /// `None` keeps each diatonic chord's scale-degree quality.
    pub chord_quality: Option<ChordQuality>,
    pub chord_direction: ops::StrumDirection,
    /// Strum spacing in steps; `0.0` stamps a block chord.
    pub chord_spacing: f64,
    /// Chord-strip inversion: how many times the lowest tone rotates up an
    /// octave (0 = root position). Applied to hover auditions and stamps.
    pub chord_inversion: u8,
    /// Slash-bass option: an extra tone voiced below the chord
    /// ([`ops::ChordBass`]), `Off` by default.
    pub chord_bass: ops::ChordBass,
    /// Tints piano-roll rows whose pitch class is in the song key's scale.
    pub scale_highlight: bool,
    /// Snaps drawn notes and vertical note drags onto the song key's scale
    /// tones. Session view state like `scale_highlight` — never serialized.
    /// Inert on the C-major default (treated as "no key chosen") and on drum
    /// grids; see [`CadenceApp::scale_snap_key`].
    pub scale_snap: bool,
    /// Keys-tab mouse state: pitch under the pointer, whether the mouse
    /// button is held on the keys (glow + glissando), and the last pitch
    /// the mouse previewed so a drag retriggers only on change.
    kb_pointer: Option<u8>,
    kb_down: bool,
    kb_last_preview: Option<u8>,
    /// Held recorded notes by pitch (MIDI recording): press inserts,
    /// release finalizes the duration / advances the step-record playhead.
    pub record_pending: HashMap<u8, PendingRecord>,
    /// Pitch of the on-screen piano key currently being recorded with the
    /// mouse (Keys tab).
    pub kb_record_pitch: Option<u8>,
    /// Whether the Keys-tab pointer was down last frame, for press/release
    /// edge detection while recording.
    kb_was_down: bool,
    /// Last-frame geometry, also used by headless tests to compute click points.
    rects: EditorRects,
    grid_id: egui::Id,
}

impl Default for EditorView {
    fn default() -> Self {
        Self {
            note_len: 2.0,
            velocity: 0.8,
            snap: 1.0,
            roll_pps: 22.0,
            roll_scroll_x: 0.0,
            roll_scroll_y: 0.0,
            drum_pps: 30.0,
            drum_scroll_x: 0.0,
            last_track: None,
            drag: None,
            draw: None,
            pending_note: None,
            marquee: None,
            right_erase: false,
            drum_paint: None,
            vel_drag: None,
            kb_octave: 4,
            chord_open: false,
            chord_tab: ChordStripTab::Diatonic,
            chord_quality: None,
            chord_direction: ops::StrumDirection::default(),
            chord_spacing: 0.0,
            chord_inversion: 0,
            chord_bass: ops::ChordBass::default(),
            scale_highlight: false,
            scale_snap: false,
            kb_pointer: None,
            kb_down: false,
            kb_last_preview: None,
            record_pending: HashMap::new(),
            kb_record_pitch: None,
            kb_was_down: false,
            rects: EditorRects::default(),
            grid_id: egui::Id::NULL,
        }
    }
}

impl EditorView {
    pub fn reset_view(&mut self) {
        self.last_track = None;
        self.drag = None;
        self.draw = None;
        self.pending_note = None;
        self.marquee = None;
        self.drum_paint = None;
        self.vel_drag = None;
        self.kb_pointer = None;
        self.kb_down = false;
        self.kb_last_preview = None;
        self.record_pending.clear();
        self.kb_record_pitch = None;
        self.kb_was_down = false;
        self.roll_scroll_x = 0.0;
        self.drum_scroll_x = 0.0;
    }

    pub fn end_gesture(&mut self) {
        self.drag = None;
        self.draw = None;
        self.pending_note = None;
        self.marquee = None;
        self.right_erase = false;
        self.drum_paint = None;
        self.vel_drag = None;
    }

    /// Zooms the piano roll horizontally about the visible centre.
    pub fn zoom(&mut self, factor: f32) {
        self.roll_pps = (self.roll_pps * factor).clamp(MIN_PPS, MAX_PPS);
        self.drum_pps = (self.drum_pps * factor).clamp(MIN_PPS, MAX_PPS);
    }
}

#[derive(Clone, Copy)]
struct Roll {
    grid: Rect,
    keys: Rect,
    ruler: Rect,
    vel: Rect,
    pps: f32,
    scroll_x: f32,
    scroll_y: f32,
}

impl Roll {
    fn x(&self, step: f64) -> f32 {
        self.grid.left() + step as f32 * self.pps - self.scroll_x
    }
    fn step(&self, x: f32) -> f64 {
        ((x - self.grid.left() + self.scroll_x) / self.pps) as f64
    }
    fn y(&self, pitch: u8) -> f32 {
        self.grid.top() + (LAST_PITCH - pitch) as f32 * ROW_H - self.scroll_y
    }
    fn pitch_at(&self, y: f32) -> Option<u8> {
        let row = ((y - self.grid.top() + self.scroll_y) / ROW_H).floor() as i64;
        let pitch = LAST_PITCH as i64 - row;
        (FIRST_PITCH as i64..=LAST_PITCH as i64)
            .contains(&pitch)
            .then_some(pitch as u8)
    }
    fn note_rect(&self, n: &Note) -> Rect {
        let x = self.x(n.start);
        let y = self.y(n.pitch);
        Rect::from_min_size(
            pos2(x + 1.0, y + 1.0),
            vec2((n.duration as f32 * self.pps - 1.5).max(3.0), ROW_H - 2.0),
        )
    }
}

#[derive(Clone, Copy)]
struct DrumGrid<'a> {
    grid: Rect,
    labels: Rect,
    ruler: Rect,
    pps: f32,
    scroll_x: f32,
    row_h: f32,
    len: f64,
    /// `(pitch, name)` per row, top to bottom — the kit's pieces for a kit
    /// preset, else the built-in `DRUMS` table. Never empty
    /// ([`Track::drum_rows`] guarantees at least one row).
    rows: &'a [(u8, &'a str)],
}

impl DrumGrid<'_> {
    fn x(&self, step: f64) -> f32 {
        self.grid.left() + step as f32 * self.pps - self.scroll_x
    }
    fn step(&self, x: f32) -> f64 {
        ((x - self.grid.left() + self.scroll_x) / self.pps) as f64
    }
    fn row_y(&self, row: usize) -> f32 {
        self.grid.top() + row as f32 * self.row_h
    }
    fn cell(&self, pos: Pos2) -> Option<(usize, u32)> {
        if !self.grid.contains(pos) {
            return None;
        }
        let row = ((pos.y - self.grid.top()) / self.row_h).floor() as usize;
        let step = self.step(pos.x).floor() as i64;
        (0..self.rows.len())
            .contains(&row)
            .then_some(row)
            .filter(|_| step >= 0 && (step as f64) < self.len)
            .map(|r| (r, step as u32))
    }
}

impl CadenceApp {
    /// Center (grid-local) point of a drum cell for tests; None if not showing drums.
    #[cfg(test)]
    pub(crate) fn drum_cell_center(&self, row: usize, step: u32) -> Option<Pos2> {
        let r = &self.editor.rects;
        let track = self.selected_track()?;
        if r.grid == Rect::NOTHING || !track.shows_drum_grid() {
            return None;
        }
        let rows = track.drum_rows();
        if row >= rows.len() {
            return None;
        }
        let len = self
            .current_pattern_id(track)
            .and_then(|pid| ops::pattern(track, pid))
            .map_or(16.0, |p| p.length as f64);
        if (step as f64) >= len {
            return None;
        }
        let g = drum_geometry(r, self.editor.drum_pps, self.editor.drum_scroll_x, &rows);
        let x = g.x(step as f64) + g.pps * 0.5;
        let y = g.row_y(row) + g.row_h * 0.5;
        (x >= g.grid.left() && x <= g.grid.right()).then(|| pos2(x, y))
    }

    #[cfg(test)]
    pub(crate) fn roll_cell_center(&self, pitch: u8, step: f64) -> Option<Pos2> {
        let r = &self.editor.rects;
        if r.grid == Rect::NOTHING {
            return None;
        }
        let roll = self.roll_for_test();
        let p = pos2(roll.x(step) + roll.pps * 0.25, roll.y(pitch) + ROW_H * 0.5);
        roll.grid.contains(p).then_some(p)
    }

    /// Screen x of a pattern step on the piano roll (tests).
    #[cfg(test)]
    pub(crate) fn roll_x(&self, step: f64) -> Option<f32> {
        let r = &self.editor.rects;
        if r.grid == Rect::NOTHING {
            return None;
        }
        let roll = self.roll_for_test();
        Some(roll.x(step))
    }

    /// Current editor canvas rects: (grid, velocity lane, ruler).
    #[cfg(test)]
    pub(crate) fn editor_test_rects(&self) -> (Rect, Rect, Rect) {
        let r = &self.editor.rects;
        (r.grid, r.vel, r.ruler)
    }

    #[cfg(test)]
    fn roll_for_test(&self) -> Roll {
        let r = &self.editor.rects;
        Roll {
            grid: r.grid,
            keys: r.keys,
            ruler: r.ruler,
            vel: r.vel,
            pps: self.editor.roll_pps,
            scroll_x: self.editor.roll_scroll_x,
            scroll_y: self.editor.roll_scroll_y,
        }
    }

    /// Sets the piano-roll scroll for tests.
    #[cfg(test)]
    pub(crate) fn set_roll_scroll(&mut self, x: f32, y: f32) {
        self.editor.roll_scroll_x = x.max(0.0);
        self.editor.roll_scroll_y = y.max(0.0);
    }

    /// Ids the pattern note-ops act on: the selection when there is one,
    /// else `None` — which every note op reads as "the whole pattern".
    fn note_op_ids(&self) -> Option<BTreeSet<u64>> {
        (!self.sel.notes.is_empty()).then(|| self.sel.notes.clone())
    }

    /// The `(track, pattern)` the pattern editor is editing — the selected
    /// track's current pattern. `None` when no track is selected or it has
    /// no patterns; the Edit-menu note ops disable then.
    pub(super) fn editor_target(&self) -> Option<(u64, u64)> {
        let track = self.selected_track()?;
        Some((track.id, self.current_pattern_id(track)?))
    }

    /// Whether the note ops (quantize, humanize, velocity scale) have
    /// anything to act on right now: a note selection, or a current
    /// pattern with notes to fall back on.
    pub(super) fn has_note_targets(&self) -> bool {
        if !self.sel.notes.is_empty() {
            return true;
        }
        self.editor_target().is_some_and(|(tid, pid)| {
            ops::track(&self.project, tid)
                .and_then(|t| ops::pattern(t, pid))
                .is_some_and(|p| !p.notes.is_empty())
        })
    }

    /// Quantize button handler: snap note starts to the grid — the editor's
    /// snap value on the roll, whole steps on the drum grid. Selected notes
    /// when any are selected, else the whole pattern.
    pub(super) fn quantize_notes(&mut self, track_id: u64, pid: u64) {
        let drum = self
            .selected_track()
            .is_some_and(|t| t.id == track_id && t.shows_drum_grid());
        let grid = if drum { 1.0 } else { self.editor.snap };
        let ids = self.note_op_ids();
        let mut moved = 0;
        self.edit("Quantize notes", None, |p| {
            if let Some(pat) = ops::pattern_mut(p, track_id, pid) {
                moved = ops::quantize_notes(pat, ids.as_ref(), grid);
            }
        });
        self.set_status(
            StatusKind::Info,
            if moved > 0 {
                format!("Quantized {moved} note{}.", sidebar::plural(moved))
            } else {
                "Already on the grid.".to_string()
            },
        );
    }

    /// Humanize button handler: seeded velocity and timing jitter on the
    /// selected notes, or the whole pattern when nothing is selected.
    /// `next_id` gives every click a fresh seed, so pressing again is a
    /// genuinely new take; undo restores the previous pass.
    pub(super) fn humanize_notes(&mut self, track_id: u64, pid: u64) {
        let ids = self.note_op_ids();
        let seed = next_id();
        let mut touched = 0usize;
        self.edit("Humanize notes", None, |p| {
            if let Some(pat) = ops::pattern_mut(p, track_id, pid) {
                touched = ops::humanize_notes(
                    pat,
                    ids.as_ref(),
                    HUMANIZE_VELOCITY,
                    HUMANIZE_TIMING,
                    seed,
                );
            }
        });
        self.set_status(
            StatusKind::Info,
            if touched > 0 {
                format!("Humanized {touched} note{}.", sidebar::plural(touched))
            } else {
                "No notes to humanize.".to_string()
            },
        );
    }

    /// Velocity-chip handler: multiplies note velocities by `gain` (the
    /// `VEL_CHIPS` ±10% buttons) — the selection, or the whole pattern
    /// when nothing is selected.
    pub(super) fn scale_velocities(&mut self, track_id: u64, pid: u64, gain: f32) {
        let ids = self.note_op_ids();
        let mut touched = 0usize;
        self.edit("Scale velocities", None, |p| {
            if let Some(pat) = ops::pattern_mut(p, track_id, pid) {
                touched = ops::scale_velocities(pat, ids.as_ref(), gain);
            }
        });
        let pct = ((f64::from(gain) - 1.0) * 100.0).round() as i32;
        self.set_status(
            StatusKind::Info,
            if touched > 0 {
                format!(
                    "Velocity {pct:+}% on {touched} note{}.",
                    sidebar::plural(touched)
                )
            } else {
                "No notes to scale.".to_string()
            },
        );
    }

    /// Paste target under the pointer: `(pattern step, pitch anchor)`. The
    /// step is snapped to the editor grid; the anchor is the pitch row under
    /// the cursor for melodic tracks (`None` on the drum grid, where rows
    /// are kit pieces rather than chromatic pitch). `None` when the pattern
    /// editor isn't the active view or the pointer isn't over the note grid —
    /// callers fall back to the playhead and unshifted pitch.
    pub(super) fn editor_paste_target(&self, ctx: &Context) -> Option<(f64, Option<u8>)> {
        if self.tab != super::Tab::Editor {
            return None;
        }
        let pos = ctx.input(|i| i.pointer.hover_pos())?;
        let r = &self.editor.rects;
        if r.grid == Rect::NOTHING {
            return None;
        }
        let track = self.selected_track()?;
        let snap = self.editor.snap;
        if track.shows_drum_grid() {
            let rows = track.drum_rows();
            let g = DrumGrid {
                grid: r.grid,
                labels: r.labels,
                ruler: r.ruler,
                pps: self.editor.drum_pps,
                scroll_x: self.editor.drum_scroll_x,
                row_h: r.grid.height() / rows.len().max(1) as f32,
                len: 1.0,
                rows: &rows,
            };
            g.grid
                .contains(pos)
                .then(|| (snap_floor(g.step(pos.x).max(0.0), snap), None))
        } else {
            let roll = Roll {
                grid: r.grid,
                keys: r.keys,
                ruler: r.ruler,
                vel: r.vel,
                pps: self.editor.roll_pps,
                scroll_x: self.editor.roll_scroll_x,
                scroll_y: self.editor.roll_scroll_y,
            };
            roll.grid.contains(pos).then(|| {
                (
                    snap_floor(roll.step(pos.x).max(0.0), snap),
                    roll.pitch_at(pos.y),
                )
            })
        }
    }
}

#[cfg(test)]
fn drum_geometry<'a>(
    rects: &EditorRects,
    pps: f32,
    scroll_x: f32,
    rows: &'a [(u8, &'a str)],
) -> DrumGrid<'a> {
    DrumGrid {
        grid: rects.grid,
        labels: rects.labels,
        ruler: rects.ruler,
        pps,
        scroll_x,
        row_h: rects.grid.height() / rows.len().max(1) as f32,
        len: 1.0,
        rows,
    }
}

impl CadenceApp {
    pub(super) fn editor_ui(&mut self, ui: &mut Ui) {
        let pal = self.palette();
        let Some(track) = self.selected_track().cloned() else {
            ui.centered_and_justified(|ui| {
                ui.label(
                    RichText::new(
                        "No track selected — pick one in the arrangement or add an instrument.",
                    )
                    .color(pal.text_dim),
                );
            });
            return;
        };
        let Some(pid) = self.current_pattern_id(&track) else {
            ui.centered_and_justified(|ui| {
                if ui.button("Create a pattern").clicked() {
                    let mut new = None;
                    let tid = track.id;
                    self.edit("New pattern", None, |p| {
                        if let Some(t) = ops::track_mut(p, tid) {
                            new = ops::new_pattern(t, 16);
                        }
                    });
                    if let Some(new) = new {
                        self.select_pattern(tid, new);
                    }
                }
            });
            return;
        };
        if self.editor.last_track != Some(track.id) {
            self.editor.last_track = Some(track.id);
            self.editor.kb_octave = kb_home_octave(&track);
            self.editor_roll_home(&track, pid);
        }
        self.editor_header(ui, &track, pid);
        self.chord_strip(ui, &track);
        if track.shows_drum_grid() {
            self.drum_editor(ui, &track, pid);
        } else {
            self.piano_roll(ui, &track, pid);
        }
    }

    /// Scrolls the piano roll so existing notes (or the instrument's home octave) are visible.
    fn editor_roll_home(&mut self, track: &Track, pid: u64) {
        let pat = ops::pattern(track, pid);
        let pitch_center = pat
            .filter(|p| !p.notes.is_empty())
            .map(|p| p.notes.iter().map(|n| n.pitch as u32).sum::<u32>() / p.notes.len() as u32)
            .unwrap_or_else(|| {
                // An empty pack-preset pattern homes on its audition note
                // (kit presets on a real piece pitch); built-ins keep their
                // instrument default pitch.
                u32::from(sidebar::audition_params(track, track.instrument.default_pitch()).0)
            });
        let row = (LAST_PITCH as u32 - pitch_center) as f32;
        // Before the first paint the grid rect is still empty; use a typical
        // editor height so the first frame already shows the right octave.
        let grid_h = self.editor.rects.grid.height();
        let h = if grid_h > 40.0 { grid_h } else { 280.0 };
        let content = PITCHES as f32 * ROW_H;
        self.editor.roll_scroll_y = (row * ROW_H - h * 0.5).clamp(0.0, (content - h).max(0.0));
        self.editor.roll_scroll_x = 0.0;
    }

    // ----- header ----------------------------------------------------------

    fn editor_header(&mut self, ui: &mut Ui, track: &Track, pid: u64) {
        let pal = self.palette();
        egui::Frame::new()
            .inner_margin(Margin {
                left: 12,
                right: 12,
                top: 4,
                bottom: 4,
            })
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                    theme::color_dot(ui, theme::rgb(track.color), 10.0);
                    ui.label(RichText::new(&track.name).size(15.5).color(pal.text));
                    ui.label(
                        RichText::new(ops::instrument_label(track).into_owned())
                            .small()
                            .color(pal.text_dim),
                    );
                    ui.label(RichText::new("/").color(pal.text_faint));

                    // Pattern selector
                    let cur = ops::pattern(track, pid)
                        .map(|p| p.name.clone())
                        .unwrap_or_default();
                    let mut pick = None;
                    egui::ComboBox::from_id_salt(("pattern", track.id))
                        .width(150.0)
                        .selected_text(cur)
                        .show_ui(ui, |ui| {
                            for p in &track.patterns {
                                if ui
                                    .selectable_label(
                                        p.id == pid,
                                        format!("{} · {}", p.name, ops::length_label(p.length)),
                                    )
                                    .clicked()
                                {
                                    pick = Some(p.id);
                                }
                            }
                        });
                    if let Some(new) = pick {
                        self.select_pattern(track.id, new);
                    }
                    // Rename
                    let mut name = ops::pattern(track, pid)
                        .map(|p| p.name.clone())
                        .unwrap_or_default();
                    let w = (name.len() as f32 * 7.5 + 24.0).clamp(70.0, 160.0);
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut name)
                            .desired_width(w)
                            .char_limit(crate::model::MAX_NAME_CHARS),
                    );
                    if resp.changed() {
                        self.edit("Rename pattern", Some(&format!("patname-{pid}")), |p| {
                            if let Some(pat) = ops::pattern_mut(p, track.id, pid) {
                                pat.name = name.clone();
                            }
                        });
                    }
                    resp.on_hover_text("Pattern name");

                    // Length
                    let plen = ops::pattern(track, pid).map(|p| p.length).unwrap_or(16);
                    let mut new_len = None;
                    egui::ComboBox::from_id_salt(("patlen", track.id))
                        .width(74.0)
                        .selected_text(ops::length_label(plen))
                        .show_ui(ui, |ui| {
                            let mut lens = ops::PATTERN_LENGTHS.to_vec();
                            if !lens.contains(&plen) {
                                lens.push(plen);
                                lens.sort_unstable();
                            }
                            for l in lens {
                                ui.selectable_value(&mut new_len, Some(l), ops::length_label(l));
                            }
                        });
                    if let Some(l) = new_len {
                        if l != plen {
                            self.edit("Pattern length", None, |p| {
                                ops::set_track_pattern_length(p, track.id, pid, l);
                            });
                            self.set_status(
                                StatusKind::Info,
                                format!("Pattern length set to {}.", ops::length_label(l)),
                            );
                        }
                    }

                    if theme::icon_button(
                        ui,
                        pal,
                        theme::Icon::Plus,
                        26.0,
                        false,
                        pal.accent,
                        "New empty pattern",
                    )
                    .clicked()
                    {
                        let len = ops::pattern(track, pid).map(|p| p.length).unwrap_or(16);
                        let mut new = None;
                        let tid = track.id;
                        self.edit("New pattern", None, |p| {
                            if let Some(t) = ops::track_mut(p, tid) {
                                new = ops::new_pattern(t, len);
                            }
                        });
                        match new {
                            Some(n) => self.select_pattern(tid, n),
                            None => self.set_status(
                                StatusKind::Warning,
                                format!(
                                    "Tracks hold up to {} patterns.",
                                    ops::MAX_PATTERNS_PER_TRACK
                                ),
                            ),
                        }
                    }
                    if theme::icon_button(
                        ui,
                        pal,
                        theme::Icon::Duplicate,
                        26.0,
                        false,
                        pal.accent,
                        "Duplicate pattern",
                    )
                    .clicked()
                    {
                        let mut new = None;
                        let tid = track.id;
                        self.edit("Duplicate pattern", None, |p| {
                            if let Some(t) = ops::track_mut(p, tid) {
                                new = ops::duplicate_pattern_in(t, pid);
                            }
                        });
                        match new {
                            Some(n) => self.select_pattern(tid, n),
                            None => self.set_status(
                                StatusKind::Warning,
                                format!(
                                    "Tracks hold up to {} patterns.",
                                    ops::MAX_PATTERNS_PER_TRACK
                                ),
                            ),
                        }
                    }
                    let note_count = ops::pattern(track, pid).map_or(0, |p| p.notes.len());
                    if theme::icon_button(
                        ui,
                        pal,
                        theme::Icon::Trash,
                        26.0,
                        false,
                        pal.danger,
                        "Clear all notes in this pattern",
                    )
                    .clicked()
                        && note_count > 0
                    {
                        self.edit("Clear pattern", None, |p| {
                            if let Some(pat) = ops::pattern_mut(p, track.id, pid) {
                                pat.notes.clear();
                            }
                        });
                        self.sel.notes.clear();
                    }
                    let many = track.patterns.len() > 1;
                    let del = egui::Button::new(RichText::new("Delete").small())
                        .min_size(vec2(0.0, 26.0));
                    if ui
                        .add_enabled(many, del)
                        .on_hover_text(if many {
                            "Delete this pattern (its clips are removed too)"
                        } else {
                            "A track keeps at least one pattern"
                        })
                        .clicked()
                    {
                        self.edit("Delete pattern", None, |p| {
                            if let Some(t) = ops::track_mut(p, track.id) {
                                ops::delete_pattern(t, pid);
                            }
                        });
                        self.fix_selection();
                    }

                    ui.separator();
                    if ui
                        .add(
                            egui::Button::new("Place clip")
                                .small()
                                .min_size(vec2(0.0, 26.0)),
                        )
                        .on_hover_text("Place this pattern at the playhead (snapped to the bar)")
                        .clicked()
                    {
                        let start = snap_floor(self.transport.position, 16.0);
                        self.place_clip(track.id, Some(pid), start);
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if theme::chip(
                            ui,
                            pal,
                            "Track",
                            self.side_tab == sidebar::SideTab::Track,
                            pal.link,
                            vec2(52.0, 24.0),
                            "Open track settings",
                        )
                        .clicked()
                        {
                            self.side_tab = sidebar::SideTab::Track;
                        }
                    });
                });
            });
    }

    // ----- piano roll ------------------------------------------------------

    fn piano_roll(&mut self, ui: &mut Ui, track: &Track, pid: u64) {
        let pal = self.palette();
        self.roll_toolbar(ui, track, pid);
        let Some(pat) = ops::pattern(track, pid).cloned() else {
            return;
        };
        let area = ui.available_rect_before_wrap();
        if area.width() < 200.0 || area.height() < RULER_H + VEL_H + 24.0 {
            return;
        }
        let grid = Rect::from_min_max(
            pos2(area.left() + KEYS_W, area.top() + RULER_H),
            pos2(area.right(), area.bottom() - VEL_H),
        );
        let keys = Rect::from_min_max(
            pos2(area.left(), grid.top()),
            pos2(grid.left(), grid.bottom()),
        );
        let ruler = Rect::from_min_max(
            pos2(grid.left(), area.top()),
            pos2(grid.right(), grid.top()),
        );
        let vel = Rect::from_min_max(
            pos2(grid.left(), grid.bottom()),
            pos2(grid.right(), area.bottom()),
        );
        let vel_labels = Rect::from_min_max(
            pos2(area.left(), vel.top()),
            pos2(grid.left(), vel.bottom()),
        );
        self.editor.rects = EditorRects {
            grid,
            vel,
            keys,
            ruler,
            kb: Rect::NOTHING,
            labels: Rect::NOTHING,
        };
        self.editor.grid_id = ui.id().with("piano_grid");

        let content_w = pat.length as f32 * self.editor.roll_pps;
        let content_h = PITCHES as f32 * ROW_H;
        // scroll/zoom input
        let grid_resp = ui.interact(grid, self.editor.grid_id, Sense::click_and_drag());
        grid_resp
            .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "Piano roll"));
        if ui.rect_contains_pointer(area) {
            let (scroll, zoom, hover) =
                ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), i.pointer.hover_pos()));
            if zoom != 1.0 {
                let anchor = hover.map_or(grid.width() * 0.5, |h| (h.x - grid.left()).max(0.0));
                let step = (anchor + self.editor.roll_scroll_x) / self.editor.roll_pps;
                self.editor.roll_pps = (self.editor.roll_pps * zoom).clamp(MIN_PPS, MAX_PPS);
                self.editor.roll_scroll_x = (step * self.editor.roll_pps - anchor).max(0.0);
            }
            if scroll != egui::Vec2::ZERO {
                self.editor.roll_scroll_x -= scroll.x;
                self.editor.roll_scroll_y -= scroll.y;
                ui.input_mut(|i| i.smooth_scroll_delta = egui::Vec2::ZERO);
            }
        }
        self.editor.roll_scroll_x = self
            .editor
            .roll_scroll_x
            .clamp(0.0, (content_w - grid.width()).max(0.0));
        self.editor.roll_scroll_y = self
            .editor
            .roll_scroll_y
            .clamp(0.0, (content_h - grid.height()).max(0.0));
        let roll = Roll {
            grid,
            keys,
            ruler,
            vel,
            pps: self.editor.roll_pps,
            scroll_x: self.editor.roll_scroll_x,
            scroll_y: self.editor.roll_scroll_y,
        };

        self.roll_input(ui, track, pid, &pat, &roll, &grid_resp);
        self.paint_roll(ui, track, &pat, &roll, grid_resp.interact_pointer_pos());
        self.paint_keys(ui, track, &roll);
        self.paint_roll_ruler(ui, track, &pat, &roll);
        self.paint_vel_lane(ui, track.id, &pat, &roll, vel_labels);
        let mut sx = self.editor.roll_scroll_x;
        // Horizontal scrollbar rides the bottom of the velocity lane.
        let hbar = Rect::from_min_max(
            pos2(grid.left(), vel.bottom() - 8.0),
            pos2(grid.right(), vel.bottom()),
        );
        theme::scrollbar(
            ui,
            pal,
            hbar.shrink2(vec2(0.0, 1.0)),
            ui.id().with("roll_hbar"),
            false,
            content_w,
            grid.width(),
            &mut sx,
        );
        self.editor.roll_scroll_x = sx;
        let mut sy = self.editor.roll_scroll_y;
        let vbar = Rect::from_min_max(
            pos2(area.left() - 0.0, grid.top()),
            pos2(area.left() + 8.0, grid.bottom()),
        );
        theme::scrollbar(
            ui,
            pal,
            vbar.shrink2(vec2(1.0, 0.0)),
            ui.id().with("roll_vbar"),
            true,
            content_h,
            grid.height(),
            &mut sy,
        );
        self.editor.roll_scroll_y = sy;
    }

    /// Humanize button + velocity-scale chips shared by the piano-roll and
    /// drum toolbars. Call only when a note op has targets (a selection, or
    /// a pattern that has notes).
    fn note_op_buttons(&mut self, ui: &mut Ui, track_id: u64, pid: u64) {
        let pal = self.palette();
        if ui
            .add(egui::Button::new("Humanize").small())
            .on_hover_text("Random velocity and timing jitter — selected notes, or every note when nothing is selected. Each click is a new take; undo to revert")
            .clicked()
        {
            self.humanize_notes(track_id, pid);
        }
        ui.label(RichText::new("Vel").small().color(pal.text_faint));
        for (label, gain) in VEL_CHIPS {
            if ui
                .add(egui::Button::new(label).small())
                .on_hover_text(format!(
                    "Velocity {label} — selected notes, or every note when nothing is selected"
                ))
                .clicked()
            {
                self.scale_velocities(track_id, pid, gain);
            }
        }
    }

    fn roll_toolbar(&mut self, ui: &mut Ui, track: &Track, pid: u64) {
        let pal = self.palette();
        egui::Frame::new().inner_margin(Margin { left: 12, right: 12, top: 0, bottom: 4 }).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(8.0, 5.0);
                theme::color_dot(ui, pal.accent, 6.0);
                ui.label(RichText::new("Piano roll").small().color(pal.text_dim));
                ui.label(RichText::new("Note").small().color(pal.text_faint));
                egui::ComboBox::from_id_salt(("notelen", track.id)).width(62.0).selected_text(note_len_label(self.editor.note_len)).show_ui(ui, |ui| {
                    for (v, l) in [(1.0, "1/16"), (2.0, "1/8"), (4.0, "1/4"), (8.0, "1/2"), (16.0, "1 bar")] {
                        ui.selectable_value(&mut self.editor.note_len, v, l);
                    }
                });
                ui.label(RichText::new("Snap").small().color(pal.text_faint));
                egui::ComboBox::from_id_salt(("editsnap", track.id)).width(62.0).selected_text(note_len_label(self.editor.snap)).show_ui(ui, |ui| {
                    for (v, l) in [(0.5, "1/32"), (1.0, "1/16"), (2.0, "1/8"), (4.0, "1/4")] {
                        ui.selectable_value(&mut self.editor.snap, v, l);
                    }
                });
                let scale_snap_on = self.editor.scale_snap;
                if theme::chip(
                    ui,
                    pal,
                    "Scale snap",
                    scale_snap_on,
                    pal.accent,
                    vec2(70.0, 22.0),
                    "Snap-to-scale: drawn notes and vertical note drags land on the song key's scale tones",
                )
                .clicked()
                {
                    self.editor.scale_snap = !scale_snap_on;
                    if self.editor.scale_snap && self.project.key == SongKey::default() {
                        self.set_status(
                            StatusKind::Info,
                            "Scale snap needs a song key other than C major — set one in the arpeggiator dialog.",
                        );
                    }
                }
                ui.label(RichText::new("Velocity").small().color(pal.text_faint));
                let mut v = self.editor.velocity;
                ui.spacing_mut().slider_width = 110.0;
                if ui.add(egui::Slider::new(&mut v, 0.05..=1.0).custom_formatter(|x, _| format!("{:.0}%", x * 100.0))).changed() {
                    self.editor.velocity = v;
                }
                let n = self.sel.notes.len();
                if n > 0 {
                    ui.label(RichText::new(format!("{n} selected")).small().color(pal.accent));
                    if ui.add(egui::Button::new("Delete").small()).on_hover_text("Delete selected notes (Del)").clicked() {
                        self.delete_selection();
                    }
                }
                let (note_count, outside) = ops::pattern(track, pid).map_or((0, 0), |p| {
                    (p.notes.len(), p.notes.iter().filter(|n| n.start.fract() != 0.0).count())
                });
                if outside > 0 {
                    ui.label(RichText::new(format!("{outside} off-grid")).small().color(pal.text_faint)).on_hover_text("These notes sit between grid steps (e.g. imported from MIDI). Moving them keeps their offset.");
                }
                if (n > 0 || outside > 0)
                    && ui.add(egui::Button::new("Quantize").small()).on_hover_text("Snap note starts to the Snap grid — selected notes, or every note when nothing is selected").clicked()
                {
                    self.quantize_notes(track.id, pid);
                }
                if n > 0 || note_count > 0 {
                    self.note_op_buttons(ui, track.id, pid);
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    theme::hint(ui, pal, "Click to draw · drag empty grid to box-select · drag selected notes as a group · right edge resizes · right-click erases");
                });
            });
        });
    }

    fn roll_note_at(&self, pat: &Pattern, roll: &Roll, pos: Pos2) -> Option<NoteHit> {
        // Topmost (last drawn) first.
        for (i, n) in pat.notes.iter().enumerate().rev() {
            let r = roll.note_rect(n);
            if r.contains(pos) {
                let zone = if pos.x >= r.right() - RESIZE_ZONE {
                    HitZone::Resize
                } else {
                    HitZone::Body
                };
                return Some(NoteHit { index: i, zone });
            }
        }
        None
    }

    fn update_note_marquee_selection(&mut self, pat: &Pattern, roll: &Roll) {
        let Some(m) = self.editor.marquee.clone() else {
            return;
        };
        let rect = Rect::from_two_pos(m.start, m.current);
        let mut ids = if m.additive { m.base } else { BTreeSet::new() };
        ids.extend(
            pat.notes
                .iter()
                .filter(|n| roll.note_rect(n).intersects(rect))
                .map(|n| n.id),
        );
        self.sel.notes = ids;
    }

    /// The key snap-to-scale resolves against, when it applies: the toolbar
    /// toggle is on, the song has a non-default key (songs always carry a
    /// `SongKey`; the C-major `Default` means "no key chosen") and the track
    /// shows the melodic roll rather than the drum grid. `None` leaves
    /// pitches exactly where the pointer puts them.
    fn scale_snap_key(&self, track: &Track) -> Option<SongKey> {
        (self.editor.scale_snap
            && !track.shows_drum_grid()
            && self.project.key != SongKey::default())
        .then_some(self.project.key)
    }

    /// `pitch` moved onto the nearest scale tone when snap-to-scale applies
    /// ([`Self::scale_snap_key`]), unchanged otherwise.
    fn snap_to_scale_pitch(&self, track: &Track, pitch: u8) -> u8 {
        match self.scale_snap_key(track) {
            Some(key) => ops::snap_pitch_to_scale(key, pitch),
            None => pitch,
        }
    }

    /// Pitch delta for a move drag: `raw_dp` as-is when snap-to-scale is
    /// off; when on, the delta that lands the drag's anchor note on the
    /// nearest scale tone — the dragged note stair-steps between scale rows
    /// and every other dragged note keeps its interval to the anchor.
    /// `raw_dp == 0` returns 0 so grabbing a chromatic note for a purely
    /// horizontal move never nudges its pitch.
    fn snapped_drag_dp(&self, track: &Track, anchor_pitch: u8, raw_dp: i32) -> i32 {
        match self.scale_snap_key(track) {
            Some(key) if raw_dp != 0 => {
                let target =
                    (i32::from(anchor_pitch) + raw_dp).clamp(0, i32::from(ops::MAX_PITCH)) as u8;
                i32::from(ops::snap_pitch_to_scale(key, target)) - i32::from(anchor_pitch)
            }
            _ => raw_dp,
        }
    }

    fn finish_pending_note(
        &mut self,
        track: &Track,
        pid: u64,
        pat: &Pattern,
        pending: PendingNote,
    ) {
        if !pending.draw_on_click {
            return;
        }
        // Snap-to-scale resolves against the pitch the note will land on, so
        // clicking a chromatic row under an existing scale-tone note selects
        // it instead of overdubbing a duplicate.
        let pitch = self.snap_to_scale_pitch(track, pending.pitch);
        let existing = pat
            .notes
            .iter()
            .find(|n| n.pitch == pitch && pending.start >= n.start && pending.start < n.end());
        match existing {
            Some(n) => {
                self.sel.notes.clear();
                self.sel.notes.insert(n.id);
            }
            None => {
                let vel = self.editor.velocity;
                let len = self.editor.note_len;
                let mut new = None;
                self.edit("Draw note", Some("draw-note"), |p| {
                    if let Some(pat) = ops::pattern_mut(p, track.id, pid) {
                        new = ops::add_note(pat, pitch, pending.start, len, vel);
                    }
                });
                if let Some(new) = new {
                    self.sel.notes.clear();
                    self.sel.notes.insert(new);
                    self.preview(track.id, pitch, vel);
                }
            }
        }
    }

    fn roll_input(
        &mut self,
        ui: &mut Ui,
        track: &Track,
        pid: u64,
        pat: &Pattern,
        roll: &Roll,
        resp: &egui::Response,
    ) {
        let (pressed, released, shift, alt, secondary_pressed, secondary_down) = ui.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.pointer.primary_released(),
                i.modifiers.shift,
                i.modifiers.alt,
                i.pointer.secondary_pressed(),
                i.pointer.secondary_down(),
            )
        });
        let hover = resp.interact_pointer_pos().or_else(|| resp.hover_pos());
        if let Some(h) = hover.filter(|_| resp.hovered() && self.editor.drag.is_none()) {
            match self.roll_note_at(pat, roll, h) {
                Some(hit) if hit.zone == HitZone::Resize => {
                    ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal)
                }
                Some(_) => ui.ctx().set_cursor_icon(CursorIcon::Move),
                None => {
                    let _ = roll.pitch_at(h.y);
                }
            }
        }
        if pressed && resp.is_pointer_button_down_on() {
            self.focus = super::Focus::Editor;
            if let Some(pos) = ui.input(|i| i.pointer.press_origin()) {
                match self.roll_note_at(pat, roll, pos) {
                    Some(hit) => {
                        let note = pat.notes[hit.index].clone();
                        if shift {
                            if !self.sel.notes.remove(&note.id) {
                                self.sel.notes.insert(note.id);
                            }
                        } else {
                            if !self.sel.notes.contains(&note.id) {
                                self.sel.notes.clear();
                                self.sel.notes.insert(note.id);
                            }
                            let originals: Vec<Note> = pat
                                .notes
                                .iter()
                                .filter(|n| self.sel.notes.contains(&n.id))
                                .cloned()
                                .collect();
                            let mode = match hit.zone {
                                HitZone::Resize => DragMode::Resize,
                                HitZone::Body => DragMode::Move,
                            };
                            self.editor.drag = Some(NoteDrag {
                                originals,
                                dstep: 0.0,
                                dpitch: 0,
                                mode,
                                anchor: note.clone(),
                                grab_step_offset: roll.step(pos.x) - note.start,
                                grab_pitch: roll.pitch_at(pos.y).unwrap_or(note.pitch),
                            });
                            self.preview(track.id, note.pitch, note.velocity);
                        }
                    }
                    None => {
                        if shift || alt {
                            self.editor.marquee = Some(NoteMarquee {
                                start: pos,
                                current: pos,
                                base: self.sel.notes.clone(),
                                additive: shift,
                            });
                            if !shift {
                                self.sel.notes.clear();
                            }
                        } else if let Some(pitch) = roll.pitch_at(pos.y) {
                            let snap = self.editor.snap;
                            let start = snap_floor(roll.step(pos.x).max(0.0), snap)
                                .min(pat.length as f64 - 0.5)
                                .max(0.0);
                            self.editor.pending_note = Some(PendingNote {
                                pos,
                                pitch,
                                start,
                                draw_on_click: true,
                            });
                        }
                    }
                }
            }
        }
        if resp.dragged_by(PointerButton::Primary) {
            let pos = resp
                .interact_pointer_pos()
                .or_else(|| ui.input(|i| i.pointer.interact_pos()));
            if let Some(pos) = pos {
                if let Some(pending) = self.editor.pending_note.take() {
                    self.editor.marquee = Some(NoteMarquee {
                        start: pending.pos,
                        current: pos,
                        base: BTreeSet::new(),
                        additive: false,
                    });
                    self.sel.notes.clear();
                    self.update_note_marquee_selection(pat, roll);
                }
                if let Some(d) = self.editor.drag.clone() {
                    let snap = self.editor.snap;
                    let (ds, dp) = match d.mode {
                        DragMode::Move => {
                            let raw_dp = (i32::from(roll.pitch_at(pos.y).unwrap_or(d.grab_pitch))
                                - i32::from(d.grab_pitch))
                            .clamp(-120, 120);
                            (
                                snap_floor(roll.step(pos.x) - d.grab_step_offset, snap)
                                    - snap_floor(d.anchor.start, snap),
                                self.snapped_drag_dp(track, d.anchor.pitch, raw_dp),
                            )
                        }
                        DragMode::Resize => (
                            (((roll.step(pos.x) - d.anchor.start) / snap).round() * snap)
                                - d.anchor.duration,
                            0,
                        ),
                    };
                    if ds != d.dstep || dp != d.dpitch {
                        let dp_changed = dp != d.dpitch;
                        let mut drag = d;
                        drag.dstep = ds;
                        drag.dpitch = dp;
                        let label = match drag.mode {
                            DragMode::Move => "Move notes",
                            DragMode::Resize => "Resize notes",
                        };
                        let originals = drag.originals.clone();
                        if drag.mode == DragMode::Move && dp_changed {
                            self.preview(
                                track.id,
                                (drag.anchor.pitch as i32 + dp).clamp(0, ops::MAX_PITCH as i32)
                                    as u8,
                                0.7,
                            );
                        }
                        self.edit(label, Some("note-drag"), |p| {
                            if let Some(pat) = ops::pattern_mut(p, track.id, pid) {
                                match drag.mode {
                                    DragMode::Move => ops::move_notes(pat, &originals, ds, dp),
                                    DragMode::Resize => ops::resize_notes(
                                        pat,
                                        &originals,
                                        ds,
                                        snap.max(ops::MIN_NOTE_DURATION),
                                    ),
                                }
                            }
                        });
                        self.editor.drag = Some(drag);
                    }
                }
                if let Some(id) = self.editor.draw {
                    let snap = self.editor.snap.max(0.5);
                    let start = pat
                        .notes
                        .iter()
                        .find(|n| n.id == id)
                        .map(|n| n.start)
                        .unwrap_or(0.0);
                    let dur = ((roll.step(pos.x) - start) / snap).ceil() * snap;
                    self.edit("Draw note", Some("draw-note"), |p| {
                        if let Some(pat) = ops::pattern_mut(p, track.id, pid) {
                            if let Some(n) = pat.notes.iter_mut().find(|n| n.id == id) {
                                n.duration = dur.clamp(
                                    ops::MIN_NOTE_DURATION,
                                    (pat.length as f64 - n.start).max(ops::MIN_NOTE_DURATION),
                                );
                            }
                        }
                    });
                }
                if let Some(m) = self.editor.marquee.as_mut() {
                    m.current = pos;
                    self.update_note_marquee_selection(pat, roll);
                }
            }
        }
        if released {
            if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
                if let Some(m) = self.editor.marquee.as_mut() {
                    m.current = pos;
                    self.update_note_marquee_selection(pat, roll);
                }
            }
            if self.editor.marquee.take().is_none() {
                if let Some(pending) = self.editor.pending_note.take() {
                    self.finish_pending_note(track, pid, pat, pending);
                }
            }
            self.editor.draw = None;
        }
        // Right button erases notes under the cursor as it moves.
        if secondary_pressed && resp.is_pointer_button_down_on() {
            self.editor.right_erase = true;
        }
        if !secondary_down {
            self.editor.right_erase = false;
        }
        if self.editor.right_erase {
            if let Some(pos) = hover {
                if let Some(hit) = self.roll_note_at(pat, roll, pos) {
                    let id = pat.notes[hit.index].id;
                    self.edit("Erase note", Some("erase-notes"), |p| {
                        if let Some(pat) = ops::pattern_mut(p, track.id, pid) {
                            ops::delete_notes(pat, &[id]);
                        }
                    });
                    self.sel.notes.remove(&id);
                }
            }
        }
        // Velocity lane input
        let vel_resp = ui.interact(roll.vel, ui.id().with("vel_lane"), Sense::click_and_drag());
        if vel_resp.is_pointer_button_down_on() || vel_resp.dragged_by(PointerButton::Primary) {
            if let Some(pos) = vel_resp.interact_pointer_pos() {
                let v =
                    (1.0 - (pos.y - roll.vel.top()) / roll.vel.height().max(1.0)).clamp(0.05, 1.0);
                let step = roll.step(pos.x);
                // Which notes are under the x position (any note starting nearest)?
                let hit_note = pat
                    .notes
                    .iter()
                    .filter(|n| step >= n.start)
                    .min_by(|a, b| {
                        (step - a.start)
                            .partial_cmp(&(step - b.start))
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .cloned();
                let targets: Vec<u64> = if !self.sel.notes.is_empty() {
                    self.sel.notes.iter().copied().collect()
                } else {
                    hit_note.map(|n| vec![n.id]).unwrap_or_default()
                };
                if !targets.is_empty() {
                    let drag_ids = self
                        .editor
                        .vel_drag
                        .clone()
                        .unwrap_or_else(|| targets.clone());
                    self.edit("Note velocity", Some("vel-drag"), |p| {
                        if let Some(pat) = ops::pattern_mut(p, track.id, pid) {
                            for n in pat.notes.iter_mut() {
                                if drag_ids.contains(&n.id) {
                                    n.velocity = v;
                                }
                            }
                        }
                    });
                    self.editor.vel_drag = Some(drag_ids);
                }
            }
        }
    }

    fn paint_roll(&self, ui: &Ui, track: &Track, pat: &Pattern, roll: &Roll, hover: Option<Pos2>) {
        let pal = self.palette();
        let p = ui.painter_at(roll.grid.union(roll.vel).union(roll.ruler));
        let scale = self
            .editor
            .scale_highlight
            .then(|| scale_pitch_classes(self.project.key));
        // Row backgrounds
        let first_row = (roll.scroll_y / ROW_H).floor().max(0.0) as usize;
        let last_row = ((roll.scroll_y + roll.grid.height()) / ROW_H)
            .ceil()
            .min(PITCHES as f32) as usize;
        for row in first_row..last_row {
            let pitch = LAST_PITCH - row as u8;
            let y = roll.grid.top() + row as f32 * ROW_H - roll.scroll_y;
            let black = matches!(pitch % 12, 1 | 3 | 6 | 8 | 10);
            let r = Rect::from_min_size(pos2(roll.grid.left(), y), vec2(roll.grid.width(), ROW_H));
            p.rect_filled(
                r,
                0.0,
                if black {
                    Color32::from_black_alpha(38)
                } else {
                    pal.canvas
                },
            );
            if scale.is_some_and(|pcs| pcs[pitch as usize % 12]) {
                p.rect_filled(r, 0.0, pal.accent.gamma_multiply(0.08));
            }
            if pitch.is_multiple_of(12) {
                p.hline(roll.grid.x_range(), y, Stroke::new(1.0_f32, pal.grid_bar));
            } else {
                p.hline(roll.grid.x_range(), y, Stroke::new(0.5_f32, pal.grid_faint));
            }
        }
        // Step/beat/bar lines
        let s0 = roll.step(roll.grid.left()).max(0.0).floor() as i64;
        let s1 = roll.step(roll.grid.right()).ceil() as i64;
        let step_lines = roll.pps >= 14.0;
        for s in s0.max(0)..=s1.min(pat.length as i64 * 2) {
            if s < 0 {
                break;
            }
            let x = roll.x(s as f64);
            if s % 16 == 0 {
                p.vline(x, roll.grid.y_range(), Stroke::new(1.0_f32, pal.grid_bar));
            } else if s % 4 == 0 {
                p.vline(x, roll.grid.y_range(), Stroke::new(1.0_f32, pal.grid_beat));
            } else if step_lines {
                p.vline(x, roll.grid.y_range(), Stroke::new(0.5_f32, pal.grid_faint));
            }
        }
        // Beyond pattern end
        let end_x = roll.x(pat.length as f64);
        if end_x < roll.grid.right() {
            p.rect_filled(
                Rect::from_min_max(
                    pos2(end_x, roll.grid.top()),
                    pos2(roll.grid.right(), roll.vel.bottom()),
                ),
                0.0,
                Color32::from_black_alpha(80),
            );
        }
        // Notes
        let color = theme::rgb(track.color);
        let visible_lo = roll
            .pitch_at(roll.grid.top() + roll.grid.height())
            .unwrap_or(0);
        let visible_hi = roll.pitch_at(roll.grid.top()).unwrap_or(127);
        for n in &pat.notes {
            if n.pitch < visible_lo || n.pitch > visible_hi {
                continue;
            }
            let r = roll.note_rect(n);
            if r.right() < roll.grid.left() || r.left() > roll.grid.right() {
                continue;
            }
            let selected = self.sel.notes.contains(&n.id);
            let body = theme::mix(pal.border_soft, color, 0.35 + n.velocity * 0.65);
            p.rect_filled(r, 3.0, body);
            if selected {
                p.rect_stroke(
                    r.expand(1.0),
                    3.0,
                    Stroke::new(1.5_f32, pal.text),
                    StrokeKind::Inside,
                );
            }
            // Resize affordance on the right edge
            if r.width() > 8.0 {
                p.rect_filled(
                    Rect::from_min_max(
                        pos2(r.right() - 3.5, r.top() + 3.0),
                        pos2(r.right() - 1.5, r.bottom() - 3.0),
                    ),
                    1.0,
                    Color32::from_black_alpha(90),
                );
            }
            if r.width() > 26.0 {
                p.text(
                    pos2(r.left() + 5.0, r.center().y),
                    Align2::LEFT_CENTER,
                    note_name(n.pitch),
                    FontId::proportional(9.5),
                    theme::ink_on(body),
                );
            }
        }
        // Ghost of dragged position? (current drag delta shown by note movement already)
        // Marquee
        if let Some(m) = &self.editor.marquee {
            let r = Rect::from_two_pos(m.start, m.current).intersect(roll.grid);
            if r.is_positive() {
                p.rect_filled(r, 0.0, pal.link.gamma_multiply(0.12));
                p.rect_stroke(r, 0.0, Stroke::new(1.0_f32, pal.link), StrokeKind::Inside);
            }
        }
        // Playhead inside a clip of this pattern
        if self.transport.playing {
            if let Some(local) = self.pattern_playhead(track, pat.id) {
                let x = roll.x(local);
                if x >= roll.grid.left() && x <= roll.grid.right() {
                    p.vline(
                        x,
                        roll.grid.top()..=roll.vel.bottom(),
                        Stroke::new(1.5_f32, pal.accent),
                    );
                }
            }
        }
        // Hover ghost cell
        if self.editor.drag.is_none() && self.editor.draw.is_none() {
            if let Some(pos) = hover
                .filter(|h| roll.grid.contains(*h) && self.roll_note_at(pat, roll, *h).is_none())
            {
                if let Some(pitch) = roll.pitch_at(pos.y) {
                    // The ghost previews the pitch a drawn note would land
                    // on — under snap-to-scale that is the scale row, not
                    // the chromatic row under the pointer.
                    let pitch = self.snap_to_scale_pitch(track, pitch);
                    let step = snap_floor(roll.step(pos.x), self.editor.snap);
                    let gh = Rect::from_min_size(
                        pos2(roll.x(step), roll.y(pitch) + 1.0),
                        vec2(self.editor.note_len as f32 * roll.pps, ROW_H - 2.0),
                    );
                    p.rect_filled(gh, 3.0, color.gamma_multiply(0.18));
                    p.rect_stroke(
                        gh,
                        3.0,
                        Stroke::new(1.0_f32, color.gamma_multiply(0.6)),
                        StrokeKind::Inside,
                    );
                }
            }
        }
        // Empty pattern hint
        if pat.notes.is_empty() {
            p.text(
                roll.grid.center(),
                Align2::CENTER_CENTER,
                "Empty pattern — click the grid to draw notes",
                FontId::proportional(13.0),
                pal.text_faint,
            );
        }
        // Borders
        p.hline(
            roll.grid.x_range(),
            roll.ruler.bottom(),
            Stroke::new(1.0_f32, pal.border),
        );
        p.vline(
            roll.grid.left(),
            roll.grid.top()..=roll.vel.bottom(),
            Stroke::new(1.0_f32, pal.border),
        );
        p.hline(
            roll.grid.x_range(),
            roll.vel.top(),
            Stroke::new(1.0_f32, pal.border),
        );
    }

    fn pattern_playhead(&self, track: &Track, pid: u64) -> Option<f64> {
        let pos = self.transport.position;
        for clip in &track.clips {
            if clip.pattern_id == pid {
                let end = clip.start + ops::pattern(track, pid).map_or(0.0, |p| p.length as f64);
                if pos >= clip.start && pos < end {
                    return Some(pos - clip.start);
                }
            }
        }
        None
    }

    fn paint_keys(&self, ui: &mut Ui, track: &Track, roll: &Roll) {
        let pal = self.palette();
        let p = ui.painter_at(roll.keys);
        p.rect_filled(roll.keys, 0.0, pal.panel);
        let scale = self
            .editor
            .scale_highlight
            .then(|| scale_pitch_classes(self.project.key));
        let first_row = (roll.scroll_y / ROW_H).floor().max(0.0) as usize;
        let last_row = ((roll.scroll_y + roll.grid.height()) / ROW_H)
            .ceil()
            .min(PITCHES as f32) as usize;
        let mut preview_pitch = None;
        for row in first_row..last_row {
            let pitch = LAST_PITCH - row as u8;
            let black = matches!(pitch % 12, 1 | 3 | 6 | 8 | 10);
            let y = roll.grid.top() + row as f32 * ROW_H - roll.scroll_y;
            let r = Rect::from_min_max(
                pos2(roll.keys.left(), y),
                pos2(
                    if black {
                        roll.keys.left() + 40.0
                    } else {
                        roll.keys.right()
                    },
                    y + ROW_H,
                ),
            );
            let resp = ui.interact(r, ui.id().with(("key", pitch)), Sense::click());
            if resp.hovered() || resp.is_pointer_button_down_on() {
                p.rect_filled(r, 0.0, pal.control_hover);
            } else {
                p.rect_filled(
                    r,
                    0.0,
                    if black {
                        Color32::from_rgb(15, 14, 13)
                    } else {
                        theme::mix(pal.panel, pal.text, 0.22)
                    },
                );
                if scale.is_some_and(|pcs| pcs[pitch as usize % 12]) {
                    p.rect_filled(r, 0.0, pal.accent.gamma_multiply(0.08));
                }
            }
            if pitch.is_multiple_of(12) {
                p.text(
                    pos2(roll.keys.right() - 6.0, y + ROW_H * 0.5),
                    Align2::RIGHT_CENTER,
                    note_name(pitch),
                    FontId::proportional(9.5),
                    pal.text_faint,
                );
            }
            if resp.clicked() || resp.dragged_by(PointerButton::Primary) {
                preview_pitch = Some(pitch);
            }
        }
        p.vline(
            roll.keys.right(),
            roll.keys.y_range(),
            Stroke::new(1.0_f32, pal.border),
        );
        if let Some(pitch) = preview_pitch {
            let vel = self.editor.velocity;
            self.preview(track.id, pitch, vel);
        }
    }

    fn paint_roll_ruler(&self, ui: &Ui, track: &Track, pat: &Pattern, roll: &Roll) {
        let pal = self.palette();
        let p = ui.painter_at(roll.ruler);
        p.rect_filled(roll.ruler, 0.0, pal.panel);
        let pps = roll.pps;
        let every = [1i64, 2, 4, 8, 16]
            .into_iter()
            .find(|k| *k as f32 * 16.0 * pps >= 44.0)
            .unwrap_or(16);
        let b0 = (roll.step(roll.ruler.left()).max(0.0) / 16.0).floor() as i64;
        let b1 = (roll.step(roll.ruler.right()) / 16.0).ceil() as i64;
        for b in b0..=b1.min((pat.length as i64 + 15) / 16) {
            let x = roll.x(b as f64 * 16.0);
            if b % every == 0 && b * 16 < pat.length as i64 {
                p.text(
                    pos2(x + 3.0, roll.ruler.center().y),
                    Align2::LEFT_CENTER,
                    format!("{}", b + 1),
                    FontId::proportional(10.5),
                    pal.text_dim,
                );
            }
            p.vline(
                x,
                roll.ruler.bottom() - 6.0..=roll.ruler.bottom(),
                Stroke::new(1.0_f32, pal.border),
            );
        }
        if self.transport.playing {
            if let Some(local) = self.pattern_playhead(track, pat.id) {
                let x = roll.x(local);
                p.add(Shape::convex_polygon(
                    vec![
                        pos2(x - 5.0, roll.ruler.bottom() - 8.0),
                        pos2(x + 5.0, roll.ruler.bottom() - 8.0),
                        pos2(x, roll.ruler.bottom() - 1.0),
                    ],
                    pal.accent,
                    Stroke::NONE,
                ));
            }
        }
    }

    fn paint_vel_lane(&self, ui: &Ui, track_id: u64, pat: &Pattern, roll: &Roll, labels: Rect) {
        let pal = self.palette();
        let p = ui.painter_at(roll.vel.union(labels));
        p.rect_filled(labels, 0.0, pal.panel);
        p.text(
            labels.center(),
            Align2::CENTER_CENTER,
            "VEL",
            FontId::proportional(9.0),
            pal.text_faint,
        );
        p.rect_filled(roll.vel, 0.0, pal.canvas_alt);
        let color = theme::rgb(self.selected_track().map_or([140, 140, 140], |t| t.color));
        let s0 = roll.step(roll.vel.left()).max(0.0);
        let s1 = roll.step(roll.vel.right());
        for n in &pat.notes {
            if n.start < s0 - n.duration || n.start > s1 {
                continue;
            }
            let x = roll.x(n.start) + 1.0;
            let w = (n.duration as f32 * roll.pps - 1.5).max(2.0);
            let h = roll.vel.height() * n.velocity;
            let r = Rect::from_min_max(
                pos2(x, roll.vel.bottom() - h),
                pos2((x + w).min(roll.vel.right()), roll.vel.bottom()),
            );
            if r.right() < roll.vel.left() {
                continue;
            }
            let selected = self.sel.notes.contains(&n.id);
            p.rect_filled(
                r,
                1.0,
                if selected {
                    theme::mix(color, pal.text, 0.4)
                } else {
                    color.gamma_multiply(0.85)
                },
            );
        }
        let _ = track_id;
    }

    // ----- computer keyboard & chord strip ---------------------------------

    // ----- keys tab ---------------------------------------------------------

    /// The dedicated Keys tab (⌘3): a full-size playable keyboard for the
    /// selected track. Melodic tracks get a three-octave piano — mouse
    /// press/drag glissandos, computer keys keep working, held keys glow.
    /// Drum tracks get one pad per kit piece instead.
    pub(super) fn keys_ui(&mut self, ui: &mut Ui) {
        let pal = self.palette();
        let Some(track_id) = self.sel.track else {
            ui.centered_and_justified(|ui| {
                ui.label(RichText::new("Select a track to play it.").color(pal.text_dim));
            });
            return;
        };
        let drum = ops::track(&self.project, track_id).is_some_and(|t| t.shows_drum_grid());
        if drum {
            self.keys_drum_ui(ui, track_id);
        } else {
            self.keys_piano_ui(ui, track_id);
        }
    }

    /// Melodic Keys tab: three octaves of piano keys centred on the play
    /// octave (`kb_octave`), mouse + computer input, held-key glow.
    fn keys_piano_ui(&mut self, ui: &mut Ui, track_id: u64) {
        let pal = self.palette();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            theme::color_dot(ui, pal.accent, 6.0);
            ui.label(RichText::new("Keyboard").small().color(pal.text_dim));
            for (label, d, tip) in [
                ("‹", -1, "Octave down (Q)"),
                ("›", 1, "Octave up (W)"),
            ] {
                if ui
                    .add(egui::Button::new(label).small())
                    .on_hover_text(tip)
                    .clicked()
                {
                    self.editor.kb_octave = (self.editor.kb_octave + d)
                        .clamp(KB_OCTAVE_MIN, KB_OCTAVE_MAX);
                }
            }
            ui.label(
                RichText::new(format!("C{}", self.editor.kb_octave))
                    .small()
                    .color(pal.text),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                theme::hint(
                    ui,
                    pal,
                    "Drag the keys to glissando · Z X C V B N M + S D G H J play · Q/W octave · Shift +12",
                );
            });
        });
        let area = ui.available_rect_before_wrap();
        if area.width() < 240.0 || area.height() < 80.0 {
            return;
        }

        // Three octaves: kb_octave-1 ..= kb_octave+1. Whites are laid out
        // left to right; each black centres on the boundary after its white.
        let first_oct = self.editor.kb_octave - 1;
        let mut whites: Vec<(u8, i32, usize)> = Vec::new(); // (pitch, octave, slot)
        for o in first_oct..=first_oct + 2 {
            for (slot, &pc) in KB_WHITE_PCS.iter().enumerate() {
                let p = (o + 1) * 12 + i32::from(pc);
                if (0..=i32::from(ops::MAX_PITCH)).contains(&p) {
                    whites.push((p as u8, o, slot));
                }
            }
        }
        if whites.is_empty() {
            return;
        }
        let white_w = area.width() / whites.len() as f32;
        let black_w = white_w * 0.62;
        let black_h = area.height() * 0.62;
        let white_rects: Vec<(Rect, u8, i32)> = whites
            .iter()
            .enumerate()
            .map(|(i, &(pitch, o, _))| {
                let x0 = area.left() + i as f32 * white_w;
                (
                    Rect::from_min_max(pos2(x0, area.top()), pos2(x0 + white_w, area.bottom())),
                    pitch,
                    o,
                )
            })
            .collect();
        let mut black_rects: Vec<(Rect, u8)> = Vec::new();
        for o in first_oct..=first_oct + 2 {
            for &(bpc, _) in &KB_BLACKS {
                let p = (o + 1) * 12 + i32::from(bpc);
                if !(0..=i32::from(ops::MAX_PITCH)).contains(&p) {
                    continue;
                }
                // The white key a semitone below is always present when the
                // black is — its right edge is the black's centre line.
                let Some(idx) = whites.iter().position(|&(wp, _, _)| i32::from(wp) == p - 1) else {
                    continue;
                };
                let boundary = area.left() + (idx + 1) as f32 * white_w;
                black_rects.push((
                    Rect::from_min_max(
                        pos2(boundary - black_w * 0.5, area.top()),
                        pos2(boundary + black_w * 0.5, area.top() + black_h),
                    ),
                    p as u8,
                ));
            }
        }

        // Interaction: press/drag across the keys glissandos — retrigger
        // only when the pitch under the pointer actually changes.
        let resp = ui.interact(area, ui.id().with("keys_tab_area"), Sense::click_and_drag());
        let dragging = resp.is_pointer_button_down_on() || resp.dragged();
        let pos = ui.input(|i| {
            if dragging {
                i.pointer.interact_pos()
            } else {
                i.pointer.hover_pos()
            }
        });
        let pointer_pitch = pos.filter(|p| area.contains(*p)).and_then(|p| {
            black_rects
                .iter()
                .find(|(r, _)| r.contains(p))
                .map(|(_, pitch)| *pitch)
                .or_else(|| {
                    white_rects
                        .iter()
                        .find(|(r, ..)| r.contains(p))
                        .map(|(_, pitch, _)| *pitch)
                })
        });
        self.editor.kb_pointer = pointer_pitch;
        self.editor.kb_down = dragging;
        if dragging {
            match pointer_pitch {
                Some(p) if self.editor.kb_last_preview != Some(p) => {
                    self.editor.kb_last_preview = Some(p);
                    let vel = self.editor.velocity;
                    self.preview(track_id, p, vel);
                }
                None => self.editor.kb_last_preview = None,
                _ => {}
            }
        } else {
            self.editor.kb_last_preview = None;
        }
        // MIDI recording: a mouse key is a note — press inserts it at the
        // playing transport step (or the step-record playhead), dragging
        // retriggers per pitch like the computer row, release finalizes.
        self.keys_record(pointer_pitch, dragging);
        resp.on_hover_cursor(CursorIcon::PointingHand);

        // Held pitches: computer keys at the play octave + the mouse key.
        let mut hot: BTreeSet<u8> = held_keyboard_keys(ui.ctx())
            .iter()
            .filter_map(|&i| {
                let p = (self.editor.kb_octave + 1) * 12 + i as i32;
                (0..=i32::from(ops::MAX_PITCH))
                    .contains(&p)
                    .then_some(p as u8)
            })
            .collect();
        if dragging {
            if let Some(p) = pointer_pitch {
                hot.insert(p);
            }
        }
        let hover_pitch = (!dragging).then_some(pointer_pitch).flatten();
        let p = ui.painter_at(area);
        let base_white = theme::mix(pal.panel, pal.text, 0.20);
        let base_black = Color32::from_rgb(14, 13, 12);
        let top_round = CornerRadius {
            nw: 4,
            ne: 4,
            sw: 0,
            se: 0,
        };
        for (rect, pitch, octave) in &white_rects {
            let is_hot = hot.contains(pitch);
            let is_hov = hover_pitch == Some(*pitch);
            let fill = if is_hot {
                pal.accent.gamma_multiply(0.40)
            } else if is_hov {
                theme::mix(base_white, pal.text, 0.16)
            } else {
                base_white
            };
            p.rect_filled(*rect, top_round, fill);
            p.rect_stroke(
                *rect,
                top_round,
                Stroke::new(1.0_f32, pal.border_soft),
                StrokeKind::Inside,
            );
            let pc = (*pitch % 12) as usize;
            let fg = if is_hot { pal.text } else { pal.text_dim };
            // Note name at the bottom of every white key.
            p.text(
                pos2(rect.center().x, rect.bottom() - 13.0),
                Align2::CENTER_CENTER,
                NOTE_NAMES[pc],
                FontId::proportional(10.0),
                fg,
            );
            // C keys carry their octave number near the top.
            if pc == 0 {
                p.text(
                    pos2(rect.center().x, rect.top() + 9.0),
                    Align2::CENTER_CENTER,
                    format!("C{octave}"),
                    FontId::proportional(8.5),
                    pal.text_faint,
                );
            }
            // The play octave's whites also show their computer key.
            if *octave == self.editor.kb_octave {
                p.text(
                    pos2(
                        rect.center().x,
                        rect.top() + if pc == 0 { 20.0 } else { 10.0 },
                    ),
                    Align2::CENTER_CENTER,
                    KB_KEYS[pc].name(),
                    FontId::proportional(9.0),
                    pal.text_faint,
                );
            }
        }
        for (rect, pitch) in &black_rects {
            let is_hot = hot.contains(pitch);
            let is_hov = hover_pitch == Some(*pitch);
            let fill = if is_hot {
                pal.accent.gamma_multiply(0.45)
            } else if is_hov {
                theme::mix(base_black, pal.text, 0.22)
            } else {
                base_black
            };
            p.rect_filled(*rect, 2.0, fill);
            p.rect_stroke(
                *rect,
                2.0,
                Stroke::new(1.0_f32, pal.border_soft),
                StrokeKind::Inside,
            );
            let pc = (*pitch % 12) as usize;
            // Play-octave black keys show their computer key letter.
            if (*pitch as i32 / 12) - 1 == self.editor.kb_octave {
                p.text(
                    pos2(rect.center().x, rect.bottom() - 4.0),
                    Align2::CENTER_CENTER,
                    KB_KEYS[pc].name(),
                    FontId::proportional(8.0),
                    if is_hot { pal.text } else { pal.text_faint },
                );
            }
        }
    }

    /// Drum Keys tab: one tappable pad per kit piece, computer-key held
    /// pieces glow.
    fn keys_drum_ui(&mut self, ui: &mut Ui, track_id: u64) {
        let pal = self.palette();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            theme::color_dot(ui, pal.danger, 6.0);
            ui.label(RichText::new("Drum kit").small().color(pal.text_dim));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                theme::hint(
                    ui,
                    pal,
                    "Click or tap a piece · the computer-key row plays them too",
                );
            });
        });
        // Owned copies so `self.preview` stays callable inside the loop.
        let (rows, kb_pitches): (Vec<(u8, String)>, Vec<u8>) = ops::track(&self.project, track_id)
            .map(|t| {
                (
                    t.drum_rows()
                        .iter()
                        .map(|(p, n)| (*p, (*n).to_string()))
                        .collect(),
                    drum_keyboard_pitches(t),
                )
            })
            .unwrap_or_default();
        if rows.is_empty() {
            ui.label(RichText::new("No drum rows.").color(pal.text_dim));
            return;
        }
        let held: BTreeSet<u8> = held_keyboard_keys(ui.ctx())
            .iter()
            .filter_map(|&i| kb_pitches.get(i).copied())
            .collect();
        let vel = self.editor.velocity;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            for (pitch, name) in &rows {
                let on = held.contains(pitch);
                let b = egui::Button::new(RichText::new(name.as_str()).small().color(if on {
                    pal.text
                } else {
                    pal.text_dim
                }))
                .fill(if on {
                    pal.accent.gamma_multiply(0.40)
                } else {
                    pal.control
                })
                .min_size(vec2(84.0, 30.0));
                if ui
                    .add(b)
                    .on_hover_text(format!("{name} · note {pitch}"))
                    .clicked()
                {
                    self.preview(track_id, *pitch, vel);
                    if self.transport.recording {
                        // A pad tap is a full note: insert and release at
                        // once (one Snap cell long; step recording then
                        // advances the playhead).
                        self.record_note_on(*pitch);
                        self.record_note_off(*pitch);
                    }
                }
            }
        });
    }

    /// Computer-keyboard performance: while the pattern editor or Keys tab
    /// is showing, unmapped-modifier key presses audition notes on the
    /// selected track. `Z X C V B N M` are the white keys C–B and
    /// `S D G H J` the black keys C# D# F# G# A#; `Q`/`W` shift the octave a
    /// full step and holding Shift raises it another octave. Drum tracks map
    /// the keys across the kit's piece pitches instead. Fires once per
    /// physical press (key auto-repeat is ignored) at the editor's velocity;
    /// release does nothing — previews are fixed-length. With record armed
    /// each press/release instead starts/ends a note in the selected track's
    /// pattern (live or step recording).
    pub(super) fn keyboard_play(&mut self, ctx: &Context) {
        if self.tab != super::Tab::Editor && self.tab != super::Tab::Keys {
            return;
        }
        let (presses, releases): (KeyEvents, KeyEvents) = ctx.input(|i| {
            let mut presses = Vec::new();
            let mut releases = Vec::new();
            for e in &i.events {
                let Event::Key {
                    key,
                    pressed,
                    repeat,
                    modifiers,
                    ..
                } = e
                else {
                    continue;
                };
                if modifiers.command || modifiers.ctrl || modifiers.alt {
                    continue;
                }
                if *pressed {
                    if !repeat {
                        presses.push((*key, modifiers.shift));
                    }
                } else {
                    releases.push((*key, modifiers.shift));
                }
            }
            (presses, releases)
        });
        if presses.is_empty() && releases.is_empty() {
            return;
        }
        let Some(track_id) = self.sel.track else {
            return;
        };
        let Some(track) = ops::track(&self.project, track_id) else {
            return;
        };
        let drum_pitches = track
            .shows_drum_grid()
            .then(|| drum_keyboard_pitches(track));
        let velocity = self.editor.velocity;
        for (key, shift) in presses {
            if drum_pitches.is_none() {
                match key {
                    Key::Q => {
                        self.editor.kb_octave =
                            (self.editor.kb_octave - 1).clamp(KB_OCTAVE_MIN, KB_OCTAVE_MAX);
                        continue;
                    }
                    Key::W => {
                        self.editor.kb_octave =
                            (self.editor.kb_octave + 1).clamp(KB_OCTAVE_MIN, KB_OCTAVE_MAX);
                        continue;
                    }
                    _ => {}
                }
            }
            let pitch = keyboard_pitch(key, shift, self.editor.kb_octave, &drum_pitches);
            if let Some(pitch) = pitch {
                self.preview(track_id, pitch, velocity);
                self.record_note_on(pitch);
            }
        }
        for (key, shift) in releases {
            let pitch = keyboard_pitch(key, shift, self.editor.kb_octave, &drum_pitches);
            if let Some(pitch) = pitch {
                self.record_note_off(pitch);
            }
        }
    }

    // ----- MIDI recording -----------------------------------------------------

    /// The snap grid recording quantizes to: the editor Snap, at least one
    /// sixty-fourth note.
    fn record_snap(&self) -> f64 {
        self.editor.snap.max(ops::MIN_NOTE_DURATION)
    }

    /// Resolves where a note played at song step `pos` lands: the pattern
    /// whose clip covers the playhead, plus the pattern-relative start step
    /// (loops fold the step back into the pattern). When nothing covers the
    /// position, the active pattern's clip is extended forward to reach it;
    /// `None` when that is impossible (backwards, or past the loop cap).
    fn record_target(&mut self, track_id: u64, pos: f64) -> Option<(u64, f64)> {
        if let Some(hit) = covering_pattern(ops::track(&self.project, track_id)?, pos) {
            return Some(hit);
        }
        let pid = self.current_pattern_id(ops::track(&self.project, track_id)?)?;
        let (clip_id, clip_start, loops, plen, cap) = {
            let track = ops::track(&self.project, track_id)?;
            let clip = track.clips.iter().find(|c| c.pattern_id == pid)?;
            let plen = ops::pattern(track, pid)?.length.max(1) as f64;
            (
                clip.id,
                clip.start,
                clip.loop_count(),
                plen,
                ops::max_clip_loops(track, clip),
            )
        };
        if pos < clip_start - 1e-9 {
            return None;
        }
        let need = ((pos - clip_start) / plen).floor() as u32 + 1;
        if need > loops && need <= cap {
            self.edit("Extend clip for recording", None, |p| {
                let _ = ops::set_clip_loops(p, track_id, clip_id, need);
            });
        }
        covering_pattern(ops::track(&self.project, track_id)?, pos)
    }

    /// Starts (or retriggers) a recorded note from the Keyboard tab or the
    /// computer-keyboard row. While the transport plays, the note starts at
    /// the snapped transport step; while it is stopped (step recording) the
    /// note lands at the playhead, which advances one Snap cell on release.
    /// Each insertion is its own undo step. Returns the new note's id.
    pub(super) fn record_note_on(&mut self, pitch: u8) -> Option<u64> {
        if !self.transport.recording {
            return None;
        }
        let track_id = self.sel.track?;
        if ops::track(&self.project, track_id).is_some_and(|t| t.audio) {
            self.set_status(
                StatusKind::Warning,
                "Recording targets MIDI tracks — audio lanes only hold imported WAV clips.",
            );
            return None;
        }
        let snap = self.record_snap();
        let start = record_start(self.transport.position, snap);
        let Some((pid, rel)) = self.record_target(track_id, start) else {
            self.set_status(
                StatusKind::Warning,
                "Nothing to record into here — no clip covers the playhead.",
            );
            return None;
        };
        let velocity = self.editor.velocity;
        let mut note_id = None;
        self.edit("Record note", None, |p| {
            if let Some(pat) = ops::pattern_mut(p, track_id, pid) {
                note_id = ops::add_note(pat, pitch, rel, snap, velocity);
            }
        });
        let Some(id) = note_id else {
            self.set_status(
                StatusKind::Warning,
                "Could not record — the pattern is at the note limit.",
            );
            return None;
        };
        self.transport.recorded_notes += 1;
        let n = self.transport.recorded_notes;
        self.set_status(
            StatusKind::Success,
            format!("Recorded {n} note{}", sidebar::plural(n)),
        );
        self.editor.record_pending.insert(
            pitch,
            PendingRecord {
                track_id,
                pattern_id: pid,
                note_id: id,
                start_step: start,
                step_record: !self.transport.playing,
            },
        );
        Some(id)
    }

    /// Ends a held recorded note: its duration becomes the release step
    /// minus the snapped start, rounded up to whole Snap cells (at least
    /// one, so a tap still sounds), clamped inside the pattern. In step
    /// recording the playhead then advances one Snap cell.
    pub(super) fn record_note_off(&mut self, pitch: u8) {
        let Some(held) = self.editor.record_pending.remove(&pitch) else {
            return;
        };
        let snap = self.record_snap();
        let duration = record_duration(held.start_step, self.transport.position, snap);
        self.edit("Record note", None, |p| {
            if let Some(pat) = ops::pattern_mut(p, held.track_id, held.pattern_id) {
                if let Some(n) = pat.notes.iter_mut().find(|n| n.id == held.note_id) {
                    let room = (pat.length as f64 - n.start).max(ops::MIN_NOTE_DURATION);
                    n.duration = duration.clamp(ops::MIN_NOTE_DURATION, room);
                }
            }
        });
        if held.step_record {
            self.seek(record_start(held.start_step, snap) + snap);
        }
    }

    /// Keys-tab mouse recording state machine: press inserts a note at the
    /// playing transport step (or the step-record playhead), dragging
    /// retriggers per pitch like the computer row, release finalizes the
    /// duration.
    fn keys_record(&mut self, pointer_pitch: Option<u8>, down: bool) {
        if !self.transport.recording {
            self.editor.kb_was_down = false;
            self.editor.kb_record_pitch = None;
            return;
        }
        let was = self.editor.kb_was_down;
        self.editor.kb_was_down = down;
        if down && !was {
            if let Some(p) = pointer_pitch {
                self.record_note_on(p);
                self.editor.kb_record_pitch = Some(p);
            }
        } else if down {
            if let Some(p) = pointer_pitch {
                if self.editor.kb_record_pitch != Some(p) {
                    if let Some(prev) = self.editor.kb_record_pitch.take() {
                        self.record_note_off(prev);
                    }
                    self.record_note_on(p);
                    self.editor.kb_record_pitch = Some(p);
                }
            }
        } else if was {
            if let Some(p) = self.editor.kb_record_pitch.take() {
                self.record_note_off(p);
            }
        }
    }

    /// Voices `spec` — a diatonic, borrowed or progression chord resolved
    /// against the song key — for the chord strip: built in the track's home
    /// octave, with the strip's inversion and slash-bass options applied,
    /// centred on the track's audition pitch. The returned symbol gains
    /// slash notation when the lowest tone isn't the chord root (a
    /// root-position C stays "C", C/G shows "C/G"). `None` when the chord
    /// voices to nothing in MIDI range.
    fn spec_pitches(&self, track: &Track, spec: &ops::ChordSpec) -> Option<(Vec<u8>, String)> {
        let key = self.project.key;
        let home = sidebar::audition_params(track, track.instrument.default_pitch()).0;
        let mut tones = ops::chord_notes(spec, kb_home_octave(track), self.editor.velocity);
        if self.editor.chord_inversion > 0 {
            tones = ops::invert_chord(&tones, usize::from(self.editor.chord_inversion));
        }
        if let Some(pc) = self.editor.chord_bass.pitch_class(key, spec.root) {
            tones = ops::with_bass(&tones, pc);
        }
        let pitches = ops::chord_midi_pitches(&tones, home);
        let mut symbol = spec.symbol();
        if let Some(&low) = pitches.first() {
            if low % 12 != spec.root {
                symbol = format!("{symbol}/{}", NOTE_NAMES[low as usize % 12]);
            }
        }
        (!pitches.is_empty()).then_some((pitches, symbol))
    }

    /// Stamps diatonic chord `degree` into the selected melodic track's
    /// current pattern at the pointer (or playhead): a block chord, or a
    /// strum rolled `chord_spacing` steps in `chord_direction` order when
    /// the spacing is non-zero. One undo step; the new notes become the
    /// selection and the chord is auditioned.
    pub(super) fn stamp_diatonic_chord(&mut self, degree: usize, ctx: &Context) {
        let key = self.project.key;
        let quality = self
            .editor
            .chord_quality
            .unwrap_or_else(|| key.degree_chord_quality(degree));
        self.stamp_chord_spec(&ops::ChordSpec::new(key.degree_root(degree), quality), ctx);
    }

    /// Stamps `spec` — a diatonic, borrowed or progression chord resolved
    /// against the song key — into the selected melodic track's current
    /// pattern at the pointer (or playhead): a block chord, or a strum
    /// rolled `chord_spacing` steps in `chord_direction` order when the
    /// spacing is non-zero. One undo step; the new notes become the
    /// selection and the chord is auditioned.
    pub(super) fn stamp_chord_spec(&mut self, spec: &ops::ChordSpec, ctx: &Context) {
        let Some(track) = self.selected_track() else {
            return;
        };
        if track.shows_drum_grid() {
            self.set_status(
                StatusKind::Warning,
                "Chords stamp into melodic tracks — the drum grid has no chord tones.",
            );
            return;
        }
        let Some(pid) = self.current_pattern_id(track) else {
            return;
        };
        let (track_id, pid) = (track.id, pid);
        let Some((pitches, symbol)) = self.spec_pitches(track, spec) else {
            self.set_status(
                StatusKind::Warning,
                "That chord has no notes in MIDI range.",
            );
            return;
        };
        let pattern_len = ops::pattern(track, pid).map_or(16, |p| p.length);
        let step = self
            .editor_paste_target(ctx)
            .map(|(s, _)| s)
            .unwrap_or(self.transport.position);
        // A chord slot: half the pattern, capped at one bar.
        let duration = (f64::from(pattern_len) * 0.5).clamp(4.0, 16.0);
        let velocity = self.editor.velocity;
        let spacing = self.editor.chord_spacing;
        let direction = self.editor.chord_direction;
        let mut new = Vec::new();
        self.edit(&format!("Stamp {symbol}"), None, |p| {
            if let Some(pat) = ops::pattern_mut(p, track_id, pid) {
                new = if spacing > 0.0 {
                    ops::stamp_strum(pat, step, &pitches, spacing, direction, duration, velocity)
                } else {
                    ops::stamp_chord(pat, step, &pitches, duration, velocity)
                };
            }
        });
        if new.is_empty() {
            self.set_status(
                StatusKind::Warning,
                format!("Could not stamp {symbol} — the pattern is at the note limit."),
            );
            return;
        }
        let n = new.len();
        self.sel.notes = new.into_iter().collect();
        self.focus = super::Focus::Editor;
        for &pitch in &pitches {
            self.preview(track_id, pitch, 0.9);
        }
        self.set_status(
            StatusKind::Success,
            format!("Stamped {symbol} — {n} note{}.", sidebar::plural(n)),
        );
    }

    /// Stamps a whole progression preset across consecutive bars starting at
    /// the pointer (or playhead): one chord per bar, each voiced with the
    /// strip's inversion and slash-bass options and rolled when the strum
    /// spacing is non-zero. The Type override replaces every chord's
    /// quality. One undo step for the whole progression; the new notes
    /// become the selection and the last chord is auditioned.
    pub(super) fn stamp_progression(
        &mut self,
        preset: &'static ops::ProgressionPreset,
        ctx: &Context,
    ) {
        let Some(track) = self.selected_track() else {
            return;
        };
        if track.shows_drum_grid() {
            self.set_status(
                StatusKind::Warning,
                "Chords stamp into melodic tracks — the drum grid has no chord tones.",
            );
            return;
        }
        let Some(pid) = self.current_pattern_id(track) else {
            return;
        };
        let (track_id, pid) = (track.id, pid);
        let specs = ops::progression_specs(self.project.key, preset, self.editor.chord_quality);
        let voiced: Vec<Option<(Vec<u8>, String)>> = specs
            .iter()
            .map(|spec| self.spec_pitches(track, spec))
            .collect();
        if voiced.iter().all(Option::is_none) {
            self.set_status(
                StatusKind::Warning,
                "That progression has no notes in MIDI range.",
            );
            return;
        }
        let chords: Vec<Vec<u8>> = voiced
            .iter()
            .flatten()
            .map(|(pitches, _)| pitches.clone())
            .collect();
        let pattern_len = ops::pattern(track, pid).map_or(16, |p| p.length);
        let step = self
            .editor_paste_target(ctx)
            .map(|(s, _)| s)
            .unwrap_or(self.transport.position);
        // A chord slot: half the pattern, capped at one bar.
        let duration = (f64::from(pattern_len) * 0.5).clamp(4.0, 16.0);
        let velocity = self.editor.velocity;
        let spacing = self.editor.chord_spacing;
        let strum = (spacing > 0.0).then_some((spacing, self.editor.chord_direction));
        let mut new = Vec::new();
        self.edit(&format!("Stamp {}", preset.name), None, |p| {
            if let Some(pat) = ops::pattern_mut(p, track_id, pid) {
                new = ops::stamp_progression(
                    pat,
                    step,
                    &chords,
                    ops::STEPS_PER_BAR as f64,
                    duration,
                    velocity,
                    strum,
                );
            }
        });
        if new.is_empty() {
            self.set_status(
                StatusKind::Warning,
                format!(
                    "Could not stamp {} — the pattern is at the note limit.",
                    preset.name
                ),
            );
            return;
        }
        let n = new.len();
        self.sel.notes = new.into_iter().collect();
        self.focus = super::Focus::Editor;
        if let Some((pitches, _)) = voiced.iter().flatten().last() {
            for &pitch in pitches.iter().take(4) {
                self.preview(track_id, pitch, 0.9);
            }
        }
        self.set_status(
            StatusKind::Success,
            format!("Stamped {} — {n} note{}.", preset.name, sidebar::plural(n)),
        );
    }

    /// Hover audition for the chord strip: plays the chord's tones once.
    fn audition_chord_spec(&self, track: &Track, spec: &ops::ChordSpec) {
        if track.shows_drum_grid() {
            return;
        }
        if let Some((pitches, _)) = self.spec_pitches(track, spec) {
            for &pitch in pitches.iter().take(4) {
                self.preview(track.id, pitch, 0.7);
            }
        }
    }

    /// Flips the scale-highlight row tint (chord-strip toggle).
    pub(super) fn toggle_scale_highlight(&mut self) {
        self.editor.scale_highlight = !self.editor.scale_highlight;
    }

    /// Collapsible chord strip above the pattern editor: three palettes on a
    /// segmented tab — the seven diatonic chords of the song key, borrowed
    /// chords / secondary dominants and one-click progression presets — all
    /// one-click stamps sharing a quality override, inversion and slash-bass
    /// voicing, strum direction/spacing and the scale-highlight toggle.
    /// View state lives on [`EditorView`]; stamping goes through
    /// [`Self::stamp_chord_spec`] and [`Self::stamp_progression`].
    fn chord_strip(&mut self, ui: &mut Ui, track: &Track) {
        let pal = self.palette();
        let open = self.editor.chord_open;
        egui::Frame::new()
            .inner_margin(Margin {
                left: 12,
                right: 12,
                top: 3,
                bottom: 3,
            })
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
                    let b = egui::Button::new(RichText::new(if open { "▾" } else { "▸" }).small())
                        .min_size(vec2(22.0, 22.0));
                    if ui
                        .add(b)
                        .on_hover_text("Show or hide the chord strip")
                        .clicked()
                    {
                        self.editor.chord_open = !self.editor.chord_open;
                    }
                    ui.label(RichText::new("Chords").small().color(pal.text_dim));
                    ui.label(
                        RichText::new(self.project.key.name())
                            .small()
                            .color(pal.text_faint),
                    );
                    if open {
                        ui.separator();
                        for tab in ChordStripTab::ALL {
                            let resp = ui
                                .selectable_label(self.editor.chord_tab == tab, tab.label())
                                .on_hover_text(match tab {
                                    ChordStripTab::Diatonic => {
                                        "The key's seven diatonic chords, I through vii°"
                                    }
                                    ChordStripTab::Borrowed => {
                                        "Colour chords from the parallel key plus secondary dominants"
                                    }
                                    ChordStripTab::Progressions => {
                                        "One-click genre/mood loops stamped one chord per bar"
                                    }
                                });
                            if resp.clicked() {
                                self.editor.chord_tab = tab;
                            }
                        }
                    }
                    if !track.shows_drum_grid() {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let on = self.editor.scale_highlight;
                            let text = if on { "Scale ●" } else { "Scale" };
                            let resp = ui.add(
                                egui::Button::new(RichText::new(text).small())
                                    .min_size(vec2(0.0, 22.0)),
                            );
                            if resp
                                .on_hover_text("Tint piano-roll rows on the song key's scale notes")
                                .clicked()
                            {
                                self.toggle_scale_highlight();
                            }
                        });
                    }
                });
            });
        if !open {
            return;
        }
        if track.shows_drum_grid() {
            ui.horizontal(|ui| {
                theme::hint(
                    ui,
                    pal,
                    "Chords stamp into melodic tracks — select a piano, bass, keys or similar track.",
                );
            });
            return;
        }
        let key = self.project.key;
        let mut stamp = None;
        let mut stamp_degree = None;
        let mut stamp_preset = None;
        let mut hover_spec = None;
        egui::Frame::new()
            .inner_margin(Margin {
                left: 12,
                right: 12,
                top: 1,
                bottom: 4,
            })
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(5.0, 4.0);
                    match self.editor.chord_tab {
                        ChordStripTab::Diatonic => {
                            for degree in 0..7 {
                                let quality = self
                                    .editor
                                    .chord_quality
                                    .unwrap_or_else(|| key.degree_chord_quality(degree));
                                let spec = ops::ChordSpec::new(key.degree_root(degree), quality);
                                let label =
                                    format!("{}·{}", key.degree_numeral(degree), spec.symbol());
                                let resp = ui
                                    .add(
                                        egui::Button::new(RichText::new(label).small())
                                            .min_size(vec2(46.0, 24.0)),
                                    )
                                    .on_hover_text(format!(
                                        "{} — click to stamp at the pointer or playhead",
                                        spec.symbol()
                                    ));
                                if resp.hovered() {
                                    hover_spec = Some(spec);
                                }
                                if resp.clicked() {
                                    stamp_degree = Some(degree);
                                }
                            }
                        }
                        ChordStripTab::Borrowed => {
                            for chord in ops::borrowed_chords(key) {
                                let label = format!("{}·{}", chord.label, chord.symbol);
                                let resp = ui
                                    .add(
                                        egui::Button::new(RichText::new(label).small())
                                            .min_size(vec2(46.0, 24.0)),
                                    )
                                    .on_hover_text(format!(
                                        "{} ({} — borrowed from the parallel key) · click to \
                                         stamp at the pointer or playhead",
                                        chord.symbol, chord.label
                                    ));
                                if resp.hovered() {
                                    hover_spec = Some(chord.spec);
                                }
                                if resp.clicked() {
                                    stamp = Some(chord.spec);
                                }
                            }
                        }
                        ChordStripTab::Progressions => {
                            for preset in &ops::PROGRESSIONS {
                                let symbols =
                                    ops::progression_specs(key, preset, self.editor.chord_quality)
                                        .iter()
                                        .map(|s| s.symbol())
                                        .collect::<Vec<_>>()
                                        .join("-");
                                let resp = ui
                                    .add(
                                        egui::Button::new(RichText::new(preset.name).small())
                                            .min_size(vec2(46.0, 24.0)),
                                    )
                                    .on_hover_text(format!(
                                        "{} — {} · one chord per bar starting at the pointer or \
                                         playhead",
                                        preset.hint, symbols
                                    ));
                                if resp.hovered() {
                                    hover_spec = ops::progression_specs(key, preset, None)
                                        .into_iter()
                                        .next();
                                }
                                if resp.clicked() {
                                    stamp_preset = Some(preset);
                                }
                            }
                        }
                    }
                    ui.separator();
                    ui.label(RichText::new("Type").small().color(pal.text_faint));
                    egui::ComboBox::from_id_salt(("chord-quality", track.id))
                        .width(96.0)
                        .selected_text(self.editor.chord_quality.map_or("Diatonic", |q| q.label()))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.editor.chord_quality, None, "Diatonic");
                            for q in ChordQuality::ALL {
                                ui.selectable_value(
                                    &mut self.editor.chord_quality,
                                    Some(q),
                                    q.label(),
                                );
                            }
                        });
                    ui.label(RichText::new("Inv").small().color(pal.text_faint));
                    egui::ComboBox::from_id_salt(("chord-inversion", track.id))
                        .width(52.0)
                        .selected_text(
                            CHORD_INVERSION_LABELS[usize::from(self.editor.chord_inversion.min(3))],
                        )
                        .show_ui(ui, |ui| {
                            for (v, l) in CHORD_INVERSION_LABELS.iter().enumerate() {
                                ui.selectable_value(&mut self.editor.chord_inversion, v as u8, *l)
                                    .on_hover_text(
                                        "Chord inversion — rotate the lowest tone up an octave",
                                    );
                            }
                        });
                    ui.label(RichText::new("Bass").small().color(pal.text_faint));
                    egui::ComboBox::from_id_salt(("chord-bass", track.id))
                        .width(52.0)
                        .selected_text(chord_bass_label(self.editor.chord_bass, key))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.editor.chord_bass,
                                ops::ChordBass::Off,
                                "Off",
                            );
                            ui.selectable_value(
                                &mut self.editor.chord_bass,
                                ops::ChordBass::Root,
                                "Root",
                            )
                            .on_hover_text("Double the chord root an octave below the voicing");
                            for degree in 0..7 {
                                ui.selectable_value(
                                    &mut self.editor.chord_bass,
                                    ops::ChordBass::Degree(degree),
                                    key.degree_numeral(degree),
                                )
                                .on_hover_text(format!(
                                    "Slash bass on {} ({})",
                                    key.degree_numeral(degree),
                                    NOTE_NAMES[key.degree_root(degree) as usize % 12],
                                ));
                            }
                        });
                    ui.label(RichText::new("Roll").small().color(pal.text_faint));
                    egui::ComboBox::from_id_salt(("chord-spacing", track.id))
                        .width(76.0)
                        .selected_text(chord_spacing_label(self.editor.chord_spacing))
                        .show_ui(ui, |ui| {
                            for (v, l) in
                                [(0.0, "Block"), (0.25, "1/64"), (0.5, "1/32"), (1.0, "1/16")]
                            {
                                ui.selectable_value(&mut self.editor.chord_spacing, v, l)
                                    .on_hover_text("Strum spacing between chord tones (steps)");
                            }
                        });
                    let strumming = self.editor.chord_spacing > 0.0;
                    ui.add_enabled_ui(strumming, |ui| {
                        egui::ComboBox::from_id_salt(("chord-direction", track.id))
                            .width(64.0)
                            .selected_text(self.editor.chord_direction.label())
                            .show_ui(ui, |ui| {
                                for d in ops::StrumDirection::ALL {
                                    ui.selectable_value(
                                        &mut self.editor.chord_direction,
                                        d,
                                        d.label(),
                                    );
                                }
                            });
                    });
                    theme::hint(
                        ui,
                        pal,
                        match self.editor.chord_tab {
                            ChordStripTab::Diatonic | ChordStripTab::Borrowed => {
                                "Click a chord to stamp it at the pointer or playhead"
                            }
                            ChordStripTab::Progressions => {
                                "Click a preset to stamp it across bars at the pointer or playhead"
                            }
                        },
                    );
                });
            });
        // Hover audition: one stab per chord entered, not per frame.
        let hover_id = egui::Id::new("cadence.editor.chord_hover");
        let prev = ui
            .ctx()
            .data(|d| d.get_temp::<String>(hover_id))
            .unwrap_or_default();
        let now = hover_spec.map_or(String::new(), |s| s.symbol());
        if now != prev {
            ui.ctx().data_mut(|d| d.insert_temp(hover_id, now));
            if let Some(spec) = hover_spec {
                self.audition_chord_spec(track, &spec);
            }
        }
        if let Some(spec) = stamp {
            self.stamp_chord_spec(&spec, ui.ctx());
        }
        if let Some(degree) = stamp_degree {
            self.stamp_diatonic_chord(degree, ui.ctx());
        }
        if let Some(preset) = stamp_preset {
            self.stamp_progression(preset, ui.ctx());
        }
    }

    // ----- drum grid --------------------------------------------------------

    fn drum_toolbar(&mut self, ui: &mut Ui, track: &Track, pid: u64, rows: &[(u8, &str)]) {
        let pal = self.palette();
        egui::Frame::new().inner_margin(Margin { left: 12, right: 12, top: 0, bottom: 4 }).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(8.0, 5.0);
                theme::color_dot(ui, pal.danger, 6.0);
                ui.label(RichText::new("Step sequencer").small().color(pal.text_dim));
                ui.label(RichText::new("Velocity").small().color(pal.text_faint));
                let mut v = self.editor.velocity;
                ui.spacing_mut().slider_width = 120.0;
                if ui.add(egui::Slider::new(&mut v, 0.05..=1.0).custom_formatter(|x, _| format!("{:.0}%", x * 100.0))).changed() {
                    self.editor.velocity = v;
                }
                // Fill hits the top row: the kick for built-in drums and the
                // conventional first piece of a kit.
                let fill_pitch = rows.first().map_or(36, |(pitch, _)| *pitch);
                if ui.add(egui::Button::new("Fill").small()).on_hover_text("Four-on-the-floor on the top row (the kick for the built-in kit)").clicked() {
                    let vel = self.editor.velocity;
                    self.edit("Fill pattern", None, |p| {
                        if let Some(pat) = ops::pattern_mut(p, track.id, pid) {
                            for s in (0..pat.length).step_by(4) {
                                ops::set_drum(pat, fill_pitch, s, true, vel);
                            }
                        }
                    });
                }
                let (note_count, stray) = ops::pattern(track, pid).map_or((0, 0), |p| {
                    (p.notes.len(), p.notes.iter().filter(|n| !rows.iter().any(|(d, _)| *d == n.pitch)).count())
                });
                if stray > 0 {
                    ui.label(RichText::new(format!("{stray} off-kit note{}", sidebar::plural(stray))).small().color(pal.warn))
                        .on_hover_text(format!("Notes on pitches outside the {} kit row{} (e.g. imported). They still play; the grid only edits row pitches — re-map them via MIDI or the piano roll.", rows.len(), sidebar::plural(rows.len())));
                }
                let n = self.sel.notes.len();
                let off_grid = ops::pattern(track, pid).map_or(0, |p| {
                    p.notes.iter().filter(|n| n.start.fract() != 0.0).count()
                });
                if (n > 0 || off_grid > 0)
                    && ui.add(egui::Button::new("Quantize").small()).on_hover_text("Snap note starts to the 1/16 grid — selected notes, or every note when nothing is selected").clicked()
                {
                    self.quantize_notes(track.id, pid);
                }
                if n > 0 || note_count > 0 {
                    self.note_op_buttons(ui, track.id, pid);
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    theme::hint(ui, pal, "Click or drag to paint steps · Shift+click cycles accents · right-click erases");
                });
            });
        });
    }

    fn drum_editor(&mut self, ui: &mut Ui, track: &Track, pid: u64) {
        let rows = track.drum_rows();
        self.drum_toolbar(ui, track, pid, &rows);
        let Some(pat) = ops::pattern(track, pid).cloned() else {
            return;
        };
        let area = ui.available_rect_before_wrap();
        if area.width() < 200.0 || area.height() < 80.0 {
            return;
        }
        let grid = Rect::from_min_max(
            pos2(area.left() + DRUM_LABEL_W, area.top() + RULER_H),
            area.max,
        );
        let labels = Rect::from_min_max(
            pos2(area.left(), grid.top()),
            pos2(grid.left(), grid.bottom()),
        );
        let ruler = Rect::from_min_max(
            pos2(grid.left(), area.top()),
            pos2(grid.right(), grid.top()),
        );
        let row_h = grid.height() / rows.len() as f32;
        // Zoom clamps: never smaller than "whole pattern fits", never below MIN_PPS.
        let fit_pps = grid.width() / pat.length as f32;
        let pps = self
            .editor
            .drum_pps
            .clamp(fit_pps.clamp(MIN_PPS, 64.0), 64.0);
        let content_w = pat.length as f32 * pps;
        self.editor.drum_pps = pps;
        self.editor.rects = EditorRects {
            grid,
            vel: Rect::NOTHING,
            keys: labels,
            ruler,
            kb: Rect::NOTHING,
            labels,
        };
        let g = DrumGrid {
            grid,
            labels,
            ruler,
            pps,
            scroll_x: self.editor.drum_scroll_x,
            row_h,
            len: pat.length as f64,
            rows: &rows,
        };

        if ui.rect_contains_pointer(grid) {
            let (scroll, zoom, hover) =
                ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), i.pointer.hover_pos()));
            if zoom != 1.0 && content_w > grid.width() {
                let anchor = hover.map_or(grid.width() * 0.5, |h| (h.x - grid.left()).max(0.0));
                let step = (anchor + self.editor.drum_scroll_x) / pps;
                self.editor.drum_pps = (pps * zoom).clamp(fit_pps.min(MIN_PPS), 64.0);
                let new = self.editor.drum_pps;
                self.editor.drum_scroll_x = (step * new - anchor).max(0.0);
            }
            if scroll.x != 0.0 {
                self.editor.drum_scroll_x -= scroll.x;
                ui.input_mut(|i| i.smooth_scroll_delta.x = 0.0);
            }
        }
        self.editor.drum_scroll_x = self
            .editor
            .drum_scroll_x
            .clamp(0.0, (content_w - grid.width()).max(0.0));
        let g = DrumGrid {
            scroll_x: self.editor.drum_scroll_x,
            pps: self.editor.drum_pps,
            ..g
        };

        let resp = ui.interact(grid, ui.id().with("drum_grid"), Sense::click_and_drag());
        resp.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "Drum step sequencer")
        });
        self.drum_input(ui, track, pid, &pat, &g, &resp);
        self.paint_drum_grid(ui, track, &pat, &g);
        self.paint_drum_labels(ui, track, &g);
        self.paint_drum_ruler(ui, track, &pat, &g);
    }

    fn drum_input(
        &mut self,
        ui: &mut Ui,
        track: &Track,
        pid: u64,
        pat: &Pattern,
        g: &DrumGrid<'_>,
        resp: &egui::Response,
    ) {
        let (pressed, shift, secondary_pressed, secondary_down) = ui.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.modifiers.shift,
                i.pointer.secondary_pressed(),
                i.pointer.secondary_down(),
            )
        });
        if pressed && resp.is_pointer_button_down_on() {
            self.focus = super::Focus::Editor;
            if let Some(pos) = ui.input(|i| i.pointer.press_origin()) {
                if let Some((row, step)) = g.cell(pos) {
                    let pitch = g.rows[row].0;
                    let hit = ops::drum_hit(pat, pitch, step).is_some();
                    if shift && hit {
                        self.edit("Drum accent", None, |p| {
                            if let Some(pat) = ops::pattern_mut(p, track.id, pid) {
                                ops::cycle_drum_velocity(pat, pitch, step);
                            }
                        });
                        let v = self
                            .selected_track()
                            .and_then(|t| ops::pattern(t, pid))
                            .and_then(|p| ops::drum_hit(p, pitch, step))
                            .map(|n| n.velocity);
                        self.preview(track.id, pitch, v.unwrap_or(0.8));
                        self.editor.drum_paint = None;
                    } else {
                        let on = !hit;
                        self.editor.drum_paint = Some(on);
                        let vel = self.editor.velocity;
                        self.edit(
                            if on {
                                "Paint drum step"
                            } else {
                                "Erase drum step"
                            },
                            Some("drum-paint"),
                            |p| {
                                if let Some(pat) = ops::pattern_mut(p, track.id, pid) {
                                    ops::set_drum(pat, pitch, step, on, vel);
                                }
                            },
                        );
                        if on {
                            self.preview(track.id, pitch, vel);
                        }
                    }
                }
            }
        }
        if resp.dragged_by(PointerButton::Primary) {
            if let (Some(on), Some(pos)) = (self.editor.drum_paint, resp.interact_pointer_pos()) {
                if let Some((row, step)) = g.cell(pos) {
                    let pitch = g.rows[row].0;
                    let cur = ops::drum_hit(pat, pitch, step).is_some();
                    if cur != on {
                        let vel = self.editor.velocity;
                        self.edit(
                            if on {
                                "Paint drum step"
                            } else {
                                "Erase drum step"
                            },
                            Some("drum-paint"),
                            |p| {
                                if let Some(pat) = ops::pattern_mut(p, track.id, pid) {
                                    ops::set_drum(pat, pitch, step, on, vel);
                                }
                            },
                        );
                    }
                }
            }
        }
        if secondary_pressed && resp.is_pointer_button_down_on() {
            self.editor.right_erase = true;
        }
        if !secondary_down {
            self.editor.right_erase = false;
        }
        if self.editor.right_erase {
            if let Some(pos) = resp.interact_pointer_pos() {
                if let Some((row, step)) = g.cell(pos) {
                    let pitch = g.rows[row].0;
                    if ops::drum_hit(pat, pitch, step).is_some() {
                        self.edit("Erase drum step", Some("drum-erase"), |p| {
                            if let Some(pat) = ops::pattern_mut(p, track.id, pid) {
                                ops::set_drum(pat, pitch, step, false, 0.0);
                            }
                        });
                    }
                }
            }
        }
    }

    fn paint_drum_grid(&self, ui: &Ui, track: &Track, pat: &Pattern, g: &DrumGrid<'_>) {
        let pal = self.palette();
        let p = ui.painter_at(g.grid);
        let color = theme::rgb(track.color);
        // Rows whose piece is held on the computer keyboard glow.
        let kb_pitches = drum_keyboard_pitches(track);
        let held: BTreeSet<u8> = held_keyboard_keys(ui.ctx())
            .iter()
            .filter_map(|&i| kb_pitches.get(i).copied())
            .collect();
        for row in 0..g.rows.len() {
            let y = g.row_y(row);
            let r = Rect::from_min_size(pos2(g.grid.left(), y), vec2(g.grid.width(), g.row_h));
            p.rect_filled(
                r,
                0.0,
                if row % 2 == 1 {
                    pal.canvas_alt
                } else {
                    pal.canvas
                },
            );
            if held.contains(&g.rows[row].0) {
                p.rect_filled(r, 0.0, pal.accent.gamma_multiply(0.10));
            }
        }
        let s0 = g.step(g.grid.left()).max(0.0).floor() as i64;
        let s1 = g.step(g.grid.right()).ceil() as i64;
        for s in s0.max(0)..=(s1.min(pat.length as i64 + 16)) {
            let x = g.x(s as f64);
            if s % 16 == 0 {
                p.vline(x, g.grid.y_range(), Stroke::new(1.0_f32, pal.grid_bar));
            } else if s % 4 == 0 {
                p.vline(x, g.grid.y_range(), Stroke::new(1.0_f32, pal.grid_beat));
            } else if g.pps > 18.0 {
                p.vline(x, g.grid.y_range(), Stroke::new(0.5_f32, pal.grid_faint));
            }
        }
        for s in 4..pat.length as i64 {
            if s % 8 == 4 {
                let x = g.x(s as f64);
                p.rect_filled(
                    Rect::from_min_max(
                        pos2(x, g.grid.top()),
                        pos2(
                            (x + 4.0 * g.pps)
                                .min(g.x(pat.length as f64))
                                .min(g.grid.right()),
                            g.grid.bottom(),
                        ),
                    ),
                    0.0,
                    pal.text.gamma_multiply(0.025),
                );
            }
        }
        let end_x = g.x(pat.length as f64);
        if end_x < g.grid.right() {
            p.rect_filled(
                Rect::from_min_max(pos2(end_x, g.grid.top()), g.grid.max),
                0.0,
                Color32::from_black_alpha(80),
            );
        }
        // Notes
        for n in &pat.notes {
            let Some(row) = g.rows.iter().position(|(d, _)| *d == n.pitch) else {
                continue;
            };
            let step = n.start.floor();
            let x = g.x(step);
            let y = g.row_y(row);
            if x + g.pps < g.grid.left() || x > g.grid.right() {
                continue;
            }
            let cell =
                Rect::from_min_size(pos2(x + 2.0, y + 3.0), vec2(g.pps - 4.0, g.row_h - 6.0));
            let selected = self.sel.notes.contains(&n.id);
            p.rect_filled(
                cell,
                4.0,
                theme::mix(pal.border_soft, color, 0.35 + n.velocity * 0.65),
            );
            if selected {
                p.rect_stroke(
                    cell.expand(1.0),
                    4.0,
                    Stroke::new(1.5_f32, pal.text),
                    StrokeKind::Inside,
                );
            }
            if n.velocity < 0.95 {
                let tick = Rect::from_min_max(
                    pos2(cell.left() + 3.0, cell.bottom() - 4.0),
                    pos2(
                        cell.left() + 3.0 + (cell.width() - 6.0) * n.velocity,
                        cell.bottom() - 2.0,
                    ),
                );
                p.rect_filled(tick, 1.0, Color32::from_black_alpha(110));
            }
        }
        // Playhead
        if self.transport.playing {
            if let Some(local) = self.pattern_playhead(track, pat.id) {
                let x = g.x(local);
                if x >= g.grid.left() && x <= g.grid.right() {
                    p.vline(x, g.grid.y_range(), Stroke::new(1.5_f32, pal.accent));
                }
            }
        }
        if pat.notes.is_empty() {
            p.text(
                g.grid.center(),
                Align2::CENTER_CENTER,
                "Empty beat — click steps to add hits",
                FontId::proportional(13.0),
                pal.text_faint,
            );
        }
        p.rect_stroke(
            g.grid,
            0.0,
            Stroke::new(1.0_f32, pal.border),
            StrokeKind::Inside,
        );
    }

    fn paint_drum_labels(&mut self, ui: &mut Ui, track: &Track, g: &DrumGrid<'_>) {
        let pal = self.palette();
        let p = ui.painter_at(g.labels);
        p.rect_filled(g.labels, 0.0, pal.panel);
        // Labels for pieces held on the computer keyboard glow too.
        let kb_pitches = drum_keyboard_pitches(track);
        let held: BTreeSet<u8> = held_keyboard_keys(ui.ctx())
            .iter()
            .filter_map(|&i| kb_pitches.get(i).copied())
            .collect();
        let mut preview = None;
        for (row, (pitch, name)) in g.rows.iter().enumerate() {
            let y = g.row_y(row);
            let r = Rect::from_min_max(
                pos2(g.labels.left(), y),
                pos2(g.labels.right(), y + g.row_h),
            );
            let resp = ui.interact(r, ui.id().with(("drum-label", row)), Sense::click());
            p.rect_filled(
                r,
                0.0,
                if held.contains(pitch) {
                    pal.accent.gamma_multiply(0.16)
                } else if resp.hovered() {
                    theme::mix(pal.panel, pal.control_hover, 0.5)
                } else {
                    pal.panel
                },
            );
            p.text(
                pos2(r.left() + 12.0, r.center().y),
                Align2::LEFT_CENTER,
                format!("{:02}", row + 1),
                FontId::monospace(10.0),
                pal.text_faint,
            );
            p.text(
                pos2(r.left() + 34.0, r.center().y),
                Align2::LEFT_CENTER,
                *name,
                FontId::proportional(12.0),
                if resp.hovered() {
                    pal.text
                } else {
                    pal.text_dim
                },
            );
            if resp.hovered() {
                p.text(
                    pos2(r.right() - 10.0, r.center().y),
                    Align2::RIGHT_CENTER,
                    "▶",
                    FontId::proportional(10.0),
                    pal.link,
                );
            }
            if resp.on_hover_text(format!("Preview {name}")).clicked() {
                preview = Some(*pitch);
            }
            p.hline(
                g.labels.x_range(),
                r.bottom(),
                Stroke::new(0.5_f32, pal.border_soft),
            );
        }
        p.vline(
            g.labels.right(),
            g.labels.y_range(),
            Stroke::new(1.0_f32, pal.border),
        );
        if let Some(pitch) = preview {
            self.preview(track.id, pitch, 0.9);
        }
    }

    fn paint_drum_ruler(&self, ui: &Ui, track: &Track, pat: &Pattern, g: &DrumGrid<'_>) {
        let pal = self.palette();
        let p = ui.painter_at(g.ruler);
        p.rect_filled(g.ruler, 0.0, pal.panel);
        for s in 0..pat.length {
            if s.is_multiple_of(4) {
                let x = g.x(s as f64);
                let label = if s.is_multiple_of(16) {
                    format!("{}", s / 16 + 1)
                } else {
                    format!("{}", s / 4 % 4 + 1)
                };
                p.text(
                    pos2(x + 3.0, g.ruler.center().y),
                    Align2::LEFT_CENTER,
                    label,
                    FontId::proportional(10.0),
                    pal.text_dim,
                );
            }
        }
        if self.transport.playing {
            if let Some(local) = self.pattern_playhead(track, pat.id) {
                let x = g.x(local);
                p.add(Shape::convex_polygon(
                    vec![
                        pos2(x - 5.0, g.ruler.bottom() - 8.0),
                        pos2(x + 5.0, g.ruler.bottom() - 8.0),
                        pos2(x, g.ruler.bottom() - 1.0),
                    ],
                    pal.accent,
                    Stroke::NONE,
                ));
            }
        }
    }
}

fn note_len_label(steps: f64) -> String {
    match steps {
        0.5 => "1/32".into(),
        1.0 => "1/16".into(),
        2.0 => "1/8".into(),
        4.0 => "1/4".into(),
        8.0 => "1/2".into(),
        16.0 => "1 bar".into(),
        other => format!("{other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Instrument, KeyMode, Project};

    /// `(pitch, start, duration, velocity)` fixture notes — one per beat.
    const THREE: [(u8, f64, f64, f32); 3] = [
        (60, 0.0, 2.0, 0.8),
        (64, 4.0, 2.0, 0.5),
        (67, 8.0, 2.0, 0.3),
    ];

    /// A headless app holding one `inst` track whose pattern carries
    /// `notes`; the track is selected. Returns `(app, track_id, pattern_id)`.
    fn app_with(inst: Instrument, notes: &[(u8, f64, f64, f32)]) -> (CadenceApp, u64, u64) {
        let mut project = Project::empty();
        project.tracks.clear();
        let tid = ops::add_track(&mut project, inst).unwrap();
        let pid = project.tracks[0].patterns[0].id;
        let pat = ops::pattern_mut(&mut project, tid, pid).unwrap();
        for &(pitch, start, dur, vel) in notes {
            ops::add_note(pat, pitch, start, dur, vel);
        }
        let mut app = CadenceApp::headless(project);
        app.select_track(tid);
        (app, tid, pid)
    }

    fn pattern_of(app: &CadenceApp, tid: u64, pid: u64) -> &Pattern {
        ops::pattern(ops::track(&app.project, tid).unwrap(), pid).unwrap()
    }

    fn status_text(app: &CadenceApp) -> &str {
        app.status.as_ref().map_or("", |s| s.text.as_str())
    }

    #[test]
    fn humanize_whole_pattern_reports_and_undoes() {
        let (mut app, tid, pid) = app_with(Instrument::Piano, &THREE);
        let before = pattern_of(&app, tid, pid).clone();
        app.humanize_notes(tid, pid);
        // Whole pattern is in scope when nothing is selected.
        assert_eq!(status_text(&app), "Humanized 3 notes.");
        let after = pattern_of(&app, tid, pid);
        assert_eq!(after.notes.len(), 3);
        for n in &after.notes {
            assert!((0.0..=1.0).contains(&n.velocity), "velocity {}", n.velocity);
            assert!(
                n.start >= 0.0 && n.start <= 16.0 - ops::MIN_NOTE_DURATION,
                "start {}",
                n.start
            );
        }
        // edit() records undo exactly when the jitter changed the pattern.
        assert_eq!(app.history.can_undo(), after.notes != before.notes);
        if app.history.can_undo() {
            assert_eq!(app.history.undo_label(), Some("Humanize notes"));
        }
        app.undo();
        assert_eq!(pattern_of(&app, tid, pid).notes, before.notes);
    }

    #[test]
    fn humanize_selection_only_touches_selected_notes() {
        let (mut app, tid, pid) = app_with(Instrument::Piano, &THREE);
        let target = pattern_of(&app, tid, pid).notes[1].id;
        app.sel.notes.insert(target);
        let before = pattern_of(&app, tid, pid).clone();
        app.humanize_notes(tid, pid);
        assert_eq!(status_text(&app), "Humanized 1 note.");
        let after = pattern_of(&app, tid, pid);
        assert_eq!(after.notes[0], before.notes[0], "unselected untouched");
        assert_eq!(after.notes[2], before.notes[2], "unselected untouched");
        assert_eq!(
            after.notes[1].id, before.notes[1].id,
            "jitter moves the note in place"
        );
        assert!(app.sel.notes.contains(&target), "selection survives");
    }

    #[test]
    fn scale_velocities_scales_all_notes_and_undoes() {
        let (mut app, tid, pid) = app_with(
            Instrument::Piano,
            &[(60, 0.0, 2.0, 0.8), (64, 4.0, 2.0, 0.95)],
        );
        app.scale_velocities(tid, pid, 0.9);
        let after = pattern_of(&app, tid, pid);
        assert!(
            (after.notes[0].velocity - 0.72).abs() < 1e-6,
            "{}",
            after.notes[0].velocity
        );
        assert!(
            (after.notes[1].velocity - 0.855).abs() < 1e-6,
            "{}",
            after.notes[1].velocity
        );
        assert_eq!(status_text(&app), "Velocity -10% on 2 notes.");
        assert_eq!(app.history.undo_label(), Some("Scale velocities"));
        app.undo();
        assert!((pattern_of(&app, tid, pid).notes[0].velocity - 0.8).abs() < 1e-6);

        // The +10% chip clamps at full velocity.
        app.scale_velocities(tid, pid, 1.1);
        let after = pattern_of(&app, tid, pid);
        assert!(
            (after.notes[0].velocity - 0.88).abs() < 1e-6,
            "{}",
            after.notes[0].velocity
        );
        assert_eq!(after.notes[1].velocity, 1.0, "clamped at max");
        assert_eq!(status_text(&app), "Velocity +10% on 2 notes.");
    }

    #[test]
    fn scale_velocities_uses_selection() {
        let (mut app, tid, pid) = app_with(
            Instrument::Piano,
            &[(60, 0.0, 2.0, 0.8), (64, 4.0, 2.0, 0.4)],
        );
        let target = pattern_of(&app, tid, pid).notes[0].id;
        app.sel.notes.insert(target);
        app.scale_velocities(tid, pid, 0.5);
        let after = pattern_of(&app, tid, pid);
        assert!(
            (after.notes[0].velocity - 0.4).abs() < 1e-6,
            "{}",
            after.notes[0].velocity
        );
        assert!(
            (after.notes[1].velocity - 0.4).abs() < 1e-6,
            "unselected untouched"
        );
        assert_eq!(status_text(&app), "Velocity -50% on 1 note.");
    }

    #[test]
    fn no_matching_notes_reports_and_records_nothing() {
        let (mut app, tid, pid) = app_with(Instrument::Piano, &THREE);
        app.sel.notes.insert(u64::MAX); // stale id — matches nothing
        app.humanize_notes(tid, pid);
        assert_eq!(status_text(&app), "No notes to humanize.");
        assert!(
            !app.history.can_undo(),
            "an untouching edit records no undo"
        );
        app.scale_velocities(tid, pid, 0.9);
        assert_eq!(status_text(&app), "No notes to scale.");
        assert!(!app.history.can_undo());
    }

    #[test]
    fn note_target_predicates_follow_selection_and_content() {
        // Empty project: the drum track is selected with an empty pattern —
        // the target pair resolves but nothing is worth acting on.
        let mut app = CadenceApp::headless(Project::empty());
        let drums = app.project.tracks[0].id;
        let beat = app.project.tracks[0].patterns[0].id;
        assert_eq!(app.editor_target(), Some((drums, beat)));
        assert!(!app.has_note_targets());
        // A note selection alone is a target.
        app.sel.notes.insert(12345);
        assert!(app.has_note_targets());

        // A pattern with notes is a target without a selection.
        let (mut app, tid, pid) = app_with(Instrument::Piano, &THREE);
        assert_eq!(app.editor_target(), Some((tid, pid)));
        assert!(app.has_note_targets());
        // No selected track → no target pair, no targets.
        app.sel.track = None;
        assert_eq!(app.editor_target(), None);
        assert!(!app.has_note_targets());
    }

    /// Pending-note fixture: a click-draw of `pitch` at step `start`.
    fn pending(pitch: u8, start: f64) -> PendingNote {
        PendingNote {
            pos: pos2(0.0, 0.0),
            pitch,
            start,
            draw_on_click: true,
        }
    }

    #[test]
    fn scale_snap_draw_lands_on_scale_tone() {
        // A major = pcs {9,11,1,2,4,6,8}: C4 (pc 0) is out of scale.
        let (mut app, tid, pid) = app_with(Instrument::Piano, &[]);
        app.project.key = SongKey::new(9, KeyMode::Major);
        app.editor.scale_snap = true;
        let track = ops::track(&app.project, tid).unwrap().clone();
        // The real roll re-clones the pattern every frame; mirror that here.
        let pat = |app: &CadenceApp| {
            ops::pattern(ops::track(&app.project, tid).unwrap(), pid)
                .unwrap()
                .clone()
        };
        // A click on the C4 row draws on its lower scale neighbour B3.
        app.finish_pending_note(&track, pid, &pat(&app), pending(60, 0.0));
        let drawn = pattern_of(&app, tid, pid).notes.clone();
        assert_eq!(drawn.len(), 1);
        assert_eq!(drawn[0].pitch, 59);
        // An in-scale row keeps its pitch.
        app.finish_pending_note(&track, pid, &pat(&app), pending(61, 4.0));
        assert_eq!(pattern_of(&app, tid, pid).notes[1].pitch, 61);
        // Clicking a chromatic row whose snap target already holds a note at
        // that step selects it instead of overdubbing a duplicate.
        app.finish_pending_note(&track, pid, &pat(&app), pending(60, 0.5));
        assert_eq!(pattern_of(&app, tid, pid).notes.len(), 2);
        assert!(app.sel.notes.contains(&drawn[0].id));
    }

    #[test]
    fn scale_snap_inert_without_toggle_or_key() {
        // Toggle off: the pitch passes through even with a non-default key.
        let (mut app, tid, pid) = app_with(Instrument::Piano, &[]);
        app.project.key = SongKey::new(9, KeyMode::Major);
        let track = ops::track(&app.project, tid).unwrap().clone();
        let pat = ops::pattern(&track, pid).unwrap().clone();
        app.finish_pending_note(&track, pid, &pat, pending(60, 0.0));
        assert_eq!(pattern_of(&app, tid, pid).notes[0].pitch, 60);
        // Toggle on with the C-major default — treated as "no key": inert.
        // (61 would snap to 60 in C major, so 61 surviving proves inertness.)
        app.project.key = SongKey::default();
        app.editor.scale_snap = true;
        app.finish_pending_note(&track, pid, &pat, pending(61, 4.0));
        assert_eq!(pattern_of(&app, tid, pid).notes[1].pitch, 61);
        // A drum track never snaps, even with the toggle on and a real key.
        app.project.key = SongKey::new(9, KeyMode::Major);
        let (drum_app, dtid, _) = app_with(Instrument::Drums, &[]);
        let drums = ops::track(&drum_app.project, dtid).unwrap().clone();
        assert!(drums.shows_drum_grid());
        assert_eq!(app.snap_to_scale_pitch(&drums, 60), 60);
    }

    #[test]
    fn scale_snap_drag_stair_steps_and_never_nudges_still_notes() {
        // A major = pcs {9,11,1,2,4,6,8}.
        let (mut app, tid, _) = app_with(Instrument::Piano, &[]);
        app.project.key = SongKey::new(9, KeyMode::Major);
        app.editor.scale_snap = true;
        let track = ops::track(&app.project, tid).unwrap().clone();
        // In-scale anchor A3 (57): one row up lands on chromatic A# so the
        // note stays; two rows lands on B; one down lands on G#.
        assert_eq!(app.snapped_drag_dp(&track, 57, 0), 0);
        assert_eq!(app.snapped_drag_dp(&track, 57, 1), 0);
        assert_eq!(app.snapped_drag_dp(&track, 57, 2), 2);
        assert_eq!(app.snapped_drag_dp(&track, 57, -1), -1);
        // Chromatic anchor A#3 (58): zero vertical movement never snaps it
        // off its row — a pure horizontal drag keeps its pitch.
        assert_eq!(app.snapped_drag_dp(&track, 58, 0), 0);
        assert_eq!(app.snapped_drag_dp(&track, 58, 1), 1, "up lands on B");
        assert_eq!(app.snapped_drag_dp(&track, 58, -1), -1, "down lands on A");
        // Dragged past the MIDI top, the anchor clamps to the nearest
        // reachable scale tone below (127 G -> 126 F#).
        assert_eq!(app.snapped_drag_dp(&track, 127, 4), -1);
        // Toggle off: deltas pass through untouched.
        app.editor.scale_snap = false;
        assert_eq!(app.snapped_drag_dp(&track, 57, 1), 1);
        assert_eq!(app.snapped_drag_dp(&track, 58, 1), 1);
    }

    fn stamped_pitches(app: &CadenceApp, tid: u64, pid: u64) -> Vec<u8> {
        let mut pitches: Vec<u8> = pattern_of(app, tid, pid)
            .notes
            .iter()
            .map(|n| n.pitch)
            .collect();
        pitches.sort_unstable();
        pitches
    }

    #[test]
    fn stamp_diatonic_chord_applies_inversion() {
        let (mut app, tid, pid) = app_with(Instrument::Piano, &[]);
        let ctx = Context::default();
        let _ = ctx.run(egui::RawInput::default(), |_| {});
        // Root position first: the I chord of the default C-major key.
        app.stamp_diatonic_chord(0, &ctx);
        assert_eq!(stamped_pitches(&app, tid, pid), vec![60, 64, 67]);

        // First inversion rotates the bottom tone up: E–G–C′, re-centred.
        app.undo();
        app.editor.chord_inversion = 1;
        app.stamp_diatonic_chord(0, &ctx);
        assert_eq!(
            stamped_pitches(&app, tid, pid),
            vec![52, 55, 60],
            "1st inversion of the I chord"
        );
        assert!(
            status_text(&app).contains("C/E"),
            "slash notation reports the inverted bass: {}",
            status_text(&app)
        );

        // Second inversion: G–C′–E′.
        app.undo();
        app.editor.chord_inversion = 2;
        app.stamp_diatonic_chord(0, &ctx);
        assert_eq!(stamped_pitches(&app, tid, pid), vec![55, 60, 64]);
        assert!(status_text(&app).contains("C/G"));
    }

    #[test]
    fn stamp_diatonic_chord_applies_slash_bass() {
        let (mut app, tid, pid) = app_with(Instrument::Piano, &[]);
        let ctx = Context::default();
        let _ = ctx.run(egui::RawInput::default(), |_| {});
        // The chord root doubled an octave under the voicing: no slash in
        // the symbol but one extra note below.
        app.editor.chord_bass = ops::ChordBass::Root;
        app.stamp_diatonic_chord(0, &ctx);
        assert_eq!(stamped_pitches(&app, tid, pid), vec![48, 60, 64, 67]);
        assert!(
            status_text(&app).contains("Stamped C"),
            "status: {}",
            status_text(&app)
        );

        // A scale degree bass stamps a real slash chord: C over G.
        app.undo();
        app.editor.chord_bass = ops::ChordBass::Degree(4); // dominant G in C major
        app.stamp_diatonic_chord(0, &ctx);
        assert_eq!(
            stamped_pitches(&app, tid, pid),
            vec![43, 60, 64, 67],
            "G bass under the C voicing"
        );
        assert!(
            status_text(&app).contains("C/G"),
            "status: {}",
            status_text(&app)
        );
    }

    #[test]
    fn stamp_diatonic_chord_combines_inversion_bass_and_override() {
        let (mut app, tid, pid) = app_with(Instrument::Piano, &[]);
        let ctx = Context::default();
        let _ = ctx.run(egui::RawInput::default(), |_| {});
        // Minor7 quality override + first inversion + dominant bass.
        app.editor.chord_quality = Some(ChordQuality::Minor7);
        app.editor.chord_inversion = 1;
        app.editor.chord_bass = ops::ChordBass::Degree(4); // G
        app.stamp_diatonic_chord(0, &ctx);
        // Cm7 = C Eb G Bb → 1st inv Eb G Bb C′ with a G under it; the
        // five-tone voicing re-centres an octave down on the piano's home:
        // G2 Eb3 G3 Bb3 C4.
        assert_eq!(
            stamped_pitches(&app, tid, pid),
            vec![43, 51, 55, 58, 60],
            "Cm7/Eb over G"
        );
        assert!(status_text(&app).contains("Cm7/G"));
    }
}
