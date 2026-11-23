//! Pattern editor: drum step sequencer and melodic piano roll, plus the
//! Keyboard tab. Both editors are custom-painted canvases with their own
//! scroll/zoom, note draw/drag/resize/box-select, a velocity lane and drum
//! accent painting.
//!
//! Navigation (shared with the arranger): wheel scrolls rows, Shift+wheel
//! scrolls time, Ctrl/Cmd+wheel zooms time around the pointer, Alt+wheel
//! zooms the piano-roll row height, trackpads pan both axes and a
//! middle-button drag pans freely.

use std::collections::{BTreeSet, HashMap};
use std::hash::Hash;

use egui::text::{CCursor, CCursorRange};
use egui::{
    pos2, vec2, Align, Align2, Color32, Context, CornerRadius, CursorIcon, Event, FontId, Key,
    KeyboardShortcut, Layout, Margin, Modifiers, Painter, PointerButton, Pos2, Rect, Response,
    RichText, Sense, Shape, Stroke, StrokeKind, Ui, Vec2,
};

use super::arranger::snap_floor;
use super::theme::{self, Palette};
use super::{ops, sidebar, CadenceApp, StatusKind};
use crate::model::{
    next_id, note_name, ChordQuality, Note, Pattern, Project, SongKey, Track, NOTE_NAMES,
};

const KEYS_W: f32 = 66.0;
const DRUM_LABEL_W: f32 = 140.0;
const RULER_H: f32 = 24.0;
const VEL_H: f32 = 44.0;
/// Default piano-roll row height; Alt+wheel zooms it within
/// `MIN_ROW_H..=MAX_ROW_H`.
const ROW_H: f32 = 16.0;
const MIN_ROW_H: f32 = 8.0;
const MAX_ROW_H: f32 = 32.0;
/// Row-height zoom per point of Alt+wheel (egui's own scroll-zoom speed).
const ROW_ZOOM_SPEED: f32 = 1.0 / 200.0;
/// Drum rows share the grid height but never shrink below this; taller
/// kits scroll vertically instead.
const DRUM_MIN_ROW_H: f32 = 22.0;
const RESIZE_ZONE: f32 = 7.0;
const MIN_PPS: f32 = 8.0;
const MAX_PPS: f32 = 64.0;
/// Header slot widths (the toolbar needs them before painting).
const HEADER_NAME_MAX_W: f32 = 180.0;
const HEADER_INST_MAX_W: f32 = 140.0;
const PATTERN_COMBO_W: f32 = 176.0;
const LENGTH_COMBO_W: f32 = 76.0;
const VEL_SLIDER_W: f32 = 88.0;
/// Chord strip: disclosure row height (closed, and open with the palette
/// tabs and voicing options on it) and chip sizes.
const CHORD_ROW_H: f32 = 32.0;
const CHORD_OPEN_ROW_H: f32 = theme::TOOLBAR_H;
const CHORD_KEY_MAX_W: f32 = 140.0;
const CHORD_CHIP_W: f32 = 64.0;
const CHORD_CHIP_H: f32 = 40.0;
/// Space under the chord chips.
const CHORD_CHIPS_PAD: f32 = 8.0;
/// Progression chips shrink (down to the minimum) to stay on one row.
const PROGRESSION_CHIP_W: f32 = 148.0;
const PROGRESSION_CHIP_MIN_W: f32 = 104.0;
/// The piano roll's chrome around its note grid with the chord strip
/// closed: header, chord row, roll toolbar, ruler and velocity lane. The
/// rows stack without gaps. The app shell sizes its default split from it.
pub(super) const ROLL_CHROME: f32 =
    theme::TOOLBAR_H + CHORD_ROW_H + theme::TOOLBAR_H + RULER_H + VEL_H;
/// What the open chord strip (one row of chips) adds to [`ROLL_CHROME`].
pub(super) const CHORD_STRIP_EXTRA: f32 =
    CHORD_OPEN_ROW_H - CHORD_ROW_H + CHORD_CHIP_H + CHORD_CHIPS_PAD;
/// Largest drum pad side on the Keyboard tab.
const PAD_MAX: f32 = 168.0;
/// Note-length and snap choices offered in the piano-roll toolbar.
const NOTE_LENGTHS: [(f64, &str); 5] = [
    (1.0, "1/16"),
    (2.0, "1/8"),
    (4.0, "1/4"),
    (8.0, "1/2"),
    (16.0, "1 bar"),
];
const SNAP_VALUES: [(f64, &str); 4] = [(0.5, "1/32"), (1.0, "1/16"), (2.0, "1/8"), (4.0, "1/4")];
const SC_DELETE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, Key::Delete);
const SC_OCTAVE_DOWN: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, Key::Q);
const SC_OCTAVE_UP: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, Key::W);
const FIRST_PITCH: u8 = 0;
const LAST_PITCH: u8 = 127;
const PITCHES: usize = (LAST_PITCH - FIRST_PITCH + 1) as usize;
/// Octave bounds for computer-keyboard audition: octave `o` covers MIDI
/// `(o + 1) * 12 ..= (o + 1) * 12 + 11`, so 0..=8 keeps the whole row inside
/// 0..=127 while still reaching C0 for bass.
const KB_OCTAVE_MIN: i32 = 0;
const KB_OCTAVE_MAX: i32 = 8;

/// Computer-keyboard key events in arrival order: `(key, pressed, shift)`.
type KeyEvents = Vec<(Key, bool, bool)>;

/// Jitter the Humanize button applies: ±10% velocity and ±0.25 steps (a
/// 1/64 note) of timing — enough to loosen a rigid line without scattering
/// it off the grid.
const HUMANIZE_VELOCITY: f32 = 0.10;
const HUMANIZE_TIMING: f64 = 0.25;
/// Velocity −10% / +10% buttons next to Humanize in both editor toolbars:
/// `(label, gain, tooltip)`.
const VEL_CHIPS: [(&str, f32, &str); 2] = [
    (
        "−10%",
        0.9,
        "Velocity −10% — selected notes, or every note when nothing is selected",
    ),
    (
        "+10%",
        1.1,
        "Velocity +10% — selected notes, or every note when nothing is selected",
    ),
];
const HUMANIZE_TIP: &str = "Random velocity and timing jitter — selected notes, or every note \
                            when nothing is selected. Each click is a new take; undo to revert";

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

/// Whether the playing transport (pattern-relative step, `None` when
/// stopped or outside this pattern) is inside `n` — piano-roll notes light
/// up like beat-grid hits while they sound.
pub(super) fn note_is_live(n: &Note, playing_at: Option<f64>) -> bool {
    playing_at.is_some_and(|t| t >= n.start && t < n.start + n.duration)
}

/// Length of a note drawn by dragging from `start` to the pointer at
/// `pointer_step`: whole snap cells reaching past the pointer, at least one.
fn draw_duration(start: f64, pointer_step: f64, snap: f64) -> f64 {
    let cell = snap.max(ops::MIN_NOTE_DURATION);
    let dur = ((pointer_step - start) / cell).ceil() * cell;
    if dur.is_finite() {
        dur.max(cell)
    } else {
        cell
    }
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

    fn tip(self) -> &'static str {
        match self {
            ChordStripTab::Diatonic => "The key's seven diatonic chords, I through vii°",
            ChordStripTab::Borrowed => {
                "Colour chords from the parallel key plus secondary dominants"
            }
            ChordStripTab::Progressions => "One-click genre/mood loops stamped one chord per bar",
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

/// What the pattern header asked for this frame (applied after painting).
#[derive(Clone, Copy, Debug, PartialEq)]
enum HeaderAction {
    Select(u64),
    Length(u32),
    NewPattern,
    Duplicate,
    StartRename,
    CommitRename,
    CancelRename,
    ClearNotes,
    DeletePattern,
    AddToArrangement,
}

/// Inline pattern rename in progress: the header shows a text field in
/// place of the pattern selector. Enter (or clicking away) commits, Esc
/// cancels.
#[derive(Clone, Debug)]
struct PatternRename {
    pid: u64,
    text: String,
    /// Focus and select the text on the next paint.
    focus: bool,
}

/// What an editor toolbar asked for this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
enum ToolAction {
    Quantize,
    Humanize,
    ScaleVelocity(f32),
    DeleteNotes,
    Fill,
    ScaleSnap,
    ScaleHighlight,
}

/// Toolbar layouts of the last frame, for the headless layout tests:
/// `(row, placed item rects, items moved into the overflow menu)`.
#[cfg(test)]
#[derive(Debug)]
struct EditorLayout {
    header: (Rect, Vec<Rect>, usize),
    tools: (Rect, Vec<Rect>, usize),
    /// Palette tabs and voicing options on the open chord row.
    chords: (Rect, Vec<Rect>, usize),
    chord_row: Rect,
    /// The part of the chord row that toggles the strip.
    chord_toggle: Rect,
}

#[cfg(test)]
impl Default for EditorLayout {
    fn default() -> Self {
        Self {
            header: (Rect::NOTHING, Vec::new(), 0),
            tools: (Rect::NOTHING, Vec::new(), 0),
            chords: (Rect::NOTHING, Vec::new(), 0),
            chord_row: Rect::NOTHING,
            chord_toggle: Rect::NOTHING,
        }
    }
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
    /// Piano-roll row height (Alt+wheel zoom).
    roll_row_h: f32,
    roll_scroll_x: f32,
    roll_scroll_y: f32,
    /// The roll homed before its first paint (guessed height): re-home once
    /// the grid is laid out.
    roll_home_pending: bool,
    pub drum_pps: f32,
    drum_scroll_x: f32,
    drum_scroll_y: f32,
    rename: Option<PatternRename>,
    #[cfg(test)]
    layout: EditorLayout,
    last_track: Option<u64>,
    drag: Option<NoteDrag>,
    /// Note being drawn by dragging from empty grid: `(id, start step)`.
    draw: Option<(u64, f64)>,
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
    /// Pitch each held computer key started, so its release ends that note
    /// even after the octave or Shift changed in between.
    kb_held: HashMap<Key, u8>,
    /// Undo group of the running recording take: every note and clip
    /// extension from arming (or play) to stop/disarm is one undo step.
    pub record_take: Option<String>,
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
            roll_row_h: ROW_H,
            roll_scroll_x: 0.0,
            roll_scroll_y: 0.0,
            roll_home_pending: false,
            drum_pps: 30.0,
            drum_scroll_x: 0.0,
            drum_scroll_y: 0.0,
            rename: None,
            #[cfg(test)]
            layout: EditorLayout::default(),
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
            kb_held: HashMap::new(),
            record_take: None,
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
        self.kb_held.clear();
        self.kb_record_pitch = None;
        self.kb_was_down = false;
        self.rename = None;
        self.roll_scroll_x = 0.0;
        self.drum_scroll_x = 0.0;
        self.drum_scroll_y = 0.0;
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
    /// Pitch-row height (Alt+wheel zoom).
    row_h: f32,
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
        self.grid.top() + (LAST_PITCH - pitch) as f32 * self.row_h - self.scroll_y
    }
    fn pitch_at(&self, y: f32) -> Option<u8> {
        let row = ((y - self.grid.top() + self.scroll_y) / self.row_h).floor() as i64;
        let pitch = LAST_PITCH as i64 - row;
        (FIRST_PITCH as i64..=LAST_PITCH as i64)
            .contains(&pitch)
            .then_some(pitch as u8)
    }
    /// Half-open range of pitch rows (0 = top pitch) intersecting the grid.
    fn visible_rows(&self) -> (usize, usize) {
        let first = (self.scroll_y / self.row_h).floor().max(0.0) as usize;
        let last = ((self.scroll_y + self.grid.height()) / self.row_h)
            .ceil()
            .clamp(0.0, PITCHES as f32) as usize;
        (first, last.max(first))
    }
    fn note_rect(&self, n: &Note) -> Rect {
        let x = self.x(n.start);
        let y = self.y(n.pitch);
        Rect::from_min_size(
            pos2(x + 1.0, y + 1.0),
            vec2(
                (n.duration as f32 * self.pps - 1.5).max(3.0),
                (self.row_h - 2.0).max(2.0),
            ),
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
    /// Vertical scroll when the kit has more rows than fit.
    scroll_y: f32,
    row_h: f32,
    len: f64,
    /// Piece pitch per row, top to bottom — the kit's pieces for a kit
    /// preset, else the built-in `DRUMS` table. Never empty
    /// ([`Track::drum_rows`] guarantees at least one row).
    rows: &'a [u8],
}

impl DrumGrid<'_> {
    fn x(&self, step: f64) -> f32 {
        self.grid.left() + step as f32 * self.pps - self.scroll_x
    }
    fn step(&self, x: f32) -> f64 {
        ((x - self.grid.left() + self.scroll_x) / self.pps) as f64
    }
    fn row_y(&self, row: usize) -> f32 {
        self.grid.top() + row as f32 * self.row_h - self.scroll_y
    }
    /// Half-open range of rows intersecting the grid.
    fn visible_rows(&self) -> (usize, usize) {
        let first = (self.scroll_y / self.row_h).floor().max(0.0) as usize;
        let last = (((self.scroll_y + self.grid.height()) / self.row_h)
            .ceil()
            .max(0.0) as usize)
            .min(self.rows.len());
        (first.min(last), last)
    }
    fn cell(&self, pos: Pos2) -> Option<(usize, u32)> {
        if !self.grid.contains(pos) {
            return None;
        }
        let row = ((pos.y - self.grid.top() + self.scroll_y) / self.row_h).floor() as usize;
        let step = self.step(pos.x).floor() as i64;
        (0..self.rows.len())
            .contains(&row)
            .then_some(row)
            .filter(|_| step >= 0 && (step as f64) < self.len)
            .map(|r| (r, step as u32))
    }
}

/// Drum row height: the rows share the grid, but never below
/// [`DRUM_MIN_ROW_H`] (taller kits scroll).
fn drum_row_height(grid_h: f32, rows: usize) -> f32 {
    (grid_h / rows.max(1) as f32).max(DRUM_MIN_ROW_H)
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
        let rows: Vec<u8> = track.drum_rows().iter().map(|(p, _)| *p).collect();
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
        let g = drum_geometry(r, &self.editor, &rows);
        let x = g.x(step as f64) + g.pps * 0.5;
        let y = g.row_y(row) + g.row_h * 0.5;
        let p = pos2(x, y);
        g.grid.contains(p).then_some(p)
    }

    #[cfg(test)]
    pub(crate) fn roll_cell_center(&self, pitch: u8, step: f64) -> Option<Pos2> {
        let r = &self.editor.rects;
        if r.grid == Rect::NOTHING {
            return None;
        }
        let roll = self.roll_for_test();
        let p = pos2(
            roll.x(step) + roll.pps * 0.25,
            roll.y(pitch) + roll.row_h * 0.5,
        );
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
            row_h: self.editor.roll_row_h,
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

    /// The pattern name is being edited inline (Esc belongs to that field).
    pub(super) fn pattern_rename_active(&self) -> bool {
        self.editor.rename.is_some()
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
        if !r.grid.contains(pos) {
            return None;
        }
        if track.shows_drum_grid() {
            let step = (pos.x - r.grid.left() + self.editor.drum_scroll_x) / self.editor.drum_pps;
            Some((snap_floor(f64::from(step).max(0.0), snap), None))
        } else {
            let roll = Roll {
                grid: r.grid,
                keys: r.keys,
                ruler: r.ruler,
                vel: r.vel,
                pps: self.editor.roll_pps,
                row_h: self.editor.roll_row_h,
                scroll_x: self.editor.roll_scroll_x,
                scroll_y: self.editor.roll_scroll_y,
            };
            Some((
                snap_floor(roll.step(pos.x).max(0.0), snap),
                roll.pitch_at(pos.y),
            ))
        }
    }
}

#[cfg(test)]
fn drum_geometry<'a>(rects: &EditorRects, ed: &EditorView, rows: &'a [u8]) -> DrumGrid<'a> {
    DrumGrid {
        grid: rects.grid,
        labels: rects.labels,
        ruler: rects.ruler,
        pps: ed.drum_pps,
        scroll_x: ed.drum_scroll_x,
        scroll_y: ed.drum_scroll_y,
        row_h: drum_row_height(rects.grid.height(), rows.len()),
        len: 1.0,
        rows,
    }
}

impl CadenceApp {
    /// The track `tid` in the song, borrowed (no per-frame clone).
    fn track_ref(&self, tid: u64) -> Option<&Track> {
        ops::track(&self.project, tid)
    }

    /// Pattern `pid` of track `tid`, borrowed.
    fn pat_ref(&self, tid: u64, pid: u64) -> Option<&Pattern> {
        self.track_ref(tid).and_then(|t| ops::pattern(t, pid))
    }

    pub(super) fn editor_ui(&mut self, ui: &mut Ui) {
        let pal = self.palette();
        let Some(track) = self.selected_track() else {
            centered_empty_state(
                ui,
                pal,
                theme::Icon::Note,
                "No track selected",
                "Pick a track in the arrangement, or add an instrument from the library.",
                None,
            );
            return;
        };
        // Audio lanes never schedule patterns, so a piano roll here would
        // invite edits that can never sound.
        if track.audio {
            centered_empty_state(
                ui,
                pal,
                theme::Icon::Note,
                "Audio track",
                "Audio lanes hold imported WAV clips, not notes — right-click a lane in the arrangement to import a file.",
                None,
            );
            return;
        }
        let (tid, drum, pid) = (
            track.id,
            track.shows_drum_grid(),
            self.current_pattern_id(track),
        );
        let Some(pid) = pid else {
            if centered_empty_state(
                ui,
                pal,
                theme::Icon::Note,
                "This track has no patterns",
                "Patterns hold the notes its clips play.",
                Some("Create a pattern"),
            ) {
                let mut new = None;
                self.edit("New pattern", None, |p| {
                    if let Some(t) = ops::track_mut(p, tid) {
                        new = ops::new_pattern(t, 16);
                    }
                });
                if let Some(new) = new {
                    self.select_pattern(tid, new);
                }
            }
            return;
        };
        if self.editor.last_track != Some(tid) {
            self.editor.last_track = Some(tid);
            self.editor.rename = None;
            if let Some(track) = self.track_ref(tid) {
                self.editor.kb_octave = kb_home_octave(track);
            }
            self.editor_roll_home(tid, pid);
        }
        // The editor's rows (header, chord row, toolbar, canvas) are full-
        // height toolbars with their own padding: stack them without gaps.
        ui.spacing_mut().item_spacing.y = 0.0;
        self.editor_header(ui, tid, pid);
        if drum {
            self.drum_editor(ui, tid, pid);
        } else {
            self.chord_strip(ui, tid);
            self.piano_roll(ui, tid, pid);
        }
    }

