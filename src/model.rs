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
/// Most imported-audio clips a single track can hold (the pattern-clip cap
/// applies to `Track::clips` only).
pub const MAX_AUDIO_CLIPS_PER_TRACK: usize = 256;
/// Largest source offset (seconds) an audio clip may carry — the bound on
/// `AudioClip::offset`, not on the file itself (the audio store caps file
/// bytes in `crate::persistence`).
pub const MAX_AUDIO_SECONDS: f64 = 600.0;
/// Shortest timeline length (steps) an audio clip may carry; smaller clips
/// are unreliably thin to grab and paint.
pub const MIN_AUDIO_CLIP_STEPS: f64 = 0.01;
pub const MAX_NOTES_PER_PATTERN: usize = 8192;
/// Longest allowed pattern (and furthest allowed position) in steps.
pub const MAX_PATTERN_STEPS: u32 = MAX_BARS * STEPS_PER_BAR;
/// Shortest note, a sixty-fourth note.
pub const MIN_NOTE_DURATION: f64 = 0.0625;
pub const MAX_NAME_CHARS: usize = 160;
/// Upper bound on flattened note events per song, keeping playback and export bounded.
pub const MAX_SONG_EVENTS: usize = 200_000;
/// Most section markers a song can carry on the arranger ruler.
pub const MAX_MARKERS: usize = 64;
/// Most per-bar chord events a song can carry on the arranger's chord strip.
pub const MAX_CHORD_EVENTS: usize = 64;
pub const MIN_CUTOFF_HZ: f32 = 100.0;
pub const MAX_CUTOFF_HZ: f32 = 20_000.0;
/// Longest per-track glide (portamento) slide time, in seconds.
pub const MAX_GLIDE_SECONDS: f32 = 2.0;
/// Most points a single track automation lane (volume or pan) can carry.
pub const MAX_AUTOMATION_POINTS: usize = 256;
/// Highest value a volume-automation point can hold. The lane multiplies the
/// fader gain, so 1.5 allows boosting up to +50% over the mixed level.
pub const MAX_AUTOMATION_GAIN: f32 = 1.5;
/// Most octaves an arpeggio may span — shared by the pattern arpeggio
/// generator and the per-track live arpeggiator.
pub const MAX_ARP_OCTAVES: u32 = 3;
/// Shortest gate (hit length as a fraction of the hit rate) the live
/// arpeggiator accepts.
pub const MIN_ARP_GATE: f32 = 0.1;
/// Number of fixed subgroup buses every song has. `Track::bus` selects one
/// (`1..=NUM_BUSES`) or routes straight to the master (`0`); there is no
/// general routing matrix.
pub const NUM_BUSES: usize = 4;
/// Display letters of the subgroup buses, A through D.
pub const BUS_LETTERS: [char; NUM_BUSES] = ['A', 'B', 'C', 'D'];

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

// Reuse Vec/String buffers when refreshing the undo gesture scratch copy.
// The exhaustive clone constructor makes new model fields explicit here.
macro_rules! snapshot_clone {
    ($ty:ident { $($field:ident),+ $(,)? }) => {
        impl Clone for $ty {
            fn clone(&self) -> Self {
                Self { $($field: Clone::clone(&self.$field)),+ }
            }

            fn clone_from(&mut self, source: &Self) {
                $(self.$field.clone_from(&source.$field);)+
            }
        }
    };
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct Pattern {
    pub id: u64,
    pub name: String,
    /// Length in steps.
    pub length: u32,
    pub notes: Vec<Note>,
}

snapshot_clone!(Pattern {
    id,
    name,
    length,
    notes
});

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

/// An imported WAV placed on the arrangement timeline
/// ([`Track::audio_clips`]).
///
/// Unlike [`Clip`] this references an audio file staged into the app's data
/// store by `crate::persistence::stage_audio_file` — the project records
/// only the store-relative file name, never the samples, so `.cadence`
/// files stay small and readable. Existence is deliberately *not* part of
/// the contract: a missing staged file leaves a perfectly valid project
/// whose clip simply plays silence (validation checks the reference's
/// *shape* via [`audio_ref_name_ok`], never the disk).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AudioClip {
    /// Forgiving for hand-edited files: a missing id loads as 0 and is
    /// re-issued by [`Project::normalize`] when it collides.
    #[serde(default)]
    pub id: u64,
    /// Name of the staged file inside the audio store — a bare filename
    /// such as `"a1b2c3d4e5f60718.wav"`, never a path. Must satisfy
    /// [`audio_ref_name_ok`].
    #[serde(default)]
    pub file: String,
    /// Display label, usually the original file's name.
    #[serde(default)]
    pub name: String,
    /// Start in steps from the beginning of the song.
    #[serde(default)]
    pub start: f64,
    /// Steps one repetition occupies on the timeline. Staged at import from
    /// the source duration (`seconds / seconds_per_step`); the clip always
    /// plays the file at its own rate — there is no time-stretch — so
    /// changing BPM after import shifts the audio against the grid
    /// (documented limitation of the MVP).
    #[serde(default)]
    pub length: f64,
    /// Seconds into the source where playback starts,
    /// `0..=MAX_AUDIO_SECONDS` after validation.
    #[serde(default)]
    pub offset: f64,
    /// Repetitions on the timeline. 1 = plays once (default). Each
    /// repetition restarts the source segment at `offset`.
    #[serde(default = "default_clip_loops")]
    pub loops: u32,
    /// Fade-in length in steps at the clip start (gain ramp). 0 = off.
    #[serde(default)]
    pub fade_in: f64,
    /// Fade-out length in steps at the clip end (gain ramp). 0 = off.
    #[serde(default)]
    pub fade_out: f64,
}

impl AudioClip {
    /// A clip for staged file `file` starting at `start` steps, `length`
    /// steps long per repetition, labelled `name`.
    pub fn new(file: impl Into<String>, name: impl Into<String>, start: f64, length: f64) -> Self {
        AudioClip {
            id: next_id(),
            file: file.into(),
            name: name.into(),
            start,
            length,
            offset: 0.0,
            loops: 1,
            fade_in: 0.0,
            fade_out: 0.0,
        }
    }

    /// Loop repetitions clamped into `1..=MAX_CLIP_LOOPS`, safe on unvalidated data.
    pub fn loop_count(&self) -> u32 {
        self.loops.clamp(1, MAX_CLIP_LOOPS)
    }

    /// Total steps the clip spans on the timeline: `length` times the loop
    /// count. Non-finite or negative lengths read as a zero span.
    pub fn span(&self) -> f64 {
        if self.length.is_finite() {
            self.length.max(0.0) * self.loop_count() as f64
        } else {
            0.0
        }
    }
}

/// Whether `name` is a safe audio-store reference for [`AudioClip::file`]:
/// a non-empty bare filename — ASCII alphanumerics plus `.`, `_`, `-`, no
/// path separators and no `..` — so it can never address anything outside
/// the audio store. Staged names are `<content-hash>.wav`; references that
/// fail this check are dropped by [`Project::normalize`] and skipped by the
/// player.
pub fn audio_ref_name_ok(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME_CHARS
        && name != "."
        && !name.contains("..")
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// `skip_serializing_if` for plain `bool` fields that default to `false` —
/// keeps the key out of saved songs unless the flag is actually set.
fn is_false(v: &bool) -> bool {
    !*v
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

/// Neutral starting point for new tracks and the mixer's double-click reset:
/// a fully dry send. Never used when loading songs — every saved `effects`
/// block carries its own `reverb` — so changing it only affects new tracks.
impl Default for Effects {
    fn default() -> Self {
        Effects {
            cutoff_hz: MAX_CUTOFF_HZ,
            reverb: 0.0,
            delay: 0.0,
            drive: 0.0,
        }
    }
}

/// Named one-click effect presets for the track FX panel. Every value is
/// inside the same ranges [`Project::validate`] enforces for [`Effects`].
pub const FX_PRESETS: &[(&str, Effects)] = &[
    (
        "Clean",
        Effects {
            cutoff_hz: MAX_CUTOFF_HZ,
            reverb: 0.15,
            delay: 0.0,
            drive: 0.0,
        },
    ),
    (
        "Warm Tape",
        Effects {
            cutoff_hz: 8_000.0,
            reverb: 0.15,
            delay: 0.05,
            drive: 0.35,
        },
    ),
    (
        "Phone",
        Effects {
            cutoff_hz: 1_200.0,
            reverb: 0.0,
            delay: 0.0,
            drive: 0.15,
        },
    ),
    (
        "Dub Delay",
        Effects {
            cutoff_hz: MAX_CUTOFF_HZ,
            reverb: 0.3,
            delay: 0.55,
            drive: 0.0,
        },
    ),
    (
        "Lo-fi",
        Effects {
            cutoff_hz: 4_000.0,
            reverb: 0.1,
            delay: 0.1,
            drive: 0.5,
        },
    ),
    (
        "Ambient",
        Effects {
            cutoff_hz: 12_000.0,
            reverb: 0.45,
            delay: 0.2,
            drive: 0.0,
        },
    ),
];

/// Serde default for [`Bus::name`] on hand-edited files missing the key;
/// index-aware names ("Bus A"..) come from [`bus_name`].
fn default_bus_name() -> String {
    "Bus".to_string()
}

/// Serde default for [`Bus::volume`]: 0.8, the same fader position the
/// master and fader double-click resets use.
fn default_bus_volume() -> f32 {
    0.8
}

/// One of the fixed subgroup buses tracks can be routed onto (see
/// [`Track::bus`]). A bus is a second mix stage between its member
/// tracks' channel strips and the master: its own smoothed fader/pan plus
/// a group mute/solo that gates the whole group (see
/// [`Project::is_bus_audible`]). Deliberately lean — no per-bus effects:
/// the renderer still runs each bus through a full channel strip with the
/// sends, drive and filter parked neutral, so an `effects` field added
/// later would need no DSP changes.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Bus {
    #[serde(default = "default_bus_name")]
    pub name: String,
    /// Fader position 0..=1 (same perceptual curve as track faders).
    #[serde(default = "default_bus_volume")]
    pub volume: f32,
    /// -1 (left) ..= 1 (right).
    #[serde(default)]
    pub pan: f32,
    #[serde(default)]
    pub mute: bool,
    #[serde(default)]
    pub solo: bool,
}

impl Default for Bus {
    fn default() -> Self {
        Bus {
            name: default_bus_name(),
            volume: default_bus_volume(),
            pan: 0.0,
            mute: false,
            solo: false,
        }
    }
}

/// Display/fallback name of the bus at `index` ("Bus A" .. "Bus D").
pub fn bus_name(index: usize) -> String {
    format!("Bus {}", BUS_LETTERS[index % NUM_BUSES])
}

/// The four default subgroup buses, "Bus A" through "Bus D".
pub fn default_buses() -> [Bus; NUM_BUSES] {
    std::array::from_fn(|i| Bus {
        name: bus_name(i),
        ..Bus::default()
    })
}

/// `skip_serializing_if` for [`Project::buses`]: untouched buses keep the
/// `buses` key out of saved files, like `markers`/`chord_events`.
fn buses_are_default(buses: &[Bus; NUM_BUSES]) -> bool {
    *buses == default_buses()
}

/// `skip_serializing_if` for [`Track::bus`]: master-routed tracks keep the
/// `bus` key out of saved files.
fn is_master_bus(bus: &u8) -> bool {
    *bus == 0
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

/// One breakpoint on a track's automation lane. `step` is a song position in
/// sixteenth-note steps; `value` is lane-specific: a fader multiplier on the
/// volume lane (0..=[`MAX_AUTOMATION_GAIN`], 1 = as mixed) or a signed offset
/// on the pan lane (-1..=1, added to [`Track::pan`]).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct AutomationPoint {
    pub step: f64,
    pub value: f32,
}

/// Selects one of a track's automation lanes.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum AutomationLane {
    /// The volume lane: multiplies `Track::volume` (and so the fader gain).
    Volume,
    /// The pan lane: offsets `Track::pan`.
    Pan,
}

