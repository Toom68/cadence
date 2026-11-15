//! Song data model shared by the GUI, the audio engine, persistence and MIDI exchange.
//!
//! All musical positions are measured in sixteenth-note *steps*; one bar of 4/4 is
//! [`STEPS_PER_BAR`] steps. Positions are `f64` so that imported MIDI can sit between grid
//! lines; the editor snaps to whole steps.
//!
//! Validation is strict about anything that could break playback or indicate a damaged file
//! (non-finite numbers, out-of-range parameters, dangling references, duplicate IDs, absurd
//! sizes) but tolerant of musically harmless leftovers such as notes past the end of a
//! shortened pattern or clips past the end of a shortened song: those are simply inaudible.

use crate::font::gm_program_name;
use crate::packs::{validate_preset, InstrumentPreset};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};

/// Current on-disk project format version.
pub const PROJECT_VERSION: u32 = 1;
/// File extension (without the dot) used for saved songs.
pub const PROJECT_EXTENSION: &str = "cadence";
/// Sixteenth-note steps per 4/4 bar.
pub const STEPS_PER_BAR: u32 = 16;
/// Sixteenth-note steps per quarter-note beat.
pub const STEPS_PER_BEAT: u32 = 4;
pub const MAX_TRACKS: usize = 32;
pub const MIN_BARS: u32 = 1;
pub const MAX_BARS: u32 = 128;
pub const MIN_BPM: f64 = 30.0;
pub const MAX_BPM: f64 = 300.0;
pub const MAX_SWING: f64 = 0.6;
pub const MAX_PATTERNS_PER_TRACK: usize = 256;
pub const MAX_CLIPS_PER_TRACK: usize = 1024;
/// Maximum loop repetitions per clip.
pub const MAX_CLIP_LOOPS: u32 = 32;
pub const MAX_NOTES_PER_PATTERN: usize = 8192;
/// Longest allowed pattern (and furthest allowed position) in steps.
pub const MAX_PATTERN_STEPS: u32 = MAX_BARS * STEPS_PER_BAR;
/// Shortest note, a sixty-fourth note.
pub const MIN_NOTE_DURATION: f64 = 0.0625;
pub const MAX_NAME_CHARS: usize = 160;
/// Upper bound on flattened note events per song, keeping playback and export bounded.
pub const MAX_SONG_EVENTS: usize = 200_000;
pub const MIN_CUTOFF_HZ: f32 = 100.0;
pub const MAX_CUTOFF_HZ: f32 = 20_000.0;

/// The eight drum voices of the kit as `(MIDI pitch, display name)`, in editor row order.
pub const DRUMS: [(u8, &str); 8] = [
    (36, "Kick"),
    (38, "Snare"),
    (42, "Closed hat"),
    (46, "Open hat"),
    (39, "Clap"),
    (45, "Low tom"),
    (49, "Crash"),
    (51, "Ride"),
];

/// Track colour palette (lime, periwinkle, coral, amber, teal, rose, sky, lavender).
pub const TRACK_COLORS: [[u8; 3]; 8] = [
    [255, 132, 104],
    [186, 230, 94],
    [242, 192, 110],
    [112, 189, 242],
    [152, 162, 255],
    [239, 145, 181],
    [94, 214, 190],
    [196, 166, 255],
];

/// Built-in synthesized instruments. Serialized in lowercase (`"drums"`, `"piano"`, ...).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Instrument {
    Drums,
    Piano,
    Bass,
    Pluck,
    Guitar,
    Pad,
    Lead,
}