    /// Scrolls the piano roll so existing notes (or the instrument's home octave) are visible.
    fn editor_roll_home(&mut self, tid: u64, pid: u64) {
        let Some(track) = self.track_ref(tid) else {
            return;
        };
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
        let row_h = self.editor.roll_row_h;
        let row = (LAST_PITCH as u32 - pitch_center) as f32 + 0.5;
        // Before the first paint the grid rect is still empty; use a typical
        // editor height so the first frame already shows the right octave.
        let grid_h = self.editor.rects.grid.height();
        let h = if grid_h > 40.0 { grid_h } else { 280.0 };
        let content = PITCHES as f32 * row_h;
        self.editor.roll_scroll_y = (row * row_h - h * 0.5).clamp(0.0, (content - h).max(0.0));
        self.editor.roll_scroll_x = 0.0;
        self.editor.roll_home_pending = grid_h <= 40.0;
    }

    // ----- header ----------------------------------------------------------

    /// The pattern header: one overflow-safe toolbar row — track identity,
    /// the pattern selector (double-click it to rename inline), length,
    /// new/duplicate, a ⋯ menu (rename, clear notes, delete pattern) and
    /// "Add to arrangement" on the right. The row only reads the song; what
    /// the user asked for is applied once the row is painted.
    fn editor_header(&mut self, ui: &mut Ui, tid: u64, pid: u64) {
        let pal = self.palette();
        if self.editor.rename.as_ref().is_some_and(|r| r.pid != pid) {
            self.editor.rename = None;
        }
        let mut action = None;
        {
            let Some(track) = ops::track(&self.project, tid) else {
                return;
            };
            let Some(pat) = ops::pattern(track, pid) else {
                return;
            };
            let color = theme::rgb(track.color);
            let name_w = text_width(ui, &track.name, theme::strong_font(ui, theme::TEXT_TITLE))
                .clamp(24.0, HEADER_NAME_MAX_W);
            let inst = ops::instrument_label(track);
            let inst_w = text_width(ui, &inst, theme::body_font(12.0)).min(HEADER_INST_MAX_W);
            let many = track.patterns.len() > 1;
            let has_notes = !pat.notes.is_empty();
            let add_tip =
                "Add this pattern to the arrangement at the playhead (snapped to the bar)";
            let rename = &mut self.editor.rename;
            let out = toolbar_row(ui, pal, ("editor-header", tid), |tb| {
                let in_menu = tb.in_menu();
                tb.item(10.0 + theme::GAP_S + name_w, |ui| {
                    ui.spacing_mut().item_spacing.x = theme::GAP_S;
                    theme::color_dot(ui, color, 10.0);
                    theme::truncated_label(
                        ui,
                        theme::strong_text(&track.name)
                            .size(theme::TEXT_TITLE)
                            .color(pal.text),
                        name_w,
                    );
                });
                if inst_w > 0.0 {
                    tb.item(inst_w, |ui| {
                        theme::truncated_label(
                            ui,
                            RichText::new(inst.as_ref()).size(12.0).color(pal.text_dim),
                            inst_w,
                        );
                    });
                }
                tb.separator();
                tb.item(PATTERN_COMBO_W, |ui| {
                    match rename.as_mut().filter(|r| r.pid == pid && !in_menu) {
                        Some(r) => {
                            if let Some(a) = rename_field(ui, r, PATTERN_COMBO_W) {
                                action = Some(a);
                            }
                        }
                        None => {
                            let combo = theme::combo(
                                ("pattern", tid),
                                pattern_label(pat),
                                PATTERN_COMBO_W - 2.0,
                            )
                            .show_ui(ui, |ui| {
                                for p in &track.patterns {
                                    if ui.selectable_label(p.id == pid, pattern_label(p)).clicked()
                                    {
                                        action = Some(HeaderAction::Select(p.id));
                                    }
                                }
                            });
                            let resp = combo
                                .response
                                .on_hover_text("Pattern being edited — double-click to rename");
                            // The combo toggles its list on each click, so
                            // read the double click from the pointer.
                            let double = ui.input(|i| {
                                i.pointer.button_double_clicked(PointerButton::Primary)
                                    && i.pointer
                                        .interact_pos()
                                        .is_some_and(|p| resp.rect.contains(p))
                            });
                            if double {
                                action = Some(HeaderAction::StartRename);
                            }
                        }
                    }
                });
                tb.labeled("Length", LENGTH_COMBO_W, |ui| {
                    let plen = pat.length;
                    theme::combo(
                        ("patlen", tid),
                        ops::length_label(plen),
                        LENGTH_COMBO_W - 2.0,
                    )
                    .show_ui(ui, |ui| {
                        let mut lens = ops::PATTERN_LENGTHS.to_vec();
                        if !lens.contains(&plen) {
                            lens.push(plen);
                            lens.sort_unstable();
                        }
                        for l in lens {
                            if ui
                                .selectable_label(l == plen, ops::length_label(l))
                                .clicked()
                                && l != plen
                            {
                                action = Some(HeaderAction::Length(l));
                            }
                        }
                    })
                    .response
                    .on_hover_text("Pattern length");
                });
                tb.separator();
                if tb
                    .icon_button(theme::Icon::Plus, "New pattern", "New empty pattern")
                    .clicked()
                {
                    action = Some(HeaderAction::NewPattern);
                }
                if tb
                    .icon_button(
                        theme::Icon::Duplicate,
                        "Duplicate pattern",
                        "Duplicate this pattern",
                    )
                    .clicked()
                {
                    action = Some(HeaderAction::Duplicate);
                }
                tb.item(theme::CONTROL_M, |ui| {
                    let resp = theme::icon_button(
                        ui,
                        pal,
                        theme::Icon::Menu,
                        theme::CONTROL_M,
                        false,
                        pal.accent,
                        "More pattern actions",
                    );
                    egui::Popup::menu(&resp).show(|ui| {
                        ui.set_min_width(190.0);
                        if ui
                            .button("Rename pattern")
                            .on_hover_text("Or double-click the pattern name")
                            .clicked()
                        {
                            action = Some(HeaderAction::StartRename);
                        }
                        if ui
                            .add_enabled(has_notes, egui::Button::new("Clear notes"))
                            .on_hover_text("Remove every note from this pattern")
                            .on_disabled_hover_text("This pattern has no notes")
                            .clicked()
                        {
                            action = Some(HeaderAction::ClearNotes);
                        }
                        ui.separator();
                        let ink = if many { pal.danger } else { pal.text_disabled };
                        if ui
                            .add_enabled(
                                many,
                                egui::Button::new(RichText::new("Delete pattern").color(ink)),
                            )
                            .on_hover_text("Delete this pattern and its clips in the arrangement")
                            .on_disabled_hover_text("A track keeps at least one pattern")
                            .clicked()
                        {
                            action = Some(HeaderAction::DeletePattern);
                        }
                    });
                });
                tb.spacer();
                if tb
                    .button(Some(theme::Icon::Export), "Add to arrangement", add_tip)
                    .clicked()
                {
                    action = Some(HeaderAction::AddToArrangement);
                }
            });
            #[cfg(test)]
            {
                self.editor.layout.header = (out.rect, out.placed.clone(), out.overflowed);
            }
            #[cfg(not(test))]
            let _ = out;
        }
        if let Some(action) = action {
            self.apply_header_action(tid, pid, action);
        }
    }

    fn apply_header_action(&mut self, tid: u64, pid: u64, action: HeaderAction) {
        let full = || {
            format!(
                "Tracks hold up to {} patterns.",
                ops::MAX_PATTERNS_PER_TRACK
            )
        };
        match action {
            HeaderAction::Select(p) => self.select_pattern(tid, p),
            HeaderAction::Length(l) => {
                self.edit("Pattern length", None, |p| {
                    ops::set_track_pattern_length(p, tid, pid, l);
                });
                self.set_status(
                    StatusKind::Info,
                    format!("Pattern length set to {}.", ops::length_label(l)),
                );
            }
            HeaderAction::NewPattern => {
                let len = self.pat_ref(tid, pid).map_or(16, |p| p.length);
                let mut new = None;
                self.edit("New pattern", None, |p| {
                    if let Some(t) = ops::track_mut(p, tid) {
                        new = ops::new_pattern(t, len);
                    }
                });
                match new {
                    Some(n) => self.select_pattern(tid, n),
                    None => self.set_status(StatusKind::Warning, full()),
                }
            }
            HeaderAction::Duplicate => {
                let mut new = None;
                self.edit("Duplicate pattern", None, |p| {
                    if let Some(t) = ops::track_mut(p, tid) {
                        new = ops::duplicate_pattern_in(t, pid);
                    }
                });
                match new {
                    Some(n) => self.select_pattern(tid, n),
                    None => self.set_status(StatusKind::Warning, full()),
                }
            }
            HeaderAction::StartRename => {
                let text = self.pat_ref(tid, pid).map(|p| p.name.clone());
                self.editor.rename = text.map(|text| PatternRename {
                    pid,
                    text,
                    focus: true,
                });
            }
            HeaderAction::CommitRename => {
                let Some(r) = self.editor.rename.take() else {
                    return;
                };
                let name = r.text.trim().to_string();
                if !name.is_empty() {
                    self.edit("Rename pattern", None, |p| {
                        if let Some(pat) = ops::pattern_mut(p, tid, r.pid) {
                            pat.name = name;
                        }
                    });
                }
            }
            HeaderAction::CancelRename => self.editor.rename = None,
            HeaderAction::ClearNotes => {
                let n = self.pat_ref(tid, pid).map_or(0, |p| p.notes.len());
                if n > 0 {
                    self.edit("Clear pattern", None, |p| {
                        if let Some(pat) = ops::pattern_mut(p, tid, pid) {
                            pat.notes.clear();
                        }
                    });
                    self.sel.notes.clear();
                    self.set_status(
                        StatusKind::Info,
                        format!("Cleared {n} note{}.", sidebar::plural(n)),
                    );
                }
            }
            HeaderAction::DeletePattern => {
                if self.track_ref(tid).is_some_and(|t| t.patterns.len() > 1) {
                    self.edit("Delete pattern", None, |p| {
                        if let Some(t) = ops::track_mut(p, tid) {
                            ops::delete_pattern(t, pid);
                        }
                    });
                    self.fix_selection();
                }
            }
            HeaderAction::AddToArrangement => {
                let start = snap_floor(self.transport.position, 16.0);
                self.place_clip(tid, Some(pid), start);
            }
        }
    }

    // ----- piano roll ------------------------------------------------------

