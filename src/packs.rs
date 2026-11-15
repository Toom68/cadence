//! Data-driven instrument packs.
//!
//! Instrument packs are pure JSON documents (`schema_version` 1) describing
//! synthesis presets: oscillators, optional oscillator/pitch/filter/LFO motion,
//! a resonant low-pass filter, an ADSR-style envelope, bit depth,
//! sample-and-hold and output gain. Packs contain no executable code and no audio
//! samples, so adding new sounds never requires a Rust change or a rebuild: drop
//! a `.json` pack into the user pack directory (or ship one under `packs/`) and
//! the app picks it up on *Reload Packs*.
//!
//! # Pipeline
//!
//! 1. Copy [`template_pack`] output or `packs/template/pack.json` and edit the
//!    JSON (see `packs/schema.json` for the full schema and `packs/8-bit/` for
//!    a real example).
//! 2. Validate: `cargo run -- --validate-pack path/to/pack.json`.
//! 3. Install: `cargo run -- --install-pack path/to/pack.json` (or just place
//!    the file in [`user_packs_dir`]).
//! 4. In the app choose *Reload Packs* and add the instrument to a track.
//!
//! Installed packs live in [`user_packs_dir`] as `<pack-id>.json`. Installing a
//! pack whose id matches an already installed pack replaces that file; the next
//! catalog load uses the new contents. A user pack whose `id` also matches an
//! *embedded* pack (e.g. an installed `eight-bit` update) replaces that
//! embedded pack wholesale, so improving a bundled pack needs no rebuild.
//! Embedded packs ship inside the binary via `include_str!`.
//!
//! JSON is strict: unknown fields are rejected at parse time so typos like
//! `samplehold` fail loudly instead of being silently ignored. Schema version 1
//! stays backward-compatible: new synthesis controls must be optional and default
//! to the legacy sound when omitted.
//!
//! # Drum kits
//!
//! A preset may instead carry an optional `kit` section ([`KitSpec`]): an ordered
//! list of up to [`MAX_KIT_PIECES`] named pieces, each with a MIDI `pitch` and its
//! own [`KitVoice`] (the same synthesis fields as a melodic preset, minus
//! id/name/monophonic/audition). At note-on the audio engine picks the piece whose
//! pitch is nearest the played note, so one preset can be a whole drum kit. Kit
//! presets may omit the top-level `oscillators` list entirely — those fields are
//! inert metadata kept only for validation symmetry — and pair with the drum
//! editor in the GUI.

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

/// The only pack format version this build understands.
pub const SCHEMA_VERSION: u32 = 1;

/// Largest pack file (JSON text) Cadence will read, parse or install.
pub const MAX_PACK_BYTES: u64 = 2 * 1024 * 1024;

/// Maximum number of instrument presets in one pack.
pub const MAX_INSTRUMENTS: usize = 128;

/// Maximum oscillators per preset.
pub const MAX_OSCILLATORS: usize = 4;

/// Maximum preset id length in characters.
pub const MAX_ID_LEN: usize = 96;

/// Maximum pack id length in characters.
pub const MAX_PACK_ID_LEN: usize = 64;

/// Maximum instrument/preset display name length in characters.
pub const MAX_NAME_LEN: usize = 120;

/// Maximum preset description length in characters.
pub const MAX_DESCRIPTION_LEN: usize = 1000;

/// Maximum pack description/author length in characters.
pub const MAX_PACK_META_LEN: usize = 1000;

/// Maximum pieces in one drum kit ([`KitSpec`]).
pub const MAX_KIT_PIECES: usize = 32;

/// Maximum kit piece display-name length in characters.
pub const MAX_KIT_NAME_LEN: usize = 64;

/// Environment variable overriding the user pack directory directly.
/// Intended for tests, portable installs and diagnostics.
pub const PACKS_DIR_ENV: &str = "CADENCE_PACKS_DIR";

/// Environment variable overriding Cadence's application data directory.
/// When set (and [`PACKS_DIR_ENV`] is not), packs live in `<dir>/packs`.
/// Matches `persistence::DATA_DIR_ENV`.
pub const DATA_DIR_ENV: &str = "CADENCE_DATA_DIR";

const PACK_FILE_EXT: &str = "json";

/// Oscillator waveform. Serialized as snake_case (`"saw"`, `"pulse"`, ...).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Waveform {
    #[default]
    Sine,
    Triangle,
    Saw,
    Square,
    Pulse,
    Noise,
}

/// One oscillator voice in a preset.
///
/// All fields have serde defaults so a minimal `{"waveform": "saw"}` is valid.
/// Valid ranges (enforced by [`validate_preset`]):
/// `level` 0.0..=1.0, `frequency_ratio` 0.125..=16.0 (1.0 = unison),
/// `detune_cents` -100.0..=100.0, `pulse_width` 0.05..=0.95 (fraction of the
/// cycle the pulse stays high; 0.5 = 50 % duty, only used by [`Waveform::Pulse`]),
/// optional `decay` 0.005..=20.0 seconds as a T60 layer fade.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct OscillatorSpec {
    pub waveform: Waveform,
    pub level: f32,
    pub frequency_ratio: f32,
    pub detune_cents: f32,
    pub pulse_width: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decay: Option<f32>,
}

impl Default for OscillatorSpec {
    fn default() -> Self {
        Self {
            waveform: Waveform::Sine,
            level: 1.0,
            frequency_ratio: 1.0,
            detune_cents: 0.0,
            pulse_width: 0.5,
            decay: None,
        }
    }
}

/// ADSD-style amplitude envelope in seconds.
///
/// Valid ranges: `attack` 0.0..=5.0, `decay` 0.0..=10.0,
/// `sustain` 0.0..=1.0 (level held after decay), `release` 0.005..=20.0.
/// Every field defaults, so `{"attack": 0.1}` fills the rest.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Envelope {
    pub attack: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
}

impl Default for Envelope {
    fn default() -> Self {
        Self {
            attack: 0.01,
            decay: 0.2,
            sustain: 0.7,
            release: 0.3,
        }
    }
}

/// Deterministic free-running LFO shape for optional pitch, filter and tremolo modulation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LfoWaveform {
    #[default]
    Sine,
    Triangle,
}

/// Pitch sweep: an initial semitone offset decaying exponentially to zero.
/// `decay` is T60: the offset reaches 0.001 of its starting value at that time.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct PitchEnvelope {
    pub amount_semitones: f32,
    pub decay: f32,
}

impl Default for PitchEnvelope {
    fn default() -> Self {
        Self {
            amount_semitones: 0.0,
            decay: 0.2,
        }
    }
}

/// Filter sweep in log2 cutoff space, decaying exponentially to the base cutoff.
/// Negative amounts start below the base cutoff and open toward it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct FilterEnvelope {
    pub amount_octaves: f32,
    pub decay: f32,
}

impl Default for FilterEnvelope {
    fn default() -> Self {
        Self {
            amount_octaves: 0.0,
            decay: 0.5,
        }
    }
}

/// Free-Hz note-local LFO. Pitch/filter outputs are bipolar; amplitude is tremolo depth.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct LfoSpec {
    pub waveform: LfoWaveform,
    pub rate_hz: f32,
    pub pitch_semitones: f32,
    pub filter_octaves: f32,
    pub amplitude: f32,
}

impl Default for LfoSpec {
    fn default() -> Self {
        Self {
            waveform: LfoWaveform::Sine,
            rate_hz: 1.0,
            pitch_semitones: 0.0,
            filter_octaves: 0.0,
            amplitude: 0.0,
        }
    }
}

/// A data-driven instrument preset that can be embedded in a project or loaded
/// from a pack. Only `id`, `name` and a non-empty `oscillators` list are
/// required in JSON; everything else has a sane default. The oscillator
/// requirement is waived when `kit` is present (see [`KitSpec`]).
///
/// `id` must be namespaced and slug-safe, e.g. `eight-bit.pulse-lead`
/// (lowercase `a-z`, `0-9`, `-`, `_`, `.`, must contain a `.`).
///
/// Valid ranges: `filter_cutoff` 100.0..=20000.0 Hz,
/// `filter_resonance` 0.0..=0.95, `bit_depth` 4..=24 (24 = full quality, lower
/// values crunch), `sample_hold` 1..=64 (1 = every sample, higher =
/// sample-and-hold grit), `gain` 0.0..=1.0, at most [`MAX_OSCILLATORS`]
/// oscillators, `audition_note` 0..=127 and `audition_duration` 0.1..=8.0 s.
/// Pitch/filter envelopes and oscillator decays use T60 semantics; LFO rates
/// are unsynced Hz and tremolo depth never amplifies the signal. The audition
/// fields only steer library preview centering and never reach the audio path.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InstrumentPreset {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub oscillators: Vec<OscillatorSpec>,
    #[serde(default)]
    pub envelope: Envelope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pitch_envelope: Option<PitchEnvelope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter_envelope: Option<FilterEnvelope>,
    #[serde(default = "default_filter_cutoff")]
    pub filter_cutoff: f32,
    #[serde(default, skip_serializing_if = "is_default_filter_resonance")]
    pub filter_resonance: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lfo: Option<LfoSpec>,
    #[serde(default = "default_bit_depth")]
    pub bit_depth: u8,
    #[serde(default = "default_sample_hold")]
    pub sample_hold: u32,
    #[serde(default = "default_gain")]
    pub gain: f32,
    #[serde(default)]
    pub monophonic: bool,
    /// MIDI note the library preview/audition centres on (0..=127, default 60).
    /// Preview metadata only: it never affects the rendered instrument signal.
    #[serde(
        default = "default_audition_note",
        skip_serializing_if = "is_default_audition_note"
    )]
    pub audition_note: u8,
    /// Seconds the library preview/audition holds the note (0.1..=8.0, default
    /// 0.8). Preview metadata only, like [`audition_note`](Self::audition_note).
    #[serde(
        default = "default_audition_duration",
        skip_serializing_if = "is_default_audition_duration"
    )]
    pub audition_duration: f32,
    /// Optional drum kit: when present this preset is a kit of named one-shot
    /// voices keyed by MIDI pitch instead of a single melodic voice. The
    /// top-level synthesis fields above become inert metadata (still
    /// range-validated when present, but the "at least one oscillator"
    /// requirement is waived) and `monophonic`/`audition_*` keep steering only
    /// the library preview. Kit-ness is keyed on this field alone: the track's
    /// `instrument` stays an editor choice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kit: Option<KitSpec>,
}