impl AutomationLane {
    pub const ALL: [AutomationLane; 2] = [AutomationLane::Volume, AutomationLane::Pan];

    /// Lowercase name used in validation messages ("volume"/"pan").
    pub fn label(self) -> &'static str {
        match self {
            AutomationLane::Volume => "volume",
            AutomationLane::Pan => "pan",
        }
    }
}

/// Mixer automation on a track: optional breakpoint lanes, each kept sorted
/// by `step` (equal steps are allowed — they draw a jump). Automation
/// modifies the static mix rather than replacing it — the volume lane
/// multiplies the fader, the pan lane offsets the knob — so deleting every
/// point returns the track to its mixed values, which is also why an empty
/// [`Automation`] is equivalent to `None`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Automation {
    /// Fader-multiplier lane (`AutomationLane::Volume`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub volume: Vec<AutomationPoint>,
    /// Pan-offset lane (`AutomationLane::Pan`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pan: Vec<AutomationPoint>,
}

impl Automation {
    /// The points of `lane`.
    pub fn lane(&self, lane: AutomationLane) -> &Vec<AutomationPoint> {
        match lane {
            AutomationLane::Volume => &self.volume,
            AutomationLane::Pan => &self.pan,
        }
    }

    /// The points of `lane`, mutably.
    pub fn lane_mut(&mut self, lane: AutomationLane) -> &mut Vec<AutomationPoint> {
        match lane {
            AutomationLane::Volume => &mut self.volume,
            AutomationLane::Pan => &mut self.pan,
        }
    }

    /// Valid `value` range of `lane`: volume multipliers run
    /// 0..=[`MAX_AUTOMATION_GAIN`], pan offsets -1..=1.
    pub fn value_range(lane: AutomationLane) -> (f32, f32) {
        match lane {
            AutomationLane::Volume => (0.0, MAX_AUTOMATION_GAIN),
            AutomationLane::Pan => (-1.0, 1.0),
        }
    }

    /// The no-op value of `lane`: unity fader multiplier, zero pan offset.
    pub fn neutral(lane: AutomationLane) -> f32 {
        match lane {
            AutomationLane::Volume => 1.0,
            AutomationLane::Pan => 0.0,
        }
    }

    /// True when neither lane carries a point — equivalent to no automation.
    pub fn is_empty(&self) -> bool {
        self.volume.is_empty() && self.pan.is_empty()
    }

    /// The lane's interpolated value at `step`, or `None` when it is empty.
    pub fn value_at(&self, lane: AutomationLane, step: f64) -> Option<f32> {
        automation_value(self.lane(lane), step)
    }

    /// Effective fader multiplier at `step` (1 when the volume lane is empty).
    pub fn volume_at(&self, step: f64) -> f32 {
        self.value_at(AutomationLane::Volume, step).unwrap_or(1.0)
    }

    /// Effective pan offset at `step` (0 when the pan lane is empty).
    pub fn pan_at(&self, step: f64) -> f32 {
        self.value_at(AutomationLane::Pan, step).unwrap_or(0.0)
    }
}

/// Piecewise-linear value of a sorted automation lane at `step`: flat at the
/// first point's value before it and the last point's value after it. `None`
/// for an empty lane — callers then keep the un-automated parameter. On
/// duplicate steps the later point wins (a jump). Robust to unvalidated
/// data: non-finite `step` reads as the first point's value.
pub fn automation_value(points: &[AutomationPoint], step: f64) -> Option<f32> {
    let first = points.first()?;
    let last = points.last()?;
    if !step.is_finite() || step < first.step {
        return Some(first.value);
    }
    if step >= last.step {
        return Some(last.value);
    }
    // `i` is the index of the first point right of `step`; `i - 1` is the
    // last point at or before it (never 0 or past the end: `step` sits
    // between `first.step` and `last.step`, so `partition_point` yields
    // 1..=len-1). With duplicate steps at `step`, `i - 1` is the last of
    // them — the later point wins.
    let i = points.partition_point(|p| p.step <= step);
    let (a, b) = (points[i - 1], points[i]);
    let span = b.step - a.step;
    if span <= 0.0 {
        return Some(b.value);
    }
    let t = ((step - a.step) / span).clamp(0.0, 1.0) as f32;
    Some(a.value + (b.value - a.value) * t)
}

/// Steps between hits for the per-track live arpeggiator (positions are
/// sixteenth-note steps: 4 steps = a quarter note).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ArpRate {
    /// Quarter notes — a hit every 4 steps.
    Quarter,
    /// Eighth notes — every 2 steps.
    Eighth,
    /// Sixteenth notes — every step.
    #[default]
    Sixteenth,
    /// Thirty-second notes — every half step.
    ThirtySecond,
}

impl ArpRate {
    pub const ALL: [ArpRate; 4] = [
        ArpRate::Quarter,
        ArpRate::Eighth,
        ArpRate::Sixteenth,
        ArpRate::ThirtySecond,
    ];

    /// Steps between hits: 4, 2, 1 or 0.5.
    pub fn steps(self) -> f64 {
        match self {
            ArpRate::Quarter => 4.0,
            ArpRate::Eighth => 2.0,
            ArpRate::Sixteenth => 1.0,
            ArpRate::ThirtySecond => 0.5,
        }
    }

    /// Compact note-value label ("1/16").
    pub fn label(self) -> &'static str {
        match self {
            ArpRate::Quarter => "1/4",
            ArpRate::Eighth => "1/8",
            ArpRate::Sixteenth => "1/16",
            ArpRate::ThirtySecond => "1/32",
        }
    }

    /// Spelled-out note name for hints and status messages.
    pub fn note_name(self) -> &'static str {
        match self {
            ArpRate::Quarter => "quarter",
            ArpRate::Eighth => "eighth",
            ArpRate::Sixteenth => "sixteenth",
            ArpRate::ThirtySecond => "thirty-second",
        }
    }
}

/// Order in which an arpeggiator cycles a chord's tones — the vocabulary
/// shared by the pattern arpeggio generator and the per-track live
/// arpeggiator ([`ArpSettings`]).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ArpDirection {
    /// Lowest to highest, looping.
    #[default]
    Up,
    /// Highest to lowest, looping.
    Down,
    /// Up then back down without repeating the top or bottom note.
    UpDown,
    /// A deterministic pseudo-random tone each hit.
    Random,
    /// Every chord tone together on each hit.
    Block,
}

impl ArpDirection {
    pub const ALL: [ArpDirection; 5] = [
        ArpDirection::Up,
        ArpDirection::Down,
        ArpDirection::UpDown,
        ArpDirection::Random,
        ArpDirection::Block,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ArpDirection::Up => "Up",
            ArpDirection::Down => "Down",
            ArpDirection::UpDown => "Up-Down",
            ArpDirection::Random => "Random",
            ArpDirection::Block => "Block chords",
        }
    }
}

/// Serde default for [`ArpSettings::octaves`].
fn default_arp_octaves() -> u32 {
    1
}

/// Serde default for [`ArpSettings::gate`]: a 90% gate keeps a small gap
/// between hits so the arpeggio articulates, matching the pattern
/// arpeggiator's built-in spacing.
fn default_arp_gate() -> f32 {
    0.9
}