impl Instrument {
    pub const ALL: [Instrument; 7] = [
        Instrument::Drums,
        Instrument::Piano,
        Instrument::Bass,
        Instrument::Pluck,
        Instrument::Guitar,
        Instrument::Pad,
        Instrument::Lead,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Instrument::Drums => "Studio drums",
            Instrument::Piano => "Electric piano",
            Instrument::Bass => "Analog bass",
            Instrument::Pluck => "Glass pluck",
            Instrument::Guitar => "Acoustic guitar",
            Instrument::Pad => "Warm pad",
            Instrument::Lead => "Mono lead",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Instrument::Drums => "Eight synthesized drum voices, from deep kicks to crisp hats.",
            Instrument::Piano => "Warm, bell-like electric keys with a soft tremolo.",
            Instrument::Bass => "Round analog bass with a filter that opens on every note.",
            Instrument::Pluck => "Bright plucked-string tones for melodies and arpeggios.",
            Instrument::Guitar => {
                "Physical-modeling steel-string guitar: plucked string, wooden body and pick."
            }
            Instrument::Pad => "Slow, wide chords from softly detuned oscillators.",
            Instrument::Lead => "A clear monophonic lead with glide and vibrato for hooks.",
        }
    }

    pub fn is_drums(self) -> bool {
        self == Instrument::Drums
    }

    /// Plays one note at a time, gliding between overlapping notes.
    pub fn is_monophonic(self) -> bool {
        self == Instrument::Lead
    }

    /// A comfortable pitch to centre the editor on and to audition.
    pub fn default_pitch(self) -> u8 {
        match self {
            Instrument::Drums | Instrument::Bass => 36,
            Instrument::Piano | Instrument::Pad => 60,
            Instrument::Pluck | Instrument::Lead => 72,
            Instrument::Guitar => 55,
        }
    }

    /// Suggested editable pitch range (inclusive). Every pitch 0..=127 still plays.
    pub fn pitch_range(self) -> (u8, u8) {
        match self {
            Instrument::Drums => (35, 59),
            Instrument::Bass => (24, 60),
            Instrument::Piano => (36, 96),
            Instrument::Pluck => (48, 100),
            Instrument::Guitar => (40, 83),
            Instrument::Pad => (36, 88),
            Instrument::Lead => (48, 96),
        }
    }

    pub fn color(self) -> [u8; 3] {
        TRACK_COLORS[self.index()]
    }

    pub fn index(self) -> usize {
        Instrument::ALL.iter().position(|i| *i == self).unwrap_or(0)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Note {
    pub id: u64,
    pub pitch: u8,
    /// Start in steps relative to the pattern start.
    pub start: f64,
    /// Length in steps.
    pub duration: f64,
    /// 0..=1.
    pub velocity: f32,
}

impl Note {
    pub fn new(pitch: u8, start: f64, duration: f64, velocity: f32) -> Self {
        Note {
            id: next_id(),
            pitch,
            start,
            duration,
            velocity,
        }
    }

    pub fn end(&self) -> f64 {
        self.start + self.duration
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Pattern {
    pub id: u64,
    pub name: String,
    /// Length in steps.
    pub length: u32,
    pub notes: Vec<Note>,
}

impl Pattern {
    /// Changes the length, removing notes that would start past the end and shortening notes
    /// that would overhang it.
    pub fn set_length(&mut self, length: u32) {
        self.length = length.clamp(1, MAX_PATTERN_STEPS);
        let len = self.length as f64;
        self.notes.retain(|n| n.start < len);
        for n in &mut self.notes {
            n.duration = n.duration.min(len - n.start).max(MIN_NOTE_DURATION);
        }
    }

    /// Sorts notes by start, then pitch, for stable display and export.
    pub fn sort_notes(&mut self) {
        self.notes
            .sort_by(|a, b| a.start.total_cmp(&b.start).then(a.pitch.cmp(&b.pitch)));
    }

    /// Copy with fresh IDs for the pattern and every note.
    pub fn duplicate(&self, name: impl Into<String>) -> Self {
        Pattern {
            id: next_id(),
            name: name.into(),
            length: self.length,
            notes: self
                .notes
                .iter()
                .map(|n| Note {
                    id: next_id(),
                    ..n.clone()
                })
                .collect(),
        }
    }
}

/// Serde default for [`Clip::loops`]: songs saved before looping existed play once.
fn default_clip_loops() -> u32 {
    1
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Clip {
    pub id: u64,
    pub pattern_id: u64,
    /// Start in steps from the beginning of the song.
    pub start: f64,
    /// Pattern repetitions on the timeline. 1 = plays once (default).
    #[serde(default = "default_clip_loops")]
    pub loops: u32,
    /// Fade-in length in steps at the clip start (velocity ramp). 0 = off.
    #[serde(default)]
    pub fade_in: f64,
    /// Fade-out length in steps at the clip end (velocity ramp). 0 = off.
    #[serde(default)]
    pub fade_out: f64,
}

impl Clip {
    /// Loop repetitions clamped into `1..=MAX_CLIP_LOOPS`, safe on unvalidated data.
    pub fn loop_count(&self) -> u32 {
        self.loops.clamp(1, MAX_CLIP_LOOPS)
    }

    /// Total steps the clip spans on the timeline: the pattern length times the
    /// loop count.
    pub fn span(&self, pattern_len: f64) -> f64 {
        pattern_len * self.loop_count() as f64
    }
}

/// Per-track insert and send effects.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Effects {
    /// Low-pass filter cutoff, 100..=20000 Hz (20 kHz is fully open).
    pub cutoff_hz: f32,
    /// Reverb send, 0..=1.
    pub reverb: f32,
    /// Tempo-synced ping-pong delay send, 0..=1.
    pub delay: f32,
    /// Saturation amount, 0..=1.
    pub drive: f32,
}

impl Default for Effects {
    fn default() -> Self {
        Effects {
            cutoff_hz: MAX_CUTOFF_HZ,
            reverb: 0.15,
            delay: 0.0,
            drive: 0.0,
        }
    }
}

/// Reference to a sampled General MIDI sound inside an installed SoundFont
/// (`.sf2`). Serialized as `{"name": "...", "program": N}`.
///
/// `name` is the font's catalog name — the `.sf2` file stem, e.g.
/// `GeneralUser` — resolved at playback time through [`crate::font::font_key`].
/// `program` is the General MIDI program number, 0..=127 (see
/// [`crate::font::GM_PROGRAM_NAMES`]). When the named font is not installed
/// the track falls back to its preset or built-in instrument.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct FontRef {
    pub name: String,
    pub program: u8,
}

impl FontRef {
    pub fn new(name: impl Into<String>, program: u8) -> Self {
        FontRef {
            name: name.into(),
            program,
        }
    }

    /// Stable key the audio engine uses to look the font up in its catalog.
    pub fn key(&self) -> u64 {
        crate::font::font_key(&self.name)
    }

    /// GM display name of the selected program (e.g. "Acoustic Grand Piano").
    pub fn program_name(&self) -> &'static str {
        gm_program_name(self.program)
    }

    /// "Font Name · Program" label for pickers and track headers.
    pub fn label(&self) -> String {
        format!("{} · {}", self.name, self.program_name())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Track {
    pub id: u64,
    pub name: String,
    pub color: [u8; 3],
    pub instrument: Instrument,
    /// Fader position 0..=1 (perceptual curve applied by the mixer).
    pub volume: f32,
    /// -1 (left) ..= 1 (right).
    pub pan: f32,
    pub mute: bool,
    pub solo: bool,
    pub effects: Effects,
    pub patterns: Vec<Pattern>,
    pub clips: Vec<Clip>,
    /// Data-driven pack sound. When `Some`, it replaces the built-in algorithm of
    /// `instrument` (which still decides the editor: drum grid vs. piano roll). The whole
    /// preset is stored in the song so it plays without the pack installed.
    #[serde(default)]
    pub preset: Option<InstrumentPreset>,
    /// Sampled General MIDI sound from an installed SoundFont. When `Some`, it
    /// takes precedence over `preset`/`instrument`; if the named font file is
    /// not installed the track falls back to that preset or built-in sound.
    /// Songs saved before fonts existed have no `font` field and load as `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font: Option<FontRef>,
}

impl Track {
    /// Name of the sound this track plays: for font tracks the GM program
    /// name, else the preset name or the built-in instrument name. Use
    /// [`FontRef::label`] (or `track.font`) when the font name itself matters.
    pub fn sound_name(&self) -> &str {
        if let Some(font) = &self.font {
            return font.program_name();
        }
        self.preset
            .as_ref()
            .map_or(self.instrument.name(), |p| p.name.as_str())
    }

    /// A track playing `preset`, named after it. `instrument` picks the editor; the sound
    /// comes from the preset.
    pub fn from_preset(instrument: Instrument, preset: InstrumentPreset) -> Self {
        let mut track = create_track(instrument);
        track.name = preset.name.chars().take(MAX_NAME_CHARS).collect();
        track.preset = Some(preset);
        track
    }

    /// A track playing a SoundFont program, named after the font. `instrument`
    /// picks the editor (drum grid vs. piano roll); any existing preset stays
    /// as the fallback when the font file is not installed.
    pub fn from_font(instrument: Instrument, font: FontRef) -> Self {
        let mut track = create_track(instrument);
        track.name = font.name.chars().take(MAX_NAME_CHARS).collect();
        track.font = Some(font);
        track
    }

    /// Whether the pattern editor shows the drum step grid for this track
    /// rather than the piano roll: the built-in drum kit (instrument
    /// [`Instrument::Drums`] with no preset) or a pack *kit* preset
    /// ([`InstrumentPreset::kit`]) on a Drums track. A melodic preset — even
    /// on a Drums track — keeps the piano roll, and a kit preset on a
    /// melodic track does too (the kit still plays, picked by note pitch).
    pub fn shows_drum_grid(&self) -> bool {
        self.instrument.is_drums() && self.preset.as_ref().is_none_or(|p| p.kit.is_some())
    }

    /// The drum-grid rows for this track as `(pitch, display name)` pairs in
    /// row order. A kit preset contributes its pieces in `pieces` array
    /// order (the pack author's chosen row order — deliberately not sorted);
    /// anything else falls back to the built-in [`DRUMS`] table. An empty
    /// piece list (only reachable in unvalidated data) also falls back to
    /// [`DRUMS`], so the grid always has at least one row.
    pub fn drum_rows(&self) -> Vec<(u8, &str)> {
        if let Some(kit) = self.preset.as_ref().and_then(|p| p.kit.as_ref()) {
            if !kit.pieces.is_empty() {
                return kit
                    .pieces
                    .iter()
                    .map(|piece| (piece.pitch, piece.name.as_str()))
                    .collect();
            }
        }
        DRUMS.to_vec()
    }

    pub fn pattern(&self, id: u64) -> Option<&Pattern> {
        self.patterns.iter().find(|p| p.id == id)
    }

    pub fn pattern_mut(&mut self, id: u64) -> Option<&mut Pattern> {
        self.patterns.iter_mut().find(|p| p.id == id)
    }

    /// End of a clip in steps (its start plus its looped span).
    pub fn clip_end(&self, clip: &Clip) -> f64 {
        clip.start
            + clip.span(
                self.pattern(clip.pattern_id)
                    .map_or(0.0, |p| p.length as f64),
            )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Project {
    pub version: u32,
    pub id: u64,
    pub name: String,
    /// Quarter-note beats per minute, 30..=300.
    pub bpm: f64,
    /// 0..=0.6; delays every off-beat sixteenth (see [`swung_step`]).
    pub swing: f64,
    /// Song length in bars, 1..=128.
    pub bars: u32,
    /// 0..=1.
    pub master_volume: f32,
    pub tracks: Vec<Track>,
    /// The song's key. Songs saved before keys existed have no `key` field and
    /// load as C major.
    #[serde(default)]
    pub key: SongKey,
}

/// Pitch-class names with ASCII sharps, indexed by `pitch % 12` (C = 0).
pub const NOTE_NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

/// Scale mode for the song key.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum KeyMode {
    #[default]
    Major,
    /// Natural minor (Aeolian).
    Minor,
}

impl KeyMode {
    pub const ALL: [KeyMode; 2] = [KeyMode::Major, KeyMode::Minor];

    /// Lowercase display name ("major" / "minor").
    pub fn name(self) -> &'static str {
        match self {
            KeyMode::Major => "major",
            KeyMode::Minor => "minor",
        }
    }

    /// The seven scale degrees as semitone offsets from the tonic.
    pub fn semitones(self) -> [u8; 7] {
        match self {
            KeyMode::Major => [0, 2, 4, 5, 7, 9, 11],
            KeyMode::Minor => [0, 2, 3, 5, 7, 8, 10],
        }
    }

    /// Quality of the diatonic triad built on each scale degree.
    pub fn triads(self) -> [ChordQuality; 7] {
        use ChordQuality::{Diminished as D, Major as M, Minor as m};
        match self {
            KeyMode::Major => [M, m, m, M, M, m, D],
            KeyMode::Minor => [m, D, M, m, m, M, M],
        }
    }
}

/// The song's key: a pitch class (0..=11, C = 0) plus major/minor mode.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct SongKey {
    pub root: u8,
    pub mode: KeyMode,
}

impl SongKey {
    pub fn new(root: u8, mode: KeyMode) -> Self {
        SongKey {
            root: root % 12,
            mode,
        }
    }

    /// e.g. "C major", "A minor".
    pub fn name(&self) -> String {
        format!(
            "{} {}",
            NOTE_NAMES[self.root as usize % 12],
            self.mode.name()
        )
    }

    /// The seven scale degrees as semitone offsets from the tonic.
    pub fn semitones(&self) -> [u8; 7] {
        self.mode.semitones()
    }

    /// Pitch class (0..=11) of scale degree `degree` (0-based, wraps past 6).
    pub fn degree_root(&self, degree: usize) -> u8 {
        (self.root + self.semitones()[degree % 7]) % 12
    }

    /// Quality of the diatonic triad on scale degree `degree` (0-based, wraps past 6).
    pub fn degree_chord_quality(&self, degree: usize) -> ChordQuality {
        self.mode.triads()[degree % 7]
    }

    /// Roman numeral for scale degree `degree` (0-based): upper case for major
    /// and augmented triads, lower case for minor, lower case + "°" for diminished.
    pub fn degree_numeral(&self, degree: usize) -> String {
        const NUMERALS: [&str; 7] = ["I", "II", "III", "IV", "V", "VI", "VII"];
        let base = NUMERALS[degree % 7];
        match self.degree_chord_quality(degree) {
            ChordQuality::Minor => base.to_lowercase(),
            ChordQuality::Diminished => format!("{}°", base.to_lowercase()),
            _ => base.to_string(),
        }
    }
}

/// Chord types offered by the arpeggiator.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ChordQuality {
    #[default]
    Major,
    Minor,
    Diminished,
    Augmented,
    Sus2,
    Sus4,
    Power,
    Sixth,
    Minor6,
    Dominant7,
    Major7,
    Minor7,
    HalfDiminished7,
    Diminished7,
    Add9,
    Dominant9,
    Major9,
    Minor9,
}

impl ChordQuality {
    pub const ALL: [ChordQuality; 18] = [
        ChordQuality::Major,
        ChordQuality::Minor,
        ChordQuality::Diminished,
        ChordQuality::Augmented,
        ChordQuality::Sus2,
        ChordQuality::Sus4,
        ChordQuality::Power,
        ChordQuality::Sixth,
        ChordQuality::Minor6,
        ChordQuality::Dominant7,
        ChordQuality::Major7,
        ChordQuality::Minor7,
        ChordQuality::HalfDiminished7,
        ChordQuality::Diminished7,
        ChordQuality::Add9,
        ChordQuality::Dominant9,
        ChordQuality::Major9,
        ChordQuality::Minor9,
    ];

    /// Chord tones as ascending semitone offsets from the root.
    pub fn intervals(self) -> &'static [u8] {
        match self {
            ChordQuality::Major => &[0, 4, 7],
            ChordQuality::Minor => &[0, 3, 7],
            ChordQuality::Diminished => &[0, 3, 6],
            ChordQuality::Augmented => &[0, 4, 8],
            ChordQuality::Sus2 => &[0, 2, 7],
            ChordQuality::Sus4 => &[0, 5, 7],
            ChordQuality::Power => &[0, 7],
            ChordQuality::Sixth => &[0, 4, 7, 9],
            ChordQuality::Minor6 => &[0, 3, 7, 9],
            ChordQuality::Dominant7 => &[0, 4, 7, 10],
            ChordQuality::Major7 => &[0, 4, 7, 11],
            ChordQuality::Minor7 => &[0, 3, 7, 10],
            ChordQuality::HalfDiminished7 => &[0, 3, 6, 10],
            ChordQuality::Diminished7 => &[0, 3, 6, 9],
            ChordQuality::Add9 => &[0, 4, 7, 14],
            ChordQuality::Dominant9 => &[0, 4, 7, 10, 14],
            ChordQuality::Major9 => &[0, 4, 7, 11, 14],
            ChordQuality::Minor9 => &[0, 3, 7, 10, 14],
        }
    }

    /// Friendly name for menus ("Major", "Dominant 7", ...).
    pub fn label(self) -> &'static str {
        match self {
            ChordQuality::Major => "Major",
            ChordQuality::Minor => "Minor",
            ChordQuality::Diminished => "Diminished",
            ChordQuality::Augmented => "Augmented",
            ChordQuality::Sus2 => "Sus2",
            ChordQuality::Sus4 => "Sus4",
            ChordQuality::Power => "Power chord",
            ChordQuality::Sixth => "6",
            ChordQuality::Minor6 => "Minor 6",
            ChordQuality::Dominant7 => "Dominant 7",
            ChordQuality::Major7 => "Major 7",
            ChordQuality::Minor7 => "Minor 7",
            ChordQuality::HalfDiminished7 => "Half-diminished (m7b5)",
            ChordQuality::Diminished7 => "Diminished 7",
            ChordQuality::Add9 => "Add9",
            ChordQuality::Dominant9 => "Dominant 9",
            ChordQuality::Major9 => "Major 9",
            ChordQuality::Minor9 => "Minor 9",
        }
    }

    /// Chord-symbol suffix appended to the root name ("" for major, "m", "maj7", ...).
    pub fn suffix(self) -> &'static str {
        match self {
            ChordQuality::Major => "",
            ChordQuality::Minor => "m",
            ChordQuality::Diminished => "dim",
            ChordQuality::Augmented => "aug",
            ChordQuality::Sus2 => "sus2",
            ChordQuality::Sus4 => "sus4",
            ChordQuality::Power => "5",
            ChordQuality::Sixth => "6",
            ChordQuality::Minor6 => "m6",
            ChordQuality::Dominant7 => "7",
            ChordQuality::Major7 => "maj7",
            ChordQuality::Minor7 => "m7",
            ChordQuality::HalfDiminished7 => "m7b5",
            ChordQuality::Diminished7 => "dim7",
            ChordQuality::Add9 => "add9",
            ChordQuality::Dominant9 => "9",
            ChordQuality::Major9 => "maj9",
            ChordQuality::Minor9 => "m9",
        }
    }

    /// Chord symbol for a root pitch (class), e.g. `symbol(9)` on Minor7 = "Am7".
    pub fn symbol(self, root: u8) -> String {
        format!("{}{}", NOTE_NAMES[root as usize % 12], self.suffix())
    }
}