fn default_filter_cutoff() -> f32 {
    20_000.0
}
fn is_default_filter_resonance(value: &f32) -> bool {
    *value == 0.0
}
fn default_audition_note() -> u8 {
    60
}
fn is_default_audition_note(value: &u8) -> bool {
    *value == 60
}
fn default_audition_duration() -> f32 {
    0.8
}
fn is_default_audition_duration(value: &f32) -> bool {
    *value == 0.8
}
fn default_bit_depth() -> u8 {
    24
}
fn default_sample_hold() -> u32 {
    1
}
fn default_gain() -> f32 {
    0.9
}

/// The synthesis fields of one kit piece — identical in shape and ranges to
/// the melodic fields of [`InstrumentPreset`] (oscillators, envelope,
/// pitch_envelope, filter_envelope, filter_cutoff, filter_resonance, lfo,
/// bit_depth, sample_hold, gain). No id/name/monophonic/audition fields.
/// All fields serde-defaulted; `deny_unknown_fields`.
///
/// Every field has the same default as the matching [`InstrumentPreset`]
/// field, so `"voice": {}` parses (it then fails validation for having no
/// oscillators, like a melodic preset would).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct KitVoice {
    pub oscillators: Vec<OscillatorSpec>,
    pub envelope: Envelope,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pitch_envelope: Option<PitchEnvelope>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter_envelope: Option<FilterEnvelope>,
    #[serde(default = "default_filter_cutoff")]
    pub filter_cutoff: f32,
    #[serde(default, skip_serializing_if = "is_default_filter_resonance")]
    pub filter_resonance: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lfo: Option<LfoSpec>,
    #[serde(default = "default_bit_depth")]
    pub bit_depth: u8,
    #[serde(default = "default_sample_hold")]
    pub sample_hold: u32,
    #[serde(default = "default_gain")]
    pub gain: f32,
}

impl Default for KitVoice {
    /// Mirrors the melodic defaults of [`InstrumentPreset`].
    fn default() -> Self {
        Self {
            oscillators: Vec::new(),
            envelope: Envelope::default(),
            pitch_envelope: None,
            filter_envelope: None,
            filter_cutoff: default_filter_cutoff(),
            filter_resonance: 0.0,
            lfo: None,
            bit_depth: default_bit_depth(),
            sample_hold: default_sample_hold(),
            gain: default_gain(),
        }
    }
}

impl KitVoice {
    /// Wraps the voice in a throwaway [`InstrumentPreset`] so validation and
    /// audio conversion reuse the melodic preset paths unchanged. The stub's
    /// id/name/audition fields are placeholders that always pass validation;
    /// only the synthesis fields are meaningful. `kit` is `None` so the stub
    /// validates exactly like a melodic preset (one or more oscillators
    /// required).
    pub fn to_preset_stub(&self) -> InstrumentPreset {
        InstrumentPreset {
            id: "kit.stub".to_string(),
            name: "stub".to_string(),
            description: String::new(),
            oscillators: self.oscillators.clone(),
            envelope: self.envelope.clone(),
            pitch_envelope: self.pitch_envelope.clone(),
            filter_envelope: self.filter_envelope.clone(),
            filter_cutoff: self.filter_cutoff,
            filter_resonance: self.filter_resonance,
            lfo: self.lfo.clone(),
            bit_depth: self.bit_depth,
            sample_hold: self.sample_hold,
            gain: self.gain,
            monophonic: false,
            audition_note: 60,
            audition_duration: 0.8,
            kit: None,
        }
    }
}

/// One kit piece: display name, MIDI pitch it answers to, and its voice.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct KitPiece {
    /// Display name, 1..=[`MAX_KIT_NAME_LEN`] characters.
    pub name: String,
    /// MIDI pitch this piece plays on, 0..=127. Pitches must be unique within
    /// a kit; unmapped notes play the nearest piece.
    pub pitch: u8,
    pub voice: KitVoice,
}

/// Ordered piece list; array order is the kit's display/row order.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct KitSpec {
    /// 1..=[`MAX_KIT_PIECES`] pieces with unique pitches.
    pub pieces: Vec<KitPiece>,
}

/// A named collection of instrument presets (`schema_version` 1).
///
/// `id` is a slug such as `eight-bit` (same character rules as preset ids but
/// the namespace dot is not required). At most [`MAX_INSTRUMENTS`] presets with
/// unique ids; the serialized pack must stay under [`MAX_PACK_BYTES`].
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InstrumentPack {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub instruments: Vec<InstrumentPreset>,
}

fn default_schema_version() -> u32 {
    SCHEMA_VERSION
}

/// All instruments currently known to the app plus non-fatal load problems.
///
/// `instruments` is in deterministic order: embedded packs first (in
/// [`EMBEDDED_PACKS`] order), then user packs sorted by file name, then pack
/// order within each file. A user pack whose `id` matches an embedded pack id
/// replaces that embedded pack wholesale while keeping the embedded pack's
/// slot, so an installed update for e.g. `eight-bit` takes effect without a
/// rebuild. When two packs define the same instrument id the first definition
/// wins and a message is pushed to `errors`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PackCatalog {
    pub instruments: Vec<InstrumentPreset>,
    pub errors: Vec<String>,
}