/// Per-track live arpeggiator settings ([`Track::arp`]). While `enabled`,
/// every chord the track plays — two or more note events starting on the
/// same step — is subdivided into a cycling run of hits at `rate` steps:
/// the chord's pitches extended `octaves - 1` octaves upward, played in
/// `direction` order, each hit sounding `rate * gate` steps. Single notes
/// always pass through unchanged.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct ArpSettings {
    /// Master switch; while off the other fields are inert.
    #[serde(default)]
    pub enabled: bool,
    /// Steps between hits.
    #[serde(default)]
    pub rate: ArpRate,
    /// Octaves the chord tones span upward, 1..=[`MAX_ARP_OCTAVES`].
    #[serde(default = "default_arp_octaves")]
    pub octaves: u32,
    /// Order the tones cycle in.
    #[serde(default)]
    pub direction: ArpDirection,
    /// Hit length as a fraction of `rate`, [`MIN_ARP_GATE`]..=1.0.
    #[serde(default = "default_arp_gate")]
    pub gate: f32,
}

impl Default for ArpSettings {
    fn default() -> Self {
        ArpSettings {
            enabled: false,
            rate: ArpRate::default(),
            octaves: default_arp_octaves(),
            direction: ArpDirection::default(),
            gate: default_arp_gate(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
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
    /// Portamento slide time in seconds, 0..=[`MAX_GLIDE_SECONDS`]; 0 = off.
    /// While above zero a melodic track plays one note at a time like the
    /// mono lead: a note starting while another is still held bends the
    /// sounding voice to the new pitch instead of stacking a second voice
    /// (legato — the envelope does not re-attack), and a released voice
    /// that is still ringing is retriggered rather than replaced. Applies
    /// to built-in and pack sounds; drums, kits and SoundFont programs
    /// ignore it. Songs saved before glide existed load as 0 (off).
    #[serde(default)]
    pub glide: f32,
    /// Mixer automation (volume/pan lanes) drawn under the track's clips in
    /// the arranger. `None` — the usual case — means "no automation"; songs
    /// saved before lanes existed load as `None`. An [`Automation`] holding
    /// no points behaves identically, and [`Project::normalize`] collapses
    /// it to `None` so saved files stay clean.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub automation: Option<Automation>,
    /// Live arpeggiator played back as a track effect: while `enabled`,
    /// chords on the timeline subdivide into a cycling run of hits (see
    /// [`ArpSettings`]). Drum and kit sounds ignore it; single notes always
    /// play unchanged. Songs saved before the arpeggiator existed load it
    /// disabled.
    #[serde(default)]
    pub arp: ArpSettings,
    /// Subgroup bus the track feeds: `0` routes straight to the master,
    /// `1..=NUM_BUSES` selects `project.buses[bus - 1]` (see [`Bus`] and
    /// [`Project::is_bus_audible`]). Songs saved before buses existed have
    /// no `bus` key and load as 0 (master); a master-routed track
    /// serializes without the key.
    #[serde(default, skip_serializing_if = "is_master_bus")]
    pub bus: u8,
    /// Whether this is an audio lane: it plays imported WAV clips
    /// (`audio_clips`) instead of synthesized patterns. Audio lanes normally
    /// carry no patterns; any `clips`/`patterns` left in them are simply
    /// ignored by playback (and kept for hand-edited or undone data).
    /// Songs saved before audio lanes existed load `false` and serialize
    /// without the key.
    #[serde(default, skip_serializing_if = "is_false")]
    pub audio: bool,
    /// Imported-WAV clips on the timeline (see [`AudioClip`]). Lives beside
    /// — not inside — `clips`, so every pattern-clip code path stays
    /// unchanged; the two lists share one clip-id space for selection and
    /// undo. Songs saved before audio clips existed load an empty list.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub audio_clips: Vec<AudioClip>,
}

snapshot_clone!(Track {
    id,
    name,
    color,
    instrument,
    volume,
    pan,
    mute,
    solo,
    effects,
    patterns,
    clips,
    preset,
    font,
    glide,
    automation,
    arp,
    bus,
    audio,
    audio_clips,
});

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

/// A named section marker ("Intro", "Verse", "Chorus", …) pinned to a song
/// step and drawn as a flag on the arranger ruler. Markers are annotations:
/// they never affect playback.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Marker {
    /// Forgiving for hand-edited files: a missing id loads as 0 and is
    /// re-issued by [`Project::normalize`] when it collides.
    #[serde(default)]
    pub id: u64,
    #[serde(default)]
    pub name: String,
    /// Position in sixteenth-note steps, `0..=MAX_PATTERN_STEPS` after
    /// validation. A marker past the song end is harmless — it just sits in
    /// the dark region beyond the END line.
    #[serde(default)]
    pub step: f64,
}

impl Marker {
    /// A marker named `name` at `step`, with a fresh process-unique id.
    pub fn new(name: impl Into<String>, step: f64) -> Self {
        Marker {
            id: next_id(),
            name: name.into(),
            step,
        }
    }
}

/// A per-bar chord label ("Am7", "F", …) pinned to a song step and drawn as
/// a chip on the arranger's chord strip. Chord events are annotations — a
/// harmonic guide only: they never affect playback.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ChordEvent {
    /// Forgiving for hand-edited files: a missing id loads as 0 and is
    /// re-issued by [`Project::normalize`] when it collides.
    #[serde(default)]
    pub id: u64,
    /// Position in sixteenth-note steps, `0..=MAX_PATTERN_STEPS` after
    /// validation. The arranger writes bar-aligned steps; any step is legal.
    #[serde(default)]
    pub step: f64,
    /// Root pitch class, `0..=11` after validation (C = 0).
    #[serde(default)]
    pub root: u8,
    /// Chord type; major when a hand-edited file omits it.
    #[serde(default)]
    pub quality: ChordQuality,
}

impl ChordEvent {
    /// A `quality` chord on `root` at `step`, with a fresh process-unique id.
    pub fn new(root: u8, quality: ChordQuality, step: f64) -> Self {
        ChordEvent {
            id: next_id(),
            step,
            root: root % 12,
            quality,
        }
    }

    /// Chord symbol, e.g. "C", "Am7" or "F#m7b5" — see [`ChordQuality::symbol`].
    pub fn label(&self) -> String {
        self.quality.symbol(self.root)
    }