/// One flattened, playable note: clip position resolved and clipped to pattern and song end.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SongEvent {
    pub step: f64,
    pub duration: f64,
    pub pitch: u8,
    pub velocity: f32,
    pub track_index: usize,
}

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

fn ensure_id_seed() {
    if NEXT_ID.load(Ordering::Relaxed) == 0 {
        // Seeding from wall-clock microseconds keeps IDs from different sessions apart while
        // staying below 2^53, so they survive any JSON tooling as exact integers.
        let micros = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_micros() as u64)
            .unwrap_or(1)
            .clamp(1, 1 << 52);
        let _ = NEXT_ID.compare_exchange(0, micros, Ordering::Relaxed, Ordering::Relaxed);
    }
}

/// Returns a new process-unique, non-zero ID.
pub fn next_id() -> u64 {
    ensure_id_seed();
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// Guarantees later [`next_id`] values are greater than `id` (used after loading files).
pub fn reserve_id(id: u64) {
    ensure_id_seed();
    NEXT_ID.fetch_max(id.saturating_add(1), Ordering::Relaxed);
}

/// Note name with sharps in ASCII, middle C (60) = "C4".
pub fn note_name(pitch: u8) -> String {
    format!(
        "{}{}",
        NOTE_NAMES[pitch as usize % 12],
        pitch as i32 / 12 - 1
    )
}

/// Display name of a drum pitch, falling back to the note name for non-kit pitches.
pub fn drum_name(pitch: u8) -> String {
    DRUMS
        .iter()
        .find(|(p, _)| *p == pitch)
        .map_or_else(|| note_name(pitch), |(_, n)| (*n).to_string())
}

/// Maps any General MIDI percussion pitch to the closest of the eight kit voices in [`DRUMS`].
pub fn map_drum_pitch(pitch: u8) -> u8 {
    match pitch {
        0..=36 => 36,
        37 | 39 => 39,
        38 | 40 => 38,
        42 | 44 => 42,
        46 => 46,
        41 | 43 | 45 | 47 | 48 | 50 => 45,
        49 | 52 | 55 | 57 => 49,
        51 | 53 | 59 => 51,
        _ => 42,
    }
}

/// Equal-temperament frequency of a MIDI pitch (A4 = 69 = 440 Hz).
pub fn pitch_hz(pitch: f32) -> f32 {
    440.0 * 2f32.powf((pitch - 69.0) / 12.0)
}

/// Maps a straight step position to its swung position.
///
/// Within every pair of sixteenths the off-beat is pushed late by `swing * 2/3` of a step and
/// the surrounding time is stretched linearly, so `swing = 0.5` is an exact triplet shuffle.
/// Even steps (eighth-note grid) never move and the mapping is continuous and monotonic, which
/// keeps fractional (imported) positions and note ends ordered.
pub fn swung_step(step: f64, swing: f64) -> f64 {
    let d = swing.clamp(0.0, MAX_SWING) * (2.0 / 3.0);
    if d <= 0.0 || !step.is_finite() {
        return step;
    }
    let base = (step / 2.0).floor() * 2.0;
    let u = step - base;
    let warped = if u < 1.0 {
        u * (1.0 + d)
    } else {
        (1.0 + d) + (u - 1.0) * (1.0 - d)
    };
    base + warped
}

fn pattern_name_for(instrument: Instrument) -> &'static str {
    if instrument.is_drums() {
        "Beat"
    } else {
        "Pattern"
    }
}

/// An empty pattern for `instrument`, `length` steps long.
pub fn create_pattern(instrument: Instrument, length: u32) -> Pattern {
    Pattern {
        id: next_id(),
        name: format!("{} 1", pattern_name_for(instrument)),
        length: length.clamp(1, MAX_PATTERN_STEPS),
        notes: Vec::new(),
    }
}

/// A new track with sensible mix defaults, one empty one-bar pattern and a clip at the start.
pub fn create_track(instrument: Instrument) -> Track {
    let pattern = create_pattern(instrument, STEPS_PER_BAR);
    let (volume, reverb, cutoff) = match instrument {
        Instrument::Drums => (0.78, 0.06, MAX_CUTOFF_HZ),
        Instrument::Bass => (0.7, 0.02, 4000.0),
        Instrument::Piano => (0.62, 0.2, MAX_CUTOFF_HZ),
        Instrument::Pluck => (0.58, 0.2, MAX_CUTOFF_HZ),
        Instrument::Guitar => (0.6, 0.22, MAX_CUTOFF_HZ),
        Instrument::Pad => (0.5, 0.35, 6000.0),
        Instrument::Lead => (0.56, 0.16, MAX_CUTOFF_HZ),
    };
    Track {
        id: next_id(),
        name: instrument.name().to_string(),
        color: instrument.color(),
        instrument,
        volume,
        pan: 0.0,
        mute: false,
        solo: false,
        effects: Effects {
            cutoff_hz: cutoff,
            reverb,
            delay: 0.0,
            drive: 0.0,
        },
        clips: vec![Clip {
            id: next_id(),
            pattern_id: pattern.id,
            start: 0.0,
            loops: 1,
            fade_in: 0.0,
            fade_out: 0.0,
        }],
        patterns: vec![pattern],
        preset: None,
        font: None,
    }
}

fn check_name(label: &str, name: &str) -> Result<(), String> {
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(format!(
            "{label} is longer than {MAX_NAME_CHARS} characters."
        ));
    }
    if name.chars().any(|c| c.is_control() && c != '\t') {
        return Err(format!("{label} contains control characters."));
    }
    Ok(())
}

fn check_range<T: PartialOrd + Copy + std::fmt::Display>(
    label: &str,
    value: T,
    min: T,
    max: T,
    finite: bool,
) -> Result<(), String> {
    if !finite || value < min || value > max {
        return Err(format!("{label} must be between {min} and {max}."));
    }
    Ok(())
}

fn clamp_name(name: &mut String, fallback: &str) {
    let cleaned: String = name
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_NAME_CHARS)
        .collect();
    *name = if cleaned.trim().is_empty() {
        fallback.to_string()
    } else {
        cleaned
    };
}

fn finite_or(value: f64, fallback: f64) -> f64 {
    if value.is_finite() {
        value
    } else {
        fallback
    }
}

fn finite_or_f32(value: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value
    } else {
        fallback
    }
}

impl Project {
    /// A blank song: one drum track with an empty pattern, ready for instruments to be added.
    pub fn empty() -> Self {
        Project {
            version: PROJECT_VERSION,
            id: next_id(),
            name: "Untitled song".to_string(),
            bpm: 110.0,
            swing: 0.0,
            bars: 8,
            master_volume: 0.8,
            tracks: vec![create_track(Instrument::Drums)],
            key: SongKey::default(),
        }
    }

    /// "Afterglow": an eight-bar C-minor groove (Cm9 - Abmaj7 - Ebmaj7 - Bb9) at 104 BPM.
    pub fn demo() -> Self {
        demo_project()
    }

    pub fn total_steps(&self) -> f64 {
        self.bars as f64 * STEPS_PER_BAR as f64
    }

    pub fn seconds_per_step(&self) -> f64 {
        60.0 / (self.bpm.clamp(MIN_BPM, MAX_BPM) * STEPS_PER_BEAT as f64)
    }

    /// Song length in seconds, without effect tails.
    pub fn duration_seconds(&self) -> f64 {
        self.total_steps() * self.seconds_per_step()
    }

    pub fn has_solo(&self) -> bool {
        self.tracks.iter().any(|t| t.solo)
    }

    /// Mute/solo rule shared by playback, export and the mixer display: a muted track is
    /// silent; if any track is soloed, only soloed, unmuted tracks are heard.
    pub fn is_track_audible(&self, index: usize) -> bool {
        let solo = self.has_solo();
        self.tracks
            .get(index)
            .is_some_and(|t| !t.mute && (!solo || t.solo))
    }

    pub fn track_index(&self, id: u64) -> Option<usize> {
        self.tracks.iter().position(|t| t.id == id)
    }