    fn piano_roll(&mut self, ui: &mut Ui, tid: u64, pid: u64) {
        let pal = self.palette();
        self.roll_toolbar(ui, tid, pid);
        let Some(len) = self.pat_ref(tid, pid).map(|p| p.length) else {
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
        if std::mem::take(&mut self.editor.roll_home_pending) {
            // The first frame homed against a guessed height; re-centre now
            // that the real grid is known.
            self.editor_roll_home(tid, pid);
        }
        self.editor.grid_id = ui.id().with("piano_grid");

        let grid_resp = ui.interact(grid, self.editor.grid_id, Sense::click_and_drag());
        grid_resp
            .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "Piano roll"));
        self.roll_navigation(ui, area, grid, &grid_resp);
        let content_w = len as f32 * self.editor.roll_pps;
        let content_h = PITCHES as f32 * self.editor.roll_row_h;
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
            row_h: self.editor.roll_row_h,
            scroll_x: self.editor.roll_scroll_x,
            scroll_y: self.editor.roll_scroll_y,
        };

        self.roll_input(ui, tid, pid, &roll, &grid_resp);
        self.paint_roll(ui, tid, pid, &roll, grid_resp.hover_pos());
        self.paint_keys(ui, tid, &roll);
        self.paint_roll_ruler(ui, tid, pid, &roll);
        self.paint_vel_lane(ui, tid, pid, &roll, vel_labels);
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
        // Vertical scrollbar on the grid's right edge, like the drum grid and
        // the arranger (it used to sit on top of the piano keys).
        let vbar = Rect::from_min_max(
            pos2(grid.right() - 8.0, grid.top()),
            pos2(grid.right(), grid.bottom()),
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

    /// Piano-roll navigation — the scheme every timeline in Cadence shares:
    /// wheel scrolls pitch rows, Shift+wheel scrolls time, Ctrl/Cmd+wheel (or
    /// a pinch) zooms time around the pointer, Alt+wheel zooms the row
    /// height around the pointer, two-axis trackpad scrolling pans both
    /// axes and a middle-button drag pans freely.
    fn roll_navigation(&mut self, ui: &Ui, area: Rect, grid: Rect, resp: &egui::Response) {
        let ed = &mut self.editor;
        if ui.rect_contains_pointer(area) {
            let (scroll, zoom, alt, hover) = ui.input(|i| {
                (
                    i.smooth_scroll_delta,
                    i.zoom_delta(),
                    i.modifiers.alt,
                    i.pointer.hover_pos(),
                )
            });
            if zoom != 1.0 {
                let anchor = hover.map_or(grid.width() * 0.5, |h| {
                    (h.x - grid.left()).clamp(0.0, grid.width())
                });
                let step = (anchor + ed.roll_scroll_x) / ed.roll_pps;
                ed.roll_pps = (ed.roll_pps * zoom).clamp(MIN_PPS, MAX_PPS);
                ed.roll_scroll_x = (step * ed.roll_pps - anchor).max(0.0);
            }
            if scroll != Vec2::ZERO {
                if alt {
                    let anchor = hover.map_or(grid.height() * 0.5, |h| {
                        (h.y - grid.top()).clamp(0.0, grid.height())
                    });
                    let row = (anchor + ed.roll_scroll_y) / ed.roll_row_h;
                    let factor = ((scroll.x + scroll.y) * ROW_ZOOM_SPEED).exp();
                    ed.roll_row_h = (ed.roll_row_h * factor).clamp(MIN_ROW_H, MAX_ROW_H);
                    ed.roll_scroll_y = (row * ed.roll_row_h - anchor).max(0.0);
                } else {
                    ed.roll_scroll_x -= scroll.x;
                    ed.roll_scroll_y -= scroll.y;
                }
                ui.input_mut(|i| i.smooth_scroll_delta = Vec2::ZERO);
            }
        }
        if resp.dragged_by(PointerButton::Middle) {
            let d = resp.drag_delta();
            ed.roll_scroll_x -= d.x;
            ed.roll_scroll_y -= d.y;
            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        }
    }

    /// The piano-roll toolbar: Draw (note length, snap, velocity), Scale
    /// (snap to scale, highlight), Edit (quantize, humanize, velocity
    /// −10%/+10%) and, on the right, the note selection.
    fn roll_toolbar(&mut self, ui: &mut Ui, tid: u64, pid: u64) {
        let pal = self.palette();
        let (note_count, off_grid) = self.pat_ref(tid, pid).map_or((0, 0), |p| {
            (
                p.notes.len(),
                p.notes.iter().filter(|n| n.start.fract() != 0.0).count(),
            )
        });
        let selected = self.sel.notes.len();
        let targets = selected > 0 || note_count > 0;
        let delete_tip = theme::shortcut_tip(ui.ctx(), "Delete the selected notes", &SC_DELETE);
        let quantize_tip = if off_grid > 0 {
            format!(
                "Snap note starts to the Snap grid — selected notes, or every note when nothing \
                 is selected. {off_grid} note{} sit between grid steps.",
                sidebar::plural(off_grid)
            )
        } else {
            "Snap note starts to the Snap grid — selected notes, or every note when nothing is \
             selected"
                .to_string()
        };
        let selection = format!("{selected} selected");
        let mut act = None;
        let ed = &mut self.editor;
        let out = toolbar_row(ui, pal, ("roll-tools", tid), |tb| {
            tb.labeled("Note", 64.0, |ui| {
                theme::combo(("notelen", tid), note_len_label(ed.note_len), 62.0)
                    .show_ui(ui, |ui| {
                        for (v, l) in NOTE_LENGTHS {
                            ui.selectable_value(&mut ed.note_len, v, l);
                        }
                    })
                    .response
                    .on_hover_text("Length of newly drawn notes");
            });
            tb.labeled("Snap", 64.0, |ui| {
                theme::combo(("editsnap", tid), note_len_label(ed.snap), 62.0)
                    .show_ui(ui, |ui| {
                        for (v, l) in SNAP_VALUES {
                            ui.selectable_value(&mut ed.snap, v, l);
                        }
                    })
                    .response
                    .on_hover_text("Grid for drawing, moving and resizing notes");
            });
            velocity_item(tb, pal, &mut ed.velocity);
            tb.separator();
            if tb
                .toggle(
                    Some(theme::Icon::Snap),
                    "Scale snap",
                    ed.scale_snap,
                    "Snap to scale: drawn notes and vertical drags land on the song key's scale tones",
                )
                .clicked()
            {
                act = Some(ToolAction::ScaleSnap);
            }
            if tb
                .toggle(
                    Some(theme::Icon::Piano),
                    "Scale",
                    ed.scale_highlight,
                    "Highlight the rows of the song key's scale",
                )
                .clicked()
            {
                act = Some(ToolAction::ScaleHighlight);
            }
            tb.separator();
            if let Some(a) = note_edit_items(tb, targets, &quantize_tip) {
                act = Some(a);
            }
            tb.spacer();
            if selected > 0 {
                tb.label(&selection);
                if tb
                    .icon_button(theme::Icon::Trash, "Delete notes", &delete_tip)
                    .clicked()
                {
                    act = Some(ToolAction::DeleteNotes);
                }
            }
        });
        #[cfg(test)]
        {
            self.editor.layout.tools = (out.rect, out.placed.clone(), out.overflowed);
        }
        #[cfg(not(test))]
        let _ = out;
        match act {
            Some(ToolAction::ScaleSnap) => {
                self.editor.scale_snap = !self.editor.scale_snap;
                if self.editor.scale_snap && self.project.key == SongKey::default() {
                    self.set_status(
                        StatusKind::Info,
                        "Scale snap needs a song key other than C major — set one in the arpeggiator dialog.",
                    );
                }
            }
            Some(ToolAction::ScaleHighlight) => self.toggle_scale_highlight(),
            Some(a) => self.apply_note_tool(tid, pid, a),
            None => {}
        }
    }

    /// Edit-group actions shared by both editor toolbars.
    fn apply_note_tool(&mut self, tid: u64, pid: u64, action: ToolAction) {
        match action {
            ToolAction::Quantize => self.quantize_notes(tid, pid),
            ToolAction::Humanize => self.humanize_notes(tid, pid),
            ToolAction::ScaleVelocity(gain) => self.scale_velocities(tid, pid, gain),
            ToolAction::DeleteNotes => self.delete_selection(),
            ToolAction::Fill => {
                let pitch = self
                    .track_ref(tid)
                    .and_then(|t| t.drum_rows().first().map(|(p, _)| *p))
                    .unwrap_or(36);
                let vel = self.editor.velocity;
                self.edit("Fill pattern", None, |p| {
                    if let Some(pat) = ops::pattern_mut(p, tid, pid) {
                        for s in (0..pat.length).step_by(4) {
                            ops::set_drum(pat, pitch, s, true, vel);
                        }
                    }
                });
            }
            ToolAction::ScaleSnap | ToolAction::ScaleHighlight => {}
        }
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
    /// shows the melodic roll rather than the drum grid (`drum`). `None`
    /// leaves pitches exactly where the pointer puts them.
    fn scale_snap_key(&self, drum: bool) -> Option<SongKey> {
        (self.editor.scale_snap && !drum && self.project.key != SongKey::default())
            .then_some(self.project.key)
    }

    /// `pitch` moved onto the nearest scale tone when snap-to-scale applies
    /// ([`Self::scale_snap_key`]), unchanged otherwise.
    fn snap_to_scale_pitch(&self, drum: bool, pitch: u8) -> u8 {
        match self.scale_snap_key(drum) {
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
    fn snapped_drag_dp(&self, drum: bool, anchor_pitch: u8, raw_dp: i32) -> i32 {
        match self.scale_snap_key(drum) {
            Some(key) if raw_dp != 0 => {
                let target =
                    (i32::from(anchor_pitch) + raw_dp).clamp(0, i32::from(ops::MAX_PITCH)) as u8;
                i32::from(ops::snap_pitch_to_scale(key, target)) - i32::from(anchor_pitch)
            }
            _ => raw_dp,
        }
    }

    fn finish_pending_note(&mut self, tid: u64, pid: u64, pat: &Pattern, pending: PendingNote) {
        if pending.draw_on_click {
            let len = self.editor.note_len;
            self.place_pending_note(tid, pid, pat, pending, len);
        }
    }

    /// Draws the pending note with `len` steps and selects it — or, when a
    /// note already covers that cell, selects that one instead. Returns the
    /// id of a newly drawn note.
    fn place_pending_note(
        &mut self,
        tid: u64,
        pid: u64,
        pat: &Pattern,
        pending: PendingNote,
        len: f64,
    ) -> Option<u64> {
        // Snap-to-scale resolves against the pitch the note will land on, so
        // clicking a chromatic row under an existing scale-tone note selects
        // it instead of overdubbing a duplicate.
        let pitch = self.snap_to_scale_pitch(false, pending.pitch);
        let existing = pat
            .notes
            .iter()
            .find(|n| n.pitch == pitch && pending.start >= n.start && pending.start < n.end());
        match existing {
            Some(n) => {
                self.sel.notes.clear();
                self.sel.notes.insert(n.id);
                None
            }
            None => {
                let vel = self.editor.velocity;
                let mut new = None;
                self.edit("Draw note", Some("draw-note"), |p| {
                    if let Some(pat) = ops::pattern_mut(p, tid, pid) {
                        new = ops::add_note(pat, pitch, pending.start, len, vel);
                    }
                });
                if let Some(new) = new {
                    self.sel.notes.clear();
                    self.sel.notes.insert(new);
                    self.preview(tid, pitch, vel);
                }
                new
            }
        }
    }

    /// Mouse editing on the piano roll. Hover-only frames hit-test against
    /// the borrowed pattern; only frames with a live gesture take a snapshot
    /// of it (edits change the song mid-gesture).
    fn roll_input(&mut self, ui: &mut Ui, tid: u64, pid: u64, roll: &Roll, resp: &egui::Response) {
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
        let vel_resp = ui.interact(roll.vel, ui.id().with("vel_lane"), Sense::click_and_drag());
        vel_resp.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "Velocity lane")
        });
        // Right button erases notes under the cursor as it moves.
        if secondary_pressed && resp.is_pointer_button_down_on() {
            self.editor.right_erase = true;
        }
        if !secondary_down {
            self.editor.right_erase = false;
        }
        let hover = resp.interact_pointer_pos().or_else(|| resp.hover_pos());
        if let Some(h) = hover.filter(|_| resp.hovered() && self.editor.drag.is_none()) {
            match self
                .pat_ref(tid, pid)
                .and_then(|pat| roll_note_at(pat, roll, h))
            {
                Some(hit) if hit.zone == HitZone::Resize => {
                    ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal)
                }
                Some(_) => ui.ctx().set_cursor_icon(CursorIcon::Move),
                None => {}
            }
        }
        let gesture = (pressed && resp.is_pointer_button_down_on())
            || resp.dragged_by(PointerButton::Primary)
            || released
            || self.editor.right_erase
            || vel_resp.is_pointer_button_down_on()
            || vel_resp.dragged_by(PointerButton::Primary);
        if !gesture {
            return;
        }
        let Some(pat) = self.pat_ref(tid, pid).cloned() else {
            return;
        };
        let pat = &pat;
        if pressed && resp.is_pointer_button_down_on() {
            self.focus = super::Focus::Editor;
            if let Some(pos) = ui.input(|i| i.pointer.press_origin()) {
                match roll_note_at(pat, roll, pos) {
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
                            self.preview(tid, note.pitch, note.velocity);
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
                // Dragging from empty grid draws a note whose length
                // follows the pointer (marquee needs Shift or Alt).
                if let Some(pending) = self.editor.pending_note.take() {
                    let cell = self.editor.snap.max(ops::MIN_NOTE_DURATION);
                    self.editor.draw = self
                        .place_pending_note(tid, pid, pat, pending, cell)
                        .map(|id| (id, pending.start));
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
                                self.snapped_drag_dp(false, d.anchor.pitch, raw_dp),
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
                                tid,
                                (drag.anchor.pitch as i32 + dp).clamp(0, ops::MAX_PITCH as i32)
                                    as u8,
                                0.7,
                            );
                        }
                        self.edit(label, Some("note-drag"), |p| {
                            if let Some(pat) = ops::pattern_mut(p, tid, pid) {
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
                if let Some((id, start)) = self.editor.draw {
                    let dur = draw_duration(start, roll.step(pos.x), self.editor.snap);
                    self.edit("Draw note", Some("draw-note"), |p| {
                        if let Some(pat) = ops::pattern_mut(p, tid, pid) {
                            if let Some(n) = pat.notes.iter_mut().find(|n| n.id == id) {
                                let room =
                                    (pat.length as f64 - n.start).max(ops::MIN_NOTE_DURATION);
                                n.duration = dur.min(room);
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
                    self.finish_pending_note(tid, pid, pat, pending);
                }
            }
            self.editor.draw = None;
        }
        if self.editor.right_erase {
            if let Some(hit) = hover.and_then(|pos| roll_note_at(pat, roll, pos)) {
                let id = pat.notes[hit.index].id;
                self.edit("Erase note", Some("erase-notes"), |p| {
                    if let Some(pat) = ops::pattern_mut(p, tid, pid) {
                        ops::delete_notes(pat, &[id]);
                    }
                });
                self.sel.notes.remove(&id);
            }
        }
        // Velocity lane input
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
                    .map(|n| n.id);
                let targets: Vec<u64> = if !self.sel.notes.is_empty() {
                    self.sel.notes.iter().copied().collect()
                } else {
                    hit_note.into_iter().collect()
                };
                if !targets.is_empty() {
                    let drag_ids = self
                        .editor
                        .vel_drag
                        .clone()
                        .unwrap_or_else(|| targets.clone());
                    self.edit("Note velocity", Some("vel-drag"), |p| {
                        if let Some(pat) = ops::pattern_mut(p, tid, pid) {
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

    fn paint_roll(&self, ui: &Ui, tid: u64, pid: u64, roll: &Roll, hover: Option<Pos2>) {
        let pal = self.palette();
        let Some(track) = self.track_ref(tid) else {
            return;
        };
        let Some(pat) = ops::pattern(track, pid) else {
            return;
        };
        let p = ui.painter_at(roll.grid.union(roll.vel).union(roll.ruler));
        let dark = is_dark(pal);
        let black_row = if dark {
            theme::mix(pal.canvas, pal.bg_deep, 0.85)
        } else {
            theme::mix(pal.canvas_alt, pal.grid_faint, 0.35)
        };
        let key = self.project.key;
        let scale = self
            .editor
            .scale_highlight
            .then(|| scale_pitch_classes(key));
        let rh = roll.row_h;
        // Row backgrounds: only the visible rows.
        let (first_row, last_row) = roll.visible_rows();
        for row in first_row..last_row {
            let pitch = LAST_PITCH - row as u8;
            let y = roll.grid.top() + row as f32 * rh - roll.scroll_y;
            let r = Rect::from_min_size(pos2(roll.grid.left(), y), vec2(roll.grid.width(), rh));
            p.rect_filled(
                r,
                0.0,
                if is_black_key(pitch) {
                    black_row
                } else {
                    pal.canvas
                },
            );
            if scale.is_some_and(|pcs| pcs[pitch as usize % 12]) {
                p.rect_filled(r, 0.0, pal.accent.gamma_multiply(0.08));
            }
            // Octave boundary under every C, a hairline between other rows.
            if pitch.is_multiple_of(12) {
                p.hline(
                    roll.grid.x_range(),
                    y + rh,
                    Stroke::new(1.0_f32, pal.grid_bar),
                );
            } else if rh >= 10.0 {
                p.hline(
                    roll.grid.x_range(),
                    y + rh,
                    Stroke::new(0.5_f32, pal.grid_faint),
                );
            }
        }
        // Step/beat/bar lines
        let s0 = roll.step(roll.grid.left()).max(0.0).floor() as i64;
        let s1 = roll.step(roll.grid.right()).ceil() as i64;
        let step_lines = roll.pps >= 14.0;
        for s in s0.max(0)..=s1.min(pat.length as i64) {
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
                    pos2(end_x.max(roll.grid.left()), roll.grid.top()),
                    pos2(roll.grid.right(), roll.vel.bottom()),
                ),
                0.0,
                outside_shade(pal),
            );
        }
        // Notes: cull to the visible pitch and time window before painting.
        let color = theme::rgb(track.color);
        let visible_lo = roll.pitch_at(roll.grid.bottom() - 0.5).unwrap_or(0);
        let visible_hi = roll.pitch_at(roll.grid.top()).unwrap_or(LAST_PITCH);
        let label_font = theme::body_font(9.5);
        let playing_at = self
            .pattern_playhead(track, pid)
            .filter(|_| self.transport.playing);
        for n in &pat.notes {
            if n.pitch < visible_lo || n.pitch > visible_hi {
                continue;
            }
            let r = roll.note_rect(n);
            if r.right() < roll.grid.left() || r.left() > roll.grid.right() {
                continue;
            }
            let selected = self.sel.notes.contains(&n.id);
            // Notes under the playing playhead light up like beat-grid hits.
            let live = note_is_live(n, playing_at);
            let mut body = note_fill(pal, color, n.velocity);
            if live {
                body = theme::mix(body, pal.text, 0.25);
            }
            p.rect_filled(r, 3.0, body);
            p.rect_stroke(
                r,
                3.0,
                Stroke::new(1.0_f32, theme::mix(color, pal.bg_deep, 0.35)),
                StrokeKind::Inside,
            );
            if selected {
                p.rect_stroke(
                    r.expand(1.0),
                    4.0,
                    Stroke::new(2.0_f32, pal.accent),
                    StrokeKind::Outside,
                );
            } else if live {
                p.rect_stroke(r, 3.0, Stroke::new(1.5_f32, pal.accent), StrokeKind::Inside);
            }
            let ink = theme::ink_on(body);
            // Resize affordance on the right edge
            if r.width() > 10.0 && rh >= 10.0 {
                p.vline(
                    r.right() - 3.5,
                    (r.top() + 3.0)..=(r.bottom() - 3.0),
                    Stroke::new(1.0_f32, ink.gamma_multiply(0.45)),
                );
            }
            if r.width() > 26.0 && rh >= 12.0 {
                p.text(
                    pos2(r.left() + 5.0, r.center().y),
                    Align2::LEFT_CENTER,
                    ops::note_name_in(n.pitch, key),
                    label_font.clone(),
                    ink,
                );
            }
        }
        // Marquee
        if let Some(m) = &self.editor.marquee {
            let r = Rect::from_two_pos(m.start, m.current).intersect(roll.grid);
            if r.is_positive() {
                p.rect_filled(r, 0.0, pal.link.gamma_multiply(0.12));
                p.rect_stroke(r, 0.0, Stroke::new(1.0_f32, pal.link), StrokeKind::Inside);
            }
        }
        // Playhead (inside a clip of this pattern): solid while playing,
        // faint while stopped so the record/paste position stays visible.
        if let Some(local) = self.pattern_playhead(track, pid) {
            let x = roll.x(local);
            if x >= roll.grid.left() && x <= roll.grid.right() {
                p.vline(
                    x,
                    roll.grid.top()..=roll.vel.bottom(),
                    playhead_stroke(pal, self.transport.playing),
                );
            }
        }
        // Hover ghost cell
        if self.editor.drag.is_none() && self.editor.draw.is_none() {
            if let Some(pos) =
                hover.filter(|h| roll.grid.contains(*h) && roll_note_at(pat, roll, *h).is_none())
            {
                if let Some(pitch) = roll.pitch_at(pos.y) {
                    // The ghost previews the pitch a drawn note would land
                    // on — under snap-to-scale that is the scale row, not
                    // the chromatic row under the pointer.
                    let pitch = self.snap_to_scale_pitch(false, pitch);
                    let step = snap_floor(roll.step(pos.x), self.editor.snap);
                    let gh = Rect::from_min_size(
                        pos2(roll.x(step), roll.y(pitch) + 1.0),
                        vec2(self.editor.note_len as f32 * roll.pps, rh - 2.0),
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
                theme::body_font(theme::TEXT_BODY),
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

    /// Pattern-relative playhead step when the transport sits inside a clip
    /// of pattern `pid` (looped repetitions fold back into the pattern).
    fn pattern_playhead(&self, track: &Track, pid: u64) -> Option<f64> {
        covering_pattern(track, self.transport.position)
            .filter(|(p, _)| *p == pid)
            .map(|(_, local)| local)
    }

    /// Pitches of `track` sounding under the playhead while the transport
    /// plays (Keyboard-tab glow). Empty when stopped.
    fn sounding_pitches(&self, track: &Track) -> BTreeSet<u8> {
        if !self.transport.playing {
            return BTreeSet::new();
        }
        let Some((pid, local)) = covering_pattern(track, self.transport.position) else {
            return BTreeSet::new();
        };
        ops::pattern(track, pid).map_or_else(BTreeSet::new, |pat| {
            pat.notes
                .iter()
                .filter(|n| local >= n.start && local < n.start + n.duration.max(1.0))
                .map(|n| n.pitch)
                .collect()
        })
    }

    fn paint_keys(&self, ui: &Ui, tid: u64, roll: &Roll) {
        let pal = self.palette();
        let p = ui.painter_at(roll.keys);
        let (white, black) = key_colors(pal);
        p.rect_filled(roll.keys, 0.0, white);
        let scale = self
            .editor
            .scale_highlight
            .then(|| scale_pitch_classes(self.project.key));
        // Pitches held on the computer keyboard glow on the key column too.
        let octave_base = (self.editor.kb_octave + 1) * 12;
        let held: BTreeSet<i32> = held_keyboard_keys(ui.ctx())
            .iter()
            .map(|&i| octave_base + i as i32)
            .collect();
        let rh = roll.row_h;
        let black_w = (roll.keys.width() * 0.62).round();
        let label_font = theme::body_font(10.0);
        let (first_row, last_row) = roll.visible_rows();
        let mut preview_pitch = None;
        for row in first_row..last_row {
            let pitch = LAST_PITCH - row as u8;
            let black_key = is_black_key(pitch);
            let y = roll.grid.top() + row as f32 * rh - roll.scroll_y;
            let full =
                Rect::from_min_max(pos2(roll.keys.left(), y), pos2(roll.keys.right(), y + rh));
            let r = if black_key {
                Rect::from_min_max(full.min, pos2(full.left() + black_w, full.bottom()))
            } else {
                full
            };
            let resp = ui.interact(r, ui.id().with(("key", pitch)), Sense::click_and_drag());
            let base = if black_key { black } else { white };
            let fill = if held.contains(&i32::from(pitch)) || resp.is_pointer_button_down_on() {
                theme::mix(base, pal.accent, 0.6)
            } else if resp.hovered() {
                theme::mix(base, pal.accent, 0.3)
            } else if scale.is_some_and(|pcs| pcs[pitch as usize % 12]) {
                theme::mix(base, pal.accent, 0.16)
            } else {
                base
            };
            p.rect_filled(r, 0.0, fill);
            if !black_key {
                p.hline(
                    full.x_range(),
                    full.bottom(),
                    Stroke::new(1.0_f32, theme::mix(white, black, 0.2)),
                );
            }
            if pitch.is_multiple_of(12) && rh >= 9.0 {
                p.text(
                    pos2(roll.keys.right() - 5.0, y + rh * 0.5),
                    Align2::RIGHT_CENTER,
                    note_name(pitch),
                    label_font.clone(),
                    theme::ink_on(fill).gamma_multiply(0.85),
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
            self.preview(tid, pitch, self.editor.velocity);
        }
    }

    fn paint_roll_ruler(&self, ui: &Ui, tid: u64, pid: u64, roll: &Roll) {
        let pal = self.palette();
        let Some(track) = self.track_ref(tid) else {
            return;
        };
        let len = ops::pattern(track, pid).map_or(16, |p| p.length);
        let p = ui.painter_at(roll.ruler);
        p.rect_filled(roll.ruler, 0.0, pal.panel);
        let pps = roll.pps;
        let every = [1i64, 2, 4, 8, 16]
            .into_iter()
            .find(|k| *k as f32 * 16.0 * pps >= 44.0)
            .unwrap_or(16);
        let font = theme::body_font(10.5);
        let b0 = (roll.step(roll.ruler.left()).max(0.0) / 16.0).floor() as i64;
        let b1 = (roll.step(roll.ruler.right()) / 16.0).ceil() as i64;
        for b in b0..=b1.min((len as i64 + 15) / 16) {
            let x = roll.x(b as f64 * 16.0);
            if b % every == 0 && b * 16 < len as i64 {
                p.text(
                    pos2(x + 4.0, roll.ruler.center().y),
                    Align2::LEFT_CENTER,
                    format!("{}", b + 1),
                    font.clone(),
                    pal.text_dim,
                );
            }
            p.vline(
                x,
                roll.ruler.bottom() - 7.0..=roll.ruler.bottom(),
                Stroke::new(1.0_f32, pal.grid_bar),
            );
            if pps * 4.0 >= 10.0 {
                for beat in 1..4 {
                    let bx = roll.x((b * 16 + beat * 4) as f64);
                    if (b * 16 + beat * 4) < len as i64 {
                        p.vline(
                            bx,
                            roll.ruler.bottom() - 4.0..=roll.ruler.bottom(),
                            Stroke::new(1.0_f32, pal.grid_beat),
                        );
                    }
                }
            }
        }
        if let Some(local) = self.pattern_playhead(track, pid) {
            paint_playhead_marker(&p, pal, roll.x(local), roll.ruler, self.transport.playing);
        }
    }

    fn paint_vel_lane(&self, ui: &Ui, tid: u64, pid: u64, roll: &Roll, labels: Rect) {
        let pal = self.palette();
        let Some(track) = self.track_ref(tid) else {
            return;
        };
        let Some(pat) = ops::pattern(track, pid) else {
            return;
        };
        let p = ui.painter_at(roll.vel.union(labels));
        p.rect_filled(labels, 0.0, pal.panel);
        p.text(
            pos2(labels.left() + 10.0, labels.center().y),
            Align2::LEFT_CENTER,
            "VEL",
            theme::strong_font(ui, theme::TEXT_KICKER),
            pal.text_faint,
        );
        p.rect_filled(roll.vel, 0.0, pal.canvas_alt);
        // 50% guide.
        p.hline(
            roll.vel.x_range(),
            roll.vel.center().y,
            Stroke::new(0.5_f32, pal.grid_faint),
        );
        let color = theme::rgb(track.color);
        let s0 = roll.step(roll.vel.left()).max(0.0);
        let s1 = roll.step(roll.vel.right());
        let top_pad = 4.0;
        for n in &pat.notes {
            if n.start < s0 - n.duration || n.start > s1 {
                continue;
            }
            let x = roll.x(n.start) + 1.0;
            let w = (n.duration as f32 * roll.pps - 1.5).clamp(2.0, 10.0);
            let h = (roll.vel.height() - top_pad) * n.velocity;
            let r = Rect::from_min_max(
                pos2(x, roll.vel.bottom() - h),
                pos2((x + w).min(roll.vel.right()), roll.vel.bottom()),
            );
            if r.right() < roll.vel.left() {
                continue;
            }
            let selected = self.sel.notes.contains(&n.id);
            let fill = if selected {
                pal.accent
            } else {
                note_fill(pal, color, n.velocity)
            };
            p.rect_filled(r, 1.0, fill.gamma_multiply(0.75));
            // Cap line marks the exact value.
            p.hline(
                r.left()..=(r.left() + w.max(6.0)).min(roll.vel.right()),
                r.top(),
                Stroke::new(2.0_f32, fill),
            );
        }
    }
}

impl CadenceApp {
    // ----- keys tab ---------------------------------------------------------

    /// The dedicated Keyboard tab (⌘3): a full-size playable instrument for
    /// the selected track. Melodic tracks get a three-octave piano — mouse
    /// press/drag glissandos, computer keys keep working, held keys glow.
    /// Drum tracks get a responsive grid of pads, one per kit piece.
    pub(super) fn keys_ui(&mut self, ui: &mut Ui) {
        let pal = self.palette();
        let Some(track) = self.selected_track() else {
            centered_empty_state(
                ui,
                pal,
                theme::Icon::Keyboard,
                "No track selected",
                "Pick a track in the arrangement to play it here.",
                None,
            );
            return;
        };
        // Playing keys on an audio lane would audition a sound the track
        // can never produce in the song.
        if track.audio {
            centered_empty_state(
                ui,
                pal,
                theme::Icon::Keyboard,
                "Audio track",
                "Audio lanes play imported WAV clips, not notes.",
                None,
            );
            return;
        }
        let (track_id, drum) = (track.id, track.shows_drum_grid());
        self.keys_toolbar(ui, drum);
        if drum {
            self.keys_drum_ui(ui, track_id);
        } else {
            self.keys_piano_ui(ui, track_id);
        }
    }

    /// Keyboard-tab toolbar: octave stepper (melodic), velocity and, while
    /// record is armed, a recording badge on the right.
    fn keys_toolbar(&mut self, ui: &mut Ui, drum: bool) {
        let pal = self.palette();
        let octave = self.editor.kb_octave;
        let down_tip = theme::shortcut_tip(ui.ctx(), "Octave down", &SC_OCTAVE_DOWN);
        let up_tip = theme::shortcut_tip(ui.ctx(), "Octave up", &SC_OCTAVE_UP);
        let octave_label = format!("C{octave}");
        let record = self
            .transport
            .recording
            .then_some(if self.transport.playing {
                "Recording"
            } else {
                "Step recording"
            });
        let record_w = record.map_or(0.0, |t| {
            text_width(ui, t, theme::strong_font(ui, theme::TEXT_SMALL)) + 30.0
        });
        let record_tip = theme::shortcut_tip(
            ui.ctx(),
            "Notes you play are written into the current pattern — toggle record",
            &super::SC_RECORD,
        );
        let mut shift = 0;
        let ed = &mut self.editor;
        toolbar_row(ui, pal, ("keys-tools", drum), |tb| {
            if !drum {
                tb.label("Octave");
                if tb
                    .enabled(octave > KB_OCTAVE_MIN)
                    .icon_button(theme::Icon::ChevronLeft, "Octave down", &down_tip)
                    .clicked()
                {
                    shift = -1;
                }
                tb.item(36.0, |ui| {
                    ui.add_sized(
                        vec2(36.0, theme::CONTROL_M),
                        egui::Label::new(
                            theme::strong_text(&octave_label)
                                .size(theme::TEXT_BODY)
                                .color(pal.text),
                        ),
                    )
                    .on_hover_text(
                        "Octave the computer keys play: Z X C V B N M are the white keys, \
                         S D G H J the black keys; hold Shift for one octave up",
                    );
                });
                if tb
                    .enabled(octave < KB_OCTAVE_MAX)
                    .icon_button(theme::Icon::ChevronRight, "Octave up", &up_tip)
                    .clicked()
                {
                    shift = 1;
                }
                tb.separator();
            }
            velocity_item(tb, pal, &mut ed.velocity);
            tb.spacer();
            if let Some(text) = record {
                tb.item(record_w, |ui| {
                    record_badge(ui, pal, text, record_w).on_hover_text(&record_tip);
                });
            }
        });
        self.editor.kb_octave = (octave + shift).clamp(KB_OCTAVE_MIN, KB_OCTAVE_MAX);
    }

    /// Melodic Keyboard tab: three octaves of piano keys centred on the play
    /// octave (`kb_octave`), mouse + computer input, held-key glow and the
    /// notes the track is playing lit in its color.
    fn keys_piano_ui(&mut self, ui: &mut Ui, track_id: u64) {
        let pal = self.palette();
        let avail = ui.available_rect_before_wrap();
        let area = Rect::from_min_max(
            pos2(avail.left() + theme::GAP_M, avail.top() + theme::GAP_XS),
            pos2(avail.right() - theme::GAP_M, avail.bottom() - theme::GAP_M),
        );
        if area.width() < 240.0 || area.height() < 80.0 {
            return;
        }

        // Three octaves: kb_octave-1 ..= kb_octave+1. Whites are laid out
        // left to right; each black centres on the boundary after its white.
        let first_oct = self.editor.kb_octave - 1;
        let mut whites: Vec<(u8, i32)> = Vec::with_capacity(21); // (pitch, octave)
        for o in first_oct..=first_oct + 2 {
            for &pc in &KB_WHITE_PCS {
                let p = (o + 1) * 12 + i32::from(pc);
                if (0..=i32::from(ops::MAX_PITCH)).contains(&p) {
                    whites.push((p as u8, o));
                }
            }
        }
        if whites.is_empty() {
            return;
        }
        let white_w = area.width() / whites.len() as f32;
        let black_w = white_w * 0.6;
        let black_h = area.height() * 0.6;
        let white_rect = |i: usize| {
            let x0 = area.left() + i as f32 * white_w;
            Rect::from_min_max(pos2(x0, area.top()), pos2(x0 + white_w, area.bottom()))
        };
        let mut black_rects: Vec<(Rect, u8)> = Vec::with_capacity(15);
        for o in first_oct..=first_oct + 2 {
            for &(bpc, _) in &KB_BLACKS {
                let p = (o + 1) * 12 + i32::from(bpc);
                if !(0..=i32::from(ops::MAX_PITCH)).contains(&p) {
                    continue;
                }
                // The white key a semitone below is always present when the
                // black is — its right edge is the black's centre line.
                let Some(idx) = whites.iter().position(|&(wp, _)| i32::from(wp) == p - 1) else {
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
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "Piano"));
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
                    (0..whites.len())
                        .find(|&i| white_rect(i).contains(p))
                        .map(|i| whites[i].0)
                })
        });
        self.editor.kb_pointer = pointer_pitch;
        self.editor.kb_down = dragging;
        if dragging {
            match pointer_pitch {
                Some(p) if self.editor.kb_last_preview != Some(p) => {
                    self.editor.kb_last_preview = Some(p);
                    self.preview(track_id, p, self.editor.velocity);
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
            hot.extend(pointer_pitch);
        }
        let Some(track) = self.track_ref(track_id) else {
            return;
        };
        let sounding = self.sounding_pitches(track);
        let color = theme::rgb(track.color);
        let hover_pitch = (!dragging).then_some(pointer_pitch).flatten();
        let (white, black) = key_colors(pal);
        let key_state = |pitch: u8, base: Color32| {
            if hot.contains(&pitch) {
                theme::mix(base, pal.accent, 0.62)
            } else if sounding.contains(&pitch) {
                theme::mix(base, color, 0.55)
            } else if hover_pitch == Some(pitch) {
                theme::mix(base, pal.accent, 0.18)
            } else {
                base
            }
        };
        let p = ui.painter_at(area.expand(1.0));
        let bottom_round = CornerRadius {
            nw: 0,
            ne: 0,
            sw: 5,
            se: 5,
        };
        let edge = theme::mix(white, black, 0.35);
        let name_font = theme::body_font(11.0);
        let key_font = theme::strong_font(ui, theme::TEXT_SMALL);
        let play_octave = self.editor.kb_octave;
        for (i, &(pitch, octave)) in whites.iter().enumerate() {
            let rect = white_rect(i).shrink2(vec2(1.0, 0.0));
            let fill = key_state(pitch, white);
            p.rect_filled(rect, bottom_round, fill);
            p.rect_stroke(
                rect,
                bottom_round,
                Stroke::new(1.0_f32, edge),
                StrokeKind::Inside,
            );
            let pc = (pitch % 12) as usize;
            let ink = theme::ink_on(fill);
            // Note name at the bottom of every white key; C keys carry their
            // octave number.
            let (name, name_ink) = if pc == 0 {
                (format!("C{octave}"), ink)
            } else {
                (NOTE_NAMES[pc].to_string(), ink.gamma_multiply(0.6))
            };
            p.text(
                pos2(rect.center().x, rect.bottom() - 12.0),
                Align2::CENTER_CENTER,
                name,
                name_font.clone(),
                name_ink,
            );
            // The play octave's whites also show their computer key.
            if octave == play_octave && white_w >= 22.0 {
                key_badge(
                    &p,
                    pos2(rect.center().x, rect.bottom() - 34.0),
                    KB_KEYS[pc].name(),
                    key_font.clone(),
                    fill,
                );
            }
        }
        for (rect, pitch) in &black_rects {
            let fill = key_state(*pitch, black);
            p.rect_filled(*rect, bottom_round, fill);
            p.rect_stroke(
                *rect,
                bottom_round,
                Stroke::new(1.0_f32, theme::mix(black, white, 0.15)),
                StrokeKind::Inside,
            );
            let pc = (*pitch % 12) as usize;
            // Play-octave black keys show their computer key letter.
            if (*pitch as i32 / 12) - 1 == play_octave && black_w >= 16.0 {
                key_badge(
                    &p,
                    pos2(rect.center().x, rect.bottom() - 14.0),
                    KB_KEYS[pc].name(),
                    key_font.clone(),
                    fill,
                );
            }
        }
    }

    /// Drum Keyboard tab: a centred grid of square-ish pads (four across),
    /// one per kit piece, sized to fill the panel. Each pad shows the
    /// piece and its computer key, lights while pressed, held or playing,
    /// and plays louder the higher it is clicked.
    fn keys_drum_ui(&mut self, ui: &mut Ui, track_id: u64) {
        let pal = self.palette();
        let avail = ui.available_rect_before_wrap();
        let area = Rect::from_min_max(
            pos2(avail.left() + theme::GAP_L, avail.top() + theme::GAP_XS),
            pos2(avail.right() - theme::GAP_L, avail.bottom() - theme::GAP_L),
        );
        if area.width() < 120.0 || area.height() < 60.0 {
            return;
        }
        let pressed = ui.input(|i| i.pointer.primary_pressed());
        let mut hit: Option<(u8, f32)> = None;
        {
            let Some(track) = self.track_ref(track_id) else {
                return;
            };
            let rows = track.drum_rows();
            let kb_pitches = drum_keyboard_pitches(track);
            let held: BTreeSet<u8> = held_keyboard_keys(ui.ctx())
                .iter()
                .filter_map(|&i| kb_pitches.get(i).copied())
                .collect();
            let sounding = self.sounding_pitches(track);
            let color = theme::rgb(track.color);
            let layout = pad_layout(rows.len(), area);
            let p = ui.painter_at(area.expand(2.0));
            let name_font = theme::strong_font(ui, (layout.size.y * 0.13).clamp(12.0, 16.0));
            let key_font = theme::strong_font(ui, theme::TEXT_SMALL);
            for (i, (pitch, name)) in rows.iter().enumerate() {
                let rect = layout.rect(i);
                let resp =
                    ui.interact(rect, ui.id().with(("drum-pad", i)), Sense::click_and_drag());
                let down = resp.is_pointer_button_down_on();
                if down && pressed && hit.is_none() {
                    // Top of the pad is loud, the bottom soft.
                    let vel = resp
                        .interact_pointer_pos()
                        .map_or(self.editor.velocity, |pos| {
                            let t = ((pos.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
                            1.0 - t * 0.7
                        });
                    hit = Some((*pitch, vel));
                }
                let lit = down || held.contains(pitch) || sounding.contains(pitch);
                let fill = if lit {
                    theme::mix(color, pal.text, 0.12)
                } else if resp.hovered() {
                    theme::mix(pal.raised, color, 0.28)
                } else {
                    theme::mix(pal.raised, color, 0.14)
                };
                p.rect_filled(rect, theme::RADIUS_L, fill);
                p.rect_stroke(
                    rect,
                    theme::RADIUS_L,
                    Stroke::new(
                        if lit { 2.0_f32 } else { 1.0_f32 },
                        if lit {
                            pal.accent
                        } else {
                            theme::mix(fill, color, 0.55)
                        },
                    ),
                    StrokeKind::Inside,
                );
                let ink = theme::ink_on(fill);
                theme::paint_elided(
                    ui,
                    rect.center(),
                    Align2::CENTER_CENTER,
                    name,
                    name_font.clone(),
                    ink,
                    rect.width() - 16.0,
                );
                let key = kb_pitches
                    .iter()
                    .position(|kp| kp == pitch)
                    .map(|k| KB_KEYS[k].name());
                if let Some(key) = key {
                    key_badge(
                        &p,
                        pos2(rect.right() - 16.0, rect.bottom() - 15.0),
                        key,
                        key_font.clone(),
                        fill,
                    );
                }
                resp.on_hover_cursor(CursorIcon::PointingHand)
                    .on_hover_ui(|ui| {
                        ui.label(match key {
                            Some(key) => {
                                format!("{name} · key {key} · click higher for a louder hit")
                            }
                            None => format!("{name} · click higher for a louder hit"),
                        });
                    });
            }
        }
        if let Some((pitch, vel)) = hit {
            self.preview(track_id, pitch, vel);
            if self.transport.recording {
                // A pad tap is a full note: insert and release at once (one
                // Snap cell long; step recording then advances the playhead).
                self.record_note_on_with(pitch, vel);
                self.record_note_off(pitch);
            }
        }
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
    /// pattern (live or step recording). Releases are always honoured — on
    /// any tab and under any modifier — and end the note their key started.
    pub(super) fn keyboard_play(&mut self, ctx: &Context) {
        let events: KeyEvents = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    Event::Key {
                        key,
                        pressed,
                        repeat,
                        modifiers,
                        ..
                    } => {
                        let chord = modifiers.command || modifiers.ctrl || modifiers.alt;
                        (!*pressed || !(*repeat || chord)).then_some((
                            *key,
                            *pressed,
                            modifiers.shift,
                        ))
                    }
                    _ => None,
                })
                .collect()
        });
        if events.is_empty() {
            return;
        }
        let playable = self.tab == super::Tab::Editor || self.tab == super::Tab::Keys;
        let target = self
            .sel
            .track
            .filter(|_| playable)
            .and_then(|id| ops::track(&self.project, id))
            .map(|t| (t.id, t.shows_drum_grid().then(|| drum_keyboard_pitches(t))));
        let velocity = self.editor.velocity;
        for (key, pressed, shift) in events {
            if !pressed {
                if let Some(pitch) = self.editor.kb_held.remove(&key) {
                    self.record_note_off(pitch);
                }
                continue;
            }
            let Some((track_id, drum_pitches)) = &target else {
                continue;
            };
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
            if let Some(pitch) = keyboard_pitch(key, shift, self.editor.kb_octave, drum_pitches) {
                self.preview(*track_id, pitch, velocity);
                self.record_note_on(pitch);
                self.editor.kb_held.insert(key, pitch);
            }
        }
    }

    /// Ends held computer-key notes whose release arrives while a dialog or
    /// text field owns the keyboard (when [`Self::keyboard_play`] doesn't run).
    pub(super) fn keyboard_releases(&mut self, ctx: &Context) {
        let released: Vec<Key> = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    Event::Key {
                        key,
                        pressed: false,
                        ..
                    } => Some(*key),
                    _ => None,
                })
                .collect()
        });
        for key in released {
            if let Some(pitch) = self.editor.kb_held.remove(&key) {
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
        let (clip_id, clip_start, loops, plen) = {
            let track = ops::track(&self.project, track_id)?;
            let clip = track.clips.iter().find(|c| c.pattern_id == pid)?;
            let plen = ops::pattern(track, pid)?.length.max(1) as f64;
            (clip.id, clip.start, clip.loop_count(), plen)
        };
        let cap = ops::song_max_clip_loops(&self.project, track_id, clip_id)?;
        if pos < clip_start - 1e-9 {
            return None;
        }
        let need = ((pos - clip_start) / plen).floor() as u32 + 1;
        if need > loops && need <= cap {
            self.record_edit(|p| {
                let _ = ops::set_clip_loops(p, track_id, clip_id, need);
            });
        }
        covering_pattern(ops::track(&self.project, track_id)?, pos)
    }

    /// Starts (or retriggers) a recorded note from the Keyboard tab or the
    /// computer-keyboard row. While the transport plays, the note starts at
    /// the snapped transport step; while it is stopped (step recording) the
    /// note lands at the playhead, which advances one Snap cell on release.
    /// The whole take is one undo step. Returns the new note's id.
    pub(super) fn record_note_on(&mut self, pitch: u8) -> Option<u64> {
        self.record_note_on_with(pitch, self.editor.velocity)
    }

    /// [`Self::record_note_on`] at an explicit `velocity` (drum pads scale
    /// it by where they are hit).
    fn record_note_on_with(&mut self, pitch: u8, velocity: f32) -> Option<u64> {
        if !self.transport.recording || self.transport.count_in_left > 0.0 {
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
        let mut note_id = None;
        self.record_edit(|p| {
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
        if let Some(held) = self.editor.record_pending.remove(&pitch) {
            self.finish_recorded_note(held);
            if held.step_record {
                let snap = self.record_snap();
                self.seek(record_start(held.start_step, snap) + snap);
            }
        }
    }

    /// Applies one recording edit inside the running take's undo group,
    /// opening a new take on the first edit after arming or a stop.
    fn record_edit(&mut self, f: impl FnOnce(&mut Project)) {
        let group = self
            .editor
            .record_take
            .get_or_insert_with(|| format!("record-take-{}", self.history.state()))
            .clone();
        self.edit("Record take", Some(&group), f);
    }

    /// Ends the recording take on stop or disarm: notes still held are
    /// finalized at the current playhead, and the next take gets its own
    /// undo step.
    pub(super) fn finish_record_take(&mut self) {
        let mut held: Vec<PendingRecord> =
            self.editor.record_pending.drain().map(|(_, h)| h).collect();
        held.sort_by_key(|h| h.note_id);
        for h in held {
            self.finish_recorded_note(h);
        }
        self.editor.kb_held.clear();
        self.editor.kb_record_pitch = None;
        if self.editor.record_take.take().is_some() {
            self.history.seal();
        }
    }

    /// Sets a held recorded note's duration from its start to the playhead.
    fn finish_recorded_note(&mut self, held: PendingRecord) {
        let snap = self.record_snap();
        let duration = record_duration(held.start_step, self.transport.position, snap);
        self.record_edit(|p| {
            if let Some(pat) = ops::pattern_mut(p, held.track_id, held.pattern_id) {
                if let Some(n) = pat.notes.iter_mut().find(|n| n.id == held.note_id) {
                    let room = (pat.length as f64 - n.start).max(ops::MIN_NOTE_DURATION);
                    n.duration = duration.clamp(ops::MIN_NOTE_DURATION, room);
                }
            }
        });
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
        let mut symbol = spec.symbol_in(key);
        if let Some(&low) = pitches.first() {
            if low % 12 != spec.root {
                symbol = format!("{symbol}/{}", key.spell(low));
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
        // The progression fills the rest of the pattern: one chord per bar
        // (repeating the loop when there is room for another pass) when it
        // fits, else subdivided evenly — four chords in a one-bar pattern
        // get a beat each, in an eight-bar pattern the loop plays twice.
        let bar = ops::STEPS_PER_BAR as f64;
        let span = f64::from(pattern_len) - step;
        let n = chords.len();
        if !span.is_finite() || span < bar * 0.5 || n == 0 {
            self.set_status(
                StatusKind::Warning,
                "No room for a progression here — stamp closer to the start.",
            );
            return;
        }
        let (slot, count) = if span >= n as f64 * bar {
            (bar, (span / bar).ceil() as usize)
        } else {
            (span / n as f64, n)
        };
        let seq: Vec<Vec<u8>> = (0..count).map(|j| chords[j % n].clone()).collect();
        // Notes run just shy of the next chord's slot so changes stay clean.
        let duration = (slot * 0.875).clamp(1.0, bar);
        let velocity = self.editor.velocity;
        let spacing = self.editor.chord_spacing;
        let strum = (spacing > 0.0).then_some((spacing, self.editor.chord_direction));
        let mut new = Vec::new();
        self.edit(&format!("Stamp {}", preset.name), None, |p| {
            if let Some(pat) = ops::pattern_mut(p, track_id, pid) {
                new = ops::stamp_progression(pat, step, &seq, slot, duration, velocity, strum);
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

    /// Collapsible chord strip above the piano roll (melodic tracks only).
    /// The disclosure row — chevron, "Chords" and the song key — toggles it
    /// anywhere along its length; open, it shows three palettes on a
    /// segmented control (the key's diatonic chords, borrowed chords /
    /// secondary dominants, progression presets), the voicing options
    /// (quality override, inversion, slash bass, strum) and the chord chips:
    /// click a chip to stamp it, hover to audition it. View state lives on
    /// [`EditorView`]; stamping goes through [`Self::stamp_chord_spec`] and
    /// [`Self::stamp_progression`].
    fn chord_strip(&mut self, ui: &mut Ui, tid: u64) {
        let pal = self.palette();
        let open = self.editor.chord_open;
        let key = self.project.key;

        // ----- disclosure row: chevron, "Chords" and the key; open, the same
        // row carries the palette tabs and the voicing options (one toolbar
        // row instead of two, so the piano roll keeps its room).
        let key_name = ops::key_label(key);
        let title_font = theme::strong_font(ui, theme::TEXT_BODY);
        let key_font = theme::body_font(12.0);
        let title_w = text_width(ui, "Chords", title_font.clone());
        let key_w = text_width(ui, &key_name, key_font.clone()).min(CHORD_KEY_MAX_W);
        let row_h = if open { CHORD_OPEN_ROW_H } else { CHORD_ROW_H };
        let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::hover());
        let disclosure = Rect::from_min_size(
            pos2(
                row.left() + theme::GAP_S,
                row.center().y - theme::CONTROL_M * 0.5,
            ),
            vec2(
                theme::GAP_XS
                    + theme::ICON_S
                    + theme::GAP_S
                    + title_w
                    + theme::GAP_S
                    + key_w
                    + theme::GAP_S,
                theme::CONTROL_M,
            ),
        );
        // Closed, the whole row opens the strip; open, only the disclosure
        // closes it, so a near miss between the voicing combos never does.
        let target = if open {
            disclosure
        } else {
            row.shrink2(vec2(theme::GAP_S, 2.0))
        };
        let resp = ui.interact(target, ui.id().with(("chord-row", tid)), Sense::click());
        resp.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::Button, true, open, "Chords")
        });
        let hovered = resp.hovered();
        let resp = resp.on_hover_text(if open {
            "Hide the chord strip"
        } else {
            "Show the chord strip: one-click chords and progressions in the song key"
        });
        if hovered {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }
        if resp.clicked() {
            self.editor.chord_open = !open;
        }
        #[cfg(test)]
        {
            self.editor.layout.chord_row = row;
            self.editor.layout.chord_toggle = target;
        }
        if hovered {
            ui.painter().rect_filled(
                target,
                theme::RADIUS_M,
                pal.control_hover.gamma_multiply(0.5),
            );
        }
        let ink = if hovered { pal.text } else { pal.text_dim };
        let mut x = disclosure.left() + theme::GAP_XS;
        theme::paint_icon(
            ui.painter(),
            Rect::from_center_size(
                pos2(x + theme::ICON_S * 0.5, row.center().y),
                Vec2::splat(theme::ICON_S),
            ),
            if open {
                theme::Icon::ChevronDown
            } else {
                theme::Icon::ChevronRight
            },
            ink,
        );
        x += theme::ICON_S + theme::GAP_S;
        let (title, _) = theme::paint_elided(
            ui,
            pos2(x, row.center().y),
            Align2::LEFT_CENTER,
            "Chords",
            title_font,
            ink,
            120.0,
        );
        x = title.right() + theme::GAP_S;
        theme::paint_elided(
            ui,
            pos2(x, row.center().y),
            Align2::LEFT_CENTER,
            &key_name,
            key_font,
            pal.text_faint,
            CHORD_KEY_MAX_W,
        );
        if !open {
            return;
        }

        // ----- palette tabs and voicing options
        let options = Rect::from_min_max(
            pos2(disclosure.right() + theme::GAP_S, row.top()),
            pos2(row.right() - 12.0, row.bottom()),
        );
        let strumming = self.editor.chord_spacing > 0.0;
        let bass_label = chord_bass_label(self.editor.chord_bass, key);
        let tabs = ChordStripTab::ALL.map(|t| theme::Segment::new(t, t.label()).tip(t.tip()));
        let tabs_w = theme::segmented_width(ui, &tabs, theme::CONTROL_M);
        let ed = &mut self.editor;
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(options)
                .layout(Layout::left_to_right(Align::Center)),
        );
        let out = theme::toolbar(&mut child, pal, ("chord-options", tid), |tb| {
            tb.item(tabs_w, |ui| {
                theme::segmented(ui, pal, &mut ed.chord_tab, &tabs, theme::CONTROL_M, None);
            });
            tb.separator();
            tb.labeled("Type", 104.0, |ui| {
                theme::combo(
                    ("chord-quality", tid),
                    ed.chord_quality.map_or("Diatonic", |q| q.label()),
                    102.0,
                )
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut ed.chord_quality, None, "Diatonic")
                        .on_hover_text("Each chord keeps its scale-degree quality");
                    for q in ChordQuality::ALL {
                        ui.selectable_value(&mut ed.chord_quality, Some(q), q.label());
                    }
                })
                .response
                .on_hover_text("Chord quality for every chip (Diatonic follows the key)");
            });
            tb.labeled("Inversion", 64.0, |ui| {
                theme::combo(
                    ("chord-inversion", tid),
                    CHORD_INVERSION_LABELS[usize::from(ed.chord_inversion.min(3))],
                    62.0,
                )
                .show_ui(ui, |ui| {
                    for (v, l) in CHORD_INVERSION_LABELS.iter().enumerate() {
                        ui.selectable_value(&mut ed.chord_inversion, v as u8, *l);
                    }
                })
                .response
                .on_hover_text("Chord inversion — rotate the lowest tone up an octave");
            });
            tb.labeled("Bass", 64.0, |ui| {
                theme::combo(("chord-bass", tid), bass_label.as_str(), 62.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut ed.chord_bass, ops::ChordBass::Off, "Off");
                        ui.selectable_value(&mut ed.chord_bass, ops::ChordBass::Root, "Root")
                            .on_hover_text("Double the chord root an octave below the voicing");
                        for degree in 0..7 {
                            ui.selectable_value(
                                &mut ed.chord_bass,
                                ops::ChordBass::Degree(degree),
                                key.degree_numeral(degree),
                            )
                            .on_hover_text(format!(
                                "Slash bass on {} ({})",
                                key.degree_numeral(degree),
                                key.spell(key.degree_root(degree)),
                            ));
                        }
                    })
                    .response
                    .on_hover_text("Extra bass tone under the chord (slash chords)");
            });
            tb.separator();
            tb.labeled("Strum", 72.0, |ui| {
                theme::combo(
                    ("chord-spacing", tid),
                    chord_spacing_label(ed.chord_spacing),
                    70.0,
                )
                .show_ui(ui, |ui| {
                    for (v, l) in [(0.0, "Block"), (0.25, "1/64"), (0.5, "1/32"), (1.0, "1/16")] {
                        ui.selectable_value(&mut ed.chord_spacing, v, l);
                    }
                })
                .response
                .on_hover_text("Strum spacing between chord tones (Block stamps them together)");
            });
            tb.enabled(strumming).item(72.0, |ui| {
                theme::combo(("chord-direction", tid), ed.chord_direction.label(), 70.0)
                    .show_ui(ui, |ui| {
                        for d in ops::StrumDirection::ALL {
                            ui.selectable_value(&mut ed.chord_direction, d, d.label());
                        }
                    })
                    .response
                    .on_hover_text("Strum direction")
                    .on_disabled_hover_text("Pick a strum spacing to roll the chord");
            });
        });
        #[cfg(test)]
        {
            self.editor.layout.chords = (out.rect, out.placed.clone(), out.overflowed);
        }
        #[cfg(not(test))]
        let _ = out;

        // ----- chord chips
        let mut stamp = None;
        let mut stamp_degree = None;
        let mut stamp_preset = None;
        let mut hover_spec = None;
        let quality = self.editor.chord_quality;
        egui::Frame::new()
            .inner_margin(Margin {
                left: 12,
                right: 12,
                top: 0,
                bottom: CHORD_CHIPS_PAD as i8,
            })
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let gap = theme::GAP_XS + 2.0;
                    ui.spacing_mut().item_spacing = vec2(gap, gap);
                    // Progression chips narrow to keep the presets on one row.
                    let n = ops::PROGRESSIONS.len() as f32;
                    let progression_w = ((ui.available_width() - gap * (n - 1.0)) / n)
                        .floor()
                        .clamp(PROGRESSION_CHIP_MIN_W, PROGRESSION_CHIP_W);
                    match self.editor.chord_tab {
                        ChordStripTab::Diatonic => {
                            for degree in 0..7 {
                                let q = quality.unwrap_or_else(|| key.degree_chord_quality(degree));
                                let spec = ops::ChordSpec::new(key.degree_root(degree), q);
                                let symbol = spec.symbol_in(key);
                                let resp = chord_chip(
                                    ui,
                                    pal,
                                    &key.degree_numeral(degree),
                                    &symbol,
                                    CHORD_CHIP_W,
                                );
                                if resp.hovered() {
                                    hover_spec = Some(spec);
                                }
                                if resp
                                    .on_hover_text(format!(
                                        "{symbol} — click to stamp at the pointer or playhead"
                                    ))
                                    .clicked()
                                {
                                    stamp_degree = Some(degree);
                                }
                            }
                        }
                        ChordStripTab::Borrowed => {
                            for chord in ops::borrowed_chords(key) {
                                let resp =
                                    chord_chip(ui, pal, chord.label, &chord.symbol, CHORD_CHIP_W);
                                if resp.hovered() {
                                    hover_spec = Some(chord.spec);
                                }
                                if resp
                                    .on_hover_text(format!(
                                        "{} ({} — borrowed from the parallel key) · click to \
                                         stamp at the pointer or playhead",
                                        chord.symbol, chord.label
                                    ))
                                    .clicked()
                                {
                                    stamp = Some(chord.spec);
                                }
                            }
                        }
                        ChordStripTab::Progressions => {
                            for preset in &ops::PROGRESSIONS {
                                let symbols = ops::progression_specs(key, preset, quality)
                                    .iter()
                                    .map(|s| s.symbol_in(key))
                                    .collect::<Vec<_>>()
                                    .join(" – ");
                                let resp =
                                    chord_chip(ui, pal, &symbols, preset.name, progression_w);
                                if resp.hovered() {
                                    hover_spec = ops::progression_specs(key, preset, None)
                                        .into_iter()
                                        .next();
                                }
                                if resp
                                    .on_hover_text(format!(
                                        "{} — {} · one chord per bar starting at the pointer or \
                                         playhead",
                                        preset.hint, symbols
                                    ))
                                    .clicked()
                                {
                                    stamp_preset = Some(preset);
                                }
                            }
                        }
                    }
                });
            });
        // Hover audition: one stab per chord entered, not per frame — and
        // only when the pointer moved there. Opening the strip moves the
        // chips under a still pointer; that must stay silent.
        let hover_id = egui::Id::new("cadence.editor.chord_hover");
        let prev = ui
            .ctx()
            .data(|d| d.get_temp::<String>(hover_id))
            .unwrap_or_default();
        let now = hover_spec.map_or(String::new(), |s| s.symbol());
        if now != prev {
            ui.ctx().data_mut(|d| d.insert_temp(hover_id, now));
            let moved = ui.input(|i| i.pointer.delta() != Vec2::ZERO);
            if let Some(spec) = hover_spec.filter(|_| moved) {
                if let Some(track) = self.track_ref(tid) {
                    self.audition_chord_spec(track, &spec);
                }
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

    /// Step-sequencer toolbar: velocity, Fill, the shared Edit group, an
    /// off-kit warning when imported notes miss the kit rows, and the
    /// selection on the right.
    fn drum_toolbar(&mut self, ui: &mut Ui, tid: u64, pid: u64, rows: &[u8]) {
        let pal = self.palette();
        let (note_count, stray, off_grid) = self.pat_ref(tid, pid).map_or((0, 0, 0), |p| {
            (
                p.notes.len(),
                p.notes.iter().filter(|n| !rows.contains(&n.pitch)).count(),
                p.notes.iter().filter(|n| n.start.fract() != 0.0).count(),
            )
        });
        let selected = self.sel.notes.len();
        let targets = selected > 0 || note_count > 0;
        let delete_tip = theme::shortcut_tip(ui.ctx(), "Delete the selected notes", &SC_DELETE);
        let quantize_tip = if off_grid > 0 {
            format!(
                "Snap note starts to the 1/16 grid — selected notes, or every note when nothing \
                 is selected. {off_grid} note{} sit between steps.",
                sidebar::plural(off_grid)
            )
        } else {
            "Snap note starts to the 1/16 grid — selected notes, or every note when nothing is \
             selected"
                .to_string()
        };
        let stray_text = format!("{stray} off-kit note{}", sidebar::plural(stray));
        let stray_tip = format!(
            "Notes on pitches outside the {} kit row{} (e.g. imported). They still play; the grid \
             only edits row pitches — re-map them via MIDI or the piano roll.",
            rows.len(),
            sidebar::plural(rows.len())
        );
        let stray_w = text_width(ui, &stray_text, theme::body_font(12.0)) + theme::ICON_S + 6.0;
        let selection = format!("{selected} selected");
        let mut act = None;
        let ed = &mut self.editor;
        let out = toolbar_row(ui, pal, ("drum-tools", tid), |tb| {
            velocity_item(tb, pal, &mut ed.velocity);
            tb.separator();
            if tb
                .button(
                    Some(theme::Icon::Wand),
                    "Fill",
                    "Four-on-the-floor on the top row (the kick for the built-in kit)",
                )
                .clicked()
            {
                act = Some(ToolAction::Fill);
            }
            tb.separator();
            if let Some(a) = note_edit_items(tb, targets, &quantize_tip) {
                act = Some(a);
            }
            if stray > 0 {
                tb.separator();
                tb.item(stray_w, |ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(theme::ICON_S), Sense::hover());
                    theme::paint_icon(ui.painter(), r, theme::Icon::Warning, pal.warn);
                    ui.label(RichText::new(&stray_text).size(12.0).color(pal.warn))
                        .on_hover_text(&stray_tip);
                });
            }
            tb.spacer();
            if selected > 0 {
                tb.label(&selection);
                if tb
                    .icon_button(theme::Icon::Trash, "Delete notes", &delete_tip)
                    .clicked()
                {
                    act = Some(ToolAction::DeleteNotes);
                }
            }
        });
        #[cfg(test)]
        {
            self.editor.layout.tools = (out.rect, out.placed.clone(), out.overflowed);
        }
        #[cfg(not(test))]
        let _ = out;
        if let Some(a) = act {
            self.apply_note_tool(tid, pid, a);
        }
    }

    fn drum_editor(&mut self, ui: &mut Ui, tid: u64, pid: u64) {
        let pal = self.palette();
        let Some(rows) = self
            .track_ref(tid)
            .map(|t| t.drum_rows().iter().map(|(p, _)| *p).collect::<Vec<u8>>())
        else {
            return;
        };
        self.drum_toolbar(ui, tid, pid, &rows);
        let Some(len) = self.pat_ref(tid, pid).map(|p| p.length) else {
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
        let row_h = drum_row_height(grid.height(), rows.len());
        let content_h = row_h * rows.len() as f32;
        // Zoom clamps: never smaller than "whole pattern fits", never below MIN_PPS.
        let fit_pps = (grid.width() / len as f32).clamp(MIN_PPS, MAX_PPS);
        self.editor.drum_pps = self.editor.drum_pps.clamp(fit_pps, MAX_PPS);
        self.editor.rects = EditorRects {
            grid,
            vel: Rect::NOTHING,
            keys: labels,
            ruler,
            kb: Rect::NOTHING,
            labels,
        };

        let resp = ui.interact(grid, ui.id().with("drum_grid"), Sense::click_and_drag());
        resp.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "Drum step sequencer")
        });
        // Navigation: the shared scheme — wheel scrolls the rows (when the
        // kit overflows), Shift+wheel scrolls time, Ctrl/Cmd+wheel zooms
        // time around the pointer, trackpads pan both axes and a middle
        // drag pans freely.
        if ui.rect_contains_pointer(area) {
            let (scroll, zoom, hover) =
                ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), i.pointer.hover_pos()));
            if zoom != 1.0 {
                let anchor = hover.map_or(grid.width() * 0.5, |h| {
                    (h.x - grid.left()).clamp(0.0, grid.width())
                });
                let pps = self.editor.drum_pps;
                let step = (anchor + self.editor.drum_scroll_x) / pps;
                self.editor.drum_pps = (pps * zoom).clamp(fit_pps, MAX_PPS);
                self.editor.drum_scroll_x = (step * self.editor.drum_pps - anchor).max(0.0);
            }
            if scroll != Vec2::ZERO {
                self.editor.drum_scroll_x -= scroll.x;
                self.editor.drum_scroll_y -= scroll.y;
                ui.input_mut(|i| i.smooth_scroll_delta = Vec2::ZERO);
            }
        }
        if resp.dragged_by(PointerButton::Middle) {
            let d = resp.drag_delta();
            self.editor.drum_scroll_x -= d.x;
            self.editor.drum_scroll_y -= d.y;
            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        }
        let pps = self.editor.drum_pps;
        let content_w = len as f32 * pps;
        self.editor.drum_scroll_x = self
            .editor
            .drum_scroll_x
            .clamp(0.0, (content_w - grid.width()).max(0.0));
        self.editor.drum_scroll_y = self
            .editor
            .drum_scroll_y
            .clamp(0.0, (content_h - grid.height()).max(0.0));
        let g = DrumGrid {
            grid,
            labels,
            ruler,
            pps,
            scroll_x: self.editor.drum_scroll_x,
            scroll_y: self.editor.drum_scroll_y,
            row_h,
            len: len as f64,
            rows: &rows,
        };
        self.drum_input(ui, tid, pid, &g, &resp);
        self.paint_drum_grid(ui, tid, pid, &g);
        self.paint_drum_labels(ui, tid, &g);
        self.paint_drum_ruler(ui, tid, pid, &g);
        if content_w > grid.width() + 0.5 {
            let mut sx = self.editor.drum_scroll_x;
            let bar = Rect::from_min_max(pos2(grid.left(), grid.bottom() - 8.0), grid.max);
            theme::scrollbar(
                ui,
                pal,
                bar.shrink2(vec2(0.0, 1.0)),
                ui.id().with("drum_hbar"),
                false,
                content_w,
                grid.width(),
                &mut sx,
            );
            self.editor.drum_scroll_x = sx;
        }
        if content_h > grid.height() + 0.5 {
            let mut sy = self.editor.drum_scroll_y;
            let bar = Rect::from_min_max(pos2(grid.right() - 8.0, grid.top()), grid.max);
            theme::scrollbar(
                ui,
                pal,
                bar.shrink2(vec2(1.0, 0.0)),
                ui.id().with("drum_vbar"),
                true,
                content_h,
                grid.height(),
                &mut sy,
            );
            self.editor.drum_scroll_y = sy;
        }
    }

    fn drum_input(
        &mut self,
        ui: &mut Ui,
        tid: u64,
        pid: u64,
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
        let hit_at = |app: &Self, pitch: u8, step: u32| {
            app.pat_ref(tid, pid)
                .and_then(|p| ops::drum_hit(p, pitch, step))
                .map(|n| n.velocity)
        };
        if pressed && resp.is_pointer_button_down_on() {
            self.focus = super::Focus::Editor;
            if let Some(pos) = ui.input(|i| i.pointer.press_origin()) {
                if let Some((row, step)) = g.cell(pos) {
                    let pitch = g.rows[row];
                    let hit = hit_at(self, pitch, step).is_some();
                    if shift && hit {
                        self.edit("Drum accent", None, |p| {
                            if let Some(pat) = ops::pattern_mut(p, tid, pid) {
                                ops::cycle_drum_velocity(pat, pitch, step);
                            }
                        });
                        let v = hit_at(self, pitch, step);
                        self.preview(tid, pitch, v.unwrap_or(0.8));
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
                                if let Some(pat) = ops::pattern_mut(p, tid, pid) {
                                    ops::set_drum(pat, pitch, step, on, vel);
                                }
                            },
                        );
                        if on {
                            self.preview(tid, pitch, vel);
                        }
                    }
                }
            }
        }
        if resp.dragged_by(PointerButton::Primary) {
            if let (Some(on), Some(pos)) = (self.editor.drum_paint, resp.interact_pointer_pos()) {
                if let Some((row, step)) = g.cell(pos) {
                    let pitch = g.rows[row];
                    if hit_at(self, pitch, step).is_some() != on {
                        let vel = self.editor.velocity;
                        self.edit(
                            if on {
                                "Paint drum step"
                            } else {
                                "Erase drum step"
                            },
                            Some("drum-paint"),
                            |p| {
                                if let Some(pat) = ops::pattern_mut(p, tid, pid) {
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
            if let Some((row, step)) = resp.interact_pointer_pos().and_then(|pos| g.cell(pos)) {
                let pitch = g.rows[row];
                if hit_at(self, pitch, step).is_some() {
                    self.edit("Erase drum step", Some("drum-erase"), |p| {
                        if let Some(pat) = ops::pattern_mut(p, tid, pid) {
                            ops::set_drum(pat, pitch, step, false, 0.0);
                        }
                    });
                }
            }
        }
    }

    fn paint_drum_grid(&self, ui: &Ui, tid: u64, pid: u64, g: &DrumGrid<'_>) {
        let pal = self.palette();
        let Some(track) = self.track_ref(tid) else {
            return;
        };
        let Some(pat) = ops::pattern(track, pid) else {
            return;
        };
        let p = ui.painter_at(g.grid);
        let color = theme::rgb(track.color);
        // Rows whose piece is held on the computer keyboard glow.
        let kb_pitches = drum_keyboard_pitches(track);
        let held: BTreeSet<u8> = held_keyboard_keys(ui.ctx())
            .iter()
            .filter_map(|&i| kb_pitches.get(i).copied())
            .collect();
        let playhead = self.pattern_playhead(track, pid);
        let current = playhead
            .filter(|_| self.transport.playing)
            .map(|l| l.floor() as i64);
        let (r0, r1) = g.visible_rows();
        for row in r0..r1 {
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
            if held.contains(&g.rows[row]) {
                p.rect_filled(r, 0.0, pal.accent.gamma_multiply(0.10));
            }
        }
        let s0 = g.step(g.grid.left()).max(0.0).floor() as i64;
        let s1 = (g.step(g.grid.right()).ceil() as i64).min(pat.length as i64);
        // Empty step slots, darker on every other beat, so the grid reads as
        // a step sequencer even with nothing in it.
        if g.pps >= 12.0 && g.row_h >= 12.0 {
            for s in s0.max(0)..s1 {
                let x = g.x(s as f64);
                let slot_fill = if (s / 4) % 2 == 1 {
                    theme::mix(pal.canvas, pal.grid_faint, 0.75)
                } else {
                    theme::mix(pal.canvas, pal.grid_faint, 0.45)
                };
                for row in r0..r1 {
                    let cell = Rect::from_min_size(
                        pos2(x + 2.0, g.row_y(row) + 3.0),
                        vec2(g.pps - 4.0, g.row_h - 6.0),
                    );
                    p.rect_filled(cell, 3.0, slot_fill);
                }
            }
        }
        for s in s0.max(0)..=s1 {
            let x = g.x(s as f64);
            if s % 16 == 0 {
                p.vline(x, g.grid.y_range(), Stroke::new(1.0_f32, pal.grid_bar));
            } else if s % 4 == 0 {
                p.vline(x, g.grid.y_range(), Stroke::new(1.0_f32, pal.grid_beat));
            }
        }
        let end_x = g.x(pat.length as f64);
        if end_x < g.grid.right() {
            p.rect_filled(
                Rect::from_min_max(pos2(end_x.max(g.grid.left()), g.grid.top()), g.grid.max),
                0.0,
                outside_shade(pal),
            );
        }
        // The step under the playhead lights up while playing.
        if let Some(cur) = current {
            let x = g.x(cur as f64);
            p.rect_filled(
                Rect::from_min_max(pos2(x, g.grid.top()), pos2(x + g.pps, g.grid.bottom())),
                0.0,
                pal.accent.gamma_multiply(0.12),
            );
        }
        // Hits: brightness follows velocity; full accents carry a dot, softer
        // hits a level bar along the bottom.
        let top = g.grid.top() - g.row_h;
        let bottom = g.grid.bottom();
        for n in &pat.notes {
            let Some(row) = g.rows.iter().position(|d| *d == n.pitch) else {
                continue;
            };
            let step = n.start.floor();
            let x = g.x(step);
            let y = g.row_y(row);
            if x + g.pps < g.grid.left() || x > g.grid.right() || y < top || y > bottom {
                continue;
            }
            let cell =
                Rect::from_min_size(pos2(x + 2.0, y + 3.0), vec2(g.pps - 4.0, g.row_h - 6.0));
            let live = current == Some(step as i64);
            let mut fill = note_fill(pal, color, n.velocity);
            if live {
                fill = theme::mix(fill, pal.text, 0.25);
            }
            p.rect_filled(cell, 4.0, fill);
            let ink = theme::ink_on(fill);
            if n.velocity > 0.85 {
                if cell.width() >= 12.0 && cell.height() >= 10.0 {
                    p.circle_filled(
                        pos2(cell.right() - 5.5, cell.top() + 5.5),
                        2.0,
                        ink.gamma_multiply(0.8),
                    );
                }
            } else {
                let level = Rect::from_min_max(
                    pos2(cell.left() + 3.0, cell.bottom() - 4.0),
                    pos2(
                        cell.left() + 3.0 + (cell.width() - 6.0).max(0.0) * n.velocity,
                        cell.bottom() - 2.0,
                    ),
                );
                p.rect_filled(level, 1.0, ink.gamma_multiply(0.55));
            }
            if self.sel.notes.contains(&n.id) {
                p.rect_stroke(
                    cell.expand(1.0),
                    5.0,
                    Stroke::new(2.0_f32, pal.accent),
                    StrokeKind::Outside,
                );
            } else if live {
                p.rect_stroke(
                    cell,
                    4.0,
                    Stroke::new(1.5_f32, pal.accent),
                    StrokeKind::Inside,
                );
            }
        }
        // Playhead
        if let Some(local) = playhead {
            let x = g.x(local);
            if x >= g.grid.left() && x <= g.grid.right() {
                p.vline(
                    x,
                    g.grid.y_range(),
                    playhead_stroke(pal, self.transport.playing),
                );
            }
        }
        if pat.notes.is_empty() {
            p.text(
                g.grid.center(),
                Align2::CENTER_CENTER,
                "Empty beat — click steps to add hits",
                theme::body_font(theme::TEXT_BODY),
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

    /// Kit-piece labels: name, the computer key that plays it, hover/click
    /// to preview.
    fn paint_drum_labels(&self, ui: &Ui, tid: u64, g: &DrumGrid<'_>) {
        let pal = self.palette();
        let Some(track) = self.track_ref(tid) else {
            return;
        };
        let rows = track.drum_rows();
        let p = ui.painter_at(g.labels);
        p.rect_filled(g.labels, 0.0, pal.panel);
        // Labels for pieces held on the computer keyboard glow too.
        let kb_pitches = drum_keyboard_pitches(track);
        let held: BTreeSet<u8> = held_keyboard_keys(ui.ctx())
            .iter()
            .filter_map(|&i| kb_pitches.get(i).copied())
            .collect();
        let name_font = theme::body_font(12.0);
        let key_font = theme::strong_font(ui, theme::TEXT_KICKER);
        let mut preview = None;
        let (r0, r1) = g.visible_rows();
        for (row, &(pitch, name)) in rows.iter().enumerate().take(r1).skip(r0) {
            let y = g.row_y(row);
            let r = Rect::from_min_max(
                pos2(g.labels.left(), y),
                pos2(g.labels.right(), y + g.row_h),
            );
            let resp = ui.interact(
                r.intersect(g.labels),
                ui.id().with(("drum-label", row)),
                Sense::click(),
            );
            let hovered = resp.hovered();
            let fill = if held.contains(&pitch) {
                theme::mix(pal.panel, pal.accent, 0.18)
            } else if hovered {
                pal.control_hover
            } else {
                pal.panel
            };
            p.rect_filled(r, 0.0, fill);
            let key = kb_pitches
                .iter()
                .position(|kp| *kp == pitch)
                .map(|k| KB_KEYS[k].name());
            let badge_w = if key.is_some() { 26.0 } else { 0.0 };
            if hovered {
                theme::paint_icon(
                    &p,
                    Rect::from_center_size(
                        pos2(r.left() + 14.0, r.center().y),
                        Vec2::splat(theme::ICON_S - 2.0),
                    ),
                    theme::Icon::Play,
                    pal.accent,
                );
            }
            theme::paint_elided(
                ui,
                pos2(r.left() + 26.0, r.center().y),
                Align2::LEFT_CENTER,
                name,
                name_font.clone(),
                if hovered { pal.text } else { pal.text_dim },
                (r.width() - 34.0 - badge_w).max(10.0),
            );
            if let Some(key) = key {
                key_badge(
                    &p,
                    pos2(r.right() - 18.0, r.center().y),
                    key,
                    key_font.clone(),
                    fill,
                );
            }
            let clicked = resp
                .on_hover_cursor(CursorIcon::PointingHand)
                .on_hover_ui(|ui| {
                    ui.label(match key {
                        Some(key) => format!("Preview {name} (key {key})"),
                        None => format!("Preview {name}"),
                    });
                })
                .clicked();
            if clicked {
                preview = Some(pitch);
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
            self.preview(tid, pitch, 0.9);
        }
    }

    fn paint_drum_ruler(&self, ui: &Ui, tid: u64, pid: u64, g: &DrumGrid<'_>) {
        let pal = self.palette();
        let Some(track) = self.track_ref(tid) else {
            return;
        };
        let len = ops::pattern(track, pid).map_or(16, |p| p.length);
        let p = ui.painter_at(g.ruler);
        p.rect_filled(g.ruler, 0.0, pal.panel);
        let bar_font = theme::strong_font(ui, 10.5);
        let beat_font = theme::body_font(10.0);
        let s0 = g.step(g.ruler.left()).max(0.0).floor() as u32;
        let s1 = (g.step(g.ruler.right()).ceil().max(0.0) as u32).min(len);
        for s in s0..s1 {
            if s.is_multiple_of(4) && (g.pps * 4.0 >= 22.0 || s.is_multiple_of(16)) {
                let x = g.x(s as f64);
                let (label, font, ink) = if s.is_multiple_of(16) {
                    (format!("{}", s / 16 + 1), bar_font.clone(), pal.text_dim)
                } else {
                    (
                        format!("{}.{}", s / 16 + 1, s / 4 % 4 + 1),
                        beat_font.clone(),
                        pal.text_faint,
                    )
                };
                p.text(
                    pos2(x + 4.0, g.ruler.center().y),
                    Align2::LEFT_CENTER,
                    label,
                    font,
                    ink,
                );
            }
        }
        if let Some(local) = self.pattern_playhead(track, pid) {
            paint_playhead_marker(&p, pal, g.x(local), g.ruler, self.transport.playing);
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

/// "Name · 2 bars" — the pattern selector's text.
fn pattern_label(p: &Pattern) -> String {
    format!("{} · {}", p.name, ops::length_label(p.length))
}

/// Natural single-line width of `text` in `font`.
fn text_width(ui: &Ui, text: &str, font: FontId) -> f32 {
    ui.painter()
        .layout_no_wrap(text.to_owned(), font, Color32::PLACEHOLDER)
        .size()
        .x
        .ceil()
}

/// A design-system toolbar row inset by the panel margin.
fn toolbar_row(
    ui: &mut Ui,
    pal: Palette,
    id_salt: impl Hash,
    add_items: impl FnMut(&mut theme::Toolbar<'_>),
) -> theme::ToolbarOutput {
    egui::Frame::new()
        .inner_margin(Margin::symmetric(12, 0))
        .show(ui, |ui| theme::toolbar(ui, pal, id_salt, add_items))
        .inner
}

/// [`theme::empty_state`] centred vertically in the remaining space.
/// Returns `true` when its action was clicked.
fn centered_empty_state(
    ui: &mut Ui,
    pal: Palette,
    icon: theme::Icon,
    title: &str,
    hint: &str,
    action: Option<&str>,
) -> bool {
    let block = if action.is_some() { 150.0 } else { 104.0 };
    ui.add_space(((ui.available_height() - block) * 0.5).max(theme::GAP_L));
    theme::empty_state(ui, pal, icon, title, hint, action)
}

/// Inline pattern-name field: focuses and selects its text when opened;
/// Enter or clicking away commits, Esc cancels.
fn rename_field(ui: &mut Ui, r: &mut PatternRename, width: f32) -> Option<HeaderAction> {
    let id = egui::Id::new(("cadence.pattern-rename", r.pid));
    // egui drops focus on Esc before the frame runs, so the app's own Esc
    // shortcut may already have consumed the key: read the raw events.
    let esc = ui.input(|i| {
        i.raw.events.iter().any(|e| {
            matches!(
                e,
                Event::Key {
                    key: Key::Escape,
                    pressed: true,
                    ..
                }
            )
        })
    });
    let out = egui::TextEdit::singleline(&mut r.text)
        .id(id)
        .desired_width(width - 8.0)
        .char_limit(crate::model::MAX_NAME_CHARS)
        .show(ui);
    if std::mem::take(&mut r.focus) {
        out.response.request_focus();
        let mut state = out.state;
        let end = r.text.chars().count();
        state
            .cursor
            .set_char_range(Some(CCursorRange::two(CCursor::new(0), CCursor::new(end))));
        state.store(ui.ctx(), id);
        return None;
    }
    let resp = out.response.on_hover_text("Enter renames · Esc cancels");
    if esc && (resp.has_focus() || resp.lost_focus()) {
        Some(HeaderAction::CancelRename)
    } else if resp.lost_focus() {
        Some(HeaderAction::CommitRename)
    } else {
        None
    }
}

/// Velocity slider + percentage readout in one toolbar slot.
fn velocity_item(tb: &mut theme::Toolbar<'_>, pal: Palette, velocity: &mut f32) {
    tb.labeled("Velocity", VEL_SLIDER_W + 40.0, |ui| {
        ui.spacing_mut().item_spacing.x = theme::GAP_XS;
        theme::slider_with(
            ui,
            pal,
            velocity,
            0.05..=1.0,
            VEL_SLIDER_W,
            pal.accent,
            Some(0.8),
        )
        .on_hover_text("Velocity of new notes, drum hits and auditions · double-click for 80%");
        ui.label(
            RichText::new(format!("{:.0}%", *velocity * 100.0))
                .size(12.0)
                .color(pal.text_dim),
        );
    });
}

/// The Edit group shared by both editor toolbars: Quantize, Humanize and
/// Velocity −10% / +10%. They act on the selected notes, or the whole
/// pattern when nothing is selected, and disable when there is nothing to
/// act on.
fn note_edit_items(
    tb: &mut theme::Toolbar<'_>,
    targets: bool,
    quantize_tip: &str,
) -> Option<ToolAction> {
    let mut act = None;
    if tb
        .enabled(targets)
        .button(Some(theme::Icon::Quantize), "Quantize", quantize_tip)
        .clicked()
    {
        act = Some(ToolAction::Quantize);
    }
    if tb
        .enabled(targets)
        .button(Some(theme::Icon::Dice), "Humanize", HUMANIZE_TIP)
        .clicked()
    {
        act = Some(ToolAction::Humanize);
    }
    tb.label("Velocity");
    for (label, gain, tip) in VEL_CHIPS {
        if tb.enabled(targets).button(None, label, tip).clicked() {
            act = Some(ToolAction::ScaleVelocity(gain));
        }
    }
    act
}

/// Whether `pal` is a dark theme.
fn is_dark(pal: Palette) -> bool {
    theme::relative_luminance(pal.panel) < 0.4
}

fn is_black_key(pitch: u8) -> bool {
    matches!(pitch % 12, 1 | 3 | 6 | 8 | 10)
}

/// `(white, black)` piano-key fills: light ivory and near-black keys in
/// every theme, tuned to sit inside the panel.
fn key_colors(pal: Palette) -> (Color32, Color32) {
    if is_dark(pal) {
        (theme::mix(pal.text, pal.panel, 0.2), pal.bg_deep)
    } else {
        (pal.raised, theme::mix(pal.text, pal.panel, 0.12))
    }
}

/// Note body: the track color, brighter and more saturated the louder the
/// note.
fn note_fill(pal: Palette, color: Color32, velocity: f32) -> Color32 {
    theme::mix(pal.canvas, color, 0.38 + velocity.clamp(0.0, 1.0) * 0.62)
}

/// Shade over the area past the pattern end.
fn outside_shade(pal: Palette) -> Color32 {
    if is_dark(pal) {
        pal.bg_deep.gamma_multiply(0.7)
    } else {
        pal.text.gamma_multiply(0.08)
    }
}

/// Playhead line: solid accent while playing, faint while stopped.
fn playhead_stroke(pal: Palette, playing: bool) -> Stroke {
    if playing {
        Stroke::new(1.5_f32, pal.accent)
    } else {
        Stroke::new(1.0_f32, pal.accent.gamma_multiply(0.45))
    }
}

/// Playhead triangle hanging from the bottom of a ruler.
fn paint_playhead_marker(p: &Painter, pal: Palette, x: f32, ruler: Rect, playing: bool) {
    if x < ruler.left() - 5.0 || x > ruler.right() + 5.0 {
        return;
    }
    let fill = if playing {
        pal.accent
    } else {
        pal.accent.gamma_multiply(0.55)
    };
    p.add(Shape::convex_polygon(
        vec![
            pos2(x - 5.0, ruler.bottom() - 8.0),
            pos2(x + 5.0, ruler.bottom() - 8.0),
            pos2(x, ruler.bottom() - 1.0),
        ],
        fill,
        Stroke::NONE,
    ));
}

/// Small rounded badge with a computer-key letter, centred at `center`,
/// readable on `under`.
fn key_badge(p: &Painter, center: Pos2, key: &str, font: FontId, under: Color32) {
    let ink = theme::ink_on(under);
    let galley = p.layout_no_wrap(key.to_owned(), font, ink);
    let size = vec2((galley.size().x + 8.0).max(16.0), 16.0);
    let rect = Rect::from_center_size(center, size);
    p.rect(
        rect,
        theme::RADIUS_S,
        theme::mix(under, ink, 0.1),
        Stroke::new(1.0_f32, theme::mix(under, ink, 0.3)),
        StrokeKind::Inside,
    );
    p.galley(rect.center() - galley.size() * 0.5, galley, ink);
}

/// "● Recording" pill for the Keyboard-tab toolbar.
fn record_badge(ui: &mut Ui, pal: Palette, text: &str, width: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, theme::CONTROL_S), Sense::hover());
    let p = ui.painter();
    p.rect(
        rect,
        theme::CONTROL_S * 0.5,
        pal.danger.gamma_multiply(0.12),
        Stroke::new(1.0_f32, pal.danger),
        StrokeKind::Inside,
    );
    p.circle_filled(pos2(rect.left() + 12.0, rect.center().y), 4.0, pal.danger);
    p.text(
        pos2(rect.left() + 22.0, rect.center().y),
        Align2::LEFT_CENTER,
        text,
        theme::strong_font(ui, theme::TEXT_SMALL),
        pal.danger,
    );
    resp
}

/// Two-line chord chip: a small caption (numeral or chord list) over a
/// strong title (chord symbol or preset name).
fn chord_chip(ui: &mut Ui, pal: Palette, caption: &str, title: &str, width: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, CHORD_CHIP_H), Sense::click());
    let enabled = ui.is_enabled();
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, title));
    if ui.is_rect_visible(rect) {
        let fill = if resp.is_pointer_button_down_on() {
            pal.control_active
        } else if resp.hovered() {
            pal.control_hover
        } else {
            pal.control
        };
        let border = if resp.hovered() {
            pal.accent
        } else {
            pal.control_border
        };
        ui.painter().rect(
            rect,
            theme::RADIUS_M,
            fill,
            Stroke::new(1.0_f32, border),
            StrokeKind::Inside,
        );
        let room = rect.width() - 10.0;
        theme::paint_elided(
            ui,
            pos2(rect.center().x, rect.top() + 12.0),
            Align2::CENTER_CENTER,
            caption,
            theme::body_font(10.5),
            pal.text_faint,
            room,
        );
        theme::paint_elided(
            ui,
            pos2(rect.center().x, rect.bottom() - 13.0),
            Align2::CENTER_CENTER,
            title,
            theme::strong_font(ui, theme::TEXT_BODY),
            pal.text,
            room,
        );
    }
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

/// Keyboard-tab drum pad grid: four pads across (fewer for small kits),
/// square-ish, capped at [`PAD_MAX`] and centred; a short last row is
/// centred too.
struct PadLayout {
    origin: Pos2,
    size: Vec2,
    gap: f32,
    cols: usize,
    count: usize,
}

fn pad_layout(count: usize, area: Rect) -> PadLayout {
    let count = count.max(1);
    let cols = count.min(4);
    let rows = count.div_ceil(cols);
    let gap = theme::GAP_M;
    let cell_w = (area.width() - gap * (cols - 1) as f32) / cols as f32;
    let cell_h = (area.height() - gap * (rows - 1) as f32) / rows as f32;
    let h = cell_h.min(cell_w).clamp(24.0, PAD_MAX);
    let w = cell_w.min(h * 1.3).max(24.0);
    let total = vec2(
        cols as f32 * w + gap * (cols - 1) as f32,
        rows as f32 * h + gap * (rows - 1) as f32,
    );
    PadLayout {
        origin: pos2(
            area.center().x - total.x * 0.5,
            area.center().y - total.y * 0.5,
        ),
        size: vec2(w, h),
        gap,
        cols,
        count,
    }
}

impl PadLayout {
    fn rect(&self, i: usize) -> Rect {
        let row = i / self.cols;
        let col = i % self.cols;
        let in_row = (self.count - row * self.cols).min(self.cols);
        let shift = (self.cols - in_row) as f32 * (self.size.x + self.gap) * 0.5;
        Rect::from_min_size(
            pos2(
                self.origin.x + shift + col as f32 * (self.size.x + self.gap),
                self.origin.y + row as f32 * (self.size.y + self.gap),
            ),
            self.size,
        )
    }
}

/// The note under `pos` on the piano roll, topmost (last drawn) first.
fn roll_note_at(pat: &Pattern, roll: &Roll, pos: Pos2) -> Option<NoteHit> {
    pat.notes.iter().enumerate().rev().find_map(|(index, n)| {
        let r = roll.note_rect(n);
        r.contains(pos).then(|| NoteHit {
            index,
            zone: if pos.x >= r.right() - RESIZE_ZONE {
                HitZone::Resize
            } else {
                HitZone::Body
            },
        })
    })
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
        // The real roll snapshots the pattern for each gesture; mirror that here.
        let pat = |app: &CadenceApp| {
            ops::pattern(ops::track(&app.project, tid).unwrap(), pid)
                .unwrap()
                .clone()
        };
        // A click on the C4 row draws on its lower scale neighbour B3.
        app.finish_pending_note(tid, pid, &pat(&app), pending(60, 0.0));
        let drawn = pattern_of(&app, tid, pid).notes.clone();
        assert_eq!(drawn.len(), 1);
        assert_eq!(drawn[0].pitch, 59);
        // An in-scale row keeps its pitch.
        app.finish_pending_note(tid, pid, &pat(&app), pending(61, 4.0));
        assert_eq!(pattern_of(&app, tid, pid).notes[1].pitch, 61);
        // Clicking a chromatic row whose snap target already holds a note at
        // that step selects it instead of overdubbing a duplicate.
        app.finish_pending_note(tid, pid, &pat(&app), pending(60, 0.5));
        assert_eq!(pattern_of(&app, tid, pid).notes.len(), 2);
        assert!(app.sel.notes.contains(&drawn[0].id));
    }

    #[test]
    fn scale_snap_inert_without_toggle_or_key() {
        // Toggle off: the pitch passes through even with a non-default key.
        let (mut app, tid, pid) = app_with(Instrument::Piano, &[]);
        app.project.key = SongKey::new(9, KeyMode::Major);
        let pat = pattern_of(&app, tid, pid).clone();
        app.finish_pending_note(tid, pid, &pat, pending(60, 0.0));
        assert_eq!(pattern_of(&app, tid, pid).notes[0].pitch, 60);
        // Toggle on with the C-major default — treated as "no key": inert.
        // (61 would snap to 60 in C major, so 61 surviving proves inertness.)
        app.project.key = SongKey::default();
        app.editor.scale_snap = true;
        app.finish_pending_note(tid, pid, &pat, pending(61, 4.0));
        assert_eq!(pattern_of(&app, tid, pid).notes[1].pitch, 61);
        // A drum track never snaps, even with the toggle on and a real key.
        app.project.key = SongKey::new(9, KeyMode::Major);
        let (drum_app, dtid, _) = app_with(Instrument::Drums, &[]);
        let drums = ops::track(&drum_app.project, dtid).unwrap().clone();
        assert!(drums.shows_drum_grid());
        assert_eq!(app.snap_to_scale_pitch(drums.shows_drum_grid(), 60), 60);
    }

    #[test]
    fn scale_snap_drag_stair_steps_and_never_nudges_still_notes() {
        // A major = pcs {9,11,1,2,4,6,8}.
        let (mut app, _, _) = app_with(Instrument::Piano, &[]);
        app.project.key = SongKey::new(9, KeyMode::Major);
        app.editor.scale_snap = true;
        // In-scale anchor A3 (57): one row up lands on chromatic A# so the
        // note stays; two rows lands on B; one down lands on G#.
        assert_eq!(app.snapped_drag_dp(false, 57, 0), 0);
        assert_eq!(app.snapped_drag_dp(false, 57, 1), 0);
        assert_eq!(app.snapped_drag_dp(false, 57, 2), 2);
        assert_eq!(app.snapped_drag_dp(false, 57, -1), -1);
        // Chromatic anchor A#3 (58): zero vertical movement never snaps it
        // off its row — a pure horizontal drag keeps its pitch.
        assert_eq!(app.snapped_drag_dp(false, 58, 0), 0);
        assert_eq!(app.snapped_drag_dp(false, 58, 1), 1, "up lands on B");
        assert_eq!(app.snapped_drag_dp(false, 58, -1), -1, "down lands on A");
        // Dragged past the MIDI top, the anchor clamps to the nearest
        // reachable scale tone below (127 G -> 126 F#).
        assert_eq!(app.snapped_drag_dp(false, 127, 4), -1);
        // Toggle off: deltas pass through untouched.
        app.editor.scale_snap = false;
        assert_eq!(app.snapped_drag_dp(false, 57, 1), 1);
        assert_eq!(app.snapped_drag_dp(false, 58, 1), 1);
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
    // ----- headless layout and navigation ---------------------------------

    /// Drives full app frames at a chosen window size.
    struct Frames {
        ctx: Context,
        app: CadenceApp,
        size: Vec2,
        events: Vec<Event>,
        modifiers: Modifiers,
        time: f64,
    }

    impl Frames {
        fn new(app: CadenceApp, w: f32, h: f32) -> Self {
            Self {
                ctx: Context::default(),
                app,
                size: vec2(w, h),
                events: Vec::new(),
                modifiers: Modifiers::NONE,
                time: 0.0,
            }
        }

        fn demo(w: f32, h: f32) -> Self {
            Self::new(CadenceApp::headless(Project::demo()), w, h)
        }

        fn step(&mut self) {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)),
                focused: true,
                time: Some(self.time),
                modifiers: self.modifiers,
                events: std::mem::take(&mut self.events),
                ..Default::default()
            };
            let app = &mut self.app;
            let _ = self.ctx.run(input, |ctx| app.frame(ctx));
            self.time += 1.0 / 60.0;
        }

        fn steps(&mut self, n: usize) {
            for _ in 0..n {
                self.step();
            }
        }

        fn button(&mut self, pos: Pos2, button: PointerButton, pressed: bool) {
            self.events.push(Event::PointerMoved(pos));
            self.events.push(Event::PointerButton {
                pos,
                button,
                pressed,
                modifiers: self.modifiers,
            });
        }

        /// Press and release in one frame (a real quick click).
        fn click(&mut self, pos: Pos2) {
            self.button(pos, PointerButton::Primary, true);
            self.button(pos, PointerButton::Primary, false);
            self.step();
        }

        fn key(&mut self, key: Key) {
            for pressed in [true, false] {
                self.events.push(Event::Key {
                    key,
                    physical_key: Some(key),
                    pressed,
                    repeat: false,
                    modifiers: self.modifiers,
                });
            }
            self.step();
        }

        /// One wheel notch over `pos` with `modifiers`, then frames for
        /// egui's smoothing to settle.
        fn wheel(&mut self, pos: Pos2, delta: Vec2, modifiers: Modifiers) {
            self.modifiers = modifiers;
            self.events.push(Event::PointerMoved(pos));
            self.events.push(Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta,
                modifiers,
            });
            self.steps(30);
            self.modifiers = Modifiers::NONE;
            self.step();
        }

        fn select(&mut self, track_idx: usize) -> (u64, u64) {
            let tid = self.app.project.tracks[track_idx].id;
            self.app.select_track(tid);
            self.steps(4);
            let pid = self.app.editor_target().unwrap().1;
            (tid, pid)
        }
    }

    fn assert_row_clean(what: &str, width: f32, row: Rect, placed: &[Rect]) {
        assert!(!placed.is_empty(), "{what} @ {width}: nothing placed");
        for (i, a) in placed.iter().enumerate() {
            assert!(
                row.expand(0.5).contains_rect(*a),
                "{what} @ {width}: item {i} {a:?} leaves the row {row:?}"
            );
            for (j, b) in placed.iter().enumerate().skip(i + 1) {
                let o = a.intersect(*b);
                assert!(
                    o.width() <= 0.5 || o.height() <= 0.5,
                    "{what} @ {width}: items {i} {a:?} and {j} {b:?} overlap"
                );
            }
        }
    }

    #[test]
    fn editor_header_and_toolbars_never_overlap() {
        for (w, h) in [(1100.0, 760.0), (1360.0, 860.0), (1920.0, 1080.0)] {
            let mut f = Frames::demo(w, h);
            f.step();
            // Track 0 is the drum kit, track 1 the bass (piano roll).
            for idx in [0, 1] {
                f.select(idx);
                let first = f.app.project.tracks[idx].patterns[0].notes[0].id;
                f.app.sel.notes.insert(first);
                f.steps(3);
                let what = if idx == 0 { "drum" } else { "roll" };
                let (row, placed, overflow) = f.app.editor.layout.header.clone();
                assert_row_clean(&format!("{what} header"), w, row, &placed);
                assert_eq!(
                    overflow, 0,
                    "{what} header @ {w}: nothing hides in the overflow menu"
                );
                let (row, placed, overflow) = f.app.editor.layout.tools.clone();
                assert_row_clean(&format!("{what} toolbar"), w, row, &placed);
                if w >= 1360.0 {
                    assert_eq!(overflow, 0, "{what} toolbar @ {w}: everything fits");
                }
                f.app.sel.notes.clear();
            }
            // The open chord row: disclosure, palette tabs and voicing
            // options side by side, never on top of each other.
            f.select(1);
            f.app.editor.chord_open = true;
            f.steps(3);
            let (row, placed, overflow) = f.app.editor.layout.chords.clone();
            assert_row_clean("chord options", w, row, &placed);
            let toggle = f.app.editor.layout.chord_toggle;
            assert!(f.app.editor.layout.chord_row.contains_rect(toggle));
            assert!(
                placed.iter().all(|r| r.left() >= toggle.right()),
                "chord options @ {w} start after the disclosure"
            );
            if w >= 1360.0 {
                assert_eq!(overflow, 0, "chord options @ {w}: everything fits");
            }
            f.app.editor.chord_open = false;
        }
    }

    #[test]
    fn header_inline_rename_commits_with_enter_and_cancels_with_escape() {
        let mut f = Frames::demo(1360.0, 860.0);
        f.step();
        let (tid, pid) = f.select(1);
        let combo = f.app.editor.layout.header.1[2];
        let before = pattern_of(&f.app, tid, pid).name.clone();
        // Double-click the pattern selector to rename inline.
        f.click(combo.center());
        f.click(combo.center());
        f.steps(2);
        assert!(
            f.app.editor.rename.is_some(),
            "double-click starts a rename"
        );
        f.events.push(Event::Text("Bassline B".into()));
        f.step();
        f.key(Key::Enter);
        f.step();
        assert!(f.app.editor.rename.is_none());
        assert_eq!(pattern_of(&f.app, tid, pid).name, "Bassline B");
        assert_eq!(f.app.history.undo_label(), Some("Rename pattern"));

        // Esc cancels without touching the song.
        f.app
            .apply_header_action(tid, pid, HeaderAction::StartRename);
        f.steps(2);
        let note = pattern_of(&f.app, tid, pid).notes[0].id;
        f.app.sel.notes.insert(note);
        f.events.push(Event::Text("Nope".into()));
        f.step();
        f.key(Key::Escape);
        f.step();
        assert!(f.app.editor.rename.is_none());
        assert_eq!(pattern_of(&f.app, tid, pid).name, "Bassline B");
        assert!(
            f.app.sel.notes.contains(&note),
            "Esc in the rename field keeps the note selection"
        );
        f.app.undo();
        assert_eq!(pattern_of(&f.app, tid, pid).name, before);
    }

    #[test]
    fn header_actions_respect_the_last_pattern_and_add_clips() {
        let mut f = Frames::demo(1360.0, 860.0);
        f.step();
        let (tid, pid) = f.select(1);
        let count = |f: &Frames| ops::track(&f.app.project, tid).unwrap().patterns.len();
        let patterns = count(&f);
        f.app
            .apply_header_action(tid, pid, HeaderAction::NewPattern);
        assert_eq!(count(&f), patterns + 1, "new pattern");
        let (_, new) = f.app.editor_target().unwrap();
        assert_ne!(new, pid, "the new pattern is selected");
        f.app
            .apply_header_action(tid, new, HeaderAction::DeletePattern);
        assert_eq!(count(&f), patterns, "delete removes it again");
        // A lone pattern can't be deleted.
        let (mut app, tid, pid) = app_with(Instrument::Piano, &THREE);
        app.apply_header_action(tid, pid, HeaderAction::DeletePattern);
        assert_eq!(ops::track(&app.project, tid).unwrap().patterns.len(), 1);
        app.apply_header_action(tid, pid, HeaderAction::ClearNotes);
        assert!(pattern_of(&app, tid, pid).notes.is_empty());
        assert_eq!(status_text(&app), "Cleared 3 notes.");
        let clips = ops::track(&app.project, tid).unwrap().clips.len();
        app.transport.position = 21.0;
        app.apply_header_action(tid, pid, HeaderAction::AddToArrangement);
        let track = ops::track(&app.project, tid).unwrap();
        assert_eq!(track.clips.len(), clips + 1);
        assert!(
            track
                .clips
                .iter()
                .any(|c| c.start == 16.0 && c.pattern_id == pid),
            "placed at the playhead, snapped to the bar"
        );
    }

    #[test]
    fn piano_roll_navigation_scheme() {
        let mut f = Frames::demo(1360.0, 860.0);
        f.step();
        let (tid, pid) = f.select(1);
        let notes_before = pattern_of(&f.app, tid, pid).notes.clone();
        f.app.editor.roll_pps = MAX_PPS;
        f.app.set_roll_scroll(0.0, 600.0);
        f.steps(2);
        let at = f.app.editor.rects.grid.center();
        let snap = |f: &Frames| {
            let e = &f.app.editor;
            (e.roll_scroll_x, e.roll_scroll_y, e.roll_pps, e.roll_row_h)
        };
        // Wheel: vertical scroll only.
        let (x0, y0, pps0, rh0) = snap(&f);
        f.wheel(at, vec2(0.0, -60.0), Modifiers::NONE);
        let (x1, y1, pps1, rh1) = snap(&f);
        assert!(y1 > y0 + 10.0, "wheel scrolls rows: {y0} -> {y1}");
        assert_eq!((x1, pps1, rh1), (x0, pps0, rh0));
        // Shift+wheel: horizontal scroll.
        f.wheel(at, vec2(0.0, -60.0), Modifiers::SHIFT);
        let (x2, y2, ..) = snap(&f);
        assert!(x2 > x1 + 10.0, "shift+wheel scrolls time: {x1} -> {x2}");
        assert_eq!(y2, y1);
        // Ctrl/Cmd+wheel: time zoom around the pointer.
        f.wheel(at, vec2(0.0, -60.0), Modifiers::COMMAND);
        let (_, y3, pps3, rh3) = snap(&f);
        assert!(pps3 < pps0, "ctrl+wheel zooms time out: {pps0} -> {pps3}");
        assert_eq!((y3, rh3), (y2, rh0));
        // Alt+wheel: row-height zoom.
        f.wheel(at, vec2(0.0, 60.0), Modifiers::ALT);
        let (_, _, pps4, rh4) = snap(&f);
        assert!(rh4 > rh0, "alt+wheel zooms rows in: {rh0} -> {rh4}");
        assert_eq!(pps4, pps3);
        // Two-axis trackpad scroll pans both axes.
        f.app.editor.roll_pps = MAX_PPS;
        f.app.set_roll_scroll(200.0, 600.0);
        f.steps(2);
        let (x5, y5, ..) = snap(&f);
        f.wheel(at, vec2(-5.0, -5.0), Modifiers::NONE);
        let (x6, y6, ..) = snap(&f);
        assert!(x6 > x5 && y6 > y5, "trackpad pans both axes");
        // Middle drag pans freely.
        f.button(at, PointerButton::Middle, true);
        f.step();
        f.events.push(Event::PointerMoved(at + vec2(-40.0, -30.0)));
        f.step();
        f.events.push(Event::PointerMoved(at + vec2(-80.0, -60.0)));
        f.step();
        f.button(at + vec2(-80.0, -60.0), PointerButton::Middle, false);
        f.step();
        let (x7, y7, ..) = snap(&f);
        assert!(
            x7 > x6 + 20.0 && y7 > y6 + 20.0,
            "middle drag pans: {x6},{y6} -> {x7},{y7}"
        );
        assert_eq!(
            pattern_of(&f.app, tid, pid).notes,
            notes_before,
            "navigating never edits"
        );
    }

    #[test]
    fn drum_grid_navigation_zooms_from_fit_and_scrolls_time() {
        let mut f = Frames::demo(1360.0, 860.0);
        f.step();
        f.select(0);
        let fit = f.app.editor.drum_pps;
        let at = f.app.editor.rects.grid.center();
        f.wheel(at, vec2(0.0, 60.0), Modifiers::COMMAND);
        let zoomed = f.app.editor.drum_pps;
        assert!(
            zoomed > fit,
            "ctrl+wheel zooms in from the fitted view: {fit} -> {zoomed}"
        );
        f.wheel(at, vec2(0.0, -60.0), Modifiers::SHIFT);
        assert!(f.app.editor.drum_scroll_x > 0.0, "shift+wheel scrolls time");
    }

    #[test]
    fn chord_strip_row_toggles_and_is_melodic_only() {
        let mut f = Frames::demo(1360.0, 860.0);
        f.step();
        f.select(1);
        let row = f.app.editor.layout.chord_row;
        assert!(row.is_positive(), "melodic tracks show the chord row");
        assert!(!f.app.editor.chord_open);
        // Anywhere along the closed row opens it — here its empty right end.
        f.click(pos2(row.right() - 20.0, row.center().y));
        f.steps(2);
        assert!(f.app.editor.chord_open, "row click opens the strip");
        // Opening can move the row (the split favors the roll), so read it
        // again from the frame.
        let row = f.app.editor.layout.chord_row;
        let toggle = f.app.editor.layout.chord_toggle;
        assert!(row.contains_rect(toggle) && toggle.width() < row.width() * 0.5);
        // Open, a click beside the voicing options leaves it open…
        f.click(pos2(row.right() - 20.0, row.center().y));
        f.steps(2);
        assert!(f.app.editor.chord_open, "a near miss keeps the strip open");
        // …and the chevron closes it.
        f.click(pos2(toggle.left() + 12.0, toggle.center().y));
        f.steps(2);
        assert!(!f.app.editor.chord_open, "chevron click closes it");
        // Drum tracks have no chord strip at all.
        f.app.editor.layout.chord_row = Rect::NOTHING;
        f.select(0);
        assert_eq!(f.app.editor.layout.chord_row, Rect::NOTHING);
    }

    #[test]
    fn drum_pads_fill_the_panel_without_overlap() {
        let area = Rect::from_min_size(pos2(10.0, 20.0), vec2(900.0, 420.0));
        for n in [1, 3, 5, 8, 12, 16] {
            let l = pad_layout(n, area);
            let rects: Vec<Rect> = (0..n).map(|i| l.rect(i)).collect();
            for (i, a) in rects.iter().enumerate() {
                assert!(
                    area.expand(0.5).contains_rect(*a),
                    "{n} pads: pad {i} {a:?} off-panel"
                );
                assert!(a.width() <= PAD_MAX * 1.3 + 0.5 && a.height() <= PAD_MAX + 0.5);
                assert!(a.width() / a.height() <= 1.31, "square-ish");
                for b in rects.iter().skip(i + 1) {
                    assert!(!a.intersects(*b), "{n} pads overlap");
                }
            }
            // The grid is centred in the panel.
            let bounds = rects.iter().fold(Rect::NOTHING, |acc, r| acc.union(*r));
            assert!(
                (bounds.center().x - area.center().x).abs() < 1.0,
                "{n}: centred"
            );
        }
        // Eight pieces: four across, two rows, pads as large as the height
        // allows.
        let l = pad_layout(8, area);
        assert_eq!(l.cols, 4);
        assert!(l.size.y > 150.0, "{:?}", l.size);
    }

    #[test]
    fn keyboard_tab_renders_pads_and_piano_in_every_theme() {
        for theme in theme::ThemeChoice::ALL {
            let mut f = Frames::demo(1100.0, 760.0);
            f.app.theme = theme;
            f.app.tab = super::super::Tab::Keys;
            f.step();
            f.select(0);
            f.app.transport.recording = true;
            f.steps(2);
            f.select(1);
            f.steps(2);
            f.app.transport.recording = false;
        }
    }
}