/// Packs compiled into the binary. Add future bundled packs here.
const EMBEDDED_PACKS: &[(&str, &str)] = &[
    ("8-bit", include_str!("../packs/8-bit/pack.json")),
    ("psy-rhythm", include_str!("../packs/psy-rhythm/pack.json")),
    ("psy-bass", include_str!("../packs/psy-bass/pack.json")),
    ("psy-keys", include_str!("../packs/psy-keys/pack.json")),
    ("psy-brass", include_str!("../packs/psy-brass/pack.json")),
    ("psy-leads", include_str!("../packs/psy-leads/pack.json")),
    ("psy-arps", include_str!("../packs/psy-arps/pack.json")),
    ("psy-space", include_str!("../packs/psy-space/pack.json")),
    ("drum-kits", include_str!("../packs/drum-kits/pack.json")),
    ("guitar", include_str!("../packs/guitar/pack.json")),
    ("neon", include_str!("../packs/neon/pack.json")),
    ("one-shots", include_str!("../packs/one-shots/pack.json")),
    ("mallets", include_str!("../packs/mallets/pack.json")),
];

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Validates one preset independently of any pack (used for presets embedded
/// in project files).
pub fn validate_preset(preset: &InstrumentPreset) -> Result<(), String> {
    validate_preset_at(preset, "")
}

fn validate_preset_at(p: &InstrumentPreset, base: &str) -> Result<(), String> {
    check_slug(&p.id, MAX_ID_LEN, true, &field(base, "id"), "instrument id")?;
    check_text(&p.name, 1, MAX_NAME_LEN, &field(base, "name"), "name")?;
    check_text(
        &p.description,
        0,
        MAX_DESCRIPTION_LEN,
        &field(base, "description"),
        "description",
    )?;
    let osc_base = format!("{}oscillators", prefix(base));
    // A kit preset may leave the melodic oscillator list empty: its voices live
    // under `kit.pieces`. Any oscillators that ARE present are still validated.
    if p.oscillators.is_empty() && p.kit.is_none() {
        return Err(format!(
            "{osc_base}: a preset needs at least one oscillator"
        ));
    }
    if p.oscillators.len() > MAX_OSCILLATORS {
        return Err(format!(
            "{osc_base}: too many oscillators ({}), max {MAX_OSCILLATORS}",
            p.oscillators.len()
        ));
    }
    for (i, osc) in p.oscillators.iter().enumerate() {
        validate_oscillator(osc, &format!("{osc_base}[{i}]"))?;
    }
    let env_base = field(base, "envelope");
    check_range(&field(&env_base, "attack"), p.envelope.attack, 0.0, 5.0)?;
    check_range(&field(&env_base, "decay"), p.envelope.decay, 0.0, 10.0)?;
    check_range(&field(&env_base, "sustain"), p.envelope.sustain, 0.0, 1.0)?;
    check_range(
        &field(&env_base, "release"),
        p.envelope.release,
        0.005,
        20.0,
    )?;
    if let Some(env) = &p.pitch_envelope {
        let env_base = field(base, "pitch_envelope");
        check_range(
            &field(&env_base, "amount_semitones"),
            env.amount_semitones,
            -48.0,
            48.0,
        )?;
        check_range(&field(&env_base, "decay"), env.decay, 0.005, 10.0)?;
    }
    if let Some(env) = &p.filter_envelope {
        let env_base = field(base, "filter_envelope");
        check_range(
            &field(&env_base, "amount_octaves"),
            env.amount_octaves,
            -6.0,
            6.0,
        )?;
        check_range(&field(&env_base, "decay"), env.decay, 0.005, 20.0)?;
    }
    check_range(
        &field(base, "filter_cutoff"),
        p.filter_cutoff,
        100.0,
        20_000.0,
    )?;
    check_range(
        &field(base, "filter_resonance"),
        p.filter_resonance,
        0.0,
        0.95,
    )?;
    if let Some(lfo) = &p.lfo {
        let lfo_base = field(base, "lfo");
        check_range(&field(&lfo_base, "rate_hz"), lfo.rate_hz, 0.05, 20.0)?;
        check_range(
            &field(&lfo_base, "pitch_semitones"),
            lfo.pitch_semitones,
            0.0,
            12.0,
        )?;
        check_range(
            &field(&lfo_base, "filter_octaves"),
            lfo.filter_octaves,
            0.0,
            4.0,
        )?;
        check_range(&field(&lfo_base, "amplitude"), lfo.amplitude, 0.0, 1.0)?;
    }
    check_range_int(&field(base, "bit_depth"), p.bit_depth, 4, 24, "bit depth")?;
    check_range_u64(
        &field(base, "sample_hold"),
        p.sample_hold as u64,
        1,
        64,
        "sample hold",
    )?;
    check_range(&field(base, "gain"), p.gain, 0.0, 1.0)?;
    check_range_int(
        &field(base, "audition_note"),
        p.audition_note,
        0,
        127,
        "audition note",
    )?;
    check_range(
        &field(base, "audition_duration"),
        p.audition_duration,
        0.1,
        8.0,
    )?;
    if let Some(kit) = &p.kit {
        validate_kit(kit, &field(base, "kit"))?;
    }
    Ok(())
}

/// Validates a [`KitSpec`]: piece count, per-piece name/pitch, each piece's
/// voice (via [`KitVoice::to_preset_stub`], so errors carry the full
/// `kit.pieces[i].voice.*` path) and pitch uniqueness.
fn validate_kit(kit: &KitSpec, base: &str) -> Result<(), String> {
    let pieces_base = field(base, "pieces");
    if kit.pieces.is_empty() {
        return Err(format!(
            "{pieces_base}: a drum kit needs at least one piece"
        ));
    }
    if kit.pieces.len() > MAX_KIT_PIECES {
        return Err(format!(
            "{pieces_base}: too many pieces ({}), max {MAX_KIT_PIECES}",
            kit.pieces.len()
        ));
    }
    let mut pitch_owner: HashMap<u8, usize> = HashMap::new();
    for (i, piece) in kit.pieces.iter().enumerate() {
        let piece_base = format!("{pieces_base}[{i}]");
        check_text(
            &piece.name,
            1,
            MAX_KIT_NAME_LEN,
            &field(&piece_base, "name"),
            "name",
        )?;
        check_range_int(&field(&piece_base, "pitch"), piece.pitch, 0, 127, "pitch")?;
        validate_preset_at(&piece.voice.to_preset_stub(), &field(&piece_base, "voice"))?;
        if let Some(&first) = pitch_owner.get(&piece.pitch) {
            return Err(format!(
                "{piece_base}.pitch: pitch {} is already used by kit piece {first}",
                piece.pitch
            ));
        }
        pitch_owner.insert(piece.pitch, i);
    }
    Ok(())
}

fn validate_oscillator(osc: &OscillatorSpec, base: &str) -> Result<(), String> {
    check_range(&field(base, "level"), osc.level, 0.0, 1.0)?;
    check_range(
        &field(base, "frequency_ratio"),
        osc.frequency_ratio,
        0.125,
        16.0,
    )?;
    check_range(
        &field(base, "detune_cents"),
        osc.detune_cents,
        -100.0,
        100.0,
    )?;
    check_range(&field(base, "pulse_width"), osc.pulse_width, 0.05, 0.95)?;
    if let Some(decay) = osc.decay {
        check_range(&field(base, "decay"), decay, 0.005, 20.0)?;
    }
    Ok(())
}

/// Validates a whole pack: schema version, ids, names, limits, unique preset
/// ids and the serialized size cap.
pub fn validate_pack(pack: &InstrumentPack) -> Result<(), String> {
    if pack.schema_version != SCHEMA_VERSION {
        return Err(format!(
            "schema_version: unsupported version {} (this Cadence supports {SCHEMA_VERSION})",
            pack.schema_version
        ));
    }
    check_slug(&pack.id, MAX_PACK_ID_LEN, false, "id", "pack id")?;
    check_text(&pack.name, 1, MAX_NAME_LEN, "name", "pack name")?;
    check_text(
        &pack.description,
        0,
        MAX_PACK_META_LEN,
        "description",
        "pack description",
    )?;
    check_text(&pack.author, 0, MAX_PACK_META_LEN, "author", "author")?;
    if pack.instruments.is_empty() {
        return Err("instruments: a pack must contain at least one instrument".to_string());
    }
    if pack.instruments.len() > MAX_INSTRUMENTS {
        return Err(format!(
            "instruments: too many instruments ({}), max {MAX_INSTRUMENTS}",
            pack.instruments.len()
        ));
    }
    let mut seen = std::collections::HashSet::new();
    for (i, preset) in pack.instruments.iter().enumerate() {
        let base = format!("instruments[{i}]");
        validate_preset_at(preset, &base)?;
        if !seen.insert(preset.id.as_str()) {
            return Err(format!(
                "{base}.id: duplicate instrument id '{}'",
                preset.id
            ));
        }
    }
    let json = serde_json::to_string(pack)
        .map_err(|err| format!("Could not serialize the pack for validation: {err}"))?;
    if json.len() as u64 > MAX_PACK_BYTES {
        return Err(too_large_message(json.len() as u64));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Parsing / loading / installing
// ---------------------------------------------------------------------------

/// Parses and validates pack JSON text.
pub fn parse_pack(json: &str) -> Result<InstrumentPack, String> {
    if json.len() as u64 > MAX_PACK_BYTES {
        return Err(too_large_message(json.len() as u64));
    }
    let pack: InstrumentPack = serde_json::from_str(json.trim_start_matches('\u{feff}'))
        .map_err(|err| format!("This is not a valid instrument pack: {err}"))?;
    validate_pack(&pack).map_err(|err| format!("Invalid instrument pack: {err}"))?;
    Ok(pack)
}

/// Loads, size-checks, parses and validates a pack file.
pub fn load_pack(path: &Path) -> Result<InstrumentPack, String> {
    let text = read_text_capped(path)?;
    parse_pack(&text).map_err(|err| format!("{}: {err}", path.display()))
}

/// Validates the pack at `path` and installs it into [`user_packs_dir`] as
/// `<pack-id>.json`, atomically replacing any pack previously installed under
/// the same id. Returns the installed file path.
pub fn install_pack(path: &Path) -> Result<PathBuf, String> {
    let pack = load_pack(path)?;
    install_validated(&pack, &user_packs_dir())
}

/// Serializes a validated pack to pretty JSON (with trailing newline).
pub fn pack_to_json(pack: &InstrumentPack) -> Result<String, String> {
    validate_pack(pack).map_err(|err| format!("Cannot save an invalid pack: {err}"))?;
    let mut text = serde_json::to_string_pretty(pack)
        .map_err(|err| format!("Could not serialize the pack: {err}"))?;
    text.push('\n');
    if text.len() as u64 > MAX_PACK_BYTES {
        return Err(too_large_message(text.len() as u64));
    }
    Ok(text)
}

/// Directory holding user-installed packs (`<id>.json` files).
///
/// Resolution order: `$CADENCE_PACKS_DIR`, then `$CADENCE_DATA_DIR/packs`,
/// then the platform application data directory (`.../Cadence/packs`).
pub fn user_packs_dir() -> PathBuf {
    resolve_packs_dir(&|key| std::env::var_os(key))
}

fn resolve_packs_dir(env: &dyn Fn(&str) -> Option<OsString>) -> PathBuf {
    if let Some(dir) = env(PACKS_DIR_ENV).filter(|dir| !dir.is_empty()) {
        return PathBuf::from(dir);
    }
    if let Some(dir) = env(DATA_DIR_ENV).filter(|dir| !dir.is_empty()) {
        return PathBuf::from(dir).join("packs");
    }
    directories::ProjectDirs::from("studio", "Cadence", "Cadence")
        .map(|dirs| dirs.data_local_dir().join("packs"))
        .unwrap_or_else(|| std::env::temp_dir().join("cadence-studio").join("packs"))
}

/// Loads the full instrument catalog: every embedded pack plus every `*.json`
/// file in [`user_packs_dir`]. Never fails as a whole; per-file problems are
/// reported in [`PackCatalog::errors`].
pub fn load_catalog() -> PackCatalog {
    load_catalog_from(&user_packs_dir())
}

/// Like [`load_catalog`] but reads user packs from `dir` instead of
/// [`user_packs_dir`]. Embedded packs are always included.
///
/// Pack-level override: a user pack whose `id` equals an embedded pack id
/// replaces that embedded pack wholesale, keeping the embedded pack's position
/// (deterministic order). The first matching file in sorted order wins; other
/// files with the same pack id load as ordinary packs. Files that fail to
/// parse or validate never override anything, so the bundled pack remains as
/// fallback and the problem is reported in [`PackCatalog::errors`].
pub fn load_catalog_from(dir: &Path) -> PackCatalog {
    let mut catalog = PackCatalog::default();
    let mut origins: HashMap<String, String> = HashMap::new();

    // Embedded packs ship inside the binary. Parse defensively: a broken
    // embedded pack is reported but never fatal.
    let mut embedded: Vec<(String, InstrumentPack)> = Vec::new();
    for (label, json) in EMBEDDED_PACKS {
        match parse_pack(json) {
            Ok(pack) => embedded.push((format!("embedded pack '{label}'"), pack)),
            Err(err) => catalog
                .errors
                .push(format!("embedded pack '{label}': {err}")),
        }
    }

    // User packs in deterministic (sorted file name) order.
    let mut user: Vec<(String, InstrumentPack)> = Vec::new();
    for path in pack_files_in(dir) {
        let origin = path.display().to_string();
        match read_text_capped(&path).and_then(|text| parse_pack(&text)) {
            Ok(pack) => user.push((origin, pack)),
            Err(err) => catalog.errors.push(format!("{origin}: {err}")),
        }
    }

    // Match user packs to embedded packs they override (embedded index ->
    // user index, and the set of user packs consumed as overrides).
    let mut embedded_override: HashMap<usize, usize> = HashMap::new();
    let mut user_is_override: HashSet<usize> = HashSet::new();
    for (ui, (_, upack)) in user.iter().enumerate() {
        let target = embedded.iter().enumerate().find_map(|(ei, (_, epack))| {
            (epack.id == upack.id && !embedded_override.contains_key(&ei)).then_some(ei)
        });
        if let Some(ei) = target {
            embedded_override.insert(ei, ui);
            user_is_override.insert(ui);
        }
    }

    for (ei, (label, epack)) in embedded.iter().enumerate() {
        match embedded_override.get(&ei) {
            Some(&ui) => {
                let (origin, upack) = &user[ui];
                absorb_pack(upack, origin, &mut catalog, &mut origins);
            }
            None => absorb_pack(epack, label, &mut catalog, &mut origins),
        }
    }
    for (ui, (origin, upack)) in user.iter().enumerate() {
        if user_is_override.contains(&ui) {
            continue;
        }
        absorb_pack(upack, origin, &mut catalog, &mut origins);
    }
    catalog
}

/// Sorted list of `*.json` files directly inside `dir` (missing dir = empty).
fn pack_files_in(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let is_pack = path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case(PACK_FILE_EXT));
            if is_pack && path.is_file() {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// Builds a minimal valid one-instrument pack for scaffolding (CLI
/// `--new-pack`). `id` is the pack id; the preset id becomes `<id>.<slug>`.
pub fn template_pack(id: &str, name: &str) -> Result<InstrumentPack, String> {
    check_slug(id, MAX_PACK_ID_LEN, false, "id", "pack id")?;
    let pack_name = if name.trim().is_empty() {
        id.to_string()
    } else {
        name.to_string()
    };
    let preset_name = if name.trim().is_empty() {
        pack_name.clone()
    } else {
        name.trim().to_string()
    };
    let mut slug = slugify(&pack_name);
    if slug.is_empty() {
        slug = "preset".to_string();
    }
    // Keep "<id>.<slug>" within MAX_ID_LEN.
    let slug_budget = MAX_ID_LEN.saturating_sub(id.len() + 1).max(1);
    let slug: String = slug.chars().take(slug_budget).collect();
    let preset_id = format!("{id}.{slug}");
    let pack = InstrumentPack {
        schema_version: SCHEMA_VERSION,
        id: id.to_string(),
        name: pack_name,
        description: "A new instrument pack.".to_string(),
        author: String::new(),
        instruments: vec![InstrumentPreset {
            id: preset_id,
            name: preset_name,
            description: "TODO: describe this preset.".to_string(),
            oscillators: vec![
                OscillatorSpec {
                    waveform: Waveform::Pulse,
                    level: 0.8,
                    frequency_ratio: 1.0,
                    detune_cents: 0.0,
                    pulse_width: 0.25,
                    decay: None,
                },
                OscillatorSpec {
                    waveform: Waveform::Sine,
                    level: 0.4,
                    frequency_ratio: 2.0,
                    detune_cents: 0.0,
                    pulse_width: 0.5,
                    decay: None,
                },
            ],
            envelope: Envelope {
                attack: 0.005,
                decay: 0.12,
                sustain: 0.0,
                release: 0.08,
            },
            pitch_envelope: None,
            filter_envelope: None,
            filter_cutoff: 4_000.0,
            filter_resonance: 0.0,
            lfo: None,
            bit_depth: 8,
            sample_hold: 4,
            gain: 0.8,
            monophonic: false,
            audition_note: 60,
            audition_duration: 0.8,
            kit: None,
        }],
    };
    validate_pack(&pack)?;
    Ok(pack)
}

/// Deterministic single-cycle lookup for previews/tests, normalized to
/// roughly [-1, 1]. `phase` is the position within the waveform cycle
/// (any real value; wrapped with `rem_euclid`). `pulse_width` is a fraction of
/// the cycle (see [`OscillatorSpec::pulse_width`]).
///
/// [`Waveform::Noise`] cannot be phase-driven, so it returns a deterministic
/// hash of `phase`; the audio engine should use its own seeded per-voice RNG
/// for real noise voices.
pub fn sample_wave(waveform: Waveform, phase: f32, pulse_width: f32) -> f32 {
    let t = phase.rem_euclid(1.0);
    match waveform {
        Waveform::Sine => (t * std::f32::consts::TAU).sin(),
        Waveform::Triangle => 4.0 * (t - 0.5).abs() - 1.0,
        Waveform::Saw => 2.0 * t - 1.0,
        Waveform::Square => {
            if t < 0.5 {
                1.0
            } else {
                -1.0
            }
        }
        Waveform::Pulse => {
            if t < pulse_width.clamp(0.0, 1.0) {
                1.0
            } else {
                -1.0
            }
        }
        Waveform::Noise => noise_hash(t),
    }
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

fn prefix(base: &str) -> String {
    if base.is_empty() {
        String::new()
    } else {
        format!("{base}.")
    }
}

fn field(base: &str, name: &str) -> String {
    format!("{}{name}", prefix(base))
}

fn is_slug_char(c: char) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.')
}

/// Slug ids: 1..=max_len chars of `[a-z0-9._-]`, never starting/ending with
/// `.` or containing `..`. Preset ids additionally must contain a `.`
/// (namespace, e.g. `eight-bit.pulse-lead`). Pack ids additionally must not be
/// a Windows device name because they become file names.
fn check_slug(
    id: &str,
    max_len: usize,
    require_namespace: bool,
    path: &str,
    what: &str,
) -> Result<(), String> {
    let len = id.chars().count();
    if len == 0 || len > max_len {
        return Err(format!(
            "{path}: {what} must be 1-{max_len} characters (got {len})"
        ));
    }
    if !id.chars().all(is_slug_char) {
        return Err(format!(
            "{path}: {what} '{id}' may only contain a-z, 0-9, '-', '_' and '.'"
        ));
    }
    if id.starts_with('.') || id.ends_with('.') || id.contains("..") {
        return Err(format!(
            "{path}: {what} '{id}' must not start or end with '.' or contain '..'"
        ));
    }
    if require_namespace && !id.contains('.') {
        return Err(format!(
            "{path}: {what} '{id}' must be namespaced like 'pack-name.instrument-name'"
        ));
    }
    if !require_namespace && is_reserved_device_name(id) {
        return Err(format!("{path}: pack id '{id}' is a reserved device name"));
    }
    Ok(())
}

fn is_reserved_device_name(id: &str) -> bool {
    let base = id.split('.').next().unwrap_or(id);
    matches!(
        base,
        "con"
            | "prn"
            | "aux"
            | "nul"
            | "com1"
            | "com2"
            | "com3"
            | "com4"
            | "com5"
            | "com6"
            | "com7"
            | "com8"
            | "com9"
            | "lpt1"
            | "lpt2"
            | "lpt3"
            | "lpt4"
            | "lpt5"
            | "lpt6"
            | "lpt7"
            | "lpt8"
            | "lpt9"
    )
}

fn check_text(s: &str, min: usize, max: usize, path: &str, what: &str) -> Result<(), String> {
    let len = s.chars().count();
    if len < min {
        return Err(format!("{path}: {what} must not be empty"));
    }
    if len > max {
        return Err(format!(
            "{path}: {what} is too long ({len} characters, max {max})"
        ));
    }
    Ok(())
}

fn check_finite(path: &str, value: f32) -> Result<(), String> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(format!("{path}: must be a finite number"))
    }
}

fn check_range(path: &str, value: f32, min: f32, max: f32) -> Result<(), String> {
    check_finite(path, value)?;
    if value >= min && value <= max {
        Ok(())
    } else {
        Err(format!(
            "{path}: {value} is outside the allowed range {min}..={max}"
        ))
    }
}

fn check_range_int(path: &str, value: u8, min: u8, max: u8, what: &str) -> Result<(), String> {
    if value >= min && value <= max {
        Ok(())
    } else {
        Err(format!(
            "{path}: {what} {value} is outside the allowed range {min}..={max}"
        ))
    }
}

fn check_range_u64(path: &str, value: u64, min: u64, max: u64, what: &str) -> Result<(), String> {
    if value >= min && value <= max {
        Ok(())
    } else {
        Err(format!(
            "{path}: {what} {value} is outside the allowed range {min}..={max}"
        ))
    }
}

fn too_large_message(bytes: u64) -> String {
    format!(
        "The pack is too large ({:.1} MB). Instrument packs are limited to {} MB.",
        bytes as f64 / (1024.0 * 1024.0),
        MAX_PACK_BYTES / (1024 * 1024)
    )
}

fn slugify(text: &str) -> String {
    let mut out = String::new();
    let mut pending_dash = false;
    for c in text.chars().flat_map(|c| c.to_lowercase()) {
        if c.is_ascii_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(c);
        } else {
            pending_dash = true;
        }
    }
    out
}

fn absorb_pack(
    pack: &InstrumentPack,
    origin: &str,
    catalog: &mut PackCatalog,
    origins: &mut HashMap<String, String>,
) {
    for preset in &pack.instruments {
        match origins.entry(preset.id.clone()) {
            Entry::Vacant(slot) => {
                slot.insert(origin.to_string());
                catalog.instruments.push(preset.clone());
            }
            Entry::Occupied(slot) => {
                catalog.errors.push(format!(
                    "duplicate instrument id '{}': keeping the version from {}, ignoring '{}' from {}",
                    preset.id,
                    slot.get(),
                    preset.name,
                    origin
                ));
            }
        }
    }
}

fn install_validated(pack: &InstrumentPack, dir: &Path) -> Result<PathBuf, String> {
    let text = pack_to_json(pack)?;
    let target = dir.join(format!("{}.json", pack.id));
    write_atomic(&target, text.as_bytes())?;
    Ok(target)
}

/// Reads a file as UTF-8 text with the pack size cap enforced both before and
/// during the read.
fn read_text_capped(path: &Path) -> Result<String, String> {
    let shown = path.display();
    let meta = fs::metadata(path).map_err(|err| format!("Could not open {shown}: {err}"))?;
    if !meta.is_file() {
        return Err(format!("{shown} is not a file."));
    }
    if meta.len() > MAX_PACK_BYTES {
        return Err(too_large_message(meta.len()));
    }
    let file = File::open(path).map_err(|err| format!("Could not open {shown}: {err}"))?;
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    file.take(MAX_PACK_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|err| format!("Could not read {shown}: {err}"))?;
    if bytes.len() as u64 > MAX_PACK_BYTES {
        return Err(too_large_message(bytes.len() as u64));
    }
    String::from_utf8(bytes)
        .map_err(|_| format!("{shown} is not an instrument pack (it is not UTF-8 text)."))
}

/// Writes `bytes` to a temp file beside `path`, syncs and renames over `path`.
/// `std::fs::rename` replaces existing files atomically on Unix and Windows.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let shown = path.display();
    let file_name = path
        .file_name()
        .ok_or_else(|| format!("{shown} is not a valid file name."))?;
    let dir = match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.to_path_buf(),
        _ => PathBuf::from("."),
    };
    fs::create_dir_all(&dir).map_err(|err| format!("Could not create {}: {err}", dir.display()))?;

    let mut tmp_name = OsString::from(".");
    tmp_name.push(file_name);
    tmp_name.push(format!(
        ".{}.{}.tmp",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let tmp = dir.join(tmp_name);

    let result = (|| -> std::io::Result<()> {
        let mut file = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        if let Ok(meta) = fs::metadata(path) {
            if meta.permissions().readonly() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "the existing file is read-only",
                ));
            }
            let _ = fs::set_permissions(&tmp, meta.permissions());
        }
        fs::rename(&tmp, path)?;
        sync_dir(&dir);
        Ok(())
    })();

    result.map_err(|err| {
        let _ = fs::remove_file(&tmp);
        format!("Could not install the pack to {shown}: {err}")
    })
}