    /// Checks the project is safe to play, edit and save. Also reserves every ID it contains
    /// so later [`next_id`] calls cannot collide with them.
    pub fn validate(&self) -> Result<(), String> {
        if self.version != PROJECT_VERSION {
            return Err(format!(
                "Unsupported project version {} (this build reads version {PROJECT_VERSION}).",
                self.version
            ));
        }
        check_name("Song name", &self.name)?;
        if self.key.root > 11 {
            return Err(format!(
                "Song key root {} is not a pitch class (0-11).",
                self.key.root
            ));
        }
        check_range("Tempo", self.bpm, MIN_BPM, MAX_BPM, self.bpm.is_finite())?;
        check_range("Swing", self.swing, 0.0, MAX_SWING, self.swing.is_finite())?;
        check_range("Song length (bars)", self.bars, MIN_BARS, MAX_BARS, true)?;
        check_range(
            "Master volume",
            self.master_volume,
            0.0,
            1.0,
            self.master_volume.is_finite(),
        )?;
        if self.tracks.len() > MAX_TRACKS {
            return Err(format!("A song can have at most {MAX_TRACKS} tracks."));
        }
        let mut max_id = self.id;
        let mut track_ids = HashSet::new();
        let mut event_count = 0usize;
        let max_pos = MAX_PATTERN_STEPS as f64;
        for (ti, track) in self.tracks.iter().enumerate() {
            let tl = format!("Track {} ({})", ti + 1, track.name);
            check_name(&format!("Track {} name", ti + 1), &track.name)?;
            if !track_ids.insert(track.id) {
                return Err(format!("{tl} has a duplicate ID."));
            }
            max_id = max_id.max(track.id);
            check_range(
                &format!("{tl} volume"),
                track.volume,
                0.0,
                1.0,
                track.volume.is_finite(),
            )?;
            check_range(
                &format!("{tl} pan"),
                track.pan,
                -1.0,
                1.0,
                track.pan.is_finite(),
            )?;
            let fx = &track.effects;
            check_range(
                &format!("{tl} filter cutoff"),
                fx.cutoff_hz,
                MIN_CUTOFF_HZ,
                MAX_CUTOFF_HZ,
                fx.cutoff_hz.is_finite(),
            )?;
            for (label, v) in [
                ("reverb", fx.reverb),
                ("delay", fx.delay),
                ("drive", fx.drive),
            ] {
                check_range(&format!("{tl} {label}"), v, 0.0, 1.0, v.is_finite())?;
            }
            if let Some(preset) = &track.preset {
                validate_preset(preset).map_err(|e| format!("{tl} instrument preset: {e}"))?;
            }
            if let Some(font) = &track.font {
                let fl = format!("{tl} font");
                if font.name.trim().is_empty() {
                    return Err(format!("{fl} name must not be empty."));
                }
                check_name(&format!("{fl} name"), &font.name)?;
                if font.program > 127 {
                    return Err(format!("{fl} program must be between 0 and 127."));
                }
            }
            if track.patterns.len() > MAX_PATTERNS_PER_TRACK {
                return Err(format!(
                    "{tl} has more than {MAX_PATTERNS_PER_TRACK} patterns."
                ));
            }
            if track.clips.len() > MAX_CLIPS_PER_TRACK {
                return Err(format!("{tl} has more than {MAX_CLIPS_PER_TRACK} clips."));
            }
            let mut pattern_ids = HashSet::new();
            for pattern in &track.patterns {
                let pl = format!("{tl}, pattern \"{}\"", pattern.name);
                check_name(&format!("{tl} pattern name"), &pattern.name)?;
                if !pattern_ids.insert(pattern.id) {
                    return Err(format!("{pl} has a duplicate ID."));
                }
                max_id = max_id.max(pattern.id);
                check_range(
                    &format!("{pl} length"),
                    pattern.length,
                    1,
                    MAX_PATTERN_STEPS,
                    true,
                )?;
                if pattern.notes.len() > MAX_NOTES_PER_PATTERN {
                    return Err(format!("{pl} has more than {MAX_NOTES_PER_PATTERN} notes."));
                }
                let mut note_ids = HashSet::new();
                for note in &pattern.notes {
                    if !note_ids.insert(note.id) {
                        return Err(format!("{pl} has notes with duplicate IDs."));
                    }
                    max_id = max_id.max(note.id);
                    if note.pitch > 127 {
                        return Err(format!("{pl} has a note pitch above 127."));
                    }
                    if !note.start.is_finite() || note.start < 0.0 || note.start >= max_pos {
                        return Err(format!("{pl} has a note with an invalid position."));
                    }
                    if !note.duration.is_finite()
                        || note.duration < MIN_NOTE_DURATION - 1e-9
                        || note.duration > max_pos
                    {
                        return Err(format!("{pl} has a note with an invalid length."));
                    }
                    if !note.velocity.is_finite() || !(0.0..=1.0).contains(&note.velocity) {
                        return Err(format!("{pl} has a note velocity outside 0-1."));
                    }
                }
            }
            let mut clip_ids = HashSet::new();
            for clip in &track.clips {
                if !clip_ids.insert(clip.id) {
                    return Err(format!("{tl} has clips with duplicate IDs."));
                }
                max_id = max_id.max(clip.id);
                if !clip.start.is_finite() || clip.start < 0.0 || clip.start >= max_pos {
                    return Err(format!("{tl} has a clip with an invalid position."));
                }
                let pattern = track
                    .pattern(clip.pattern_id)
                    .ok_or_else(|| format!("{tl} has a clip that refers to a missing pattern."))?;
                if clip.loops == 0 || clip.loops > MAX_CLIP_LOOPS {
                    return Err(format!("{tl} has a clip with an invalid loop count."));
                }
                // Fades ramp velocity across the whole looped span, so the honest
                // bound is the span itself.
                let span = pattern.length as f64 * clip.loops as f64;
                check_range(
                    &format!("{tl} clip fade-in"),
                    clip.fade_in,
                    0.0,
                    span,
                    clip.fade_in.is_finite(),
                )?;
                check_range(
                    &format!("{tl} clip fade-out"),
                    clip.fade_out,
                    0.0,
                    span,
                    clip.fade_out.is_finite(),
                )?;
                event_count += pattern.notes.len().saturating_mul(clip.loops as usize);
            }
        }
        if event_count > MAX_SONG_EVENTS {
            return Err(format!(
                "The arrangement contains {event_count} note events; the limit is {MAX_SONG_EVENTS}."
            ));
        }
        reserve_id(max_id);
        Ok(())
    }

    /// Repairs a structurally loaded but possibly invalid project in place so that
    /// [`Project::validate`] succeeds: clamps numbers, replaces non-finite values, trims
    /// oversized collections, drops dangling clips and re-issues duplicate IDs.
    pub fn normalize(&mut self) {
        self.version = PROJECT_VERSION;
        clamp_name(&mut self.name, "Untitled song");
        self.key.root %= 12;
        self.bpm = finite_or(self.bpm, 120.0).clamp(MIN_BPM, MAX_BPM);
        self.swing = finite_or(self.swing, 0.0).clamp(0.0, MAX_SWING);
        self.bars = self.bars.clamp(MIN_BARS, MAX_BARS);
        self.master_volume = finite_or_f32(self.master_volume, 0.8).clamp(0.0, 1.0);
        self.tracks.truncate(MAX_TRACKS);
        let max_pos = MAX_PATTERN_STEPS as f64;
        let mut track_ids = HashSet::new();
        let mut budget = MAX_SONG_EVENTS;
        for track in &mut self.tracks {
            if !track_ids.insert(track.id) {
                track.id = next_id();
                track_ids.insert(track.id);
            }
            clamp_name(&mut track.name, track.instrument.name());
            track.volume = finite_or_f32(track.volume, 0.7).clamp(0.0, 1.0);
            track.pan = finite_or_f32(track.pan, 0.0).clamp(-1.0, 1.0);
            let fx = &mut track.effects;
            fx.cutoff_hz =
                finite_or_f32(fx.cutoff_hz, MAX_CUTOFF_HZ).clamp(MIN_CUTOFF_HZ, MAX_CUTOFF_HZ);
            fx.reverb = finite_or_f32(fx.reverb, 0.0).clamp(0.0, 1.0);
            fx.delay = finite_or_f32(fx.delay, 0.0).clamp(0.0, 1.0);
            fx.drive = finite_or_f32(fx.drive, 0.0).clamp(0.0, 1.0);
            if track
                .preset
                .as_ref()
                .is_some_and(|p| validate_preset(p).is_err())
            {
                // An unusable preset falls back to the track's built-in instrument.
                track.preset = None;
            }
            if let Some(font) = &mut track.font {
                // Repair like other names: drop control characters, cap the
                // length, trim edges (font_key trims too, so keys still match).
                font.name = font
                    .name
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(MAX_NAME_CHARS)
                    .collect::<String>()
                    .trim()
                    .to_string();
                font.program = font.program.min(127);
                if font.name.is_empty() {
                    track.font = None;
                }
            }
            track.patterns.truncate(MAX_PATTERNS_PER_TRACK);
            let mut pattern_ids = HashSet::new();
            for pattern in &mut track.patterns {
                if !pattern_ids.insert(pattern.id) {
                    pattern.id = next_id();
                    pattern_ids.insert(pattern.id);
                }
                clamp_name(&mut pattern.name, "Pattern");
                pattern.length = pattern.length.clamp(1, MAX_PATTERN_STEPS);
                pattern.notes.retain(|n| {
                    n.pitch <= 127
                        && n.start.is_finite()
                        && n.start >= 0.0
                        && n.start < max_pos
                        && n.duration.is_finite()
                        && n.velocity.is_finite()
                });
                pattern.notes.truncate(MAX_NOTES_PER_PATTERN);
                let mut note_ids = HashSet::new();
                for note in &mut pattern.notes {
                    if !note_ids.insert(note.id) {
                        note.id = next_id();
                        note_ids.insert(note.id);
                    }
                    note.duration = note.duration.clamp(MIN_NOTE_DURATION, max_pos);
                    note.velocity = note.velocity.clamp(0.0, 1.0);
                }
            }
            let patterns = &track.patterns;
            track.clips.retain(|c| {
                c.start.is_finite()
                    && c.start >= 0.0
                    && c.start < max_pos
                    && patterns.iter().any(|p| p.id == c.pattern_id)
            });
            track.clips.truncate(MAX_CLIPS_PER_TRACK);
            // Loop/fade repair needs each pattern's length; index them up front
            // because the clip loop below can't borrow `track.patterns` while it
            // takes `track.clips` mutably. The retain above guarantees every
            // remaining clip's pattern exists; `map_or` stays defensive.
            let pattern_lengths: HashMap<u64, u32> =
                track.patterns.iter().map(|p| (p.id, p.length)).collect();
            let mut clip_ids = HashSet::new();
            let mut kept = Vec::with_capacity(track.clips.len());
            for mut clip in std::mem::take(&mut track.clips) {
                clip.loops = clip.loops.clamp(1, MAX_CLIP_LOOPS);
                let span =
                    clip.span(pattern_lengths.get(&clip.pattern_id).copied().unwrap_or(0) as f64);
                clip.fade_in = finite_or(clip.fade_in, 0.0).clamp(0.0, span);
                clip.fade_out = finite_or(clip.fade_out, 0.0).clamp(0.0, span);
                let notes = track.pattern(clip.pattern_id).map_or(0, |p| p.notes.len());
                // The event budget counts every looped repetition, matching validate().
                let events = notes.saturating_mul(clip.loops as usize);
                if events > budget {
                    continue;
                }
                budget -= events;
                if !clip_ids.insert(clip.id) {
                    clip.id = next_id();
                    clip_ids.insert(clip.id);
                }
                kept.push(clip);
            }
            track.clips = kept;
        }
    }