    /// Chord symbol spelled for the song `key` ("Bb9" in C minor) — what
    /// the arranger's chord strip shows. See [`SongKey::spell`].
    pub fn label_in(&self, key: SongKey) -> String {
        self.quality.symbol_in(self.root, key)
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
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
    /// Named section markers shown on the arranger ruler, kept sorted by
    /// `step` (then id). Songs saved before markers existed have no `markers`
    /// field and load as empty; songs with none serialize without the key.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub markers: Vec<Marker>,
    /// Per-bar chord labels shown on the arranger's chord strip, kept sorted
    /// by `step` (then id). Songs saved before chord events existed have no
    /// `chord_events` field and load as empty; songs with none serialize
    /// without the key.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chord_events: Vec<ChordEvent>,
    /// The fixed subgroup buses tracks can route onto (`Track::bus` = index
    /// + 1; 0 is the master itself, which has no `Bus`). Songs saved before
    ///   buses existed have no `buses` key and load the four defaults ("Bus
    ///   A".."Bus D"); a song whose buses are all default serializes without
    ///   the key. The array length is fixed, so a malformed `buses` array is
    ///   rejected at parse time.
    #[serde(default = "default_buses", skip_serializing_if = "buses_are_default")]
    pub buses: [Bus; NUM_BUSES],
}

snapshot_clone!(Project {
    version,
    id,
    name,
    bpm,
    swing,
    bars,
    master_volume,
    tracks,
    key,
    markers,
    chord_events,
    buses,
});

/// Pitch-class names with ASCII sharps, indexed by `pitch % 12` (C = 0).
pub const NOTE_NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

/// Pitch-class names with ASCII flats, indexed by `pitch % 12` (C = 0) —
/// the spelling [`SongKey::spell`] uses in flat keys.
pub const NOTE_NAMES_FLAT: [&str; 12] = [
    "C", "Db", "D", "Eb", "E", "F", "Gb", "G", "Ab", "A", "Bb", "B",
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

    /// The twelve pitch classes (index = `pitch % 12`, C = 0), `true` where
    /// the class belongs to the key's scale — its seven diatonic degrees.
    /// Backs the editor's scale-highlight row tint and snap-to-scale.
    pub fn pitch_classes(&self) -> [bool; 12] {
        let mut pcs = [false; 12];
        for degree in 0..7 {
            pcs[self.degree_root(degree) as usize] = true;
        }
        pcs
    }

    /// Whether names in this key are spelled with flats: the flat key
    /// signatures (F, Bb, Eb, Ab, Db major and their relative minors) plus
    /// C major / A minor, whose chromatic chords are usually borrowed ones
    /// (bVII, bVI, bIII) written with flats. Sharp keys (G, D, A, E, B, F#
    /// major and relatives) spell sharps.
    pub fn prefers_flats(&self) -> bool {
        let relative_major = match self.mode {
            KeyMode::Major => self.root % 12,
            KeyMode::Minor => (self.root + 3) % 12,
        };
        matches!(relative_major, 0 | 1 | 3 | 5 | 8 | 10)
    }

    /// Name of pitch class `pc` spelled for this key: "Ab" in C minor,
    /// "G#" in E major. Display only — stored data keeps pitch classes.
    pub fn spell(&self, pc: u8) -> &'static str {
        if self.prefers_flats() {
            NOTE_NAMES_FLAT[pc as usize % 12]
        } else {
            NOTE_NAMES[pc as usize % 12]
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
    MinorAdd9,
    Dominant11,
    Minor11,
    Dominant13,
    Dominant7Sus4,
    MinorMajor7,
}

impl ChordQuality {
    pub const ALL: [ChordQuality; 24] = [
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
        ChordQuality::MinorAdd9,
        ChordQuality::Dominant11,
        ChordQuality::Minor11,
        ChordQuality::Dominant13,
        ChordQuality::Dominant7Sus4,
        ChordQuality::MinorMajor7,
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
            ChordQuality::MinorAdd9 => &[0, 3, 7, 14],
            ChordQuality::Dominant11 => &[0, 4, 7, 10, 14, 17],
            ChordQuality::Minor11 => &[0, 3, 7, 10, 14, 17],
            ChordQuality::Dominant13 => &[0, 4, 7, 10, 14, 21],
            ChordQuality::Dominant7Sus4 => &[0, 5, 7, 10],
            ChordQuality::MinorMajor7 => &[0, 3, 7, 11],
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
            ChordQuality::MinorAdd9 => "Minor add9",
            ChordQuality::Dominant11 => "Dominant 11",
            ChordQuality::Minor11 => "Minor 11",
            ChordQuality::Dominant13 => "Dominant 13",
            ChordQuality::Dominant7Sus4 => "Dominant 7 sus4",
            ChordQuality::MinorMajor7 => "Minor-major 7",
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
            ChordQuality::MinorAdd9 => "madd9",
            ChordQuality::Dominant11 => "11",
            ChordQuality::Minor11 => "m11",
            ChordQuality::Dominant13 => "13",
            ChordQuality::Dominant7Sus4 => "7sus4",
            ChordQuality::MinorMajor7 => "mMaj7",
        }
    }

    /// Chord symbol for a root pitch (class), e.g. `symbol(9)` on Minor7 = "Am7".
    pub fn symbol(self, root: u8) -> String {
        format!("{}{}", NOTE_NAMES[root as usize % 12], self.suffix())
    }

    /// [`Self::symbol`] with the root spelled for `key` — "Abmaj7" in
    /// C minor where [`Self::symbol`] says "G#maj7".
    pub fn symbol_in(self, root: u8, key: SongKey) -> String {
        format!("{}{}", key.spell(root), self.suffix())
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
    // Gentle sends only: every track shares one long reverb, so wet defaults
    // stack into a washed-out mix. Bass stays dry to keep the low end tight.
    let (volume, reverb, cutoff) = match instrument {
        Instrument::Drums => (0.78, 0.04, MAX_CUTOFF_HZ),
        Instrument::Bass => (0.7, 0.0, 4000.0),
        Instrument::Piano => (0.62, 0.12, MAX_CUTOFF_HZ),
        Instrument::Pluck => (0.58, 0.12, MAX_CUTOFF_HZ),
        Instrument::Guitar => (0.6, 0.12, MAX_CUTOFF_HZ),
        Instrument::Pad => (0.5, 0.25, 6000.0),
        Instrument::Lead => (0.56, 0.1, MAX_CUTOFF_HZ),
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
        glide: 0.0,
        automation: None,
        arp: ArpSettings::default(),
        bus: 0,
        audio: false,
        audio_clips: Vec::new(),
    }
}

/// Lane color for audio tracks — a desaturated slate that reads
/// differently from every instrument color in [`TRACK_COLORS`].
pub const AUDIO_TRACK_COLOR: [u8; 3] = [120, 132, 150];

/// A new audio lane (`Track::audio`): no patterns or pattern clips, just an
/// empty `audio_clips` list ready for imported WAVs. `name` falls back to
/// "Audio" when blank. The `instrument` field is inert on audio lanes —
/// [`Instrument::Pad`] is parked there only because the field is required.
pub fn create_audio_track(name: &str) -> Track {
    let name = name.trim();
    Track {
        id: next_id(),
        name: if name.is_empty() {
            "Audio".to_string()
        } else {
            name.chars().take(MAX_NAME_CHARS).collect()
        },
        color: AUDIO_TRACK_COLOR,
        instrument: Instrument::Pad,
        volume: 0.7,
        pan: 0.0,
        mute: false,
        solo: false,
        effects: Effects::default(),
        patterns: Vec::new(),
        clips: Vec::new(),
        preset: None,
        font: None,
        glide: 0.0,
        automation: None,
        arp: ArpSettings::default(),
        bus: 0,
        audio: true,
        audio_clips: Vec::new(),
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

/// Trims a name the way [`Project::normalize`] repairs names on load: control
/// characters are dropped, the length is capped at [`MAX_NAME_CHARS`] and an
/// empty (or all-blank) result falls back to `fallback`. Shared by the marker
/// ops so renames go through the same cleaning as every other song name.
pub(crate) fn clamp_name(name: &mut String, fallback: &str) {
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
            markers: Vec::new(),
            chord_events: Vec::new(),
            buses: default_buses(),
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

    /// Whether any subgroup bus is soloed — the bus counterpart of
    /// [`Project::has_solo`].
    pub fn has_bus_solo(&self) -> bool {
        self.buses.iter().any(|b| b.solo)
    }

    /// The bus mute/solo gate composed into [`Project::is_track_audible`].
    /// A subgroup is heard only when it is not muted and — while any bus is
    /// soloed — soloed itself. `bus` 0 stands for the master itself: it has
    /// no controls of its own, so it passes unless a subgroup is soloed
    /// (soloing a bus therefore isolates its group, silencing
    /// master-routed tracks too). An out-of-range bus — only reachable on
    /// unvalidated data — counts as inaudible rather than passed through.
    pub fn is_bus_audible(&self, bus: u8) -> bool {
        if bus == 0 {
            return !self.has_bus_solo();
        }
        self.buses
            .get(bus as usize - 1)
            .is_some_and(|b| !b.mute && (!self.has_bus_solo() || b.solo))
    }

    /// Mute/solo rule shared by playback, export and the mixer display. Two
    /// gates compose: a track is heard only if it passes its own gate —
    /// not muted, and soloed whenever any track is soloed — AND its bus
    /// passes [`Project::is_bus_audible`]. So a soloed track on a muted or
    /// non-soloed bus stays silent, and muting a bus silences its whole
    /// group including the post-fader sends of its member tracks.
    pub fn is_track_audible(&self, index: usize) -> bool {
        let Some(track) = self.tracks.get(index) else {
            return false;
        };
        if !self.is_bus_audible(track.bus) {
            return false;
        }
        let solo = self.has_solo();
        !track.mute && (!solo || track.solo)
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
        // Clip ids are unique across the whole project — selection and the
        // clipboard key on the id alone, with no track qualifier.
        let mut clip_ids = HashSet::new();
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
            check_range(
                &format!("{tl} glide"),
                track.glide,
                0.0,
                MAX_GLIDE_SECONDS,
                track.glide.is_finite(),
            )?;
            check_range(
                &format!("{tl} arp octaves"),
                track.arp.octaves,
                1,
                MAX_ARP_OCTAVES,
                true,
            )?;
            check_range(
                &format!("{tl} arp gate"),
                track.arp.gate,
                MIN_ARP_GATE,
                1.0,
                track.arp.gate.is_finite(),
            )?;
            check_range(&format!("{tl} bus"), track.bus, 0, NUM_BUSES as u8, true)?;
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
            if let Some(auto) = &track.automation {
                for kind in AutomationLane::ALL {
                    let lane = auto.lane(kind);
                    let (lo, hi) = Automation::value_range(kind);
                    let al = format!("{tl} {} automation", kind.label());
                    if lane.len() > MAX_AUTOMATION_POINTS {
                        return Err(format!(
                            "{al} has more than {MAX_AUTOMATION_POINTS} points."
                        ));
                    }
                    let mut prev = f64::NEG_INFINITY;
                    for pt in lane {
                        check_range(
                            &format!("{al} point position"),
                            pt.step,
                            0.0,
                            max_pos,
                            pt.step.is_finite(),
                        )?;
                        if pt.step < prev {
                            return Err(format!("{al} points are not sorted by step."));
                        }
                        prev = pt.step;
                        check_range(
                            &format!("{al} point value"),
                            pt.value,
                            lo,
                            hi,
                            pt.value.is_finite(),
                        )?;
                    }
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
            // Audio clips share the pattern clips' id space so selection and
            // undo never have to disambiguate. The referenced file's presence
            // is intentionally not checked — a missing staged file is a
            // silent clip, not an invalid project.
            if track.audio_clips.len() > MAX_AUDIO_CLIPS_PER_TRACK {
                return Err(format!(
                    "{tl} has more than {MAX_AUDIO_CLIPS_PER_TRACK} audio clips."
                ));
            }
            for clip in &track.audio_clips {
                if !clip_ids.insert(clip.id) {
                    return Err(format!("{tl} has clips with duplicate IDs."));
                }
                max_id = max_id.max(clip.id);
                check_name(&format!("{tl} audio clip name"), &clip.name)?;
                if !audio_ref_name_ok(&clip.file) {
                    return Err(format!(
                        "{tl} has an audio clip with an invalid file reference."
                    ));
                }
                if !clip.start.is_finite() || clip.start < 0.0 || clip.start >= max_pos {
                    return Err(format!("{tl} has an audio clip with an invalid position."));
                }
                if !clip.length.is_finite()
                    || clip.length < MIN_AUDIO_CLIP_STEPS
                    || clip.length > max_pos
                {
                    return Err(format!("{tl} has an audio clip with an invalid length."));
                }
                check_range(
                    &format!("{tl} audio clip offset"),
                    clip.offset,
                    0.0,
                    MAX_AUDIO_SECONDS,
                    clip.offset.is_finite(),
                )?;
                if clip.loops == 0 || clip.loops > MAX_CLIP_LOOPS {
                    return Err(format!(
                        "{tl} has an audio clip with an invalid loop count."
                    ));
                }
                let span = clip.length * clip.loops as f64;
                check_range(
                    &format!("{tl} audio clip fade-in"),
                    clip.fade_in,
                    0.0,
                    span,
                    clip.fade_in.is_finite(),
                )?;
                check_range(
                    &format!("{tl} audio clip fade-out"),
                    clip.fade_out,
                    0.0,
                    span,
                    clip.fade_out.is_finite(),
                )?;
            }
        }
        if event_count > MAX_SONG_EVENTS {
            return Err(format!(
                "The arrangement contains {event_count} note events; the limit is {MAX_SONG_EVENTS}."
            ));
        }
        // The fixed array already guarantees the bus count at type and
        // serde level; this loop only vets the fields a hand-edited file
        // can fill in.
        for (bi, bus) in self.buses.iter().enumerate() {
            let bl = format!("Bus {}", BUS_LETTERS[bi]);
            check_name(&format!("{bl} name"), &bus.name)?;
            check_range(
                &format!("{bl} volume"),
                bus.volume,
                0.0,
                1.0,
                bus.volume.is_finite(),
            )?;
            check_range(
                &format!("{bl} pan"),
                bus.pan,
                -1.0,
                1.0,
                bus.pan.is_finite(),
            )?;
        }
        if self.markers.len() > MAX_MARKERS {
            return Err(format!(
                "A song can have at most {MAX_MARKERS} section markers."
            ));
        }
        let mut marker_ids = HashSet::new();
        for (mi, marker) in self.markers.iter().enumerate() {
            let ml = format!("Section marker {} ({})", mi + 1, marker.name);
            check_name(&format!("Section marker {} name", mi + 1), &marker.name)?;
            if !marker_ids.insert(marker.id) {
                return Err(format!("{ml} has a duplicate ID."));
            }
            max_id = max_id.max(marker.id);
            check_range(
                &format!("{ml} position"),
                marker.step,
                0.0,
                max_pos,
                marker.step.is_finite(),
            )?;
        }
        if self.chord_events.len() > MAX_CHORD_EVENTS {
            return Err(format!(
                "A song can have at most {MAX_CHORD_EVENTS} chord events."
            ));
        }
        let mut chord_ids = HashSet::new();
        for (ci, ev) in self.chord_events.iter().enumerate() {
            let cl = format!("Chord event {} ({})", ci + 1, ev.label());
            if !chord_ids.insert(ev.id) {
                return Err(format!("{cl} has a duplicate ID."));
            }
            max_id = max_id.max(ev.id);
            check_range(
                &format!("{cl} position"),
                ev.step,
                0.0,
                max_pos,
                ev.step.is_finite(),
            )?;
            check_range(&format!("{cl} root"), ev.root, 0, 11, true)?;
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
        let mut clip_ids = HashSet::new();
        let mut budget = MAX_SONG_EVENTS;
        for track in &mut self.tracks {
            if !track_ids.insert(track.id) {
                track.id = next_id();
                track_ids.insert(track.id);
            }
            clamp_name(&mut track.name, track.instrument.name());
            track.volume = finite_or_f32(track.volume, 0.7).clamp(0.0, 1.0);
            track.pan = finite_or_f32(track.pan, 0.0).clamp(-1.0, 1.0);
            track.glide = finite_or_f32(track.glide, 0.0).clamp(0.0, MAX_GLIDE_SECONDS);
            track.bus = track.bus.min(NUM_BUSES as u8);
            track.arp.octaves = track.arp.octaves.clamp(1, MAX_ARP_OCTAVES);
            track.arp.gate =
                finite_or_f32(track.arp.gate, default_arp_gate()).clamp(MIN_ARP_GATE, 1.0);
            let fx = &mut track.effects;
            fx.cutoff_hz =
                finite_or_f32(fx.cutoff_hz, MAX_CUTOFF_HZ).clamp(MIN_CUTOFF_HZ, MAX_CUTOFF_HZ);
            fx.reverb = finite_or_f32(fx.reverb, 0.0).clamp(0.0, 1.0);
            fx.delay = finite_or_f32(fx.delay, 0.0).clamp(0.0, 1.0);
            fx.drive = finite_or_f32(fx.drive, 0.0).clamp(0.0, 1.0);
            if let Some(auto) = &mut track.automation {
                for kind in AutomationLane::ALL {
                    let (lo, hi) = Automation::value_range(kind);
                    let lane = auto.lane_mut(kind);
                    lane.retain(|pt| pt.step.is_finite() && pt.value.is_finite());
                    lane.truncate(MAX_AUTOMATION_POINTS);
                    for pt in lane.iter_mut() {
                        pt.step = pt.step.clamp(0.0, max_pos);
                        pt.value = pt.value.clamp(lo, hi);
                    }
                    lane.sort_by(|a, b| a.step.total_cmp(&b.step));
                }
                // An automation block holding no points is indistinguishable
                // from `None`; collapse it so saved files stay clean.
                if auto.is_empty() {
                    track.automation = None;
                }
            }
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
            // Audio clips: drop anything that can never be resolved or
            // placed, repair the rest — the id space is shared with the
            // pattern clips above so `clip_ids` keeps deduplicating.
            track.audio_clips.retain(|c| {
                c.start.is_finite()
                    && c.start >= 0.0
                    && c.start < max_pos
                    && audio_ref_name_ok(&c.file)
            });
            track.audio_clips.truncate(MAX_AUDIO_CLIPS_PER_TRACK);
            for clip in &mut track.audio_clips {
                clamp_name(&mut clip.name, "Audio");
                clip.length = finite_or(clip.length, STEPS_PER_BAR as f64)
                    .clamp(MIN_AUDIO_CLIP_STEPS, max_pos);
                clip.offset = finite_or(clip.offset, 0.0).clamp(0.0, MAX_AUDIO_SECONDS);
                clip.loops = clip.loops.clamp(1, MAX_CLIP_LOOPS);
                let span = clip.span();
                clip.fade_in = finite_or(clip.fade_in, 0.0).clamp(0.0, span);
                clip.fade_out = finite_or(clip.fade_out, 0.0).clamp(0.0, span);
                if !clip_ids.insert(clip.id) {
                    clip.id = next_id();
                    clip_ids.insert(clip.id);
                }
            }
        }
        // The array length needs no repair — only the field values.
        for (bi, bus) in self.buses.iter_mut().enumerate() {
            clamp_name(&mut bus.name, &bus_name(bi));
            bus.volume = finite_or_f32(bus.volume, default_bus_volume()).clamp(0.0, 1.0);
            bus.pan = finite_or_f32(bus.pan, 0.0).clamp(-1.0, 1.0);
        }
        self.markers.truncate(MAX_MARKERS);
        let mut marker_ids = HashSet::new();
        for marker in &mut self.markers {
            if !marker_ids.insert(marker.id) {
                marker.id = next_id();
                marker_ids.insert(marker.id);
            }
            clamp_name(&mut marker.name, "Section");
            marker.step = finite_or(marker.step, 0.0).clamp(0.0, max_pos);
        }
        self.markers
            .sort_by(|a, b| a.step.total_cmp(&b.step).then(a.id.cmp(&b.id)));
        self.chord_events.truncate(MAX_CHORD_EVENTS);
        let mut chord_ids = HashSet::new();
        for ev in &mut self.chord_events {
            if !chord_ids.insert(ev.id) {
                ev.id = next_id();
                chord_ids.insert(ev.id);
            }
            ev.step = finite_or(ev.step, 0.0).clamp(0.0, max_pos);
            ev.root %= 12;
        }
        self.chord_events
            .sort_by(|a, b| a.step.total_cmp(&b.step).then(a.id.cmp(&b.id)));
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
            // Audio lanes play imported clips, not patterns — the snapshot
            // schedules them from `Track::audio_clips` instead.
            if track.audio {
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
        // One section per two-bar chord change.
        markers: vec![
            Marker::new("Verse", 0.0),
            Marker::new("Lift", 32.0),
            Marker::new("Chorus", 64.0),
            Marker::new("Turnaround", 96.0),
        ],
        // The strip mirrors the chords the arrangement actually plays:
        // Cm9, Abmaj7, Ebmaj7, Bb9, two bars each.
        chord_events: vec![
            ChordEvent::new(0, ChordQuality::Minor9, 0.0),
            ChordEvent::new(8, ChordQuality::Major7, 32.0),
            ChordEvent::new(3, ChordQuality::Major7, 64.0),
            ChordEvent::new(10, ChordQuality::Dominant9, 96.0),
        ],
        buses: default_buses(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// New tracks start with gentle sends; saved songs keep exactly the
    /// reverb they were written with (the field is required, so the
    /// constructor default never reaches the loader).
    #[test]
    fn new_tracks_default_to_light_reverb_and_saved_songs_keep_theirs() {
        assert_eq!(Effects::default().reverb, 0.0);
        assert_eq!(create_audio_track("Vox").effects.reverb, 0.0);
        for inst in Instrument::ALL {
            let t = create_track(inst);
            assert!(t.effects.reverb <= 0.25, "{inst:?} too wet by default");
        }
        assert_eq!(create_track(Instrument::Bass).effects.reverb, 0.0);
        let legacy = r#"{"version":1,"id":1,"name":"Old","bpm":110,"swing":0,"bars":8,"master_volume":0.8,"tracks":[{"id":2,"name":"Keys","color":[255,132,104],"instrument":"piano","volume":0.8,"pan":0,"mute":false,"solo":false,"effects":{"cutoff_hz":20000,"reverb":0.15,"delay":0,"drive":0},"patterns":[],"clips":[]}]}"#;
        let old: Project = serde_json::from_str(legacy).unwrap();
        old.validate().unwrap();
        assert_eq!(old.tracks[0].effects.reverb, 0.15);
        // A missing reverb is a damaged file, not a silent default.
        let missing = legacy.replace(r#""reverb":0.15,"#, "");
        assert!(serde_json::from_str::<Project>(&missing).is_err());
    }

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

    #[test]
    fn chord_symbols_follow_the_key_spelling() {
        let c_minor = SongKey::new(0, KeyMode::Minor);
        // The demo song: flats in C minor, matching its clip names.
        assert!(c_minor.prefers_flats());
        assert_eq!(ChordQuality::Major7.symbol_in(8, c_minor), "Abmaj7");
        assert_eq!(ChordQuality::Major7.symbol_in(3, c_minor), "Ebmaj7");
        assert_eq!(ChordQuality::Dominant9.symbol_in(10, c_minor), "Bb9");
        let ev = ChordEvent::new(8, ChordQuality::Major7, 0.0);
        assert_eq!(ev.label_in(c_minor), "Abmaj7");
        assert_eq!(ev.label(), "G#maj7", "the key-free label keeps sharps");
        // Sharp keys keep sharps.
        for key in [
            SongKey::new(4, KeyMode::Major),
            SongKey::new(7, KeyMode::Major),
            SongKey::new(4, KeyMode::Minor),
            SongKey::new(11, KeyMode::Minor),
        ] {
            assert!(!key.prefers_flats(), "{}", key.name());
            assert_eq!(key.spell(8), "G#");
        }
        // Flat keys and their relative minors use flats.
        for key in [
            SongKey::new(5, KeyMode::Major),
            SongKey::new(10, KeyMode::Major),
            SongKey::new(2, KeyMode::Minor),
            SongKey::new(5, KeyMode::Minor),
            SongKey::new(0, KeyMode::Major),
            SongKey::new(9, KeyMode::Minor),
        ] {
            assert!(key.prefers_flats(), "{}", key.name());
            assert_eq!(key.spell(1), "Db");
        }
        // Naturals never change.
        assert_eq!(SongKey::new(4, KeyMode::Major).spell(9), "A");
        assert_eq!(c_minor.spell(9), "A");
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
            (
                "glide range",
                Box::new(|p| p.tracks[0].glide = MAX_GLIDE_SECONDS + 1.0),
            ),
            ("glide nan", Box::new(|p| p.tracks[0].glide = f32::NAN)),
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
            hidden: false,
            audition_note: 60,
            audition_duration: 0.8,
            kit: None,
            sample: None,
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

    #[test]
    fn markers_round_trip_and_legacy_default() {
        // Songs saved before markers existed have no `markers` key and load
        // with an empty list.
        let legacy = r#"{"version":1,"id":1,"name":"Old","bpm":110,"swing":0,"bars":8,"master_volume":0.8,"tracks":[]}"#;
        let old: Project = serde_json::from_str(legacy).unwrap();
        assert!(old.markers.is_empty());
        old.validate().unwrap();

        // Markers serialize inside the song and come back identical.
        let mut p = Project::empty();
        p.markers = vec![
            Marker::new("Intro", 0.0),
            Marker::new("Chorus", 32.0),
            Marker::new("Outro", 128.0),
        ];
        p.validate().unwrap();
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("\"markers\":"), "{json}");
        let back: Project = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
        back.validate().unwrap();

        // An empty list is skipped entirely so files stay tidy.
        let json = serde_json::to_string(&Project::empty()).unwrap();
        assert!(!json.contains("\"markers\""), "{json}");
    }

    #[test]
    fn marker_validation_rejects_and_normalize_repairs() {
        let mut p = Project::empty();
        let bad_step = Marker::new("Nowhere", -4.0);
        let dup_id = bad_step.id;
        p.markers = vec![
            Marker {
                id: bad_step.id,
                name: "bad\u{0}name".into(),
                step: f64::NAN,
            },
            bad_step,
            Marker::new("Late", 48.0),
            Marker::new("Early", 8.0),
        ];
        p.markers[1].id = dup_id;
        assert!(p.validate().is_err());
        p.normalize();
        p.validate().unwrap();
        // Sorted by step; steps clamped into 0..=MAX_PATTERN_STEPS; the NaN
        // repaired to 0 and the control-char name cleaned; the duplicate id
        // re-issued.
        let steps: Vec<f64> = p.markers.iter().map(|m| m.step).collect();
        assert_eq!(steps, vec![0.0, 0.0, 8.0, 48.0]);
        assert!(p
            .markers
            .iter()
            .all(|m| !m.name.chars().any(|c| c.is_control())));
        let ids: HashSet<u64> = p.markers.iter().map(|m| m.id).collect();
        assert_eq!(ids.len(), p.markers.len(), "duplicate ids re-issued");

        // The count cap is enforced by validate and repaired by normalize.
        p.markers = (0..MAX_MARKERS + 5)
            .map(|i| Marker::new(format!("S{i}"), i as f64))
            .collect();
        assert!(p.validate().is_err());
        p.normalize();
        assert_eq!(p.markers.len(), MAX_MARKERS);
        p.validate().unwrap();
    }

    #[test]
    fn chord_events_round_trip_and_legacy_default() {
        // Songs saved before the chord strip existed have no `chord_events`
        // key and load with an empty list.
        let legacy = r#"{"version":1,"id":1,"name":"Old","bpm":110,"swing":0,"bars":8,"master_volume":0.8,"tracks":[]}"#;
        let old: Project = serde_json::from_str(legacy).unwrap();
        assert!(old.chord_events.is_empty());
        old.validate().unwrap();

        // Chord events serialize inside the song and come back identical.
        let mut p = Project::empty();
        p.chord_events = vec![
            ChordEvent::new(9, ChordQuality::Minor7, 16.0),
            ChordEvent::new(0, ChordQuality::Major, 0.0),
            ChordEvent::new(6, ChordQuality::HalfDiminished7, 32.0),
        ];
        p.validate().unwrap();
        assert_eq!(p.chord_events[0].label(), "Am7");
        assert_eq!(p.chord_events[2].label(), "F#m7b5");
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("\"chord_events\":"), "{json}");
        let back: Project = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
        back.validate().unwrap();

        // An empty list is skipped entirely so files stay tidy.
        let json = serde_json::to_string(&Project::empty()).unwrap();
        assert!(!json.contains("\"chord_events\""), "{json}");
    }

    #[test]
    fn chord_event_validation_rejects_and_normalize_repairs() {
        let mut p = Project::empty();
        let bad_step = ChordEvent::new(0, ChordQuality::Major, -4.0);
        let dup_id = bad_step.id;
        p.chord_events = vec![
            ChordEvent {
                id: dup_id,
                step: f64::NAN,
                root: 99,
                quality: ChordQuality::Minor,
            },
            bad_step,
            ChordEvent::new(7, ChordQuality::Dominant7, 48.0),
            ChordEvent::new(0, ChordQuality::Major, 8.0),
        ];
        assert!(p.validate().is_err());
        p.normalize();
        p.validate().unwrap();
        // Sorted by step; steps clamped into 0..=MAX_PATTERN_STEPS with the
        // NaN repaired to 0; the out-of-range root reduced to a pitch class;
        // the duplicate id re-issued.
        let steps: Vec<f64> = p.chord_events.iter().map(|e| e.step).collect();
        assert_eq!(steps, vec![0.0, 0.0, 8.0, 48.0]);
        assert!(p.chord_events.iter().all(|e| e.root < 12));
        let ids: HashSet<u64> = p.chord_events.iter().map(|e| e.id).collect();
        assert_eq!(ids.len(), p.chord_events.len(), "duplicate ids re-issued");

        // Roots are pitch classes: 0..=11 only.
        p.chord_events = vec![ChordEvent::new(0, ChordQuality::Major, 0.0)];
        p.chord_events[0].root = 12;
        assert!(p.validate().is_err());
        p.normalize();
        assert_eq!(p.chord_events[0].root, 0);
        p.validate().unwrap();

        // The count cap is enforced by validate and repaired by normalize.
        p.chord_events = (0..MAX_CHORD_EVENTS + 5)
            .map(|i| ChordEvent::new(0, ChordQuality::Major, i as f64))
            .collect();
        assert!(p.validate().is_err());
        p.normalize();
        assert_eq!(p.chord_events.len(), MAX_CHORD_EVENTS);
        p.validate().unwrap();
    }

    fn pt(step: f64, value: f32) -> AutomationPoint {
        AutomationPoint { step, value }
    }

    #[test]
    fn automation_value_interpolates_and_flattens_at_the_ends() {
        // An empty lane has no value — callers keep the un-automated mix.
        assert_eq!(automation_value(&[], 8.0), None);
        let lane = [pt(4.0, 0.0), pt(12.0, 1.0)];
        // Flat at the first point's value before it, linear between points.
        assert_eq!(automation_value(&lane, -3.0), Some(0.0));
        assert_eq!(automation_value(&lane, 4.0), Some(0.0));
        assert_eq!(automation_value(&lane, 8.0), Some(0.5));
        assert_eq!(automation_value(&lane, 10.0), Some(0.75));
        assert_eq!(automation_value(&lane, 12.0), Some(1.0));
        // Flat at the last point's value after it.
        assert_eq!(automation_value(&lane, 99.0), Some(1.0));
        // A single point is flat everywhere; a non-finite query is defensive.
        let one = [pt(8.0, 0.4)];
        assert_eq!(automation_value(&one, -1.0), Some(0.4));
        assert_eq!(automation_value(&one, 64.0), Some(0.4));
        assert_eq!(automation_value(&lane, f64::NAN), Some(0.0));
        // Duplicate steps draw a jump: the later point wins at that step —
        // also when the duplicates sit on the lane's first or last step.
        let jump = [pt(4.0, 0.0), pt(8.0, 0.25), pt(8.0, 0.75), pt(12.0, 1.0)];
        assert_eq!(automation_value(&jump, 8.0), Some(0.75));
        assert_eq!(automation_value(&jump, 10.0), Some(0.875));
        let head = [pt(4.0, 0.2), pt(4.0, 0.8), pt(12.0, 1.0)];
        assert_eq!(automation_value(&head, 4.0), Some(0.8));
        let tail = [pt(4.0, 0.0), pt(12.0, 0.5), pt(12.0, 0.9)];
        assert_eq!(automation_value(&tail, 12.0), Some(0.9));
        let flat = [pt(6.0, 0.3), pt(6.0, 0.7)];
        assert_eq!(automation_value(&flat, 0.0), Some(0.3));
        assert_eq!(automation_value(&flat, 6.0), Some(0.7));
        assert_eq!(automation_value(&flat, 99.0), Some(0.7));
    }

    #[test]
    fn automation_serde_round_trip_and_legacy_default() {
        let mut p = Project::empty();
        p.tracks[0].automation = Some(Automation {
            volume: vec![pt(0.0, 1.0), pt(16.0, 0.0)],
            pan: vec![pt(8.0, -0.5)],
        });
        p.validate().unwrap();
        let json = serde_json::to_string(&p).unwrap();
        let back: Project = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);

        // `None` and empty lanes stay out of the JSON entirely, which is what
        // makes songs saved before automation existed load as `None`.
        let json = serde_json::to_string(&Project::empty()).unwrap();
        assert!(!json.contains("\"automation\""), "{json}");
        let legacy: Project = serde_json::from_str(&json).unwrap();
        assert!(legacy.tracks.iter().all(|t| t.automation.is_none()));
        let mut p2 = Project::empty();
        p2.tracks[0].automation = Some(Automation {
            volume: vec![],
            pan: vec![pt(0.0, 0.5)],
        });
        let json = serde_json::to_string(&p2).unwrap();
        // Match the `"lane":[points]` shape — the plain `"volume"` substring
        // also appears on `master_volume` and the track fader.
        assert!(!json.contains("\"volume\":["), "{json}");
        assert!(json.contains("\"pan\":["), "{json}");
    }

    #[test]
    fn automation_validation_rejects_and_normalize_repairs() {
        let mut p = Project::empty();
        let valid = |p: &mut Project| {
            let mut q = p.clone();
            q.normalize();
            q.validate().unwrap();
            q
        };
        // Unsorted lane.
        p.tracks[0].automation = Some(Automation {
            volume: vec![pt(8.0, 0.5), pt(4.0, 0.5)],
            pan: vec![],
        });
        assert!(p.validate().is_err());
        let q = valid(&mut p);
        assert_eq!(
            q.tracks[0].automation.as_ref().unwrap().volume,
            vec![pt(4.0, 0.5), pt(8.0, 0.5)]
        );

        // Out-of-range steps and values, non-finite data.
        p.tracks[0].automation = Some(Automation {
            volume: vec![
                pt(-2.0, 3.0),
                pt(9.0, f32::NAN),
                pt(MAX_PATTERN_STEPS as f64 + 1.0, -1.0),
            ],
            pan: vec![pt(f64::INFINITY, 5.0)],
        });
        assert!(p.validate().is_err());
        let q = valid(&mut p);
        let auto = q.tracks[0].automation.as_ref().unwrap();
        assert!(auto.pan.is_empty());
        assert_eq!(
            auto.volume,
            vec![
                pt(0.0, MAX_AUTOMATION_GAIN),
                pt(MAX_PATTERN_STEPS as f64, 0.0)
            ]
        );

        // The per-lane point cap is enforced.
        p.tracks[0].automation = Some(Automation {
            volume: (0..MAX_AUTOMATION_POINTS + 1)
                .map(|i| pt(i as f64, 0.5))
                .collect(),
            pan: vec![],
        });
        assert!(p.validate().is_err());
        let q = valid(&mut p);
        assert_eq!(
            q.tracks[0].automation.as_ref().unwrap().volume.len(),
            MAX_AUTOMATION_POINTS
        );

        // Removing the last point drops the whole block back to `None`.
        p.tracks[0].automation = Some(Automation::default());
        let q = valid(&mut p);
        assert!(q.tracks[0].automation.is_none());
    }

    #[test]
    fn track_arp_validated_normalized_and_serde_defaults() {
        // A well-formed arpeggiator block validates and round-trips.
        let mut p = Project::empty();
        p.tracks[0].arp = ArpSettings {
            enabled: true,
            rate: ArpRate::ThirtySecond,
            octaves: 2,
            direction: ArpDirection::UpDown,
            gate: 0.5,
        };
        p.validate().unwrap();
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("\"rate\":\"thirty_second\""), "{json}");
        assert!(json.contains("\"direction\":\"up_down\""), "{json}");
        let back: Project = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);

        // Out-of-range fields are rejected and repaired.
        let mut p = Project::empty();
        p.tracks[0].arp.octaves = 0;
        assert!(p.validate().is_err(), "octaves 0 rejected");
        p.tracks[0].arp.octaves = MAX_ARP_OCTAVES + 1;
        assert!(p.validate().is_err(), "octaves above max rejected");
        p.tracks[0].arp.octaves = 1;
        p.tracks[0].arp.gate = f32::NAN;
        assert!(p.validate().is_err(), "NaN gate rejected");
        p.tracks[0].arp.gate = 0.05;
        assert!(p.validate().is_err(), "gate below min rejected");
        p.normalize();
        p.validate().unwrap();
        assert_eq!(p.tracks[0].arp.octaves, 1);
        assert!((p.tracks[0].arp.gate - MIN_ARP_GATE).abs() < 1e-6);

        // Songs saved before the arpeggiator existed have no `arp` key and
        // load disabled with defaults.
        let mut value = serde_json::to_value(&p).unwrap();
        for t in value["tracks"].as_array_mut().unwrap() {
            t.as_object_mut().unwrap().remove("arp");
        }
        let legacy: Project = serde_json::from_value(value).unwrap();
        assert_eq!(legacy.tracks[0].arp, ArpSettings::default());
        assert!(!legacy.tracks[0].arp.enabled);

        // A hand-edited partial block fills the rest from defaults.
        let partial: ArpSettings = serde_json::from_str(r#"{"enabled":true}"#).unwrap();
        assert!(partial.enabled);
        assert_eq!(partial.rate, ArpRate::default());
        assert_eq!(partial.gate, default_arp_gate());
    }

    // ----- subgroup buses ----------------------------------------------------

    #[test]
    fn buses_default_for_legacy_songs_and_skip_when_default() {
        // Songs saved before buses existed have no `buses` key: they load
        // with the four defaults.
        let mut value = serde_json::to_value(Project::empty()).unwrap();
        value.as_object_mut().unwrap().remove("buses");
        let legacy: Project = serde_json::from_value(value).unwrap();
        assert_eq!(legacy.buses, default_buses());
        assert_eq!(legacy.buses[0].name, "Bus A");
        assert_eq!(legacy.buses[3].name, "Bus D");

        // Untouched buses stay out of the saved file; a changed one writes
        // the key and round-trips.
        let json = serde_json::to_string(&legacy).unwrap();
        assert!(!json.contains("\"buses\""), "{json}");
        let mut p = legacy;
        p.buses[0].name = "Drums".into();
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("\"buses\""), "{json}");
        let back: Project = serde_json::from_str(&json).unwrap();
        assert_eq!(back.buses[0].name, "Drums");
    }

    #[test]
    fn track_bus_validated_normalized_and_defaulted() {
        // Legacy tracks have no `bus` key — they load master-routed, and
        // master routing stays out of the saved file.
        let mut p = Project::empty();
        p.tracks[0].bus = 2;
        p.validate().unwrap();
        let mut value = serde_json::to_value(&p).unwrap();
        for t in value["tracks"].as_array_mut().unwrap() {
            t.as_object_mut().unwrap().remove("bus");
        }
        let legacy: Project = serde_json::from_value(value).unwrap();
        assert_eq!(legacy.tracks[0].bus, 0);
        let json = serde_json::to_string(&legacy).unwrap();
        assert!(!json.contains("\"bus\":"), "{json}");

        // Values past NUM_BUSES are rejected and clamped on repair.
        p.tracks[0].bus = NUM_BUSES as u8 + 1;
        assert!(p.validate().is_err());
        p.normalize();
        p.validate().unwrap();
        assert_eq!(p.tracks[0].bus, NUM_BUSES as u8);
    }

    #[test]
    fn bus_fields_validated_normalized_and_partial_defaulted() {
        let mut p = Project::empty();
        p.buses[0].volume = 1.5;
        assert!(p.validate().is_err());
        p.buses[0].volume = f32::NAN;
        assert!(p.validate().is_err());
        p.buses[0].volume = 0.8;
        p.buses[1].pan = -3.0;
        assert!(p.validate().is_err());
        p.buses[1].pan = 0.0;
        p.buses[2].name = "bad\u{0007}name".into();
        assert!(p.validate().is_err());
        p.normalize();
        p.validate().unwrap();
        assert_eq!(p.buses[0].volume, 0.8);
        assert_eq!(p.buses[1].pan, 0.0);

        // A hand-edited partial bus object fills the rest from defaults.
        let mut value = serde_json::to_value(&p).unwrap();
        let arr = value["buses"].as_array_mut().unwrap();
        arr[0] = serde_json::json!({ "name": "Drums" });
        let partial: Project = serde_json::from_value(value).unwrap();
        assert_eq!(partial.buses[0].name, "Drums");
        assert_eq!(partial.buses[0].volume, default_bus_volume());
    }

    #[test]
    fn bus_mute_solo_compose_with_track_mute_solo() {
        let mut p = Project::empty();
        p.tracks.push(create_track(Instrument::Piano));
        // Track 0 joins Bus A; track 1 stays master-direct.
        p.tracks[0].bus = 1;

        // A muted bus silences its whole group, even a soloed member.
        p.buses[0].mute = true;
        assert!(!p.is_track_audible(0));
        assert!(p.is_track_audible(1));
        p.tracks[0].solo = true;
        assert!(!p.is_track_audible(0), "solo cannot override a muted bus");
        p.tracks[0].solo = false;
        p.buses[0].mute = false;

        // A soloed bus isolates its group: master-direct tracks go silent.
        p.buses[0].solo = true;
        assert!(p.is_track_audible(0));
        assert!(!p.is_track_audible(1));
        assert!(
            !p.is_bus_audible(0),
            "bus 0 (master) fails while a bus is soloed"
        );
        assert!(p.is_bus_audible(1));

        // Track mute/solo still applies on top of the bus gate.
        p.tracks[0].mute = true;
        assert!(!p.is_track_audible(0));
        p.tracks[0].mute = false;
        p.buses[0].solo = false;

        // Soloing a different bus silences this group too.
        p.buses[2].solo = true;
        assert!(!p.is_track_audible(0));
        assert!(!p.is_track_audible(1));
        p.buses[2].solo = false;
        assert!(p.is_track_audible(0) && p.is_track_audible(1));
    }

    // ----- audio clips ------------------------------------------------------

    /// An audio track holding one clip; `file` defaults to a valid staged
    /// name so tests can mutate single fields.
    fn audio_project() -> Project {
        let mut p = Project::empty();
        let mut t = create_audio_track("Imported");
        t.audio_clips.push(AudioClip {
            id: next_id(),
            file: "0123456789abcdef.wav".to_string(),
            name: "take 1.wav".to_string(),
            start: 8.0,
            length: 32.0,
            offset: 0.0,
            loops: 2,
            fade_in: 4.0,
            fade_out: 8.0,
        });
        p.tracks.push(t);
        p
    }

    #[test]
    fn audio_clip_model_round_trip_and_legacy_default() {
        let p = audio_project();
        p.validate().unwrap();
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("\"audio\":true"), "{json}");
        assert!(json.contains("\"audio_clips\""), "{json}");
        assert!(json.contains("0123456789abcdef.wav"), "{json}");
        let back: Project = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);

        // Tracks without the new keys load as ordinary pattern tracks and
        // re-serialize without them.
        let mut value = serde_json::to_value(&p).unwrap();
        for t in value["tracks"].as_array_mut().unwrap() {
            let o = t.as_object_mut().unwrap();
            o.remove("audio");
            o.remove("audio_clips");
        }
        let legacy: Project = serde_json::from_value(value).unwrap();
        assert!(legacy
            .tracks
            .iter()
            .all(|t| !t.audio && t.audio_clips.is_empty()));
        let json = serde_json::to_string(&legacy).unwrap();
        assert!(!json.contains("\"audio_clips\""), "{json}");

        // `audio_ref_name_ok` fences the store reference: bare hash names
        // pass, paths and traversal never do.
        assert!(audio_ref_name_ok("0123456789abcdef.wav"));
        for bad in ["", ".", "..", "../x.wav", "a/b.wav", "b\\c.wav", "x y.wav"] {
            assert!(!audio_ref_name_ok(bad), "{bad}");
        }
    }

    #[test]
    fn audio_clip_validation_and_normalize() {
        let p = audio_project();
        // A missing staged file is *not* an error — the clip plays silence.
        p.validate().unwrap();

        for (field, bad) in [
            (
                "file",
                Box::new(|c: &mut AudioClip| c.file = "../etc/passwd".into())
                    as Box<dyn Fn(&mut AudioClip)>,
            ),
            ("file", Box::new(|c| c.file = "abs/path.wav".into())),
            ("start", Box::new(|c| c.start = -1.0)),
            ("start", Box::new(|c| c.start = f64::NAN)),
            ("length", Box::new(|c| c.length = 0.0)),
            ("length", Box::new(|c| c.length = f64::INFINITY)),
            ("offset", Box::new(|c| c.offset = -0.5)),
            ("offset", Box::new(|c| c.offset = MAX_AUDIO_SECONDS + 1.0)),
            ("loops", Box::new(|c| c.loops = 0)),
            ("loops", Box::new(|c| c.loops = MAX_CLIP_LOOPS + 1)),
            ("fade_in", Box::new(|c| c.fade_in = c.span() + 1.0)),
            ("fade_out", Box::new(|c| c.fade_out = -0.5)),
        ] {
            let mut p = audio_project();
            bad(&mut p.tracks[1].audio_clips[0]);
            assert!(p.validate().is_err(), "bad {field} accepted");
            let _ = field;
        }

        // Id shared with pattern clips: a collision across the two lists
        // rejects, and normalize re-issues it.
        let mut p = audio_project();
        p.tracks[1].audio_clips[0].id = p.tracks[0].clips[0].id;
        assert!(p.validate().is_err());
        p.normalize();
        p.validate().unwrap();
        assert_ne!(p.tracks[1].audio_clips[0].id, p.tracks[0].clips[0].id);

        // Normalize drops unresolvable references and repairs the rest.
        let mut p = audio_project();
        p.tracks[1].audio_clips[0].offset = -3.0;
        p.tracks[1].audio_clips[0].loops = 99;
        p.tracks[1].audio_clips[0].fade_in = 10_000.0;
        p.tracks[1]
            .audio_clips
            .push(AudioClip::new("a/b.wav", "bad", 0.0, 16.0));
        p.tracks[1]
            .audio_clips
            .push(AudioClip::new("ok.wav", "lost", -4.0, 16.0));
        p.validate().unwrap_err();
        p.normalize();
        p.validate().unwrap();
        assert_eq!(p.tracks[1].audio_clips.len(), 1);
        let c = &p.tracks[1].audio_clips[0];
        assert_eq!(c.offset, 0.0);
        assert_eq!(c.loops, MAX_CLIP_LOOPS);
        assert!(c.fade_in <= c.span());

        // The per-track cap is enforced and repaired.
        let mut p = audio_project();
        p.tracks[1].audio_clips = (0..MAX_AUDIO_CLIPS_PER_TRACK + 3)
            .map(|i| AudioClip::new(format!("f{i}.wav",), "n", i as f64, 1.0))
            .collect();
        assert!(p.validate().is_err());
        p.normalize();
        assert_eq!(p.tracks[1].audio_clips.len(), MAX_AUDIO_CLIPS_PER_TRACK);
        p.validate().unwrap();
    }

    #[test]
    fn audio_track_ignores_pattern_events_but_validates() {
        // An audio lane contributes no note events even if a stray clip is
        // hand-edited into `clips` — the snapshot schedules `audio_clips`.
        let mut p = audio_project();
        let mut pattern = create_pattern(Instrument::Pad, 16);
        pattern.notes.push(Note::new(60, 0.0, 4.0, 0.8));
        let pat_id = pattern.id;
        p.tracks[1].patterns.push(pattern);
        p.tracks[1].clips.push(Clip {
            id: next_id(),
            pattern_id: pat_id,
            start: 0.0,
            loops: 1,
            fade_in: 0.0,
            fade_out: 0.0,
        });
        p.validate().unwrap();
        let lane_events = |p: &Project| {
            p.events(false)
                .iter()
                .filter(|e| e.track_index == 1)
                .count()
        };
        assert_eq!(lane_events(&p), 0, "audio lane ignores pattern clips");
        p.tracks[1].audio = false;
        assert_eq!(lane_events(&p), 1, "the same clip plays when unflagged");
    }

    #[test]
    fn chord_events_stay_out_of_note_events() {
        // Chord events are harmonic annotations, not playable notes:
        // adding them to the strip leaves `Project::events` untouched.
        let mut p = Project::empty();
        p.bars = 2;
        let t = &mut p.tracks[0];
        t.patterns[0].notes = vec![n(36, 0.0, 1.0, 1.0), n(38, 8.0, 1.0, 1.0)];
        let before = p.events(false);
        assert_eq!(p.events(true), before, "everything audible");
        p.chord_events = vec![
            ChordEvent::new(0, ChordQuality::Major, 0.0),
            ChordEvent::new(5, ChordQuality::Minor, 8.0),
            ChordEvent::new(9, ChordQuality::Dominant7, 24.0),
        ];
        p.validate().unwrap();
        assert_eq!(p.events(false), before);
        assert_eq!(p.events(true), before);
    }

    #[test]
    fn chord_events_survive_normalize_sorted() {
        // Valid but unsorted chord events are repaired into step order and
        // kept — normalize never drops annotations.
        let mut p = Project::empty();
        p.chord_events = vec![
            ChordEvent::new(9, ChordQuality::Major, 48.0),
            ChordEvent::new(0, ChordQuality::Minor7, 0.0),
            ChordEvent::new(5, ChordQuality::Dominant9, 24.0),
        ];
        p.validate().unwrap();
        p.normalize();
        p.validate().unwrap();
        assert_eq!(p.chord_events.len(), 3);
        let steps: Vec<f64> = p.chord_events.iter().map(|e| e.step).collect();
        assert_eq!(steps, [0.0, 24.0, 48.0]);
        assert!(p.chord_events.windows(2).all(|w| w[0].step <= w[1].step));
        // Identical through a JSON round trip.
        let back: Project = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(back.chord_events, p.chord_events);
    }

    #[test]
    fn normalized_project_events_subset_and_bounds() {
        // For a normalized project the audible event set is a subset of the
        // full set (mute/solo only ever removes events), and every event
        // lies inside the song span.
        let mut p = Project::empty();
        p.bars = 2;
        p.tracks[0].patterns[0].notes = vec![n(36, 0.0, 2.0, 1.0)];
        p.tracks[0].mute = true;
        let mut pad = create_track(Instrument::Pad);
        pad.patterns[0].notes = vec![n(60, 0.0, 4.0, 0.8)];
        pad.clips.push(clip(&pad.patterns[0], 16.0));
        p.tracks.push(pad);
        p.tracks[1].solo = true;
        p.normalize();
        p.validate().unwrap();

        let full = p.events(false);
        let audible = p.events(true);
        assert!(!full.is_empty() && !audible.is_empty());
        assert!(
            audible.iter().all(|e| full.contains(e)),
            "audible events must be a subset of all events"
        );
        assert!(audible.len() < full.len(), "mute/solo actually filters");
        assert!(
            audible.iter().all(|e| e.track_index != 0),
            "the muted track contributes nothing audible"
        );
        let end = p.total_steps();
        assert!(full
            .iter()
            .chain(&audible)
            .all(|e| e.step >= 0.0 && e.step + e.duration <= end + 1e-9));
        assert!(full.windows(2).all(|w| w[0].step <= w[1].step));
    }
}