#[cfg(unix)]
fn sync_dir(dir: &Path) {
    if let Ok(handle) = File::open(dir) {
        let _ = handle.sync_all();
    }
}

#[cfg(not(unix))]
fn sync_dir(_dir: &Path) {}

fn noise_hash(phase: f32) -> f32 {
    // Deterministic integer hash of the phase bits (splitmix-style finalizer).
    let mut x = phase.to_bits() ^ 0x9E37_79B9;
    x = x.wrapping_mul(0x85EB_CA6B);
    x ^= x >> 13;
    x = x.wrapping_mul(0xC2B2_AE35);
    x ^= x >> 16;
    (x as f32 / u32::MAX as f32) * 2.0 - 1.0
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_preset() -> InstrumentPreset {
        InstrumentPreset {
            id: "test-pack.lead".to_string(),
            name: "Test Lead".to_string(),
            description: String::new(),
            oscillators: vec![OscillatorSpec {
                waveform: Waveform::Pulse,
                level: 0.8,
                frequency_ratio: 1.0,
                detune_cents: 0.0,
                pulse_width: 0.25,
                decay: None,
            }],
            envelope: Envelope {
                attack: 0.005,
                decay: 0.12,
                sustain: 0.3,
                release: 0.08,
            },
            pitch_envelope: None,
            filter_envelope: None,
            filter_cutoff: 4_000.0,
            filter_resonance: 0.0,
            lfo: None,
            bit_depth: 8,
            sample_hold: 4,
            gain: 0.8,
            monophonic: true,
            audition_note: 60,
            audition_duration: 0.8,
            kit: None,
        }
    }

    fn valid_pack() -> InstrumentPack {
        InstrumentPack {
            schema_version: SCHEMA_VERSION,
            id: "test-pack".to_string(),
            name: "Test Pack".to_string(),
            description: String::new(),
            author: String::new(),
            instruments: vec![valid_preset()],
        }
    }

    /// A valid two-piece kit preset: empty top-level `oscillators` (allowed
    /// for kits) and per-piece voices.
    fn kit_preset() -> InstrumentPreset {
        InstrumentPreset {
            id: "test-pack.kit".to_string(),
            name: "Test Kit".to_string(),
            description: String::new(),
            oscillators: Vec::new(),
            envelope: Envelope::default(),
            pitch_envelope: None,
            filter_envelope: None,
            filter_cutoff: 20_000.0,
            filter_resonance: 0.0,
            lfo: None,
            bit_depth: 24,
            sample_hold: 1,
            gain: 0.9,
            monophonic: false,
            audition_note: 60,
            audition_duration: 0.8,
            kit: Some(KitSpec {
                pieces: vec![
                    KitPiece {
                        name: "Kick".to_string(),
                        pitch: 36,
                        voice: KitVoice {
                            oscillators: vec![OscillatorSpec {
                                waveform: Waveform::Sine,
                                level: 1.0,
                                frequency_ratio: 0.5,
                                ..Default::default()
                            }],
                            envelope: Envelope {
                                attack: 0.001,
                                decay: 0.4,
                                sustain: 0.0,
                                release: 0.05,
                            },
                            pitch_envelope: Some(PitchEnvelope {
                                amount_semitones: -24.0,
                                decay: 0.08,
                            }),
                            filter_cutoff: 4_000.0,
                            ..Default::default()
                        },
                    },
                    KitPiece {
                        name: "Snare".to_string(),
                        pitch: 38,
                        voice: KitVoice {
                            oscillators: vec![
                                OscillatorSpec {
                                    waveform: Waveform::Noise,
                                    level: 0.7,
                                    ..Default::default()
                                },
                                OscillatorSpec {
                                    waveform: Waveform::Triangle,
                                    level: 0.4,
                                    frequency_ratio: 1.8,
                                    ..Default::default()
                                },
                            ],
                            envelope: Envelope {
                                attack: 0.001,
                                decay: 0.18,
                                sustain: 0.0,
                                release: 0.04,
                            },
                            filter_cutoff: 8_000.0,
                            ..Default::default()
                        },
                    },
                ],
            }),
        }
    }

    #[test]
    fn valid_preset_and_pack_pass() {
        assert!(validate_preset(&valid_preset()).is_ok());
        assert!(validate_pack(&valid_pack()).is_ok());
    }

    #[test]
    fn serde_defaults_fill_missing_fields() {
        let json = r#"{
            "id": "a.b",
            "name": "Minimal",
            "oscillators": [{"waveform": "saw"}]
        }"#;
        let preset: InstrumentPreset = serde_json::from_str(json).unwrap();
        assert_eq!(preset.bit_depth, 24);
        assert_eq!(preset.sample_hold, 1);
        assert_eq!(preset.gain, 0.9);
        assert_eq!(preset.filter_cutoff, 20_000.0);
        assert_eq!(preset.filter_resonance, 0.0);
        assert_eq!(preset.pitch_envelope, None);
        assert_eq!(preset.filter_envelope, None);
        assert_eq!(preset.lfo, None);
        assert_eq!(preset.audition_note, 60);
        assert_eq!(preset.audition_duration, 0.8);
        assert!(!preset.monophonic);
        assert_eq!(preset.oscillators[0].waveform, Waveform::Saw);
        assert_eq!(preset.oscillators[0].decay, None);
        assert!(validate_preset(&preset).is_ok());

        // Explicit audition metadata parses and round-trips.
        let json = r#"{
            "id": "a.b",
            "name": "Cue",
            "oscillators": [{}],
            "audition_note": 36,
            "audition_duration": 1.5
        }"#;
        let preset: InstrumentPreset = serde_json::from_str(json).unwrap();
        assert_eq!(preset.audition_note, 36);
        assert_eq!(preset.audition_duration, 1.5);
        assert!(validate_preset(&preset).is_ok());
        let text = serde_json::to_string(&preset).unwrap();
        assert!(text.contains("\"audition_note\":36"), "{text}");
        assert!(text.contains("\"audition_duration\":1.5"), "{text}");
        let back: InstrumentPreset = serde_json::from_str(&text).unwrap();
        assert_eq!(back, preset);

        let pack_json =
            r#"{"id":"a","name":"P","instruments":[{"id":"a.b","name":"B","oscillators":[{}]}]}"#;
        let pack = parse_pack(pack_json).unwrap();
        assert_eq!(pack.schema_version, 1);
        assert_eq!(pack.instruments[0].oscillators[0].waveform, Waveform::Sine);
    }

    #[test]
    fn preset_id_must_be_namespaced_slug() {
        for bad in [
            "",
            "lead",
            "Eight.Bit",
            "a..b",
            ".a.b",
            "a.b.",
            "a/b.c",
            "a b.c",
        ] {
            let mut p = valid_preset();
            p.id = bad.to_string();
            let err = validate_preset(&p).unwrap_err();
            assert!(err.starts_with("id:"), "bad id {bad:?} gave: {err}");
        }
        for good in ["a.b", "eight-bit.pulse-lead", "x.y_z-9.q"] {
            let mut p = valid_preset();
            p.id = good.to_string();
            assert!(validate_preset(&p).is_ok(), "id {good:?} should pass");
        }
    }

    #[test]
    fn preset_field_ranges_are_enforced_with_paths() {
        type Mutate = fn(&mut InstrumentPreset);
        let cases: Vec<(Mutate, &str)> = vec![
            (|p| p.oscillators[0].level = 1.5, "oscillators[0].level"),
            (|p| p.oscillators[0].level = -0.1, "oscillators[0].level"),
            (
                |p| p.oscillators[0].frequency_ratio = 0.01,
                "oscillators[0].frequency_ratio",
            ),
            (
                |p| p.oscillators[0].frequency_ratio = 20.0,
                "oscillators[0].frequency_ratio",
            ),
            (
                |p| p.oscillators[0].detune_cents = -101.0,
                "oscillators[0].detune_cents",
            ),
            (
                |p| p.oscillators[0].pulse_width = 0.0,
                "oscillators[0].pulse_width",
            ),
            (
                |p| p.oscillators[0].pulse_width = 1.0,
                "oscillators[0].pulse_width",
            ),
            (
                |p| p.oscillators[0].decay = Some(0.001),
                "oscillators[0].decay",
            ),
            (|p| p.envelope.attack = 6.0, "envelope.attack"),
            (|p| p.envelope.decay = -1.0, "envelope.decay"),
            (|p| p.envelope.sustain = 1.5, "envelope.sustain"),
            (|p| p.envelope.release = 0.001, "envelope.release"),
            (
                |p| {
                    p.pitch_envelope = Some(PitchEnvelope {
                        amount_semitones: 49.0,
                        decay: 0.2,
                    })
                },
                "pitch_envelope.amount_semitones",
            ),
            (
                |p| {
                    p.pitch_envelope = Some(PitchEnvelope {
                        amount_semitones: 0.0,
                        decay: 0.001,
                    })
                },
                "pitch_envelope.decay",
            ),
            (
                |p| {
                    p.filter_envelope = Some(FilterEnvelope {
                        amount_octaves: -7.0,
                        decay: 0.5,
                    })
                },
                "filter_envelope.amount_octaves",
            ),
            (
                |p| {
                    p.filter_envelope = Some(FilterEnvelope {
                        amount_octaves: 0.0,
                        decay: 30.0,
                    })
                },
                "filter_envelope.decay",
            ),
            (|p| p.filter_cutoff = 50.0, "filter_cutoff"),
            (|p| p.filter_cutoff = 30_000.0, "filter_cutoff"),
            (|p| p.filter_resonance = 1.0, "filter_resonance"),
            (
                |p| {
                    p.lfo = Some(LfoSpec {
                        rate_hz: 0.01,
                        ..Default::default()
                    })
                },
                "lfo.rate_hz",
            ),
            (
                |p| {
                    p.lfo = Some(LfoSpec {
                        pitch_semitones: 13.0,
                        ..Default::default()
                    })
                },
                "lfo.pitch_semitones",
            ),
            (
                |p| {
                    p.lfo = Some(LfoSpec {
                        filter_octaves: 5.0,
                        ..Default::default()
                    })
                },
                "lfo.filter_octaves",
            ),
            (
                |p| {
                    p.lfo = Some(LfoSpec {
                        amplitude: 1.1,
                        ..Default::default()
                    })
                },
                "lfo.amplitude",
            ),
            (|p| p.bit_depth = 3, "bit_depth"),
            (|p| p.bit_depth = 25, "bit_depth"),
            (|p| p.sample_hold = 0, "sample_hold"),
            (|p| p.sample_hold = 65, "sample_hold"),
            (|p| p.gain = 1.5, "gain"),
            (|p| p.audition_note = 128, "audition_note"),
            (|p| p.audition_duration = 0.05, "audition_duration"),
            (|p| p.audition_duration = 8.5, "audition_duration"),
        ];
        for (mutate, path) in cases {
            let mut p = valid_preset();
            mutate(&mut p);
            let err = validate_preset(&p).unwrap_err();
            assert!(err.contains(path), "expected path {path:?} in: {err}");
        }
    }

    #[test]
    fn non_finite_values_are_rejected() {
        let mut p = valid_preset();
        p.oscillators[0].level = f32::NAN;
        assert!(validate_preset(&p).unwrap_err().contains("finite"));
        let mut p = valid_preset();
        p.filter_cutoff = f32::INFINITY;
        assert!(validate_preset(&p).unwrap_err().contains("finite"));
        let mut p = valid_preset();
        p.pitch_envelope = Some(PitchEnvelope {
            amount_semitones: f32::NAN,
            decay: 0.2,
        });
        let err = validate_preset(&p).unwrap_err();
        assert!(err.contains("pitch_envelope.amount_semitones") && err.contains("finite"));
        let mut p = valid_preset();
        p.audition_duration = f32::INFINITY;
        let err = validate_preset(&p).unwrap_err();
        assert!(err.contains("audition_duration") && err.contains("finite"));
        // Audition note is an integer field: JSON floats/strings/out-of-u8-range
        // values all fail at parse time.
        for bad in ["60.5", "\"60\"", "-1", "300"] {
            parse_pack(&format!(
                r#"{{"id":"a","name":"P","instruments":[{{"id":"a.b","name":"B","oscillators":[{{}}],"audition_note":{bad}}}]}}"#
            ))
            .unwrap_err();
        }
    }

    #[test]
    fn oscillator_count_limits() {
        let mut p = valid_preset();
        p.oscillators.clear();
        assert!(validate_preset(&p).unwrap_err().contains("oscillators"));
        let mut p = valid_preset();
        p.oscillators = vec![OscillatorSpec::default(); MAX_OSCILLATORS + 1];
        assert!(validate_preset(&p)
            .unwrap_err()
            .contains("too many oscillators"));
    }

    #[test]
    fn pack_level_rules() {
        // Unsupported schema version.
        let mut pack = valid_pack();
        pack.schema_version = 2;
        assert!(validate_pack(&pack).unwrap_err().contains("schema_version"));

        // Empty instruments.
        let mut pack = valid_pack();
        pack.instruments.clear();
        assert!(validate_pack(&pack).unwrap_err().contains("at least one"));

        // Too many instruments.
        let mut pack = valid_pack();
        pack.instruments = (0..=MAX_INSTRUMENTS)
            .map(|i| {
                let mut p = valid_preset();
                p.id = format!("test-pack.p{i}");
                p
            })
            .collect();
        assert!(validate_pack(&pack).unwrap_err().contains("too many"));

        // Duplicate ids inside one pack, error path points at the preset.
        let mut pack = valid_pack();
        let mut dup = valid_preset();
        dup.name = "Copy".to_string();
        pack.instruments.push(dup);
        assert!(validate_pack(&pack)
            .unwrap_err()
            .contains("instruments[1].id: duplicate"));

        // Pack id traversal/charset.
        for bad in ["../evil", "a/b", "UPPER", "", "a..b"] {
            let mut pack = valid_pack();
            pack.id = bad.to_string();
            assert!(validate_pack(&pack).is_err(), "pack id {bad:?} should fail");
        }
        // Reserved device name (becomes a file name on install).
        let mut pack = valid_pack();
        pack.id = "con".to_string();
        assert!(validate_pack(&pack).unwrap_err().contains("reserved"));

        // Nested preset error is reported with its full path.
        let mut pack = valid_pack();
        pack.instruments[0].gain = 2.0;
        assert!(validate_pack(&pack)
            .unwrap_err()
            .contains("instruments[0].gain"));
    }

    #[test]
    fn parse_pack_rejects_bad_json_and_oversize() {
        assert!(parse_pack("not json").is_err());
        assert!(parse_pack("{\"id\":\"a\"").is_err());
        let huge = " ".repeat(MAX_PACK_BYTES as usize + 1);
        let err = parse_pack(&huge).unwrap_err();
        assert!(err.contains("too large"));
    }

    #[test]
    fn template_pack_scaffolds_valid_pack() {
        let pack = template_pack("my-pack", "My Lead!").unwrap();
        assert_eq!(pack.id, "my-pack");
        assert_eq!(pack.instruments[0].id, "my-pack.my-lead");
        assert!(validate_pack(&pack).is_ok());
        assert!(template_pack("../evil", "x").is_err());
        // Non-ascii/empty names still produce a valid slug.
        let pack = template_pack("p", "!!!").unwrap();
        assert!(pack.instruments[0].id.starts_with("p."));
    }

    /// Total presets served by the embedded packs alone.
    fn embedded_preset_count() -> usize {
        EMBEDDED_PACKS
            .iter()
            .map(|(_, json)| parse_pack(json).unwrap().instruments.len())
            .sum()
    }

    #[test]
    fn embedded_packs_are_valid_unique_and_ordered() {
        // Every bundled pack must parse and validate through the same path as
        // user packs; a broken embedded pack would surface as a catalog error.
        let mut seen = std::collections::HashSet::new();
        let mut total = 0;
        for (label, json) in EMBEDDED_PACKS {
            let pack = parse_pack(json)
                .unwrap_or_else(|err| panic!("embedded pack '{label}' must parse: {err}"));
            for preset in &pack.instruments {
                assert!(
                    seen.insert(preset.id.clone()),
                    "duplicate embedded preset id {}",
                    preset.id
                );
            }
            total += pack.instruments.len();
        }
        // 10 eight-bit + 128 psy-bank + 4 drum-kits + 12 guitar + 16 neon +
        // 12 one-shots + 14 mallets presets.
        assert_eq!(total, 196, "embedded preset total changed");
        assert_eq!(seen.len(), total);

        // The catalog serves the embedded bank in EMBEDDED_PACKS order with
        // no problems on an empty user dir.
        let dir = tempfile::tempdir().unwrap();
        let catalog = load_catalog_from(dir.path());
        assert!(catalog.errors.is_empty(), "{:?}", catalog.errors);
        assert_eq!(catalog.instruments.len(), total);
        let mut cursor = 0;
        for (_, json) in EMBEDDED_PACKS {
            let pack = parse_pack(json).unwrap();
            for preset in &pack.instruments {
                assert_eq!(&catalog.instruments[cursor], preset);
                cursor += 1;
            }
        }
    }

    #[test]
    fn bundled_8bit_pack_is_valid() {
        let pack = parse_pack(include_str!("../packs/8-bit/pack.json")).unwrap();
        assert_eq!(pack.id, "eight-bit");
        assert!(pack.instruments.len() >= 8);
        let ids: std::collections::HashSet<_> =
            pack.instruments.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids.len(), pack.instruments.len());
        assert!(pack
            .instruments
            .iter()
            .all(|p| p.id.starts_with("eight-bit.")));
    }

    #[test]
    fn bundled_psy_packs_are_valid_with_expected_counts() {
        let expected: [(&str, usize); 7] = [
            ("psy-rhythm", 24),
            ("psy-bass", 24),
            ("psy-keys", 16),
            ("psy-brass", 16),
            ("psy-leads", 16),
            ("psy-arps", 16),
            ("psy-space", 16),
        ];
        for (label, count) in expected {
            let json = EMBEDDED_PACKS
                .iter()
                .find(|(name, _)| *name == label)
                .map(|(_, json)| *json)
                .unwrap_or_else(|| panic!("embedded pack '{label}' missing"));
            let pack = parse_pack(json).unwrap();
            assert_eq!(pack.id, label);
            assert_eq!(pack.instruments.len(), count, "{label} preset count");
            let ids: std::collections::HashSet<_> =
                pack.instruments.iter().map(|p| p.id.as_str()).collect();
            assert_eq!(ids.len(), count, "{label} preset ids must be unique");
        }
    }

    #[test]
    fn template_file_is_valid() {
        let pack = parse_pack(include_str!("../packs/template/pack.json")).unwrap();
        assert!(!pack.instruments.is_empty());
    }

    #[test]
    fn pack_json_roundtrip() {
        let pack = valid_pack();
        let text = pack_to_json(&pack).unwrap();
        let back = parse_pack(&text).unwrap();
        assert_eq!(pack, back);
    }

    #[test]
    fn load_pack_errors_are_clear() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_pack(&dir.path().join("missing.json")).is_err());
        assert!(load_pack(dir.path()).is_err()); // a directory, not a file
        let bad = dir.path().join("bad.json");
        fs::write(&bad, "{not json").unwrap();
        let err = load_pack(&bad).unwrap_err();
        assert!(err.contains("bad.json"));
    }

    #[test]
    fn install_writes_named_file_and_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let packs_dir = dir.path().join("packs");
        let pack = valid_pack();
        let target = install_validated(&pack, &packs_dir).unwrap();
        assert_eq!(target, packs_dir.join("test-pack.json"));
        assert!(target.is_file());
        assert_eq!(load_pack(&target).unwrap(), pack);

        // Re-installing the same id replaces the file atomically.
        let mut updated = pack.clone();
        updated.name = "Renamed Pack".to_string();
        install_validated(&updated, &packs_dir).unwrap();
        assert_eq!(load_pack(&target).unwrap().name, "Renamed Pack");
    }

    #[test]
    fn install_rejects_invalid_pack_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let mut pack = valid_pack();
        pack.id = "../escape".to_string();
        assert!(install_validated(&pack, dir.path()).is_err());
        assert!(fs::read_dir(dir.path()).unwrap().next().is_none());
    }

    #[test]
    fn catalog_merges_embedded_and_user_packs() {
        let dir = tempfile::tempdir().unwrap();
        let mut user = valid_pack();
        user.id = "user-pack".to_string();
        user.instruments[0].id = "user-pack.lead".to_string();
        fs::write(dir.path().join("a-user.json"), pack_to_json(&user).unwrap()).unwrap();
        // Non-pack files and invalid packs are ignored/reported, not fatal.
        fs::write(dir.path().join("notes.txt"), "hello").unwrap();
        fs::write(dir.path().join("broken.json"), "{oops").unwrap();

        let catalog = load_catalog_from(dir.path());
        assert!(catalog.errors.iter().any(|e| e.contains("broken.json")));
        let ids: Vec<_> = catalog.instruments.iter().map(|p| p.id.as_str()).collect();
        assert!(ids.contains(&"eight-bit.pulse-lead"));
        assert!(ids.contains(&"user-pack.lead"));
        // Embedded instruments come before user ones.
        assert!(
            ids.iter().position(|id| *id == "eight-bit.pulse-lead")
                < ids.iter().position(|id| *id == "user-pack.lead")
        );
    }

    #[test]
    fn catalog_duplicate_ids_first_wins() {
        let dir = tempfile::tempdir().unwrap();
        let mut user = valid_pack();
        user.id = "user-pack".to_string();
        // Reuse an embedded instrument id.
        user.instruments[0].id = "eight-bit.pulse-lead".to_string();
        user.instruments[0].name = "Impostor".to_string();
        fs::write(dir.path().join("z.json"), pack_to_json(&user).unwrap()).unwrap();

        let catalog = load_catalog_from(dir.path());
        let lead = catalog
            .instruments
            .iter()
            .find(|p| p.id == "eight-bit.pulse-lead")
            .unwrap();
        assert_ne!(lead.name, "Impostor");
        assert!(catalog.errors.iter().any(|e| e.contains("duplicate")));
    }

    #[test]
    fn serde_rejects_unknown_fields_at_every_level() {
        // Pack level.
        let err = parse_pack(r#"{"id":"a","name":"P","bogus":1,"instruments":[{"id":"a.b","name":"B","oscillators":[{}]}]}"#)
            .unwrap_err();
        assert!(err.contains("unknown field `bogus`"), "{err}");
        // Preset level (the classic `samplehold` typo).
        let err = parse_pack(
            r#"{"id":"a","name":"P","instruments":[{"id":"a.b","name":"B","samplehold":4,"oscillators":[{}]}]}"#,
        )
        .unwrap_err();
        assert!(err.contains("unknown field `samplehold`"), "{err}");
        // Oscillator level.
        let err = parse_pack(
            r#"{"id":"a","name":"P","instruments":[{"id":"a.b","name":"B","oscillators":[{"waveform":"saw","level2":0.5}]}]}"#,
        )
        .unwrap_err();
        assert!(err.contains("unknown field `level2`"), "{err}");
        // Envelope level.
        let err = parse_pack(
            r#"{"id":"a","name":"P","instruments":[{"id":"a.b","name":"B","oscillators":[{}],"envelope":{"attack":0.1,"hold":2.0}}]}"#,
        )
        .unwrap_err();
        assert!(err.contains("unknown field `hold`"), "{err}");
        // New optional nested controls stay strict too.
        let err = parse_pack(
            r#"{"id":"a","name":"P","instruments":[{"id":"a.b","name":"B","oscillators":[{}],"pitch_envelope":{"amount_semitones":1,"curve":"linear"}}]}"#,
        )
        .unwrap_err();
        assert!(err.contains("unknown field `curve`"), "{err}");
        let err = parse_pack(
            r#"{"id":"a","name":"P","instruments":[{"id":"a.b","name":"B","oscillators":[{}],"lfo":{"shape":"square"}}]}"#,
        )
        .unwrap_err();
        assert!(err.contains("unknown field `shape`"), "{err}");
    }

    #[test]
    fn user_pack_overrides_embedded_pack() {
        let dir = tempfile::tempdir().unwrap();
        let mut pack = valid_pack();
        pack.id = "eight-bit".to_string();
        pack.name = "8-Bit v2".to_string();
        // Two replacement instruments; one reuses an embedded preset id.
        let mut p1 = valid_preset();
        p1.id = "eight-bit.pulse-lead".to_string();
        p1.name = "Custom Lead".to_string();
        p1.bit_depth = 12;
        let mut p2 = valid_preset();
        p2.id = "eight-bit.brand-new".to_string();
        p2.name = "Brand New".to_string();
        pack.instruments = vec![p1, p2];
        fs::write(
            dir.path().join("eight-bit.json"),
            pack_to_json(&pack).unwrap(),
        )
        .unwrap();

        let catalog = load_catalog_from(dir.path());
        assert!(catalog.errors.is_empty(), "{:?}", catalog.errors);
        // The whole embedded pack is replaced, keeping its (first) slot:
        // exactly the two user instruments, in order, then the rest of the
        // embedded bank.
        let eight_bit = parse_pack(EMBEDDED_PACKS[0].1).unwrap();
        assert_eq!(eight_bit.id, "eight-bit");
        assert_eq!(
            catalog.instruments.len(),
            embedded_preset_count() - eight_bit.instruments.len() + 2
        );
        assert_eq!(catalog.instruments[0].name, "Custom Lead");
        assert_eq!(catalog.instruments[0].bit_depth, 12);
        assert_eq!(catalog.instruments[1].name, "Brand New");
        // The other embedded packs still serve their presets.
        assert!(catalog
            .instruments
            .iter()
            .any(|p| p.id.starts_with("psy-rhythm.")));
        assert!(!catalog
            .instruments
            .iter()
            .any(|p| p.name == "Triangle Bass" || p.name == "Game Over"));
    }

    #[test]
    fn invalid_override_keeps_embedded_fallback() {
        let dir = tempfile::tempdir().unwrap();
        // A broken file named like the embedded pack must not remove it.
        fs::write(dir.path().join("eight-bit.json"), "{not json").unwrap();
        let catalog = load_catalog_from(dir.path());
        assert!(catalog.errors.iter().any(|e| e.contains("eight-bit.json")));
        assert!(catalog
            .instruments
            .iter()
            .any(|p| p.id == "eight-bit.pulse-lead"));
        assert!(catalog
            .instruments
            .iter()
            .any(|p| p.id == "eight-bit.game-over"));
    }

    #[test]
    fn duplicate_preset_ids_across_user_packs_first_sorted_wins() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = valid_pack();
        a.id = "aaa".to_string();
        a.instruments[0].id = "shared.inst".to_string();
        a.instruments[0].name = "First".to_string();
        let mut b = valid_pack();
        b.id = "bbb".to_string();
        b.instruments[0].id = "shared.inst".to_string();
        b.instruments[0].name = "Second".to_string();
        fs::write(dir.path().join("a.json"), pack_to_json(&a).unwrap()).unwrap();
        fs::write(dir.path().join("b.json"), pack_to_json(&b).unwrap()).unwrap();

        let catalog = load_catalog_from(dir.path());
        let inst = catalog
            .instruments
            .iter()
            .find(|p| p.id == "shared.inst")
            .unwrap();
        assert_eq!(inst.name, "First");
        assert!(catalog.errors.iter().any(|e| e.contains("duplicate")));
    }

    #[test]
    fn embedded_override_beats_same_id_user_instruments() {
        let dir = tempfile::tempdir().unwrap();
        // A normal user pack (sorts first) tries to reuse an embedded id.
        let mut normal = valid_pack();
        normal.id = "normal".to_string();
        normal.instruments[0].id = "eight-bit.pulse-lead".to_string();
        normal.instruments[0].name = "Copycat".to_string();
        fs::write(
            dir.path().join("a-normal.json"),
            pack_to_json(&normal).unwrap(),
        )
        .unwrap();
        // The override pack sorts later but still wins its embedded slot.
        let mut over = valid_pack();
        over.id = "eight-bit".to_string();
        over.instruments[0].id = "eight-bit.pulse-lead".to_string();
        over.instruments[0].name = "Official v2".to_string();
        fs::write(
            dir.path().join("z-eight.json"),
            pack_to_json(&over).unwrap(),
        )
        .unwrap();

        let catalog = load_catalog_from(dir.path());
        let lead = catalog
            .instruments
            .iter()
            .find(|p| p.id == "eight-bit.pulse-lead")
            .unwrap();
        assert_eq!(lead.name, "Official v2");
        assert!(catalog.errors.iter().any(|e| e.contains("duplicate")));
    }

    #[test]
    fn catalog_missing_dir_still_has_embedded() {
        let catalog = load_catalog_from(Path::new("/definitely/not/a/real/dir"));
        assert!(catalog.errors.is_empty());
        assert_eq!(catalog.instruments.len(), embedded_preset_count());
    }

    #[test]
    fn user_packs_dir_resolution() {
        // CADENCE_PACKS_DIR wins outright.
        let env = |key: &str| match key {
            PACKS_DIR_ENV => Some(OsString::from("/tmp/packs-override")),
            DATA_DIR_ENV => Some(OsString::from("/tmp/data-override")),
            _ => None,
        };
        assert_eq!(
            resolve_packs_dir(&env),
            PathBuf::from("/tmp/packs-override")
        );
        // CADENCE_DATA_DIR gets a /packs suffix.
        let env = |key: &str| match key {
            DATA_DIR_ENV => Some(OsString::from("/tmp/data-override")),
            _ => None,
        };
        assert_eq!(
            resolve_packs_dir(&env),
            PathBuf::from("/tmp/data-override/packs")
        );
        // Default branch always ends in "packs".
        let none = |_key: &str| None;
        assert!(resolve_packs_dir(&none).ends_with("packs"));
    }

    #[test]
    fn sample_wave_is_deterministic_and_bounded() {
        assert!((sample_wave(Waveform::Sine, 0.0, 0.5) - 0.0).abs() < 1e-6);
        assert!((sample_wave(Waveform::Sine, 0.25, 0.5) - 1.0).abs() < 1e-6);
        assert_eq!(sample_wave(Waveform::Square, 0.25, 0.5), 1.0);
        assert_eq!(sample_wave(Waveform::Square, 0.75, 0.5), -1.0);
        // Pulse duty cycle.
        assert_eq!(sample_wave(Waveform::Pulse, 0.2, 0.25), 1.0);
        assert_eq!(sample_wave(Waveform::Pulse, 0.3, 0.25), -1.0);
        // Phase wraps.
        assert_eq!(
            sample_wave(Waveform::Saw, 1.25, 0.5),
            sample_wave(Waveform::Saw, 0.25, 0.5)
        );
        // Noise is deterministic for identical input.
        assert_eq!(
            sample_wave(Waveform::Noise, 0.37, 0.5),
            sample_wave(Waveform::Noise, 0.37, 0.5)
        );
        // Everything stays within [-1, 1].
        for i in 0..1000 {
            let phase = i as f32 / 1000.0;
            for w in [
                Waveform::Sine,
                Waveform::Triangle,
                Waveform::Saw,
                Waveform::Square,
                Waveform::Pulse,
                Waveform::Noise,
            ] {
                let v = sample_wave(w, phase, 0.3);
                assert!((-1.0..=1.0).contains(&v), "{w:?} at {phase} gave {v}");
            }
        }
    }

    // ------------------------------------------------------------------
    // Drum kits
    // ------------------------------------------------------------------

    #[test]
    fn kit_preset_validates_with_empty_top_level_oscillators() {
        let p = kit_preset();
        assert!(p.oscillators.is_empty());
        assert!(p.kit.is_some());
        assert!(validate_preset(&p).is_ok());
        // A kit preset may still carry melodic oscillators.
        let mut ok = kit_preset();
        ok.oscillators = vec![OscillatorSpec::default()];
        assert!(validate_preset(&ok).is_ok());
        // Top-level melodic fields stay range-validated when present.
        let mut bad = kit_preset();
        bad.gain = 2.0;
        assert!(validate_preset(&bad).unwrap_err().contains("gain"));
        let mut bad = kit_preset();
        bad.oscillators = vec![OscillatorSpec {
            level: 2.0,
            ..Default::default()
        }];
        assert!(validate_preset(&bad)
            .unwrap_err()
            .contains("oscillators[0].level"));
    }

    #[test]
    fn kit_json_shape_parses_and_validates() {
        // The documented authoring shape: nested `voice` objects, no
        // top-level oscillators.
        let json = r#"{
            "id": "my-kit.studio", "name": "Studio Kit",
            "kit": { "pieces": [
                { "name": "Kick", "pitch": 36, "voice": {
                    "oscillators": [{"waveform":"sine","level":1.0,"frequency_ratio":1.0}],
                    "envelope": {"attack":0.001,"decay":0.4,"sustain":0.0,"release":0.05},
                    "pitch_envelope": {"amount_semitones":-24.0,"decay":0.08},
                    "filter_cutoff": 4000.0, "gain": 0.9 } },
                { "name": "Snare", "pitch": 38, "voice": { "oscillators": [
                    {"waveform":"noise","level":0.7},{"waveform":"triangle","level":0.4,"frequency_ratio":1.8}],
                    "envelope": {"attack":0.001,"decay":0.18,"sustain":0.0,"release":0.04},
                    "filter_cutoff": 8000.0 } }
            ] } }"#;
        let preset: InstrumentPreset = serde_json::from_str(json).unwrap();
        assert!(validate_preset(&preset).is_ok());
        let kit = preset.kit.as_ref().unwrap();
        assert_eq!(kit.pieces.len(), 2);
        assert_eq!(kit.pieces[0].name, "Kick");
        assert_eq!(kit.pieces[0].pitch, 36);
        // KitVoice serde defaults mirror the melodic preset defaults.
        assert_eq!(kit.pieces[1].voice.bit_depth, 24);
        assert_eq!(kit.pieces[1].voice.sample_hold, 1);
        assert_eq!(kit.pieces[1].voice.gain, 0.9);
        assert_eq!(kit.pieces[1].voice.filter_resonance, 0.0);
        assert_eq!(
            kit.pieces[0]
                .voice
                .pitch_envelope
                .as_ref()
                .unwrap()
                .amount_semitones,
            -24.0
        );
    }

    #[test]
    fn kit_validation_rules_report_piece_paths() {
        // Zero pieces.
        let mut p = kit_preset();
        p.kit.as_mut().unwrap().pieces.clear();
        assert!(validate_preset(&p).unwrap_err().contains("kit.pieces"));

        // Too many pieces.
        let mut p = kit_preset();
        let voice = kit_preset().kit.unwrap().pieces[0].voice.clone();
        p.kit.as_mut().unwrap().pieces = (0..=MAX_KIT_PIECES)
            .map(|i| KitPiece {
                name: format!("Piece {i}"),
                pitch: (i % 128) as u8,
                voice: voice.clone(),
            })
            .collect();
        // Pitches repeat mod 128 but the count check fires first.
        assert!(validate_preset(&p).unwrap_err().contains("too many pieces"));

        // Duplicate pitch: error names the piece path, pitch and first index.
        let mut p = kit_preset();
        p.kit.as_mut().unwrap().pieces[1].pitch = 36;
        let err = validate_preset(&p).unwrap_err();
        assert!(err.contains("kit.pieces[1].pitch"), "{err}");
        assert!(err.contains("36"), "{err}");
        assert!(err.contains("piece 0"), "{err}");

        // Piece name limits.
        let mut p = kit_preset();
        p.kit.as_mut().unwrap().pieces[0].name = String::new();
        assert!(validate_preset(&p)
            .unwrap_err()
            .contains("kit.pieces[0].name"));
        let mut p = kit_preset();
        p.kit.as_mut().unwrap().pieces[0].name = "x".repeat(MAX_KIT_NAME_LEN + 1);
        assert!(validate_preset(&p)
            .unwrap_err()
            .contains("kit.pieces[0].name"));

        // Pitch above 127 (u8 allows it; MIDI does not).
        let mut p = kit_preset();
        p.kit.as_mut().unwrap().pieces[0].pitch = 200;
        assert!(validate_preset(&p)
            .unwrap_err()
            .contains("kit.pieces[0].pitch"));

        // Piece voice fields are validated through the preset stub path.
        let mut p = kit_preset();
        p.kit.as_mut().unwrap().pieces[0].voice.gain = 1.5;
        assert!(validate_preset(&p)
            .unwrap_err()
            .contains("kit.pieces[0].voice.gain"));
        let mut p = kit_preset();
        p.kit.as_mut().unwrap().pieces[0].voice.oscillators[0].level = 2.0;
        assert!(validate_preset(&p)
            .unwrap_err()
            .contains("kit.pieces[0].voice.oscillators[0].level"));
        // A voice with no oscillators fails like a melodic preset.
        let mut p = kit_preset();
        p.kit.as_mut().unwrap().pieces[0].voice.oscillators.clear();
        assert!(validate_preset(&p)
            .unwrap_err()
            .contains("kit.pieces[0].voice.oscillators"));
    }

    #[test]
    fn kit_unknown_fields_are_rejected() {
        // Strictness extends into the kit section.
        for bad in [
            r#"{"id":"a.b","name":"B","kit":{"pieces":[],"bogus":1}}"#,
            r#"{"id":"a.b","name":"B","kit":{"pieces":[{"name":"K","pitch":36,"voice":{},"extra":1}]}}"#,
            r#"{"id":"a.b","name":"B","kit":{"pieces":[{"name":"K","pitch":36,"voice":{"oscillators":[{}],"bogus":1}}]}}"#,
        ] {
            let err = parse_pack(&format!(r#"{{"id":"a","name":"P","instruments":[{bad}]}}"#))
                .unwrap_err();
            assert!(err.contains("unknown field"), "{err}");
        }
    }

    #[test]
    fn kit_voice_stub_maps_fields_and_validates() {
        let stub = kit_preset().kit.unwrap().pieces[0].voice.to_preset_stub();
        // Placeholder identity fields always pass validation.
        assert_eq!(stub.id, "kit.stub");
        assert_eq!(stub.name, "stub");
        assert!(!stub.monophonic);
        assert_eq!(stub.audition_note, 60);
        assert_eq!(stub.audition_duration, 0.8);
        assert_eq!(stub.kit, None);
        // Synthesis fields are copied through.
        assert_eq!(stub.oscillators.len(), 1);
        assert_eq!(stub.filter_cutoff, 4_000.0);
        assert_eq!(
            stub.pitch_envelope.as_ref().unwrap().amount_semitones,
            -24.0
        );
        assert!(validate_preset(&stub).is_ok());

        // KitVoice defaults mirror the melodic preset defaults.
        let d = KitVoice::default();
        assert!(d.oscillators.is_empty());
        assert_eq!(d.filter_cutoff, 20_000.0);
        assert_eq!(d.filter_resonance, 0.0);
        assert_eq!(d.bit_depth, 24);
        assert_eq!(d.sample_hold, 1);
        assert_eq!(d.gain, 0.9);
        assert_eq!(d.pitch_envelope, None);
        assert_eq!(d.lfo, None);
        // An empty voice fails exactly like an oscillator-less preset.
        let err = validate_preset(&KitVoice::default().to_preset_stub()).unwrap_err();
        assert!(err.contains("oscillators"), "{err}");
    }

    #[test]
    fn kit_preset_serialization_roundtrip() {
        let p = kit_preset();
        let text = serde_json::to_string(&p).unwrap();
        assert!(text.contains("\"kit\""), "{text}");
        let back: InstrumentPreset = serde_json::from_str(&text).unwrap();
        assert_eq!(back, p);

        // Kit-less presets serialize without the key and parse back equal.
        let plain = valid_preset();
        let text = serde_json::to_string(&plain).unwrap();
        assert!(!text.contains("\"kit\""), "{text}");
        let parsed: InstrumentPreset = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed.kit, None);
        assert_eq!(parsed, plain);

        // Pack-level round trip keeps the kit intact.
        let mut pack = valid_pack();
        pack.instruments[0] = kit_preset();
        let text = pack_to_json(&pack).unwrap();
        assert_eq!(parse_pack(&text).unwrap(), pack);
    }
}