    /// Flattens clips into note events sorted by step (then track, then pitch), clipped to
    /// each pattern's length and to the song end. With `audible_only`, mute/solo is applied
    /// exactly as in playback (see [`Project::is_track_audible`]).
    pub fn events(&self, audible_only: bool) -> Vec<SongEvent> {
        let end = self.total_steps();
        let mut events = Vec::new();
        'tracks: for (track_index, track) in self.tracks.iter().enumerate() {
            if audible_only && !self.is_track_audible(track_index) {
                continue;
            }
            for clip in &track.clips {
                if !clip.start.is_finite() || clip.start < 0.0 || clip.start >= end {
                    continue;
                }
                let Some(pattern) = track.pattern(clip.pattern_id) else {
                    continue;
                };
                let len = pattern.length as f64;
                let loops = clip.loop_count();
                let span = len * loops as f64;
                // Fades ramp velocity once across the whole looped span (fade-in at
                // the very start, fade-out at the very end of the last repetition).
                // Non-finite or negative lengths simply mean "off".
                let fade_in = finite_or(clip.fade_in, 0.0).max(0.0);
                let fade_out = finite_or(clip.fade_out, 0.0).max(0.0);
                for rep in 0..loops {
                    let offset = rep as f64 * len;
                    if clip.start + offset >= end {
                        break;
                    }
                    for note in &pattern.notes {
                        if !(note.start >= 0.0 && note.start < len) || !note.duration.is_finite() {
                            continue;
                        }
                        let pos_in_clip = offset + note.start;
                        let step = clip.start + pos_in_clip;
                        if step >= end {
                            continue;
                        }
                        let duration = note.duration.min(len - note.start).min(end - step);
                        if duration <= 0.0 {
                            continue;
                        }
                        if events.len() >= MAX_SONG_EVENTS {
                            break 'tracks;
                        }
                        let mut gain = 1.0f64;
                        if fade_in > 0.0 {
                            gain *= (pos_in_clip / fade_in).clamp(0.0, 1.0);
                        }
                        if fade_out > 0.0 {
                            gain *= ((span - pos_in_clip) / fade_out).clamp(0.0, 1.0);
                        }
                        events.push(SongEvent {
                            step,
                            duration,
                            pitch: note.pitch.min(127),
                            velocity: if note.velocity.is_finite() {
                                (note.velocity.clamp(0.0, 1.0) as f64 * gain).clamp(0.0, 1.0) as f32
                            } else {
                                0.0
                            },
                            track_index,
                        });
                    }
                }
            }
        }
        events.sort_by(|a, b| {
            a.step
                .total_cmp(&b.step)
                .then(a.track_index.cmp(&b.track_index))
                .then(a.pitch.cmp(&b.pitch))
        });
        events
    }
}

impl Default for Project {
    fn default() -> Self {
        Project::empty()
    }
}

fn n(pitch: u8, start: f64, duration: f64, velocity: f32) -> Note {
    Note::new(pitch, start, duration, velocity)
}

fn pattern(name: &str, length: u32, notes: Vec<Note>) -> Pattern {
    let mut p = Pattern {
        id: next_id(),
        name: name.to_string(),
        length,
        notes,
    };
    p.sort_notes();
    p
}

fn clip(pattern: &Pattern, start: f64) -> Clip {
    Clip {
        id: next_id(),
        pattern_id: pattern.id,
        start,
        loops: 1,
        fade_in: 0.0,
        fade_out: 0.0,
    }
}

/// Builds the drum grooves. Hats, ghost notes and fills vary bar to bar.
fn demo_drums() -> Track {
    const KICK: u8 = 36;
    const SNARE: u8 = 38;
    const HAT: u8 = 42;
    const OPEN: u8 = 46;
    const CLAP: u8 = 39;
    const TOM: u8 = 45;
    const CRASH: u8 = 49;
    const RIDE: u8 = 51;
    let hats = |skip: &[u32]| -> Vec<Note> {
        (0..8u32)
            .map(|i| i * 2)
            .filter(|s| !skip.contains(s))
            .map(|s| n(HAT, s as f64, 0.5, if s % 4 == 0 { 0.62 } else { 0.44 }))
            .collect()
    };
    let backbeat = || {
        vec![
            n(SNARE, 4.0, 1.0, 0.86),
            n(SNARE, 12.0, 1.0, 0.9),
            n(CLAP, 12.0, 1.0, 0.34),
        ]
    };

    let mut pocket = vec![
        n(KICK, 0.0, 1.0, 0.95),
        n(KICK, 6.0, 1.0, 0.66),
        n(KICK, 8.0, 1.0, 0.9),
        n(KICK, 11.0, 1.0, 0.58),
        n(SNARE, 9.0, 0.5, 0.18),
        n(HAT, 13.0, 0.5, 0.26),
        n(OPEN, 14.0, 1.5, 0.4),
    ];
    pocket.extend(hats(&[14]));
    pocket.extend(backbeat());

    let mut lift = vec![
        n(KICK, 0.0, 1.0, 0.95),
        n(KICK, 3.0, 1.0, 0.52),
        n(KICK, 8.0, 1.0, 0.9),
        n(KICK, 10.0, 1.0, 0.7),
        n(SNARE, 7.0, 0.5, 0.2),
        n(SNARE, 15.0, 0.5, 0.24),
        n(HAT, 5.0, 0.5, 0.24),
        n(OPEN, 6.0, 1.5, 0.36),
    ];
    lift.extend(hats(&[6]));
    lift.extend(backbeat());

    let mut crash_in = vec![
        n(CRASH, 0.0, 4.0, 0.72),
        n(KICK, 0.0, 1.0, 1.0),
        n(KICK, 6.0, 1.0, 0.64),
        n(KICK, 8.0, 1.0, 0.88),
        n(KICK, 11.0, 1.0, 0.56),
        n(SNARE, 9.0, 0.5, 0.18),
        n(OPEN, 14.0, 1.5, 0.38),
    ];
    crash_in.extend(hats(&[0, 14]));
    crash_in.extend(backbeat());

    let mut ride = vec![
        n(KICK, 0.0, 1.0, 0.94),
        n(KICK, 6.0, 1.0, 0.62),
        n(KICK, 8.0, 1.0, 0.88),
        n(KICK, 14.0, 1.0, 0.56),
        n(SNARE, 9.0, 0.5, 0.2),
        n(SNARE, 15.0, 0.5, 0.22),
        n(HAT, 4.0, 0.5, 0.3),
        n(HAT, 12.0, 0.5, 0.3),
    ];
    ride.extend((0..8).map(|i| {
        n(
            RIDE,
            i as f64 * 2.0,
            1.0,
            if i % 2 == 0 { 0.52 } else { 0.38 },
        )
    }));
    ride.extend(backbeat());

    let mut fill = vec![
        n(KICK, 0.0, 1.0, 0.95),
        n(KICK, 6.0, 1.0, 0.64),
        n(KICK, 8.0, 1.0, 0.9),
        n(SNARE, 4.0, 1.0, 0.86),
        n(SNARE, 12.0, 1.0, 0.84),
        n(SNARE, 11.0, 0.5, 0.3),
        n(TOM, 13.0, 1.0, 0.62),
        n(TOM, 14.0, 1.0, 0.74),
        n(TOM, 15.0, 0.5, 0.82),
        n(SNARE, 15.5, 0.5, 0.5),
        n(CLAP, 12.0, 1.0, 0.36),
    ];
    fill.extend(hats(&[12, 14]));

    let pocket = pattern("Pocket", 16, pocket);
    let lift = pattern("Pocket lift", 16, lift);
    let fill = pattern("Turnaround", 16, fill);
    let crash_in = pattern("Crash in", 16, crash_in);
    let ride = pattern("Ride groove", 16, ride);

    let mut track = create_track(Instrument::Drums);
    track.name = "Pocket drums".into();
    track.volume = 0.8;
    track.effects = Effects {
        cutoff_hz: MAX_CUTOFF_HZ,
        reverb: 0.08,
        delay: 0.0,
        drive: 0.12,
    };
    let order = [
        &crash_in, &pocket, &lift, &fill, &crash_in, &ride, &ride, &fill,
    ];
    track.clips = order
        .iter()
        .enumerate()
        .map(|(bar, p)| clip(p, bar as f64 * 16.0))
        .collect();
    track.patterns = vec![pocket, lift, fill, crash_in, ride];
    track
}

fn demo_project() -> Project {
    let drums = demo_drums();

    // Two bars per chord: Cm9, Abmaj7, Ebmaj7, Bb9 (rootless, voice-led keyboard voicings).
    let chord_names = ["Cm9", "Abmaj7", "Ebmaj7", "Bb9"];
    let roots: [u8; 4] = [36, 32, 39, 34];
    let keys_voicings: [[u8; 4]; 4] = [
        [51, 55, 58, 62],
        [51, 55, 56, 60],
        [50, 55, 58, 63],
        [50, 53, 56, 60],
    ];
    let pad_voicings: [[u8; 3]; 4] = [[60, 63, 67], [60, 63, 68], [58, 63, 67], [58, 62, 65]];

    let mut bass = create_track(Instrument::Bass);
    bass.name = "Velvet bass".into();
    bass.volume = 0.74;
    bass.effects = Effects {
        cutoff_hz: 2600.0,
        reverb: 0.02,
        delay: 0.0,
        drive: 0.2,
    };
    let mut keys = create_track(Instrument::Piano);
    keys.name = "Afterglow keys".into();
    keys.volume = 0.6;
    keys.pan = -0.18;
    keys.effects = Effects {
        cutoff_hz: 9000.0,
        reverb: 0.26,
        delay: 0.06,
        drive: 0.04,
    };
    let mut pad = create_track(Instrument::Pad);
    pad.name = "Horizon pad".into();
    pad.volume = 0.46;
    pad.pan = 0.06;
    pad.effects = Effects {
        cutoff_hz: 3800.0,
        reverb: 0.42,
        delay: 0.0,
        drive: 0.0,
    };
    for t in [&mut bass, &mut keys, &mut pad] {
        t.patterns.clear();
        t.clips.clear();
    }

    for (i, &r) in roots.iter().enumerate() {
        let start = i as f64 * 32.0;
        let mut bass_notes = vec![
            n(r, 0.0, 5.0, 0.9),
            n(r, 6.0, 1.5, 0.62),
            n(r + 12, 8.0, 3.0, 0.72),
            n(r + 7, 12.0, 3.0, 0.68),
            n(r, 16.0, 5.0, 0.88),
            n(r + 12, 22.0, 1.5, 0.66),
            n(r, 24.0, 3.0, 0.74),
        ];
        if i == 3 {
            // Walk up D - B natural into the C minor downbeat.
            bass_notes.extend([n(38, 28.0, 2.0, 0.68), n(35, 30.0, 2.0, 0.74)]);
        } else {
            bass_notes.push(n(r + 7, 28.0, 2.5, 0.66));
        }
        let bp = pattern(&format!("{} groove", chord_names[i]), 32, bass_notes);
        bass.clips.push(clip(&bp, start));
        bass.patterns.push(bp);

        let voicing = keys_voicings[i];
        let hits: [(f64, f64, f32); 5] = [
            (0.0, 7.0, 0.6),
            (10.0, 2.0, 0.42),
            (14.0, 2.0, 0.48),
            (16.0, 7.0, 0.56),
            (26.0, 5.0, 0.46),
        ];
        let key_notes = hits
            .iter()
            .flat_map(|&(s, d, v)| {
                voicing
                    .iter()
                    .enumerate()
                    .map(move |(vi, &p)| n(p, s, d, v + vi as f32 * 0.03))
            })
            .collect();
        let kp = pattern(chord_names[i], 32, key_notes);
        keys.clips.push(clip(&kp, start));
        keys.patterns.push(kp);

        let pad_notes = pad_voicings[i]
            .iter()
            .map(|&p| n(p, 0.0, 31.5, 0.56))
            .collect();
        let pp = pattern(&format!("{} air", chord_names[i]), 32, pad_notes);
        pad.clips.push(clip(&pp, start));
        pad.patterns.push(pp);
    }

    let mut pluck = create_track(Instrument::Pluck);
    pluck.name = "Glass melody".into();
    pluck.volume = 0.54;
    pluck.pan = 0.22;
    pluck.effects = Effects {
        cutoff_hz: 12000.0,
        reverb: 0.24,
        delay: 0.3,
        drive: 0.0,
    };
    let call = pattern(
        "Call",
        64,
        vec![
            n(79, 0.0, 3.0, 0.66),
            n(75, 4.0, 2.0, 0.54),
            n(77, 6.0, 2.0, 0.56),
            n(79, 8.0, 4.0, 0.64),
            n(82, 14.0, 2.0, 0.6),
            n(84, 16.0, 6.0, 0.7),
            n(82, 24.0, 2.0, 0.56),
            n(79, 26.0, 4.0, 0.6),
            n(80, 32.0, 3.0, 0.66),
            n(79, 36.0, 2.0, 0.54),
            n(75, 38.0, 2.0, 0.52),
            n(72, 40.0, 6.0, 0.62),
            n(75, 50.0, 2.0, 0.5),
            n(77, 52.0, 2.0, 0.54),
            n(79, 54.0, 2.0, 0.58),
            n(80, 56.0, 4.0, 0.62),
            n(79, 60.0, 4.0, 0.52),
        ],
    );
    let answer = pattern(
        "Answer",
        64,
        vec![
            n(82, 0.0, 3.0, 0.68),
            n(79, 4.0, 2.0, 0.54),
            n(82, 6.0, 2.0, 0.58),
            n(86, 8.0, 4.0, 0.7),
            n(84, 14.0, 2.0, 0.6),
            n(82, 16.0, 6.0, 0.66),
            n(79, 24.0, 2.0, 0.54),
            n(75, 26.0, 4.0, 0.58),
            n(77, 32.0, 3.0, 0.64),
            n(74, 36.0, 2.0, 0.52),
            n(77, 38.0, 2.0, 0.56),
            n(82, 40.0, 4.0, 0.66),
            n(79, 46.0, 2.0, 0.52),
            n(77, 48.0, 4.0, 0.6),
            n(74, 52.0, 2.0, 0.5),
            n(75, 54.0, 2.0, 0.54),
            n(74, 56.0, 8.0, 0.6),
        ],
    );
    pluck.clips = vec![clip(&call, 0.0), clip(&answer, 64.0)];
    pluck.patterns = vec![call, answer];

    Project {
        version: PROJECT_VERSION,
        id: next_id(),
        name: "Afterglow".to_string(),
        bpm: 104.0,
        swing: 0.12,
        bars: 8,
        master_volume: 0.8,
        tracks: vec![drums, bass, keys, pluck, pad],
        // Cm9 - Abmaj7 - Ebmaj7 - Bb9 is i - VI - III - VII in C natural minor.
        key: SongKey::new(0, KeyMode::Minor),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_is_valid_and_musical() {
        let p = Project::demo();
        p.validate().unwrap();
        assert_eq!(p.name, "Afterglow");
        assert_eq!(p.bpm, 104.0);
        assert_eq!(p.bars, 8);
        assert_eq!(p.tracks.len(), 5);
        let instruments: HashSet<_> = p.tracks.iter().map(|t| t.instrument).collect();
        assert_eq!(instruments.len(), 5, "demo tracks use distinct instruments");
        for t in &p.tracks {
            assert!(t.patterns.len() >= 2, "{} has variation", t.name);
            for c in &t.clips {
                assert!(
                    t.clip_end(c) <= p.total_steps() + 1e-9,
                    "{} clip overruns the song",
                    t.name
                );
            }
            for pat in &t.patterns {
                assert!(
                    t.clips.iter().any(|c| c.pattern_id == pat.id),
                    "pattern {} unused",
                    pat.name
                );
                for note in &pat.notes {
                    assert!(
                        note.end() <= pat.length as f64 + 1e-9,
                        "{}: note overhangs",
                        pat.name
                    );
                }
            }
        }
        let events = p.events(true);
        // Every clip produces several notes: the demo is a real arrangement, not a sketch.
        for (i, t) in p.tracks.iter().enumerate() {
            let n = events.iter().filter(|e| e.track_index == i).count();
            assert!(
                n >= t.clips.len() * 2,
                "{}: only {n} events over {} clips",
                t.name,
                t.clips.len()
            );
        }
        assert!(events
            .iter()
            .all(|e| e.step >= 0.0 && e.step + e.duration <= p.total_steps() + 1e-9));
        assert!(events.windows(2).all(|w| w[0].step <= w[1].step));
        // Every bar is covered by all five parts; sustained notes count as sounding.
        for bar in 0..8 {
            let range = bar as f64 * 16.0..(bar + 1) as f64 * 16.0;
            let sounding: HashSet<usize> = events
                .iter()
                .filter(|e| e.step < range.end && e.step + e.duration > range.start)
                .map(|e| e.track_index)
                .collect();
            assert_eq!(sounding.len(), p.tracks.len(), "bar {bar} is missing parts");
        }
        let drum_pitches: HashSet<u8> = p.tracks[0]
            .patterns
            .iter()
            .flat_map(|p| p.notes.iter().map(|n| n.pitch))
            .collect();
        assert!(drum_pitches
            .iter()
            .all(|pitch| DRUMS.iter().any(|(d, _)| d == pitch)));
        assert_eq!(drum_pitches.len(), 8, "demo uses the whole kit");
    }

    #[test]
    fn empty_project_has_a_drum_track() {
        let p = Project::empty();
        p.validate().unwrap();
        assert_eq!(p.tracks.len(), 1);
        assert_eq!(p.tracks[0].instrument, Instrument::Drums);
        assert_eq!(p.tracks[0].clips.len(), 1);
        assert!(p.events(true).is_empty());
    }

    #[test]
    fn instruments_are_described() {
        let names: HashSet<_> = Instrument::ALL.iter().map(|i| i.name()).collect();
        assert_eq!(names.len(), Instrument::ALL.len());
        for i in Instrument::ALL {
            assert!(!i.description().is_empty());
            let (lo, hi) = i.pitch_range();
            assert!(lo < hi && (lo..=hi).contains(&i.default_pitch()));
            let t = create_track(i);
            assert_eq!(t.instrument, i);
            assert_eq!(t.patterns.len(), 1);
            assert_eq!(t.clips[0].pattern_id, t.patterns[0].id);
        }
        assert_eq!(DRUMS.map(|d| d.0), [36, 38, 42, 46, 39, 45, 49, 51]);
    }

    #[test]
    fn guitar_is_a_first_class_builtin() {
        // Present in ALL, immediately after Pluck, serialized by lowercase name.
        assert!(Instrument::ALL.contains(&Instrument::Guitar));
        assert_eq!(Instrument::Guitar.index(), Instrument::Pluck.index() + 1);
        let json = serde_json::to_string(&Instrument::Guitar).unwrap();
        assert_eq!(json, "\"guitar\"");
        let back: Instrument = serde_json::from_str(&json).unwrap();
        assert_eq!(back, Instrument::Guitar);
        // A track on the instrument round-trips through a whole Project.
        let mut p = Project::demo();
        p.tracks[0].instrument = Instrument::Guitar;
        let back: Project = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(p, back);
        back.validate().unwrap();
    }

    #[test]
    fn note_names() {
        assert_eq!(note_name(60), "C4");
        assert_eq!(note_name(61), "C#4");
        assert_eq!(note_name(69), "A4");
        assert_eq!(note_name(0), "C-1");
        assert_eq!(note_name(127), "G9");
        assert_eq!(drum_name(38), "Snare");
        assert!((pitch_hz(69.0) - 440.0).abs() < 1e-3);
    }

    /// A two-piece kit preset whose pitches are deliberately out of order:
    /// `drum_rows` must keep the `pieces` array order, not sort by pitch.
    fn kit_preset() -> InstrumentPreset {
        serde_json::from_str(
            r#"{
                "id": "test.kit", "name": "Kit",
                "kit": { "pieces": [
                    { "name": "Hat", "pitch": 42,
                      "voice": { "oscillators": [{"waveform": "noise"}] } },
                    { "name": "Kick", "pitch": 60,
                      "voice": { "oscillators": [{"waveform": "sine"}] } }
                ] }
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn drum_grid_helpers_cover_builtin_melodic_and_kit_tracks() {
        // Built-in drums: the grid shows the eight DRUMS rows.
        let t = create_track(Instrument::Drums);
        assert!(t.shows_drum_grid());
        assert_eq!(t.drum_rows(), DRUMS.to_vec());

        // Melodic built-ins keep the piano roll; drum_rows still falls back
        // to DRUMS (only consulted when the grid is actually shown).
        for inst in [
            Instrument::Piano,
            Instrument::Bass,
            Instrument::Pluck,
            Instrument::Guitar,
            Instrument::Pad,
            Instrument::Lead,
        ] {
            let t = create_track(inst);
            assert!(!t.shows_drum_grid(), "{inst:?}");
            assert_eq!(t.drum_rows(), DRUMS.to_vec(), "{inst:?}");
        }

        // A melodic preset on a Drums track still gets the piano roll.
        let mut t = create_track(Instrument::Drums);
        t.preset = Some(
            crate::packs::template_pack("test-pack", "Test")
                .unwrap()
                .instruments[0]
                .clone(),
        );
        assert!(!t.shows_drum_grid());
        assert_eq!(t.drum_rows(), DRUMS.to_vec());

        // A kit preset on a Drums track gets the grid, one row per piece in
        // pieces order (Hat@42 before Kick@60 here — 60 is outside DRUMS).
        let kit = kit_preset();
        let t = Track::from_preset(Instrument::Drums, kit);
        assert!(t.shows_drum_grid());
        assert_eq!(t.drum_rows(), vec![(42, "Hat"), (60, "Kick")]);

        // A kit preset on a melodic track keeps the piano roll, but
        // drum_rows still reports the kit's pieces.
        let t = Track::from_preset(Instrument::Lead, kit_preset());
        assert!(!t.shows_drum_grid());
        assert_eq!(t.drum_rows(), vec![(42, "Hat"), (60, "Kick")]);

        // An empty piece list (unvalidated data) falls back to DRUMS so the
        // grid can never divide by zero.
        let mut empty = kit_preset();
        empty.kit.as_mut().unwrap().pieces.clear();
        let t = Track::from_preset(Instrument::Drums, empty);
        assert!(t.shows_drum_grid());
        assert_eq!(t.drum_rows(), DRUMS.to_vec());
    }

    #[test]
    fn ids_are_unique_and_reserved() {
        let a = next_id();
        let b = next_id();
        assert!(b > a && a > 0);
        let mut p = Project::empty();
        p.tracks[0].id = b + 1_000_000;
        p.validate().unwrap();
        assert!(next_id() > b + 1_000_000);
    }

    #[test]
    fn json_round_trip() {
        let p = Project::demo();
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("\"instrument\":\"drums\""));
        let back: Project = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
        back.validate().unwrap();
    }

    #[test]
    fn validation_rejects_damage() {
        type Damage = (&'static str, Box<dyn Fn(&mut Project)>);
        let base = Project::demo();
        let cases: Vec<Damage> = vec![
            ("version", Box::new(|p| p.version = 2)),
            ("nan tempo", Box::new(|p| p.bpm = f64::NAN)),
            ("fast tempo", Box::new(|p| p.bpm = 301.0)),
            ("swing", Box::new(|p| p.swing = 0.7)),
            ("bars", Box::new(|p| p.bars = 0)),
            ("master", Box::new(|p| p.master_volume = 1.5)),
            (
                "tracks",
                Box::new(|p| p.tracks = (0..33).map(|_| create_track(Instrument::Pad)).collect()),
            ),
            ("dup track", Box::new(|p| p.tracks[1].id = p.tracks[0].id)),
            ("pan", Box::new(|p| p.tracks[0].pan = -2.0)),
            ("cutoff", Box::new(|p| p.tracks[0].effects.cutoff_hz = 50.0)),
            (
                "reverb nan",
                Box::new(|p| p.tracks[0].effects.reverb = f32::NAN),
            ),
            (
                "pitch",
                Box::new(|p| p.tracks[1].patterns[0].notes[0].pitch = 128),
            ),
            (
                "note pos",
                Box::new(|p| p.tracks[1].patterns[0].notes[0].start = -1.0),
            ),
            (
                "note len",
                Box::new(|p| p.tracks[1].patterns[0].notes[0].duration = 0.0),
            ),
            (
                "velocity",
                Box::new(|p| p.tracks[1].patterns[0].notes[0].velocity = 1.2),
            ),
            (
                "dup note",
                Box::new(|p| {
                    let id = p.tracks[1].patterns[0].notes[0].id;
                    p.tracks[1].patterns[0].notes[1].id = id;
                }),
            ),
            (
                "pattern len",
                Box::new(|p| p.tracks[1].patterns[0].length = 0),
            ),
            (
                "missing pattern",
                Box::new(|p| p.tracks[1].clips[0].pattern_id = 1),
            ),
            (
                "clip pos",
                Box::new(|p| p.tracks[1].clips[0].start = f64::INFINITY),
            ),
            (
                "clip loops zero",
                Box::new(|p| p.tracks[1].clips[0].loops = 0),
            ),
            (
                "clip loops max",
                Box::new(|p| p.tracks[1].clips[0].loops = MAX_CLIP_LOOPS + 1),
            ),
            (
                "clip fade in",
                Box::new(|p| p.tracks[1].clips[0].fade_in = f64::INFINITY),
            ),
            (
                "clip fade in range",
                Box::new(|p| p.tracks[1].clips[0].fade_in = 1e9),
            ),
            (
                "clip fade out",
                Box::new(|p| p.tracks[1].clips[0].fade_out = -1.0),
            ),
            (
                "long name",
                Box::new(|p| p.name = "x".repeat(MAX_NAME_CHARS + 1)),
            ),
            (
                "control name",
                Box::new(|p| p.tracks[0].name = "a\u{0}b".into()),
            ),
        ];
        for (label, damage) in cases {
            let mut p = base.clone();
            damage(&mut p);
            assert!(p.validate().is_err(), "{label} should be rejected");
            p.normalize();
            p.validate()
                .unwrap_or_else(|e| panic!("normalize did not repair {label}: {e}"));
        }
    }

    #[test]
    fn validation_tolerates_shortened_song_and_patterns() {
        let mut p = Project::demo();
        p.bars = 2;
        p.tracks[1].patterns[0].length = 4;
        p.validate().unwrap();
        let events = p.events(true);
        assert!(events.iter().all(|e| e.step + e.duration <= 32.0 + 1e-9));
        assert!(events
            .iter()
            .filter(|e| e.track_index == 1)
            .all(|e| e.step < 4.0 || e.step >= 32.0));
    }

    #[test]
    fn mute_and_solo_semantics() {
        let mut p = Project::demo();
        let all = p.events(false).len();
        assert_eq!(p.events(true).len(), all);
        p.tracks[0].mute = true;
        assert!(p.events(true).iter().all(|e| e.track_index != 0));
        assert_eq!(p.events(false).len(), all);
        p.tracks[2].solo = true;
        assert!(p.events(true).iter().all(|e| e.track_index == 2));
        p.tracks[0].solo = true; // muted beats soloed
        assert!(p.events(true).iter().all(|e| e.track_index == 2));
        assert!(!p.is_track_audible(0) && p.is_track_audible(2) && !p.is_track_audible(1));
    }

    #[test]
    fn events_clip_to_pattern_and_song() {
        let mut p = Project::empty();
        p.bars = 1;
        let t = &mut p.tracks[0];
        t.patterns[0].notes = vec![n(36, 0.0, 30.0, 1.0), n(38, 12.0, 1.0, 1.0)];
        t.clips.push(Clip {
            id: next_id(),
            pattern_id: t.patterns[0].id,
            start: 8.0,
            loops: 1,
            fade_in: 0.0,
            fade_out: 0.0,
        });
        let ev = p.events(true);
        // Clip at 0: kick 0..16 (pattern length), snare 12. Clip at 8: kick 8..16 (song end), snare at 20 dropped.
        assert_eq!(ev.len(), 3);
        assert_eq!((ev[0].step, ev[0].duration), (0.0, 16.0));
        assert_eq!((ev[1].step, ev[1].duration), (8.0, 8.0));
        assert_eq!(ev[2].step, 12.0);
    }

    #[test]
    fn clip_loops_repeat_the_pattern_across_the_span() {
        let mut p = Project::empty();
        p.bars = 8;
        let t = &mut p.tracks[0];
        t.patterns[0].notes = vec![n(36, 0.0, 1.0, 1.0), n(38, 8.0, 1.0, 1.0)];
        t.clips[0].loops = 4;
        assert_eq!(t.clip_end(&t.clips[0]), 64.0);
        assert_eq!(t.clips[0].span(16.0), 64.0);
        let ev = p.events(true);
        assert_eq!(ev.len(), 8);
        let steps: Vec<u64> = ev.iter().map(|e| e.step as u64).collect();
        assert_eq!(steps, [0, 8, 16, 24, 32, 40, 48, 56]);
        for (i, e) in ev.iter().enumerate() {
            assert_eq!(e.pitch, if i % 2 == 0 { 36 } else { 38 });
            assert_eq!(e.velocity, 1.0);
        }
        // The loop count is defensive on unvalidated data: 0 still plays once.
        p.tracks[0].clips[0].loops = 0;
        assert_eq!(p.tracks[0].clips[0].loop_count(), 1);
        assert_eq!(p.events(true).len(), 2);
    }

    #[test]
    fn clip_fades_ramp_velocity_once_across_the_whole_span() {
        let mut p = Project::empty();
        p.bars = 8;
        let t = &mut p.tracks[0];
        t.patterns[0].notes = vec![
            n(60, 0.0, 1.0, 1.0),
            n(62, 4.0, 1.0, 1.0),
            n(64, 8.0, 1.0, 1.0),
            n(65, 12.0, 1.0, 1.0),
        ];
        t.clips[0].fade_in = 8.0;
        t.clips[0].fade_out = 8.0;
        // span = 16: gain is pos/8 ramping in and (16-pos)/8 ramping out.
        let velocity = |pitch: u8| {
            p.events(true)
                .iter()
                .find(|e| e.pitch == pitch)
                .unwrap()
                .velocity
        };
        assert_eq!(velocity(60), 0.0, "pos 0 is the bottom of the fade-in");
        assert!(
            (velocity(62) - 0.5).abs() < 1e-6,
            "pos 4 is halfway through the fade-in"
        );
        assert_eq!(velocity(64), 1.0, "pos 8 is past both ramps");
        assert!(
            (velocity(65) - 0.5).abs() < 1e-6,
            "pos 12 is halfway through the fade-out"
        );
    }

    #[test]
    fn clip_fades_span_all_repetitions_not_each_one() {
        let mut p = Project::empty();
        p.bars = 8;
        let t = &mut p.tracks[0];
        t.patterns[0].notes = vec![n(60, 0.0, 1.0, 1.0), n(62, 15.0, 1.0, 1.0)];
        t.clips[0].loops = 2;
        t.clips[0].fade_in = 4.0;
        t.clips[0].fade_out = 4.0;
        // span = 32: the fade-in only touches positions 0..4 of repetition 1 and
        // the fade-out only touches positions 28..32 of repetition 2.
        let ev = p.events(true);
        assert_eq!(ev.len(), 4);
        let at = |step: f64| ev.iter().find(|e| e.step == step).unwrap();
        assert_eq!(at(0.0).velocity, 0.0, "start of the first repetition");
        assert_eq!(
            at(16.0).velocity,
            1.0,
            "repetition 2 does not fade in again"
        );
        assert_eq!(
            at(15.0).velocity,
            1.0,
            "end of rep 1 is not in the fade-out"
        );
        assert_eq!(at(31.0).velocity, 0.25, "(32-31)/4 into the fade-out");
    }

    #[test]
    fn swing_is_monotonic_and_anchored() {
        for swing in [0.0, 0.12, 0.5, 0.6] {
            let mut last = -1.0;
            for i in 0..400 {
                let s = i as f64 * 0.05;
                let w = swung_step(s, swing);
                assert!(w > last, "monotonic at {s} swing {swing}");
                last = w;
            }
            for even in [0.0, 2.0, 4.0, 16.0] {
                assert!((swung_step(even, swing) - even).abs() < 1e-12);
            }
        }
        assert!((swung_step(1.0, 0.5) - 4.0 / 3.0).abs() < 1e-12);
        assert_eq!(swung_step(1.0, 0.0), 1.0);
    }

    #[test]
    fn pattern_length_trims() {
        let mut p = create_pattern(Instrument::Piano, 32);
        p.notes = vec![n(60, 0.0, 20.0, 0.8), n(62, 20.0, 2.0, 0.8)];
        p.set_length(16);
        assert_eq!(p.notes.len(), 1);
        assert_eq!(p.notes[0].duration, 16.0);
        let d = p.duplicate("Copy");
        assert_ne!(d.id, p.id);
        assert_ne!(d.notes[0].id, p.notes[0].id);
    }

    #[test]
    fn drum_mapping_targets_kit() {
        for pitch in 0..=127u8 {
            let m = map_drum_pitch(pitch);
            assert!(DRUMS.iter().any(|(d, _)| *d == m));
        }
        for (d, _) in DRUMS {
            assert_eq!(map_drum_pitch(d), d);
        }
    }

    #[test]
    fn preset_round_trips_and_validates() {
        let pack = crate::packs::template_pack("unit-pack", "Unit Pack").unwrap();
        let preset = pack.instruments[0].clone();
        let mut p = Project::empty();
        p.tracks[0] = Track::from_preset(Instrument::Lead, preset.clone());
        assert_eq!(p.tracks[0].name, preset.name);
        assert_eq!(p.tracks[0].sound_name(), preset.name);
        p.validate().unwrap();
        let json = serde_json::to_string(&p).unwrap();
        assert!(
            json.contains(&format!("\"id\":\"{}\"", preset.id)),
            "preset embedded in song JSON"
        );
        let back: Project = serde_json::from_str(&json).unwrap();
        assert_eq!(back.tracks[0].preset, Some(preset));
        back.validate().unwrap();

        // Songs saved before packs existed still load (preset is optional).
        let legacy = r#"{"version":1,"id":1,"name":"Old","bpm":110,"swing":0,"bars":8,"master_volume":0.8,"tracks":[{"id":2,"name":"Drums","color":[255,132,104],"instrument":"drums","volume":0.8,"pan":0,"mute":false,"solo":false,"effects":{"cutoff_hz":20000,"reverb":0,"delay":0,"drive":0},"patterns":[{"id":3,"name":"Beat 1","length":16,"notes":[]}],"clips":[]}]}"#;
        let old: Project = serde_json::from_str(legacy).unwrap();
        assert_eq!(old.tracks[0].preset, None);
        old.validate().unwrap();

        // A damaged preset is rejected by validate and repaired to built-in by normalize.
        p.tracks[0].preset = Some(InstrumentPreset {
            id: "BAD ID".into(),
            name: String::new(),
            description: String::new(),
            oscillators: vec![],
            envelope: Default::default(),
            pitch_envelope: None,
            filter_envelope: None,
            filter_cutoff: f32::NAN,
            filter_resonance: 0.0,
            lfo: None,
            bit_depth: 99,
            sample_hold: 0,
            gain: 3.0,
            monophonic: false,
            audition_note: 60,
            audition_duration: 0.8,
            kit: None,
        });
        assert!(p.validate().unwrap_err().contains("preset"));
        p.normalize();
        assert_eq!(p.tracks[0].preset, None);
        p.validate().unwrap();
    }

    #[test]
    fn font_refs_round_trip_and_validate() {
        let mut p = Project::empty();
        p.tracks[0] = Track::from_font(Instrument::Piano, FontRef::new("GeneralUser", 0));
        assert_eq!(
            p.tracks[0].font,
            Some(FontRef::new("GeneralUser", 0)),
            "from_font sets the reference"
        );
        assert_eq!(p.tracks[0].sound_name(), "Acoustic Grand Piano");
        assert_eq!(
            p.tracks[0].font.as_ref().unwrap().label(),
            "GeneralUser · Acoustic Grand Piano"
        );
        assert_eq!(
            p.tracks[0].font.as_ref().unwrap().key(),
            crate::font::font_key("GeneralUser")
        );
        p.validate().unwrap();
        let json = serde_json::to_string(&p).unwrap();
        assert!(
            json.contains(r#""font":{"name":"GeneralUser","program":0}"#),
            "font embedded in song JSON: {json}"
        );
        let back: Project = serde_json::from_str(&json).unwrap();
        assert_eq!(back.tracks[0].font, p.tracks[0].font);
        back.validate().unwrap();

        // Songs saved before fonts existed have no `font` key and load as None.
        let legacy = r#"{"version":1,"id":1,"name":"Old","bpm":110,"swing":0,"bars":8,"master_volume":0.8,"tracks":[{"id":2,"name":"Drums","color":[255,132,104],"instrument":"drums","volume":0.8,"pan":0,"mute":false,"solo":false,"effects":{"cutoff_hz":20000,"reverb":0,"delay":0,"drive":0},"patterns":[{"id":3,"name":"Beat 1","length":16,"notes":[]}],"clips":[]}]}"#;
        let old: Project = serde_json::from_str(legacy).unwrap();
        assert_eq!(old.tracks[0].font, None);
        old.validate().unwrap();
        // And a font-free track serializes without the key at all.
        let json = serde_json::to_string(&old).unwrap();
        assert!(!json.contains("\"font\""), "{json}");
    }

    #[test]
    fn font_validation_rejects_and_normalize_repairs() {
        let mut p = Project::empty();
        // Empty/whitespace names are rejected.
        p.tracks[0].font = Some(FontRef::new("   ", 0));
        assert!(p.validate().unwrap_err().contains("font"));
        // Over-long names are rejected.
        p.tracks[0].font = Some(FontRef::new("x".repeat(MAX_NAME_CHARS + 1), 0));
        assert!(p.validate().unwrap_err().contains("font"));
        // Programs above 127 are rejected (u8 can hold up to 255).
        p.tracks[0].font = Some(FontRef::new("GeneralUser", 200));
        assert!(p.validate().unwrap_err().contains("font"));

        // Normalize repairs like other names: trims edges, clamps the program.
        p.tracks[0].font = Some(FontRef::new("  GeneralUser  ", 200));
        p.normalize();
        assert_eq!(
            p.tracks[0].font,
            Some(FontRef::new("GeneralUser", 127)),
            "normalize trims and clamps"
        );
        p.validate().unwrap();
        // A name that repairs to nothing drops the whole reference.
        p.tracks[0].font = Some(FontRef::new(" \n\t ", 0));
        p.normalize();
        assert_eq!(p.tracks[0].font, None);
        p.validate().unwrap();
    }

    #[test]
    fn legacy_clips_without_loop_fields_load_with_defaults() {
        // Songs saved before clip loops/fades existed have no `loops`, `fade_in`
        // or `fade_out` keys on their clips; serde defaults must fill them in.
        let legacy = r#"{"version":1,"id":1,"name":"Old","bpm":110,"swing":0,"bars":8,"master_volume":0.8,"tracks":[{"id":2,"name":"Keys","color":[255,132,104],"instrument":"piano","volume":0.8,"pan":0,"mute":false,"solo":false,"effects":{"cutoff_hz":20000,"reverb":0,"delay":0,"drive":0},"patterns":[{"id":3,"name":"Pattern 1","length":16,"notes":[{"id":4,"pitch":60,"start":0,"duration":4,"velocity":0.8}]}],"clips":[{"id":5,"pattern_id":3,"start":0}]}]}"#;
        let p: Project = serde_json::from_str(legacy).unwrap();
        let clip = &p.tracks[0].clips[0];
        assert_eq!(clip.loops, 1);
        assert_eq!(clip.fade_in, 0.0);
        assert_eq!(clip.fade_out, 0.0);
        assert_eq!(clip.loop_count(), 1);
        assert_eq!(clip.span(16.0), 16.0);
        p.validate().unwrap();
        assert_eq!(p.events(false).len(), 1);
    }

    #[test]
    fn song_key_defaults_names_and_round_trips() {
        assert_eq!(SongKey::default(), SongKey::new(0, KeyMode::Major));
        assert_eq!(Project::empty().key, SongKey::default());
        assert_eq!(SongKey::default().name(), "C major");
        assert_eq!(SongKey::new(9, KeyMode::Minor).name(), "A minor");
        assert_eq!(SongKey::new(6, KeyMode::Major).name(), "F# major");
        assert_eq!(SongKey::new(21, KeyMode::Major).root, 9, "new() wraps");
        // The demo (Cm9 - Abmaj7 - Ebmaj7 - Bb9) is in C minor.
        assert_eq!(Project::demo().key, SongKey::new(0, KeyMode::Minor));

        let mut p = Project::empty();
        p.key = SongKey::new(4, KeyMode::Minor);
        let json = serde_json::to_string(&p).unwrap();
        assert!(
            json.contains(r#""key":{"root":4,"mode":"minor"}"#),
            "{json}"
        );
        let back: Project = serde_json::from_str(&json).unwrap();
        assert_eq!(back.key, p.key);
        assert_eq!(back, p);
    }

    #[test]
    fn projects_without_a_key_load_as_c_major() {
        let legacy = r#"{"version":1,"id":1,"name":"Old","bpm":110,"swing":0,"bars":8,"master_volume":0.8,"tracks":[]}"#;
        let p: Project = serde_json::from_str(legacy).unwrap();
        assert_eq!(
            p.key,
            SongKey {
                root: 0,
                mode: KeyMode::Major
            }
        );
        p.validate().unwrap();
    }

    #[test]
    fn validation_rejects_and_normalize_repairs_bad_key_root() {
        let mut p = Project::empty();
        p.key.root = 12;
        assert!(p.validate().unwrap_err().contains("key"));
        p.normalize();
        assert_eq!(p.key.root, 0);
        p.validate().unwrap();
        let bad: Result<Project, _> = serde_json::from_str(
            r#"{"version":1,"id":1,"name":"x","bpm":110,"swing":0,"bars":8,"master_volume":0.8,"tracks":[],"key":{"root":1,"mode":"lydian"}}"#,
        );
        assert!(bad.is_err(), "unknown modes are rejected by serde");
    }

    #[test]
    fn scale_tables() {
        assert_eq!(KeyMode::Major.semitones(), [0, 2, 4, 5, 7, 9, 11]);
        assert_eq!(KeyMode::Minor.semitones(), [0, 2, 3, 5, 7, 8, 10]);
    }

    #[test]
    fn diatonic_chords_of_c_major_and_a_minor() {
        let c = SongKey::new(0, KeyMode::Major);
        let labels: Vec<String> = (0..7)
            .map(|d| {
                format!(
                    "{} {}",
                    c.degree_numeral(d),
                    c.degree_chord_quality(d).symbol(c.degree_root(d))
                )
            })
            .collect();
        assert_eq!(
            labels,
            [
                "I C",
                "ii Dm",
                "iii Em",
                "IV F",
                "V G",
                "vi Am",
                "vii° Bdim"
            ]
        );
        let a = SongKey::new(9, KeyMode::Minor);
        let labels: Vec<String> = (0..7)
            .map(|d| {
                format!(
                    "{} {}",
                    a.degree_numeral(d),
                    a.degree_chord_quality(d).symbol(a.degree_root(d))
                )
            })
            .collect();
        assert_eq!(
            labels,
            [
                "i Am",
                "ii° Bdim",
                "III C",
                "iv Dm",
                "v Em",
                "VI F",
                "VII G"
            ]
        );
    }

    #[test]
    fn chord_intervals_are_ascending_and_rooted() {
        use ChordQuality as Q;
        assert_eq!(Q::Major.intervals(), &[0, 4, 7]);
        assert_eq!(Q::Minor.intervals(), &[0, 3, 7]);
        assert_eq!(Q::Diminished.intervals(), &[0, 3, 6]);
        assert_eq!(Q::Augmented.intervals(), &[0, 4, 8]);
        assert_eq!(Q::Dominant7.intervals(), &[0, 4, 7, 10]);
        assert_eq!(Q::Major7.intervals(), &[0, 4, 7, 11]);
        assert_eq!(Q::Minor7.intervals(), &[0, 3, 7, 10]);
        assert_eq!(Q::HalfDiminished7.intervals(), &[0, 3, 6, 10]);
        assert_eq!(Q::Diminished7.intervals(), &[0, 3, 6, 9]);
        assert_eq!(Q::Minor9.intervals(), &[0, 3, 7, 10, 14]);
        let mut labels = HashSet::new();
        for q in Q::ALL {
            let iv = q.intervals();
            assert_eq!(iv[0], 0, "{q:?} starts on the root");
            assert!(iv.windows(2).all(|w| w[0] < w[1]), "{q:?} ascending");
            assert!(labels.insert(q.label()), "{q:?} label unique");
        }
        assert_eq!(Q::Minor7.symbol(9), "Am7");
    }
}
