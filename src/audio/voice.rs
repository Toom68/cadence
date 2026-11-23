//! Synthesizer voices. A [`Voice`] is a preallocated slot that can play any built-in
//! instrument or a data-driven pack preset; starting, releasing and rendering a voice never
//! allocates.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use super::dsp::{
    advance, flush, poly_blep, saw, sine, soft_clip, square, t60_coef, time_coef, OnePole, Rng,
    Svf, SvfCoefs,
};
use crate::model::{map_drum_pitch, pitch_hz, Instrument};
use crate::packs::{
    InstrumentPreset, LfoWaveform, SampleSpec, Waveform, DEFAULT_VELOCITY_TO_CUTOFF,
    MAX_CHOKE_GROUP, MAX_KIT_PIECES,
};

pub const MAX_PRESET_OSCILLATORS: usize = 4;
/// Lowest pitch the plucked-string model tunes to; lower notes are folded up an octave.
const PLUCK_MIN_HZ: f32 = 30.0;
/// The mono lead's fixed legato slide: the `glide` time constant armed at
/// note start so [`Voice::legato`] bends to the new pitch instead of
/// snapping. Per-track glides pass their own time to `legato` directly.
pub const LEAD_GLIDE_SECONDS: f32 = 0.045;
/// Filter coefficients for modulated filters are refreshed at this sample interval.
const CONTROL_INTERVAL: u32 = 16;

/// One oscillator of a pack preset, pre-resolved for the audio thread.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OscParams {
    pub wave: Waveform,
    pub level: f32,
    /// Frequency multiplier including detune.
    pub ratio: f32,
    pub width: f32,
    /// Per-layer T60 in seconds; 0.0 disables the layer envelope.
    pub decay: f32,
}

/// A pack preset flattened into a fixed-size `Copy` value so it can be sent to and stored on
/// the audio thread without allocation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PresetParams {
    pub oscs: [OscParams; MAX_PRESET_OSCILLATORS],
    pub count: usize,
    pub attack: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
    pub pitch_env_amount: f32,
    pub pitch_env_decay: f32,
    pub filter_env_amount: f32,
    pub filter_env_decay: f32,
    pub cutoff: f32,
    pub resonance: f32,
    pub lfo_wave: LfoWaveform,
    pub lfo_rate: f32,
    pub lfo_pitch: f32,
    pub lfo_filter: f32,
    pub lfo_amp: f32,
    pub bits: u8,
    pub hold: u32,
    pub gain: f32,
    pub mono: bool,
    /// Octaves of filter-cutoff swing across the velocity range, centred on
    /// velocity 0.8 (see [`velocity_cutoff`]).
    pub vel_cutoff: f32,
    /// Stereo width 0..=1 (0 = mono): a detuned, phase-offset second copy
    /// of the oscillators (or a second sample read head) panned apart.
    pub spread: f32,
    /// Kit-piece choke group (0 = none); see [`Voice::choke_group`].
    pub choke_group: u8,
    /// Published slot for the embedded clip, or `NO_SAMPLE_SLOT` for the
    /// ordinary oscillator path. Only the slot travels on a note-on.
    pub sample_slot: usize,
    /// The clip's declared sample rate in Hz (0 when there is no clip).
    pub sample_rate: f32,
    /// MIDI note that plays the clip at unity speed (0..=127).
    pub sample_root: u8,
    /// Loop the clip while the note is held; false = one-shot.
    pub sample_loop: bool,
}

fn finite(v: f32, fallback: f32) -> f32 {
    if v.is_finite() {
        v
    } else {
        fallback
    }
}

impl PresetParams {
    /// A plain sine with default envelope; placeholder for idle voices.
    pub const SINE: PresetParams = PresetParams {
        oscs: [OscParams {
            wave: Waveform::Sine,
            level: 1.0,
            ratio: 1.0,
            width: 0.5,
            decay: 0.0,
        }; MAX_PRESET_OSCILLATORS],
        count: 1,
        attack: 0.01,
        decay: 0.2,
        sustain: 0.7,
        release: 0.3,
        pitch_env_amount: 0.0,
        pitch_env_decay: 0.2,
        filter_env_amount: 0.0,
        filter_env_decay: 0.5,
        cutoff: 20_000.0,
        resonance: 0.0,
        lfo_wave: LfoWaveform::Sine,
        lfo_rate: 1.0,
        lfo_pitch: 0.0,
        lfo_filter: 0.0,
        lfo_amp: 0.0,
        bits: 24,
        hold: 1,
        gain: 0.9,
        mono: false,
        vel_cutoff: DEFAULT_VELOCITY_TO_CUTOFF,
        spread: 0.0,
        choke_group: 0,
        sample_slot: NO_SAMPLE_SLOT,
        sample_rate: 0.0,
        sample_root: 60,
        sample_loop: false,
    };

    /// Converts a preset, clamping every value into its documented range so that even an
    /// unvalidated preset can never produce non-finite audio.
    pub fn from_preset(p: &InstrumentPreset) -> Self {
        let mut oscs = [OscParams {
            wave: Waveform::Sine,
            level: 0.0,
            ratio: 1.0,
            width: 0.5,
            decay: 0.0,
        }; MAX_PRESET_OSCILLATORS];
        let mut count = 0;
        for spec in p.oscillators.iter().take(MAX_PRESET_OSCILLATORS) {
            let detune = finite(spec.detune_cents, 0.0).clamp(-100.0, 100.0);
            oscs[count] = OscParams {
                wave: spec.waveform,
                level: finite(spec.level, 0.0).clamp(0.0, 1.0),
                ratio: finite(spec.frequency_ratio, 1.0).clamp(0.125, 16.0)
                    * 2f32.powf(detune / 1200.0),
                width: finite(spec.pulse_width, 0.5).clamp(0.05, 0.95),
                decay: spec
                    .decay
                    .map(|decay| finite(decay, 0.0).clamp(0.005, 20.0))
                    .unwrap_or(0.0),
            };
            count += 1;
        }
        if count == 0 {
            oscs[0].level = 1.0;
            count = 1;
        }
        let pitch_env = p.pitch_envelope.as_ref();
        let filter_env = p.filter_envelope.as_ref();
        let lfo = p.lfo.as_ref();
        PresetParams {
            oscs,
            count,
            attack: finite(p.envelope.attack, 0.01).clamp(0.0, 5.0),
            decay: finite(p.envelope.decay, 0.2).clamp(0.0, 10.0),
            sustain: finite(p.envelope.sustain, 0.7).clamp(0.0, 1.0),
            release: finite(p.envelope.release, 0.3).clamp(0.005, 20.0),
            pitch_env_amount: pitch_env
                .map(|env| finite(env.amount_semitones, 0.0).clamp(-48.0, 48.0))
                .unwrap_or(0.0),
            pitch_env_decay: pitch_env
                .map(|env| finite(env.decay, 0.2).clamp(0.005, 10.0))
                .unwrap_or(0.2),
            filter_env_amount: filter_env
                .map(|env| finite(env.amount_octaves, 0.0).clamp(-6.0, 6.0))
                .unwrap_or(0.0),
            filter_env_decay: filter_env
                .map(|env| finite(env.decay, 0.5).clamp(0.005, 20.0))
                .unwrap_or(0.5),
            cutoff: finite(p.filter_cutoff, 20_000.0).clamp(100.0, 20_000.0),
            resonance: finite(p.filter_resonance, 0.0).clamp(0.0, 0.95),
            lfo_wave: lfo.map_or(LfoWaveform::Sine, |lfo| lfo.waveform),
            lfo_rate: lfo
                .map(|lfo| finite(lfo.rate_hz, 1.0).clamp(0.05, 20.0))
                .unwrap_or(1.0),
            lfo_pitch: lfo
                .map(|lfo| finite(lfo.pitch_semitones, 0.0).clamp(0.0, 12.0))
                .unwrap_or(0.0),
            lfo_filter: lfo
                .map(|lfo| finite(lfo.filter_octaves, 0.0).clamp(0.0, 4.0))
                .unwrap_or(0.0),
            lfo_amp: lfo
                .map(|lfo| finite(lfo.amplitude, 0.0).clamp(0.0, 1.0))
                .unwrap_or(0.0),
            bits: p.bit_depth.clamp(4, 24),
            hold: p.sample_hold.clamp(1, 64),
            gain: finite(p.gain, 0.9).clamp(0.0, 1.0),
            mono: p.monophonic,
            vel_cutoff: finite(p.velocity_to_cutoff, DEFAULT_VELOCITY_TO_CUTOFF).clamp(0.0, 6.0),
            spread: finite(p.spread, 0.0).clamp(0.0, 1.0),
            // Choke groups are a kit-piece concept: set by `KitParams::from_kit`.
            choke_group: 0,
            // Register the clip off the audio thread (decoding allocates);
            // a missing or undecodable clip uses the oscillator fallback.
            sample_slot: p
                .sample
                .as_ref()
                .and_then(register_sample)
                .unwrap_or(NO_SAMPLE_SLOT),
            sample_rate: p.sample.as_ref().map_or(0.0, |s| s.sample_rate as f32),
            sample_root: p.sample.as_ref().map_or(60, |s| s.root_pitch.min(127)),
            sample_loop: p.sample.as_ref().is_some_and(|s| s.looped),
        }
    }

    #[inline]
    fn uses_legacy_preset_path(&self) -> bool {
        self.pitch_env_amount == 0.0
            && self.filter_env_amount == 0.0
            && self.resonance == 0.0
            && self.lfo_pitch == 0.0
            && self.lfo_filter == 0.0
            && self.lfo_amp == 0.0
            && self.oscs.iter().take(self.count).all(|o| o.decay == 0.0)
            && self.spread == 0.0
    }

    #[inline]
    fn resonance_q(&self) -> f32 {
        let r = (finite(self.resonance, 0.0) / 0.95).clamp(0.0, 1.0);
        0.707 + r * r * 9.293
    }
}

/// A drum kit preset flattened into a fixed-size `Copy` value: `(pitch,
/// voice)` pairs sorted by pitch so note-on can pick a piece without
/// allocating. Built once per track snapshot by [`KitParams::from_kit`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KitParams {
    /// `(pitch, voice)` pairs sorted by ascending pitch; only the first
    /// `count` entries are meaningful.
    pub pieces: [(u8, PresetParams); MAX_KIT_PIECES],
    pub count: usize,
}

impl KitParams {
    /// Flattens a preset's `kit` section into audio-ready piece params. Each
    /// piece voice goes through
    /// [`KitVoice::to_preset_stub`](crate::packs::KitVoice::to_preset_stub)
    /// and then [`PresetParams::from_preset`], so the same clamping applies as
    /// for melodic presets. Over-long or pitch-duplicated kits (only possible
    /// on unvalidated data) are tolerated: pieces are truncated to
    /// [`MAX_KIT_PIECES`] and sorted by pitch.
    pub fn from_kit(p: &InstrumentPreset) -> Self {
        let mut pieces = [(0u8, PresetParams::SINE); MAX_KIT_PIECES];
        let mut count = 0;
        if let Some(kit) = &p.kit {
            for piece in kit.pieces.iter().take(MAX_KIT_PIECES) {
                let mut params = PresetParams::from_preset(&piece.voice.to_preset_stub());
                params.choke_group = piece.voice.choke_group.min(MAX_CHOKE_GROUP);
                pieces[count] = (piece.pitch, params);
                count += 1;
            }
            pieces[..count].sort_by_key(|(pitch, _)| *pitch);
        }
        KitParams { pieces, count }
    }

    /// The voice for `pitch`: an exact pitch match wins; otherwise the
    /// NEAREST piece pitch (ties resolve to the lower pitch, which sorted
    /// order guarantees is seen first). An empty kit yields
    /// [`PresetParams::SINE`] — validation prevents that, this is defensive.
    pub fn piece_for(&self, pitch: u8) -> PresetParams {
        if self.count == 0 {
            return PresetParams::SINE;
        }
        let mut best = 0usize;
        let mut best_dist = u16::MAX;
        for (i, &(piece_pitch, _)) in self.pieces.iter().take(self.count).enumerate() {
            let dist = (piece_pitch as i16 - pitch as i16).unsigned_abs();
            if dist < best_dist {
                best_dist = dist;
                best = i;
                if dist == 0 {
                    break;
                }
            }
        }
        self.pieces[best].1
    }
}

/// Address of a General MIDI program inside an installed SoundFont: the
/// stable [`crate::font::font_key`] of the font name plus the program number.
/// `Copy` so it can live inside [`Sound`] on the audio thread; the parsed font
/// itself is resolved through the renderer's font bank.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FontKey {
    pub font_key: u64,
    /// GM program 0..=127.
    pub program: u8,
}

const NO_SAMPLE_SLOT: usize = usize::MAX;
const MAX_SAMPLE_BANK_ENTRIES: usize = 512;

// Published slots never move or expire. Registering deduplicates clips by
// content off the audio thread; note-on needs neither a lock nor a refcount.
static SAMPLE_BANK: OnceLock<Mutex<HashMap<u64, usize>>> = OnceLock::new();
static SAMPLE_FRAMES: [OnceLock<Box<[f32]>>; MAX_SAMPLE_BANK_ENTRIES] =
    [const { OnceLock::new() }; MAX_SAMPLE_BANK_ENTRIES];

fn bank() -> std::sync::MutexGuard<'static, HashMap<u64, usize>> {
    SAMPLE_BANK
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn sample_clip(slot: usize) -> Option<&'static [f32]> {
    SAMPLE_FRAMES.get(slot)?.get().map(Box::as_ref)
}

fn register_sample(spec: &SampleSpec) -> Option<usize> {
    let key = spec.key();
    if key == 0 {
        return None;
    }
    {
        let bank = bank();
        if let Some(&slot) = bank.get(&key) {
            return Some(slot);
        }
        if bank.len() >= MAX_SAMPLE_BANK_ENTRIES {
            return None;
        }
    }
    let frames = spec.decode().ok()?;
    if frames.is_empty() {
        return None;
    }
    let mut bank = bank();
    if let Some(&slot) = bank.get(&key) {
        return Some(slot);
    }
    let slot = bank.len();
    SAMPLE_FRAMES
        .get(slot)?
        .set(frames.into_boxed_slice())
        .ok()?;
    bank.insert(key, slot);
    Some(slot)
}

/// What a track sounds like: one of the six built-in algorithms, a pack
/// preset (oscillator or embedded-sample), a pack drum kit, or a sampled
/// SoundFont program.
// `Kit` is large (~5 KB of piece params) but must stay `Copy`: sounds are
// duplicated into per-note `NoteStart`s and per-track `TrackParams` on the
// audio thread, where allocation is forbidden. The copy is a flat memcpy.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sound {
    Builtin(Instrument),
    /// A data-driven preset — oscillator synthesis, or PCM playback via
    /// `PresetParams::sample_slot` when the preset carries a `sample` clip.
    Preset(PresetParams),
    /// A data-driven drum kit ([`crate::packs::KitSpec`]): one-shot voices
    /// picked by pitch at note-on.
    Kit(KitParams),
    /// A sampled General MIDI program from an installed `.sf2` font. Rendered
    /// by a per-track `rustysynth` instance, not by [`Voice`]; when the font is
    /// missing the renderer substitutes the track's fallback sound instead.
    Font(FontKey),
}

impl Sound {
    pub fn is_mono(&self) -> bool {
        match self {
            Sound::Builtin(i) => i.is_monophonic(),
            Sound::Preset(p) => p.mono,
            // Kit pieces are one-shot polyphonic voices; `monophonic` is not a
            // kit concept. Fonts are rendered by rustysynth, which is
            // polyphonic per note.
            Sound::Kit(_) | Sound::Font(_) => false,
        }
    }

    /// One-shot drum-kit voices: the built-in eight-piece kit or a pack kit.
    /// Used for seek-chase suppression (a drum hit is never re-armed
    /// mid-ring). The built-in kit additionally gets same-kind choking in the
    /// renderer — that path is matched on `Sound::Builtin(Instrument::Drums)`
    /// directly because it relies on [`Voice::drum`]/[`drum_kind`], which kit
    /// voices do not populate.
    pub fn is_drums(&self) -> bool {
        matches!(self, Sound::Builtin(Instrument::Drums) | Sound::Kit(_))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrumKind {
    Kick,
    Snare,
    ClosedHat,
    OpenHat,
    Clap,
    Tom,
    Crash,
    Ride,
}

/// The kit voice that plays `pitch` (any MIDI pitch maps to one of the eight kit voices).
pub fn drum_kind(pitch: u8) -> DrumKind {
    match map_drum_pitch(pitch) {
        36 => DrumKind::Kick,
        38 => DrumKind::Snare,
        46 => DrumKind::OpenHat,
        39 => DrumKind::Clap,
        45 => DrumKind::Tom,
        49 => DrumKind::Crash,
        51 => DrumKind::Ride,
        _ => DrumKind::ClosedHat,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Drum,
    Piano,
    Bass,
    Pluck,
    Guitar,
    Pad,
    Lead,
    Preset,
    /// Embedded-clip playback (a preset or kit piece `sample` block): reads
    /// `sample_buf` with linear interpolation, then shares the preset DSP tail.
    Sample,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Attack,
    Decay,
    Sustain,
    Release,
    Off,
}

/// Linear-attack, exponential decay/release envelope.
#[derive(Clone, Copy, Debug)]
struct Adsr {
    stage: Stage,
    level: f32,
    attack_inc: f32,
    decay_coef: f32,
    sustain: f32,
    release_coef: f32,
}

impl Adsr {
    const OFF: Adsr = Adsr {
        stage: Stage::Off,
        level: 0.0,
        attack_inc: 1.0,
        decay_coef: 0.0,
        sustain: 0.0,
        release_coef: 0.0,
    };

    /// `decay_tau`/`release_tau` are exponential time constants in seconds.
    fn new(sr: f32, attack: f32, decay_tau: f32, sustain: f32, release_tau: f32) -> Self {
        Adsr {
            stage: Stage::Attack,
            level: 0.0,
            attack_inc: 1.0 / (attack.max(0.0005) * sr),
            decay_coef: time_coef(decay_tau.max(0.0005), sr),
            sustain,
            release_coef: time_coef(release_tau.max(0.0005), sr),
        }
    }

    /// Restarts the attack from the current level (no click on retrigger).
    fn retrigger(&mut self, fresh: Adsr) {
        let level = self.level;
        *self = fresh;
        self.level = level;
    }

    #[inline]
    fn next(&mut self) -> f32 {
        match self.stage {
            Stage::Attack => {
                self.level += self.attack_inc;
                if self.level >= 1.0 {
                    self.level = 1.0;
                    self.stage = Stage::Decay;
                }
            }
            Stage::Decay => {
                self.level = self.sustain + (self.level - self.sustain) * self.decay_coef;
                if self.level - self.sustain < 1e-5 {
                    self.level = self.sustain;
                    self.stage = if self.sustain <= 0.0 {
                        Stage::Off
                    } else {
                        Stage::Sustain
                    };
                }
            }
            Stage::Sustain => {}
            Stage::Release => {
                self.level *= self.release_coef;
                if self.level < 1e-5 {
                    self.level = 0.0;
                    self.stage = Stage::Off;
                }
            }
            Stage::Off => self.level = 0.0,
        }
        self.level
    }

    fn release(&mut self) {
        if self.stage != Stage::Off {
            self.stage = Stage::Release;
        }
    }

    fn is_off(&self) -> bool {
        self.stage == Stage::Off
    }
}

/// Parameters describing a note to start.
#[derive(Clone, Copy, Debug)]
pub struct NoteStart {
    pub sound: Sound,
    pub strip: usize,
    pub pitch: u8,
    pub velocity: f32,
    /// Samples until the note is released.
    pub gate: u64,
    pub from_song: bool,
    pub age: u64,
}

const HAT_FREQS: [f32; 6] = [205.3, 304.4, 369.6, 522.7, 540.0, 800.0];
const RIDE_FREQS: [f32; 6] = [312.0, 461.0, 548.0, 751.0, 887.0, 1237.0];
/// Lowest pitch the guitar model tunes to; lower notes are folded up an octave.
const GUITAR_MIN_HZ: f32 = 45.0;
/// Guitar body resonances `(Hz, Q, mix weight)`: air/Helmholtz ~105 Hz, the
/// main top "honk" ~215 Hz, presence ~450 Hz and the bridge "snap" ~3 kHz.
/// Fixed frequencies — a real top's modes don't track the note.
const GUITAR_BODY: [(f32, f32, f32); 4] = [
    (105.0, 4.5, 0.5),
    (215.0, 6.5, 0.8),
    (450.0, 5.0, 0.6),
    (3000.0, 8.0, 0.3),
];
/// Overall blend of the resonator sum into the string output.
const GUITAR_BODY_MIX: f32 = 0.5;

#[derive(Clone, Debug)]
pub struct Voice {
    pub active: bool,
    pub strip: usize,
    pub pitch: u8,
    pub age: u64,
    pub from_song: bool,
    /// Key is down: the gate has not elapsed and the note was not released.
    pub held: bool,
    /// Envelope level times gain, refreshed per block; used to pick steal victims.
    pub level: f32,
    pub drum: DrumKind,
    kind: Kind,
    preset: PresetParams,
    sr: f32,
    inv_sr: f32,
    gain: f32,
    vel: f32,
    pub(crate) freq: f32,
    pub(crate) target_freq: f32,
    glide: f32,
    gate: u64,
    t: u32,
    amp: Adsr,
    menv: Adsr,
    ph: [f32; 8],
    e: [f32; 4],
    c: [f32; 4],
    f1: Svf,
    f2: Svf,
    fc: SvfCoefs,
    ctl: u32,
    lp: OnePole,
    rng: Rng,
    fade: f32,
    fade_step: f32,
    lfo: f32,
    pitch_env: f32,
    pitch_env_coef: f32,
    filter_env: f32,
    filter_env_coef: f32,
    hold_left: u32,
    hold_value: f32,
    /// Right-channel sample-and-hold value for `spread` presets.
    hold_value_r: f32,
    /// Velocity-scaled base filter cutoff of a preset/sample voice.
    cutoff: f32,
    /// Frequency multipliers of the two `spread` copies.
    spread_ratio: [f32; 2],
    /// Kit-piece choke group of the playing note (0 = none). The renderer
    /// fades same-group voices on the same strip when a new piece starts.
    pub choke_group: u8,
    noise_value: [f32; MAX_PRESET_OSCILLATORS],
    /// Decoded clip for `Kind::Sample`, resolved from its published slot.
    sample_buf: Option<&'static [f32]>,
    /// Read position in clip frames (fractional; linear interp uses the rest).
    sample_pos: f64,
    /// Second read head for `spread` sample voices.
    sample_pos2: f64,
    /// Clip frames advanced per output Hz: `clip_rate / pitch_hz(root)`.
    sample_ratio: f32,
    done: bool,
    ks: Vec<f32>,
    ks_len: usize,
    ks_pos: usize,
    ks_c: f32,
    ks_x1: f32,
    ks_y1: f32,
    ks_last: f32,
    ks_g: f32,
    ks_g_release: f32,
    // Guitar-only state (the KS fields above are shared with `Kind::Pluck`).
    /// Loop low-pass (2nd-order SVF) state and coefficients: bright at the
    /// pick, closing toward `ks_fc_lo` as `ks_fenv` decays by `ks_fstep`.
    /// A 2nd-order filter is used because its passband stays flat at the
    /// note's own frequency — a one-pole's ~1.5%/cycle loss would kill
    /// high notes outright (hundreds of loop cycles per second).
    ks_sv: Svf,
    ks_svc: SvfCoefs,
    ks_fenv: f32,
    ks_fstep: f32,
    ks_fc_lo: f32,
    ks_fc_hi: f32,
    /// Parallel body resonators driven by the string output.
    gt: [Svf; 4],
    gtc: [SvfCoefs; 4],
    /// Pick-burst transient length in samples (counted by `t`); the burst
    /// itself is `f2` band-passed noise under `fc` coefficients.
    g_pick: u32,
}

fn velocity_gain(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    0.12 + 0.88 * v * v
}

/// Velocity at which a preset plays exactly at its `filter_cutoff`.
const VELOCITY_CUTOFF_PIVOT: f32 = 0.8;
/// Full detune between the two copies of a `spread = 1` voice, in cents
/// (each copy sits half of it away from the played pitch).
const SPREAD_CENTS: f32 = 14.0;
/// Start-phase offset of the second copy at `spread = 1`, in cycles.
const SPREAD_PHASE: f32 = 0.25;

/// Velocity → brightness for preset voices: `cutoff * 2^(amount * (vel -
/// 0.8))`, capped at 20 kHz. A fully open filter (20 kHz) is left alone so
/// unfiltered presets keep their sound at every velocity.
pub(crate) fn velocity_cutoff(cutoff: f32, amount: f32, vel: f32) -> f32 {
    let cutoff = finite(cutoff, 20_000.0);
    if cutoff >= 20_000.0 {
        return cutoff;
    }
    let amount = finite(amount, 0.0).clamp(0.0, 6.0);
    let delta = vel.clamp(0.0, 1.0) - VELOCITY_CUTOFF_PIVOT;
    if amount == 0.0 || delta == 0.0 {
        return cutoff;
    }
    (cutoff * 2f32.powf(amount * delta)).clamp(10.0, 20_000.0)
}

impl Voice {
    pub fn new(sample_rate: f32) -> Self {
        let mut v = Voice {
            active: false,
            strip: 0,
            pitch: 0,
            age: 0,
            from_song: false,
            held: false,
            level: 0.0,
            drum: DrumKind::Kick,
            kind: Kind::Piano,
            preset: PresetParams::SINE,
            sr: sample_rate,
            inv_sr: 1.0 / sample_rate,
            gain: 0.0,
            vel: 0.0,
            freq: 440.0,
            target_freq: 440.0,
            glide: 0.0,
            gate: 0,
            t: 0,
            amp: Adsr::OFF,
            menv: Adsr::OFF,
            ph: [0.0; 8],
            e: [0.0; 4],
            c: [0.0; 4],
            f1: Svf::default(),
            f2: Svf::default(),
            fc: SvfCoefs::default(),
            ctl: 0,
            lp: OnePole::default(),
            rng: Rng::new(1),
            fade: 1.0,
            fade_step: 0.0,
            lfo: 0.0,
            pitch_env: 0.0,
            pitch_env_coef: 0.0,
            filter_env: 0.0,
            filter_env_coef: 0.0,
            hold_left: 0,
            hold_value: 0.0,
            hold_value_r: 0.0,
            cutoff: 20_000.0,
            spread_ratio: [1.0; 2],
            choke_group: 0,
            noise_value: [0.0; MAX_PRESET_OSCILLATORS],
            sample_buf: None,
            sample_pos: 0.0,
            sample_pos2: 0.0,
            sample_ratio: 0.0,
            done: false,
            ks: Vec::new(),
            ks_len: 2,
            ks_pos: 0,
            ks_c: 0.0,
            ks_x1: 0.0,
            ks_y1: 0.0,
            ks_last: 0.0,
            ks_g: 0.0,
            ks_g_release: 0.0,
            ks_sv: Svf::default(),
            ks_svc: SvfCoefs::default(),
            ks_fenv: 0.0,
            ks_fstep: 0.0,
            ks_fc_lo: 0.0,
            ks_fc_hi: 0.0,
            gt: [Svf::default(); 4],
            gtc: [SvfCoefs::default(); 4],
            g_pick: 0,
        };
        v.set_sample_rate(sample_rate);
        v
    }

    /// Reallocates the string buffer; call only off the audio thread (or while silent).
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sr = sample_rate;
        self.inv_sr = 1.0 / sample_rate;
        self.ks = vec![0.0; (sample_rate / PLUCK_MIN_HZ) as usize + 4];
        for (i, &(freq, q, _)) in GUITAR_BODY.iter().enumerate() {
            self.gtc[i] = SvfCoefs::new(freq, q, sample_rate);
        }
        self.sample_buf = None;
        self.active = false;
    }

    pub fn is_fading(&self) -> bool {
        self.fade_step > 0.0
    }

    /// Starts a note, reinitialising every field used by the chosen sound.
    pub fn start(&mut self, n: &NoteStart) {
        let sr = self.sr;
        self.active = true;
        self.done = false;
        self.strip = n.strip;
        self.pitch = n.pitch;
        self.age = n.age;
        self.from_song = n.from_song;
        self.vel = n.velocity.clamp(0.0, 1.0);
        self.gain = velocity_gain(self.vel);
        self.gate = n.gate;
        self.held = true;
        self.t = 0;
        self.fade = 1.0;
        self.fade_step = 0.0;
        self.ph = [0.0; 8];
        self.e = [0.0; 4];
        self.c = [0.0; 4];
        self.f1.reset();
        self.f2.reset();
        self.ctl = 0;
        self.rng = Rng::new(
            (n.age as u32).wrapping_mul(2_654_435_761)
                ^ ((n.pitch as u32) << 16)
                ^ n.from_song as u32,
        );
        self.lfo = self.rng.unit();
        self.freq = pitch_hz(n.pitch as f32);
        self.target_freq = self.freq;
        self.glide = 0.0;
        self.level = self.gain;
        self.sample_buf = None;
        self.sample_pos = 0.0;
        self.sample_pos2 = 0.0;
        self.sample_ratio = 0.0;
        self.choke_group = 0;
        match n.sound {
            Sound::Builtin(Instrument::Drums) => self.start_drum(),
            Sound::Builtin(Instrument::Piano) => {
                self.kind = Kind::Piano;
                let t60 = (5.0 * 2f32.powf((48.0 - n.pitch as f32) / 24.0)).clamp(1.2, 7.0);
                self.amp = Adsr::new(sr, 0.0015, t60 / 6.9, 0.0, 0.09);
                let key_scale = (1.0 - (n.pitch as f32 - 60.0) / 72.0).clamp(0.45, 1.25);
                self.e = [1.0, 1.0, (0.9 + 2.3 * self.vel) * key_scale, 0.0];
                self.c = [time_coef(0.38, sr), time_coef(0.035, sr), 0.0, 0.0];
            }
            Sound::Builtin(Instrument::Bass) => {
                self.kind = Kind::Bass;
                self.amp = Adsr::new(sr, 0.003, 0.25, 0.78, 0.035);
                self.menv = Adsr::new(sr, 0.0015, 0.09, 0.12, 0.05);
            }
            Sound::Builtin(Instrument::Pluck) => self.start_pluck(),
            Sound::Builtin(Instrument::Guitar) => self.start_guitar(),
            Sound::Builtin(Instrument::Pad) => {
                self.kind = Kind::Pad;
                self.amp = Adsr::new(sr, 0.55, 0.6, 0.85, 0.26);
                self.menv = Adsr::new(sr, 0.9, 1.0, 0.7, 0.4);
                for p in &mut self.ph[..4] {
                    *p = self.rng.unit();
                }
            }
            Sound::Builtin(Instrument::Lead) => {
                self.kind = Kind::Lead;
                self.amp = Adsr::new(sr, 0.006, 0.15, 0.8, 0.045);
                self.menv = Adsr::new(sr, 0.004, 0.18, 0.3, 0.1);
                self.glide = time_coef(LEAD_GLIDE_SECONDS, sr);
            }
            Sound::Preset(p) => self.start_preset(p),
            // A kit piece is picked by pitch at note-on, then runs the exact
            // same preset DSP path as a melodic preset voice.
            Sound::Kit(k) => self.start_preset(k.piece_for(n.pitch)),
            // `Sound::Font` never reaches a `Voice`: the renderer routes font
            // notes to per-track rustysynth instances (or the fallback sound
            // when the font is missing). This arm only keeps the match total
            // for direct/unexpected uses — a quiet placeholder.
            Sound::Font(_) => self.start_preset(PresetParams::SINE),
        }
    }

    /// Mono legato: change pitch without restarting the envelopes.
    /// `glide_seconds` is the portamento time constant — the note bends
    /// exponentially toward the new pitch (see [`Voice::set_glide`]);
    /// 0 snaps to it immediately while staying legato (no re-attack).
    pub fn legato(&mut self, pitch: u8, velocity: f32, gate: u64, glide_seconds: f32) {
        self.pitch = pitch;
        self.target_freq = pitch_hz(pitch as f32);
        self.set_glide(glide_seconds);
        if self.glide == 0.0 {
            self.freq = self.target_freq;
        }
        self.vel = velocity.clamp(0.0, 1.0);
        self.gain = velocity_gain(self.vel);
        self.gate = gate;
        self.held = true;
    }

    /// Arms the per-sample pitch slew [`Voice::legato`] uses: `seconds` is the
    /// exponential time constant in which a retargeted note closes ~63% of the
    /// distance to its target — the same `time_coef` curve the mono lead uses
    /// at [`LEAD_GLIDE_SECONDS`]. `0` (or a non-finite value) snaps the pitch
    /// instead. Only the oscillator-driven melodic kinds can arm it: drums and
    /// Karplus-Strong strings don't read `self.freq` for pitch (the string's
    /// delay line holds its own tuning), so they retrigger rather than bend.
    fn set_glide(&mut self, seconds: f32) {
        let bends = matches!(
            self.kind,
            Kind::Piano | Kind::Bass | Kind::Pad | Kind::Lead | Kind::Preset | Kind::Sample
        );
        self.glide = if bends && seconds.is_finite() && seconds > 0.0 {
            time_coef(seconds, self.sr)
        } else {
            0.0
        };
    }

    /// Mono retrigger of a voice still ringing out: restart envelopes from the current level.
    pub fn retrigger(&mut self, n: &NoteStart) {
        let (amp, menv) = (self.amp, self.menv);
        let (f1, f2, ph) = (self.f1, self.f2, self.ph);
        self.start(n);
        let (fresh_amp, fresh_menv) = (self.amp, self.menv);
        self.amp = amp;
        self.menv = menv;
        self.amp.retrigger(fresh_amp);
        self.menv.retrigger(fresh_menv);
        self.f1 = f1;
        self.f2 = f2;
        self.ph = ph;
    }

    /// Key up: enter the release phase.
    pub fn release(&mut self) {
        if !self.held {
            return;
        }
        self.held = false;
        self.amp.release();
        self.menv.release();
    }

    /// Fades the voice out over `seconds` and frees it.
    pub fn choke(&mut self, seconds: f32) {
        let step = 1.0 / (seconds.max(0.0005) * self.sr);
        if self.fade_step == 0.0 || step > self.fade_step {
            self.fade_step = step;
        }
        self.held = false;
    }

    fn start_drum(&mut self) {
        let sr = self.sr;
        self.kind = Kind::Drum;
        self.drum = drum_kind(self.pitch);
        self.held = false;
        self.amp = Adsr::OFF;
        self.e = [1.0, 1.0, 1.0, 0.0];
        match self.drum {
            DrumKind::Kick => {
                self.c = [
                    time_coef(0.22, sr),
                    time_coef(0.032, sr),
                    time_coef(0.004, sr),
                    0.0,
                ];
                self.lp = OnePole::new(2500.0, sr);
            }
            DrumKind::Snare => {
                self.c = [time_coef(0.045, sr), time_coef(0.085, sr), 0.0, 0.0];
                self.fc = SvfCoefs::new(1500.0, 0.7, sr);
                self.lp = OnePole::new(9000.0, sr);
            }
            DrumKind::ClosedHat | DrumKind::OpenHat => {
                let tau = if self.drum == DrumKind::ClosedHat {
                    0.02
                } else {
                    0.2
                };
                self.c = [time_coef(tau, sr), 0.0, 0.0, 0.0];
                self.fc = SvfCoefs::new(7200.0, 0.8, sr);
                for p in &mut self.ph[..6] {
                    *p = self.rng.unit();
                }
            }
            DrumKind::Clap => {
                self.fc = SvfCoefs::new(1150.0, 1.8, sr);
                self.lp = OnePole::new(3200.0, sr);
            }
            DrumKind::Tom => {
                let base = match self.pitch {
                    41 => 82.0,
                    43 => 98.0,
                    47 => 131.0,
                    48 => 147.0,
                    50 => 175.0,
                    _ => 110.0,
                };
                self.freq = base;
                self.c = [
                    time_coef(0.2, sr),
                    time_coef(0.05, sr),
                    time_coef(0.008, sr),
                    0.0,
                ];
            }
            DrumKind::Crash => {
                self.e = [1.0, 0.0, 0.0, 0.0];
                self.c = [time_coef(0.7, sr), 0.0, 0.0, 0.0];
                self.fc = SvfCoefs::new(4500.0, 0.7, sr);
                for p in &mut self.ph[..6] {
                    *p = self.rng.unit();
                }
            }
            DrumKind::Ride => {
                self.c = [time_coef(0.5, sr), time_coef(0.12, sr), 0.0, 0.0];
                self.fc = SvfCoefs::new(5200.0, 0.9, sr);
                for p in &mut self.ph[..6] {
                    *p = self.rng.unit();
                }
            }
        }
    }

    fn start_pluck(&mut self) {
        let sr = self.sr;
        self.kind = Kind::Pluck;
        let mut f = self.freq.min(sr * 0.25);
        while f < PLUCK_MIN_HZ {
            f *= 2.0;
        }
        self.freq = f;
        // Loop delay = N samples + 0.5 (averaging filter) + all-pass fractional delay.
        let d = sr / f - 0.5;
        let n = ((d - 0.1).floor() as usize).clamp(2, self.ks.len() - 1);
        let frac = (d - n as f32).clamp(0.05, 1.5);
        self.ks_len = n;
        self.ks_pos = 0;
        self.ks_c = (1.0 - frac) / (1.0 + frac);
        self.ks_x1 = 0.0;
        self.ks_y1 = 0.0;
        self.ks_last = 0.0;
        let t60 = (2.6 * 2f32.powf((60.0 - self.pitch as f32) / 30.0)).clamp(0.4, 6.0);
        self.ks_g = 10f32.powf(-3.0 / (t60 * f));
        self.ks_g_release = 10f32.powf(-3.0 / (0.16 * f));
        // Excitation: velocity-filtered noise mixed with one sine cycle for a rounder body.
        let mut lp = OnePole {
            a: 0.22 + 0.6 * self.vel,
            z: 0.0,
        };
        let mut mean = 0.0;
        for (i, s) in self.ks[..n].iter_mut().enumerate() {
            let body = sine(i as f32 / n as f32) * 0.5;
            let v = lp.low(self.rng.noise()) + body;
            *s = v;
            mean += v;
        }
        mean /= n as f32;
        let mut peak = 1e-6f32;
        for s in &mut self.ks[..n] {
            *s -= mean;
            peak = peak.max(s.abs());
        }
        for s in &mut self.ks[..n] {
            *s /= peak;
        }
        self.amp = Adsr::new(sr, 0.0005, 1.0, 1.0, 0.12);
        self.e = [1.0, 0.0, 0.0, 1.0];
        self.c = [t60_coef(0.7, sr), 0.0, 0.0, 0.0];
    }

    /// Steel-string acoustic: same Karplus-Strong delay loop as the pluck, but
    /// with a register-dependent loop low-pass that closes over the first
    /// ~0.7 s (brightness decays faster than loudness, like a real string),
    /// a static pluck-shape excitation with pick-contact noise, four fixed
    /// body-mode resonators and a short band-passed pick burst — all mixed in
    /// [`Self::guitar_sample`].
    fn start_guitar(&mut self) {
        let sr = self.sr;
        self.kind = Kind::Guitar;
        let mut f = self.freq.min(sr * 0.25);
        while f < GUITAR_MIN_HZ {
            f *= 2.0;
        }
        self.freq = f;
        // Loop low-pass cutoff tracks the register (~16 harmonics, darker on
        // the wound bass strings) and opens further for harder picks. The
        // brightness envelope closes it toward `ks_fc_lo` over ~0.7 s.
        let fc_lo = (16.0 * f).clamp(650.0, 12_000.0).min(sr * 0.45);
        let fc_hi = (fc_lo * (2.0 + 0.8 * self.vel))
            .clamp(3600.0, 18_000.0)
            .min(sr * 0.45);
        self.ks_fc_lo = fc_lo;
        self.ks_fc_hi = fc_hi;
        self.ks_svc = SvfCoefs::new(fc_hi, 0.707, sr);
        self.ks_fstep = time_coef(0.7, sr);
        // Loop delay = N samples + 0.5 (averaging filter) + all-pass fractional
        // delay, shortened by the loop filter's group delay so the note stays
        // in tune (the SVF passband lags the bare delay line by ~0.09·sr/fc_lo
        // samples — measured, not derived).
        let gd = 0.09 * sr / fc_lo;
        let d = (sr / f - 0.5 - gd).max(2.0);
        let n = ((d - 0.1).floor() as usize).clamp(2, self.ks.len() - 1);
        let frac = (d - n as f32).clamp(0.05, 1.5);
        self.ks_len = n;
        self.ks_pos = 0;
        self.ks_c = (1.0 - frac) / (1.0 + frac);
        self.ks_x1 = 0.0;
        self.ks_y1 = 0.0;
        self.ks_last = 0.0;
        self.ks_sv.reset();
        self.ks_fenv = 1.0;
        // Amplitude damping: ~60 dB T60 of 1.4–5.5 s across the compass, so
        // wound low strings ring longest — the real instrument's 1.5–4 s
        // ring-out. Released notes are stopped/palm-muted and damp ~7x faster.
        let t60 = (3.4 * 2f32.powf((58.0 - self.pitch as f32) / 26.0)).clamp(1.4, 5.5);
        self.ks_g = 10f32.powf(-3.0 / (t60 * f));
        self.ks_g_release = 10f32.powf(-3.0 / (0.55 * f));
        for s in &mut self.gt {
            s.reset();
        }
        // Pick burst: ~55 ms of noise through a ~2.6–3.4 kHz band-pass —
        // harder picks snap brighter. `f2`/`fc` are unused by the guitar
        // otherwise (f1/f2 are reset for every note in `start`).
        self.fc = SvfCoefs::new(2600.0 + 900.0 * self.vel, 1.1, sr);
        self.g_pick = (0.055 * sr) as u32;
        // Excitation: the static pluck shape — a tent peaking at the pick
        // point ~1/10 of the period from the bridge end (closer to the
        // bridge = brighter attack) — plus localized noise for the pick's
        // contact crunch. Harder picks add more crunch; the loop low-pass
        // above supplies the brightness decay.
        let p = ((n / 10).max(1)).min(n - 1);
        let w = ((0.0022 * sr) as usize).min(n / 3).clamp(2, n);
        let noise_amp = 0.26 * (0.5 + 0.7 * self.vel);
        let mut mean = 0.0;
        for (i, s) in self.ks[..n].iter_mut().enumerate() {
            let tent = if i < p {
                i as f32 / p as f32
            } else {
                (n - i) as f32 / (n - p) as f32
            };
            let d = i.abs_diff(p);
            let di = d.min(n - d);
            let env = if di < w {
                0.5 + 0.5 * (std::f32::consts::PI * di as f32 / w as f32).cos()
            } else {
                0.0
            };
            let v = tent + self.rng.noise() * env * noise_amp;
            *s = v;
            mean += v;
        }
        mean /= n as f32;
        let mut peak = 1e-6f32;
        for s in &mut self.ks[..n] {
            *s -= mean;
            peak = peak.max(s.abs());
        }
        for s in &mut self.ks[..n] {
            *s /= peak;
        }
        self.amp = Adsr::new(sr, 0.0015, 1.0, 1.0, 0.3);
        self.e = [0.0, 0.0, 0.0, 1.0];
    }

    /// Shared init for pack presets and drum-kit pieces: both run the same
    /// oscillator/ADSR/filter DSP ([`Kind::Preset`]); they differ only in
    /// which [`PresetParams`] the note picked.
    fn start_preset(&mut self, p: PresetParams) {
        let sr = self.sr;
        self.kind = Kind::Preset;
        self.preset = p;
        self.amp = Adsr::new(
            sr,
            finite(p.attack, 0.01).clamp(0.0, 5.0).max(0.001),
            finite(p.decay, 0.2).clamp(0.0, 10.0).max(0.001) / 4.0,
            finite(p.sustain, 0.7).clamp(0.0, 1.0),
            finite(p.release, 0.3).clamp(0.005, 20.0) / 5.0,
        );
        self.cutoff = velocity_cutoff(p.cutoff, p.vel_cutoff, self.vel);
        self.fc = SvfCoefs::new(self.cutoff, p.resonance_q(), sr);
        self.hold_left = 0;
        self.hold_value = 0.0;
        self.hold_value_r = 0.0;
        self.choke_group = p.choke_group;
        let spread = finite(p.spread, 0.0).clamp(0.0, 1.0);
        if spread > 0.0 {
            let half = 2f32.powf(spread * SPREAD_CENTS * 0.5 / 1200.0);
            self.spread_ratio = [1.0 / half, half];
            for k in 0..MAX_PRESET_OSCILLATORS {
                self.ph[MAX_PRESET_OSCILLATORS + k] = spread * SPREAD_PHASE;
            }
        } else {
            self.spread_ratio = [1.0; 2];
        }
        self.pitch_env = finite(p.pitch_env_amount, 0.0).clamp(-48.0, 48.0);
        self.pitch_env_coef = t60_coef(finite(p.pitch_env_decay, 0.2).clamp(0.005, 10.0), sr);
        self.filter_env = finite(p.filter_env_amount, 0.0).clamp(-6.0, 6.0);
        self.filter_env_coef = t60_coef(finite(p.filter_env_decay, 0.5).clamp(0.005, 20.0), sr);
        self.lfo = 0.0;
        for k in 0..MAX_PRESET_OSCILLATORS {
            self.e[k] = 1.0;
            self.c[k] = if p.oscs[k].decay > 0.0 {
                t60_coef(p.oscs[k].decay, sr)
            } else {
                1.0
            };
        }
        let rng = &mut self.rng;
        self.noise_value.iter_mut().for_each(|x| *x = rng.noise());
        // An embedded clip (preset `sample` block, or a kit piece sample)
        // upgrades the voice to the sampler kind; the clip's own rate and
        // root pitch set the resample step so `self.freq` — the played note —
        // yields `2^((pitch - root)/12)` times the declared rate. The
        // oscillator path stays armed as the fallback for a bank miss.
        if let Some(clip) = sample_clip(p.sample_slot) {
            self.kind = Kind::Sample;
            self.sample_buf = Some(clip);
            self.sample_pos = 0.0;
            let rate = finite(p.sample_rate, 44_100.0).clamp(4_000.0, 384_000.0);
            self.sample_ratio = rate / pitch_hz(f32::from(p.sample_root.min(127))).max(1.0);
        }
    }

    /// Adds this voice's output to `l`/`r`.
    pub fn render(&mut self, l: &mut [f32], r: &mut [f32]) {
        for (sl, sr) in l.iter_mut().zip(r.iter_mut()) {
            if self.held {
                if self.gate == 0 {
                    self.release();
                } else {
                    self.gate -= 1;
                }
            }
            // Portamento slew: exponential approach to `target_freq`, shared
            // by every kind that can arm `glide` (the mono lead's fixed
            // constant, or a per-track glide time via `set_glide`). Pluck,
            // guitar, drums and kits keep `glide == 0`, so this stays a
            // no-op for them and `target_freq` never diverges from `freq`.
            if self.glide != 0.0 {
                self.freq = self.target_freq + (self.freq - self.target_freq) * self.glide;
            }
            let (a, b) = match self.kind {
                Kind::Drum => self.drum_sample(),
                Kind::Piano => self.piano_sample(),
                Kind::Bass => self.bass_sample(),
                Kind::Pluck => self.pluck_sample(),
                Kind::Guitar => self.guitar_sample(),
                Kind::Pad => self.pad_sample(),
                Kind::Lead => self.lead_sample(),
                Kind::Preset => self.preset_sample(),
                Kind::Sample => self.sampler_sample(),
            };
            let mut g = 1.0;
            if self.fade_step > 0.0 {
                self.fade -= self.fade_step;
                if self.fade <= 0.0 {
                    self.done = true;
                }
                g = self.fade.max(0.0);
            }
            *sl += a * g;
            *sr += b * g;
            self.t = self.t.saturating_add(1);
            if self.done {
                break;
            }
        }
        if self.done {
            self.active = false;
            self.held = false;
            self.level = 0.0;
        } else {
            let env = if self.kind == Kind::Drum {
                self.e[0]
            } else {
                self.amp.level
            };
            self.level = env * self.gain * self.fade;
        }
    }

    fn drum_sample(&mut self) -> (f32, f32) {
        let inv = self.inv_sr;
        let g = self.gain;
        match self.drum {
            DrumKind::Kick => {
                self.e[0] *= self.c[0];
                self.e[1] *= self.c[1];
                self.e[2] *= self.c[2];
                advance(&mut self.ph[0], (46.0 + 125.0 * self.e[1]) * inv);
                let body = sine(self.ph[0]) * self.e[0];
                let click = self.lp.low(self.rng.noise()) * self.e[2] * 0.6;
                if self.e[0] < 2e-4 {
                    self.done = true;
                }
                let y = soft_clip((body + click) * 1.5) * 0.8 * g;
                (y, y)
            }
            DrumKind::Snare => {
                self.e[0] *= self.c[0];
                self.e[1] *= self.c[1];
                advance(&mut self.ph[0], 185.0 * (1.0 + 0.25 * self.e[0]) * inv);
                advance(&mut self.ph[1], 330.0 * inv);
                let tone = (sine(self.ph[0]) * 0.6 + sine(self.ph[1]) * 0.3) * self.e[0];
                let n = self.rng.noise();
                let noise = self.lp.low(self.f1.process(&self.fc, n).high) * self.e[1] * 0.95;
                if self.e[1] < 2e-4 {
                    self.done = true;
                }
                let y = soft_clip((tone + noise) * 1.3) * 0.7 * g;
                (y, y)
            }
            DrumKind::ClosedHat | DrumKind::OpenHat => {
                self.e[0] *= self.c[0];
                let m = self.metal(&HAT_FREQS, 1.0);
                let x = m * 0.55 + self.rng.noise() * 0.5;
                let y = self.f1.process(&self.fc, x).high * self.e[0] * 0.6 * g;
                if self.e[0] < 2e-4 {
                    self.done = true;
                }
                (y, y)
            }
            DrumKind::Clap => {
                let tt = self.t as f32 * inv;
                let env = if tt < 0.03 {
                    (-(tt % 0.0095) / 0.0022).exp()
                } else {
                    (-(tt - 0.03) / 0.09).exp()
                };
                self.e[0] = env;
                if tt > 0.03 && env < 2e-4 {
                    self.done = true;
                }
                let n = self.rng.noise();
                let y = self.f1.process(&self.fc, n).band * env * 1.6 * g;
                // A slightly later right channel widens the clap.
                let r = self.lp.low(y);
                (y, y * 0.75 + r * 0.25)
            }
            DrumKind::Tom => {
                self.e[0] *= self.c[0];
                self.e[1] *= self.c[1];
                self.e[2] *= self.c[2];
                advance(&mut self.ph[0], self.freq * (1.0 + 0.45 * self.e[1]) * inv);
                let y = sine(self.ph[0]) * self.e[0] + self.rng.noise() * self.e[2] * 0.15;
                if self.e[0] < 2e-4 {
                    self.done = true;
                }
                let y = soft_clip(y * 1.2) * 0.75 * g;
                (y, y)
            }
            DrumKind::Crash => {
                self.e[0] *= self.c[0];
                self.e[1] += (1.0 - self.e[1]) * 0.08;
                let m = self.metal(&HAT_FREQS, 1.47) * 0.4;
                let nl = self.rng.noise();
                let nr = self.rng.noise();
                let env = self.e[0] * self.e[1] * 0.42 * g;
                let l = self.f1.process(&self.fc, m + nl * 0.7).high * env;
                let r = self.f2.process(&self.fc, m + nr * 0.7).high * env;
                if self.e[0] < 2e-4 {
                    self.done = true;
                }
                (l, r)
            }
            DrumKind::Ride => {
                self.e[0] *= self.c[0];
                self.e[1] *= self.c[1];
                let m = self.metal(&RIDE_FREQS, 2.1);
                let n = self.rng.noise();
                advance(&mut self.ph[6], 3150.0 * inv);
                let ping = sine(self.ph[6]) * self.e[1] * 0.12;
                let y = (self.f1.process(&self.fc, m * 0.7 + n * 0.15).band * 1.3 + ping)
                    * self.e[0]
                    * 0.5
                    * g;
                if self.e[0] < 2e-4 {
                    self.done = true;
                }
                (y, y)
            }
        }
    }

    /// Six detuned square oscillators, the classic analog cymbal source.
    #[inline]
    fn metal(&mut self, freqs: &[f32; 6], scale: f32) -> f32 {
        let mut sum = 0.0;
        for (k, f) in freqs.iter().enumerate() {
            advance(&mut self.ph[k], f * scale * self.inv_sr);
            sum += if self.ph[k] < 0.5 { 1.0 } else { -1.0 };
        }
        sum / 6.0
    }

    fn piano_sample(&mut self) -> (f32, f32) {
        let a = self.amp.next();
        if self.amp.is_off() {
            self.done = true;
        }
        self.e[0] *= self.c[0];
        self.e[1] *= self.c[1];
        let dt = self.freq * self.inv_sr;
        let index = self.e[2] * (0.18 + 0.82 * self.e[0]);
        let modulator = sine(self.ph[1]);
        let carrier = (std::f32::consts::TAU * self.ph[0] + index * modulator).sin();
        let tine_dt = dt * 14.0;
        let tine = if tine_dt < 0.45 {
            sine(self.ph[2]) * self.e[1] * 0.07 * self.vel
        } else {
            0.0
        };
        advance(&mut self.ph[0], dt);
        advance(&mut self.ph[1], dt * 1.0006);
        advance(&mut self.ph[2], tine_dt.min(0.49));
        advance(&mut self.lfo, 4.6 * self.inv_sr);
        let trem = sine(self.lfo) * 0.1;
        let s = (carrier + tine) * a * self.gain * 0.3;
        (s * (1.0 + trem), s * (1.0 - trem))
    }

    fn bass_sample(&mut self) -> (f32, f32) {
        let a = self.amp.next();
        if self.amp.is_off() {
            self.done = true;
        }
        let m = self.menv.next();
        if self.ctl == 0 {
            let cutoff = self.freq * 1.2 + 90.0 + m * (250.0 + 2600.0 * self.vel);
            self.fc = SvfCoefs::new(cutoff, 1.3, self.sr);
        }
        self.ctl = (self.ctl + 1) % CONTROL_INTERVAL;
        let dt = self.freq * self.inv_sr;
        let x = saw(self.ph[0], dt) * 0.55
            + sine(self.ph[1]) * 0.75
            + saw(self.ph[2], dt * 1.004) * 0.2;
        advance(&mut self.ph[0], dt);
        advance(&mut self.ph[1], dt);
        advance(&mut self.ph[2], dt * 1.004);
        let y = soft_clip(self.f1.low(&self.fc, x) * 1.6) * 0.62 * a * self.gain * 0.6;
        (y, y)
    }

    fn pluck_sample(&mut self) -> (f32, f32) {
        let a = self.amp.next();
        let x = self.ks[self.ks_pos];
        let avg = 0.5 * (x + self.ks_last);
        self.ks_last = x;
        let ap = self.ks_c * avg + self.ks_x1 - self.ks_c * self.ks_y1;
        self.ks_x1 = avg;
        self.ks_y1 = flush(ap);
        let g = if self.held {
            self.ks_g
        } else {
            self.ks_g_release
        };
        self.ks[self.ks_pos] = flush(ap * g);
        self.ks_pos += 1;
        if self.ks_pos >= self.ks_len {
            self.ks_pos = 0;
        }
        // Glassy octave partial that fades faster than the string.
        self.e[0] *= self.c[0];
        let dt2 = (self.freq * 2.0 * self.inv_sr).min(0.49);
        let bell = sine(self.ph[0]) * self.e[0] * 0.22;
        advance(&mut self.ph[0], dt2);
        self.e[3] = (x.abs()).max(self.e[3] * 0.9995);
        if self.amp.is_off() || (self.t as f32 > self.sr * 0.05 && self.e[3] < 2e-5) {
            self.done = true;
        }
        let y = (x * 0.9 + bell) * a * self.gain * 0.55;
        (y, y)
    }

    /// Steel-string guitar voice: Karplus-Strong string with a closing loop
    /// low-pass, parallel body-mode resonators, a band-passed pick burst and
    /// a gentle output saturation. Mono, like the pluck.
    fn guitar_sample(&mut self) -> (f32, f32) {
        let a = self.amp.next();
        let x = self.ks[self.ks_pos];
        let avg = 0.5 * (x + self.ks_last);
        self.ks_last = x;
        let ap = self.ks_c * avg + self.ks_x1 - self.ks_c * self.ks_y1;
        self.ks_x1 = avg;
        self.ks_y1 = flush(ap);
        // Loop low-pass glides from the bright pick toward the darker sustain;
        // coefficients refresh every 64 samples (inaudible zipper on a 0.5 s glide).
        self.ks_fenv *= self.ks_fstep;
        if (self.t & 63) == 0 && self.ks_fenv > 0.002 {
            let fc = self.ks_fc_lo + (self.ks_fc_hi - self.ks_fc_lo) * self.ks_fenv;
            self.ks_svc = SvfCoefs::new(fc, 0.707, self.sr);
        }
        let lp = self.ks_sv.low(&self.ks_svc, ap);
        let g = if self.held {
            self.ks_g
        } else {
            self.ks_g_release
        };
        self.ks[self.ks_pos] = flush(lp * g);
        self.ks_pos += 1;
        if self.ks_pos >= self.ks_len {
            self.ks_pos = 0;
        }
        // Wooden body: the string drives four fixed resonances in parallel.
        let mut body = 0.0;
        for i in 0..GUITAR_BODY.len() {
            body += self.gt[i].process(&self.gtc[i], x).band * GUITAR_BODY[i].2;
        }
        let mut y = x * 0.85 + body * GUITAR_BODY_MIX;
        // Pick burst: band-passed noise with a ~20 ms decay — the attack's
        // brightness and "quack" that the mellower sustain lacks.
        if self.t < self.g_pick {
            let env = (-3.0 * self.t as f32 / self.g_pick as f32).exp();
            y += self.f2.process(&self.fc, self.rng.noise()).band * env * (0.08 + 0.13 * self.vel);
        }
        // Same ring-out detector as the pluck: `e[3]` follows the string's
        // recent peak so the voice frees itself once the note has died.
        self.e[3] = (x.abs()).max(self.e[3] * 0.9995);
        if self.amp.is_off() || (self.t as f32 > self.sr * 0.05 && self.e[3] < 2e-5) {
            self.done = true;
        }
        // Gentle saturation for pick-edge warmth — kept subtle for a clean tone.
        let y = soft_clip(y * 1.45) * 0.52 * a * self.gain;
        (y, y)
    }

    fn pad_sample(&mut self) -> (f32, f32) {
        let a = self.amp.next();
        if self.amp.is_off() {
            self.done = true;
        }
        let m = self.menv.next();
        advance(&mut self.lfo, 0.11 * self.inv_sr);
        if self.ctl == 0 {
            // Key tracking: never filter below the third harmonic, so high notes keep
            // their fundamental while low notes stay dark.
            let cutoff = (600.0 + 2400.0 * self.vel * self.vel).max(self.freq * 3.0)
                * (0.55 + 0.45 * m)
                * (1.0 + 0.22 * sine(self.lfo));
            self.fc = SvfCoefs::new(cutoff, 0.9, self.sr);
        }
        self.ctl = (self.ctl + 1) % CONTROL_INTERVAL;
        let dt = self.freq * self.inv_sr;
        const DETUNE: [f32; 3] = [0.994_816, 1.0, 1.005_212]; // -9, 0, +9 cents
        let s0 = saw(self.ph[0], dt * DETUNE[0]);
        let s1 = saw(self.ph[1], dt);
        let s2 = saw(self.ph[2], dt * DETUNE[2]);
        let sub = sine(self.ph[3]) * 0.2;
        advance(&mut self.ph[0], dt * DETUNE[0]);
        advance(&mut self.ph[1], dt);
        advance(&mut self.ph[2], dt * DETUNE[2]);
        advance(&mut self.ph[3], dt * 0.5);
        let l = s0 * 0.75 + s1 * 0.5 + s2 * 0.25 + sub;
        let r = s2 * 0.75 + s1 * 0.5 + s0 * 0.25 + sub;
        let g = a * self.gain * 0.2;
        (self.f1.low(&self.fc, l) * g, self.f2.low(&self.fc, r) * g)
    }

    fn lead_sample(&mut self) -> (f32, f32) {
        let a = self.amp.next();
        if self.amp.is_off() {
            self.done = true;
        }
        let m = self.menv.next();
        let ramp = (self.t as f32 / (0.35 * self.sr)).min(1.0);
        advance(&mut self.lfo, 5.4 * self.inv_sr);
        let f = self.freq * (1.0 + ramp * ramp * 0.0105 * sine(self.lfo));
        if self.ctl == 0 {
            let cutoff = f * 1.5 + 500.0 + m * (1500.0 + 4200.0 * self.vel);
            self.fc = SvfCoefs::new(cutoff, 1.0, self.sr);
        }
        self.ctl = (self.ctl + 1) % CONTROL_INTERVAL;
        let dt = f * self.inv_sr;
        let x = saw(self.ph[0], dt * 1.0023) * 0.5
            + square(self.ph[1], dt * 0.9977) * 0.42
            + sine(self.ph[2]) * 0.18;
        advance(&mut self.ph[0], dt * 1.0023);
        advance(&mut self.ph[1], dt * 0.9977);
        advance(&mut self.ph[2], dt * 0.5);
        let y = self.f1.low(&self.fc, x) * a * self.gain * 0.36;
        (y, y)
    }

    fn preset_sample(&mut self) -> (f32, f32) {
        let a = self.amp.next();
        if self.amp.is_off() {
            self.done = true;
        }
        let p = self.preset;
        if p.uses_legacy_preset_path() {
            return self.preset_sample_legacy(a, p);
        }

        let lfo = self.next_preset_lfo(p);
        let pitch = (self.pitch_env + lfo * finite(p.lfo_pitch, 0.0)).clamp(-60.0, 60.0);
        let ratio = if pitch == 0.0 {
            1.0
        } else {
            2f32.powf(pitch / 12.0)
        };
        let freq = (self.freq * ratio).clamp(1.0, self.sr * 0.45);
        self.pitch_env *= self.pitch_env_coef;

        if self.ctl == 0 {
            let octaves = (self.filter_env + lfo * finite(p.lfo_filter, 0.0)).clamp(-8.0, 8.0);
            let cutoff = (self.cutoff * 2f32.powf(octaves)).clamp(10.0, self.sr * 0.47);
            self.fc = SvfCoefs::new(cutoff, p.resonance_q(), self.sr);
        }
        self.ctl = (self.ctl + 1) % CONTROL_INTERVAL;
        self.filter_env *= self.filter_env_coef;

        let mut sum = 0.0;
        let mut total = 0.0;
        for (k, o) in p.oscs.iter().take(p.count).enumerate() {
            // Defensive re-clamp: PresetParams may be built by hand, bypassing
            // from_preset's validation clamps.
            let o = OscParams {
                level: finite(o.level, 0.0).clamp(0.0, 1.0),
                ratio: finite(o.ratio, 1.0).clamp(0.0, 64.0),
                width: finite(o.width, 0.5).clamp(0.05, 0.95),
                ..*o
            };
            let inc = (freq * o.ratio * self.inv_sr).clamp(0.0, 0.49);
            let v = self.preset_osc_sample(k, o, inc, freq);
            let layer = if o.decay > 0.0 {
                let env = self.e[k];
                self.e[k] *= self.c[k];
                env
            } else {
                1.0
            };
            sum += v * o.level * layer;
            total += o.level;
        }
        let trem = 1.0 - finite(p.lfo_amp, 0.0).clamp(0.0, 1.0) * (0.5 + 0.5 * lfo);
        let raw = sum / total.max(1.0) * a * trem.clamp(0.0, 1.0);
        self.finish_preset_sample(raw, p)
    }

    fn preset_sample_legacy(&mut self, a: f32, p: PresetParams) -> (f32, f32) {
        let mut sum = 0.0;
        let mut total = 0.0;
        for (k, &o) in p.oscs.iter().take(p.count).enumerate() {
            let inc = (self.freq * o.ratio * self.inv_sr).min(0.49);
            let v = match o.wave {
                Waveform::Sine => sine(self.ph[k]),
                Waveform::Triangle => 4.0 * (self.ph[k] - 0.5).abs() - 1.0,
                Waveform::Saw => saw(self.ph[k], inc),
                Waveform::Square => square(self.ph[k], inc),
                Waveform::Pulse => pulse(self.ph[k], inc, o.width),
                Waveform::Noise => self.noise_value[k],
            };
            if o.wave == Waveform::Noise {
                // Pitched noise: a new random value at a rate that follows the note.
                self.ph[k] += (self.freq * o.ratio * 64.0 * self.inv_sr).min(1.0);
                if self.ph[k] >= 1.0 {
                    self.ph[k] -= self.ph[k].floor();
                    self.noise_value[k] = self.rng.noise();
                }
            } else {
                advance(&mut self.ph[k], inc);
            }
            sum += v * o.level;
            total += o.level;
        }
        self.finish_preset_sample(sum / total.max(1.0) * a, p)
    }

    fn finish_preset_sample(&mut self, raw: f32, p: PresetParams) -> (f32, f32) {
        if self.hold_left == 0 {
            self.hold_value = raw;
            self.hold_left = ((p.hold as f32 * self.sr / 44_100.0).round() as u32).max(1);
        }
        self.hold_left -= 1;
        let mut x = self.hold_value;
        if p.bits < 24 {
            let q = (1u32 << (p.bits - 1)) as f32;
            x = (x * q).round() / q;
        }
        let y = self.f1.low(&self.fc, x) * p.gain * self.gain * 0.55;
        (y, y)
    }

    /// PCM clip playback for `sample` presets and kit pieces: the registered
    /// buffer is read with linear interpolation at `2^((pitch - root)/12)`
    /// times its declared rate — `self.freq` (the played note, with portamento
    /// plus the preset's pitch envelope and LFO) times `sample_ratio` sets the
    /// step — one-shot or looping while held, then shaped by the shared
    /// amp/tremolo/bitcrush/hold/filter/gain tail. Bounded like every voice:
    /// a one-shot ends when the clip does, a loop when the amp envelope ends.
    fn sampler_sample(&mut self) -> (f32, f32) {
        let a = self.amp.next();
        if self.amp.is_off() {
            self.done = true;
        }
        let p = self.preset;
        let lfo = if p.uses_legacy_preset_path() {
            // No modulation armed: keep the clip path as cheap as the legacy
            // oscillator path (no LFO, no filter retuning, no envelope decay).
            0.0
        } else {
            let lfo = self.next_preset_lfo(p);
            if self.ctl == 0 {
                let octaves = (self.filter_env + lfo * finite(p.lfo_filter, 0.0)).clamp(-8.0, 8.0);
                let cutoff = (self.cutoff * 2f32.powf(octaves)).clamp(10.0, self.sr * 0.47);
                self.fc = SvfCoefs::new(cutoff, p.resonance_q(), self.sr);
            }
            self.ctl = (self.ctl + 1) % CONTROL_INTERVAL;
            self.filter_env *= self.filter_env_coef;
            lfo
        };
        self.pitch_env *= self.pitch_env_coef;
        let pitch = (self.pitch_env + lfo * finite(p.lfo_pitch, 0.0)).clamp(-60.0, 60.0);
        let ratio = if pitch == 0.0 {
            1.0
        } else {
            2f32.powf(pitch / 12.0)
        };
        let freq = (self.freq * ratio).clamp(1.0, self.sr * 0.45);
        // Frames of clip per output sample: played rate = freq * ratio Hz.
        // +512× per output sample is far past any usable transposition.
        let inc = (freq * self.sample_ratio * self.inv_sr).clamp(0.0, 512.0) as f64;

        let raw = match self.sample_buf {
            Some(buf) if !buf.is_empty() => {
                let len = buf.len();
                let pos = self.sample_pos;
                let i = pos as usize;
                let s0 = buf.get(i).copied().unwrap_or(0.0);
                // At the loop seam the interp target wraps to the first
                // frame so a seamless loop stays seamless.
                let j = if p.sample_loop && i + 1 >= len {
                    0
                } else {
                    i.saturating_add(1).min(len - 1)
                };
                let s1 = buf.get(j).copied().unwrap_or(0.0);
                let v = s0 + (s1 - s0) * (pos - i as f64) as f32;
                let next = pos + inc;
                if p.sample_loop {
                    self.sample_pos = next.rem_euclid(len as f64);
                } else {
                    self.sample_pos = next;
                    if next >= len as f64 {
                        // The one-shot has played through: let the tail apply
                        // to this last frame, then free the voice.
                        self.done = true;
                    }
                }
                v
            }
            _ => {
                self.done = true;
                0.0
            }
        };
        let trem = 1.0 - finite(p.lfo_amp, 0.0).clamp(0.0, 1.0) * (0.5 + 0.5 * lfo);
        self.finish_preset_sample(raw * a * trem.clamp(0.0, 1.0), p)
    }

    fn preset_osc_sample(&mut self, k: usize, o: OscParams, inc: f32, freq: f32) -> f32 {
        let v = match o.wave {
            Waveform::Sine => sine(self.ph[k]),
            Waveform::Triangle => 4.0 * (self.ph[k] - 0.5).abs() - 1.0,
            Waveform::Saw => saw(self.ph[k], inc),
            Waveform::Square => square(self.ph[k], inc),
            Waveform::Pulse => pulse(self.ph[k], inc, o.width),
            Waveform::Noise => self.noise_value[k],
        };
        if o.wave == Waveform::Noise {
            self.ph[k] += (freq * o.ratio * 64.0 * self.inv_sr).min(1.0);
            if self.ph[k] >= 1.0 {
                self.ph[k] -= self.ph[k].floor();
                self.noise_value[k] = self.rng.noise();
            }
        } else {
            advance(&mut self.ph[k], inc);
        }
        v
    }

    fn next_preset_lfo(&mut self, p: PresetParams) -> f32 {
        let value = match p.lfo_wave {
            LfoWaveform::Sine => sine(self.lfo),
            LfoWaveform::Triangle => 1.0 - 4.0 * (self.lfo - 0.5).abs(),
        };
        advance(
            &mut self.lfo,
            finite(p.lfo_rate, 1.0).clamp(0.05, 20.0) * self.inv_sr,
        );
        value.clamp(-1.0, 1.0)
    }
}

/// Band-limited pulse with duty `width`, DC removed.
#[inline]
fn pulse(phase: f32, dt: f32, width: f32) -> f32 {
    let mut v = if phase < width { 1.0 } else { -1.0 };
    v += poly_blep(phase, dt);
    let shifted = phase - width;
    v -= poly_blep(
        if shifted < 0.0 {
            shifted + 1.0
        } else {
            shifted
        },
        dt,
    );
    v - (2.0 * width - 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packs::{Envelope, FilterEnvelope, LfoSpec, OscillatorSpec, PitchEnvelope};

    const SR: f32 = 44_100.0;

    fn render(sound: Sound, pitch: u8, seconds: f32, gate_seconds: f32) -> Vec<f32> {
        render_chunked(sound, pitch, seconds, gate_seconds, 256)
    }

    fn render_chunked(
        sound: Sound,
        pitch: u8,
        seconds: f32,
        gate_seconds: f32,
        chunk: usize,
    ) -> Vec<f32> {
        let mut v = Voice::new(SR);
        v.start(&NoteStart {
            sound,
            strip: 0,
            pitch,
            velocity: 0.8,
            gate: (gate_seconds * SR) as u64,
            from_song: true,
            age: 1,
        });
        let n = (seconds * SR) as usize;
        let mut l = vec![0.0; n];
        let mut r = vec![0.0; n];
        for (cl, cr) in l.chunks_mut(chunk).zip(r.chunks_mut(chunk)) {
            if v.active {
                v.render(cl, cr);
            }
        }
        l.iter().zip(&r).map(|(a, b)| (a + b) * 0.5).collect()
    }

    /// Spectral-ish fingerprint: loudness envelope, brightness and pitch-crossing rate.
    fn features(x: &[f32]) -> [f32; 6] {
        let rms = |s: &[f32]| (s.iter().map(|v| v * v).sum::<f32>() / s.len().max(1) as f32).sqrt();
        let total = rms(x).max(1e-9);
        let seg = |a: f32, b: f32| rms(&x[(a * SR) as usize..(b * SR) as usize]) / total;
        let diff: Vec<f32> = x.windows(2).map(|w| w[1] - w[0]).collect();
        let brightness = rms(&diff) / total;
        let zc = x
            .windows(2)
            .filter(|w| (w[0] >= 0.0) != (w[1] >= 0.0))
            .count() as f32
            / x.len() as f32;
        [
            seg(0.0, 0.03),
            seg(0.03, 0.15),
            seg(0.15, 0.5),
            seg(0.5, 1.2),
            brightness * 4.0,
            zc * 40.0,
        ]
    }

    fn distance(a: &[f32; 6], b: &[f32; 6]) -> f32 {
        a.iter().zip(b).map(|(x, y)| (x - y).abs()).sum()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    fn mean_abs_diff(a: &[f32], b: &[f32]) -> f32 {
        a.iter().zip(b).map(|(x, y)| (x - y).abs()).sum::<f32>()
            / a.len().min(b.len()).max(1) as f32
    }

    fn zc_rate(x: &[f32]) -> f32 {
        x.windows(2)
            .filter(|w| (w[0] >= 0.0) != (w[1] >= 0.0))
            .count() as f32
            / x.len().max(1) as f32
    }

    fn brightness(x: &[f32]) -> f32 {
        let diff: Vec<f32> = x.windows(2).map(|w| w[1] - w[0]).collect();
        rms(&diff) / rms(x).max(1e-9)
    }

    fn preset_sound(preset: InstrumentPreset) -> Sound {
        crate::packs::validate_preset(&preset).unwrap();
        Sound::Preset(PresetParams::from_preset(&preset))
    }

    pub(crate) fn chip_preset() -> InstrumentPreset {
        InstrumentPreset {
            id: "test.chip".into(),
            name: "Chip".into(),
            description: String::new(),
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
                    waveform: Waveform::Triangle,
                    level: 0.4,
                    frequency_ratio: 0.5,
                    detune_cents: 3.0,
                    pulse_width: 0.5,
                    decay: None,
                },
            ],
            envelope: Envelope {
                attack: 0.002,
                decay: 0.2,
                sustain: 0.5,
                release: 0.1,
            },
            pitch_envelope: None,
            filter_envelope: None,
            filter_cutoff: 8000.0,
            filter_resonance: 0.0,
            lfo: None,
            bit_depth: 6,
            sample_hold: 3,
            gain: 0.9,
            monophonic: false,
            velocity_to_cutoff: crate::packs::DEFAULT_VELOCITY_TO_CUTOFF,
            spread: 0.0,
            hidden: false,
            audition_note: 60,
            audition_duration: 0.8,
            kit: None,
            sample: None,
        }
    }

    #[test]
    fn every_sound_is_finite_audible_and_ends() {
        let mut sounds: Vec<Sound> = Instrument::ALL.iter().map(|i| Sound::Builtin(*i)).collect();
        sounds.push(Sound::Preset(PresetParams::from_preset(&chip_preset())));
        sounds.push(sample_sound(false));
        sounds.push(kit_sound());
        for sound in sounds {
            let pitches: Vec<u8> = if sound.is_drums() {
                crate::model::DRUMS.iter().map(|d| d.0).collect()
            } else {
                vec![24, 60, 96, 127]
            };
            for pitch in pitches {
                let x = render(sound, pitch, 8.0, 0.3);
                assert!(
                    x.iter().all(|s| s.is_finite()),
                    "{sound:?} {pitch} not finite"
                );
                let peak = x.iter().fold(0.0f32, |m, s| m.max(s.abs()));
                assert!(peak > 0.01, "{sound:?} pitch {pitch} silent (peak {peak})");
                assert!(peak < 1.5, "{sound:?} pitch {pitch} too hot (peak {peak})");
                let tail = &x[(7.5 * SR) as usize..];
                assert!(
                    tail.iter().all(|s| s.abs() < 1e-3),
                    "{sound:?} {pitch} never ends"
                );
            }
        }
    }

    #[test]
    fn built_in_instruments_have_distinct_timbres() {
        let mut prints: Vec<(String, [f32; 6])> = Instrument::ALL[1..]
            .iter()
            .map(|i| {
                (
                    i.name().to_string(),
                    features(&render(Sound::Builtin(*i), 60, 1.2, 0.6)),
                )
            })
            .collect();
        prints.push((
            "chip".into(),
            features(&render(
                Sound::Preset(PresetParams::from_preset(&chip_preset())),
                60,
                1.2,
                0.6,
            )),
        ));
        for (kick, name) in [(36u8, "kick"), (38, "snare"), (42, "hat")] {
            prints.push((
                name.into(),
                features(&render(Sound::Builtin(Instrument::Drums), kick, 1.2, 0.1)),
            ));
        }
        for (i, a) in prints.iter().enumerate() {
            for b in prints.iter().skip(i + 1) {
                let d = distance(&a.1, &b.1);
                assert!(
                    d > 0.25,
                    "{} and {} sound too alike ({d}): {:?} {:?}",
                    a.0,
                    b.0,
                    a.1,
                    b.1
                );
            }
        }
    }

    #[test]
    fn pluck_and_piano_are_in_tune() {
        for (sound, pitch) in [
            (Instrument::Pluck, 69u8),
            (Instrument::Pluck, 57),
            (Instrument::Piano, 69),
            (Instrument::Bass, 45),
        ] {
            let x = render(Sound::Builtin(sound), pitch, 0.6, 0.6);
            let seg = &x[(0.1 * SR) as usize..(0.5 * SR) as usize];
            // Autocorrelation peak near the expected period.
            let expected = SR / pitch_hz(pitch as f32);
            let lo = (expected * 0.9) as usize;
            let hi = (expected * 1.1) as usize;
            let best = (lo..=hi)
                .max_by(|&a, &b| {
                    let ca: f32 = seg.iter().zip(&seg[a..]).map(|(p, q)| p * q).sum();
                    let cb: f32 = seg.iter().zip(&seg[b..]).map(|(p, q)| p * q).sum();
                    ca.total_cmp(&cb)
                })
                .unwrap();
            assert!(
                (best as f32 - expected).abs() <= 1.5,
                "{sound:?} {pitch}: period {best} vs {expected}"
            );
        }
    }

    #[test]
    fn legacy_defaults_keep_identical_preset_audio() {
        let legacy = chip_preset();
        let mut explicit = legacy.clone();
        explicit.pitch_envelope = Some(PitchEnvelope::default());
        explicit.filter_envelope = Some(FilterEnvelope::default());
        explicit.filter_resonance = 0.0;
        explicit.lfo = Some(LfoSpec::default());
        for osc in &mut explicit.oscillators {
            osc.decay = None;
        }
        let a = render(preset_sound(legacy), 60, 0.8, 0.5);
        let b = render(preset_sound(explicit), 60, 0.8, 0.5);
        assert_eq!(
            a, b,
            "default new controls must preserve legacy preset output"
        );
    }

    #[test]
    fn preset_params_clamp_new_fields_for_audio_safety() {
        let mut p = chip_preset();
        p.oscillators[0].decay = Some(f32::INFINITY);
        p.pitch_envelope = Some(PitchEnvelope {
            amount_semitones: f32::NAN,
            decay: -2.0,
        });
        p.filter_envelope = Some(FilterEnvelope {
            amount_octaves: 12.0,
            decay: f32::INFINITY,
        });
        p.filter_resonance = 3.0;
        p.lfo = Some(LfoSpec {
            rate_hz: f32::NAN,
            pitch_semitones: 90.0,
            filter_octaves: 9.0,
            amplitude: 2.0,
            ..Default::default()
        });
        p.bit_depth = 99;
        p.sample_hold = 0;
        p.gain = f32::NAN;
        let params = PresetParams::from_preset(&p);
        assert_eq!(params.oscs[0].decay, 0.005);
        assert_eq!(params.pitch_env_amount, 0.0);
        assert_eq!(params.pitch_env_decay, 0.005);
        assert_eq!(params.filter_env_amount, 6.0);
        assert_eq!(params.filter_env_decay, 0.5);
        assert_eq!(params.resonance, 0.95);
        assert_eq!(params.lfo_rate, 1.0);
        assert_eq!(params.lfo_pitch, 12.0);
        assert_eq!(params.lfo_filter, 4.0);
        assert_eq!(params.lfo_amp, 1.0);
        assert_eq!(params.bits, 24);
        assert_eq!(params.hold, 1);
        assert_eq!(params.gain, 0.9);
    }

    #[test]
    fn pitch_envelope_filter_motion_resonance_decay_and_lfo_are_audible() {
        let mut pitchy = chip_preset();
        pitchy.bit_depth = 24;
        pitchy.sample_hold = 1;
        pitchy.oscillators = vec![OscillatorSpec {
            waveform: Waveform::Sine,
            ..Default::default()
        }];
        pitchy.envelope = Envelope {
            attack: 0.0,
            decay: 0.0,
            sustain: 1.0,
            release: 0.05,
        };
        pitchy.pitch_envelope = Some(PitchEnvelope {
            amount_semitones: 24.0,
            decay: 0.2,
        });
        let pitch_audio = render(preset_sound(pitchy), 60, 0.6, 0.55);
        assert!(
            zc_rate(&pitch_audio[(0.0 * SR) as usize..(0.04 * SR) as usize])
                > zc_rate(&pitch_audio[(0.4 * SR) as usize..(0.55 * SR) as usize]) * 1.45,
            "positive pitch envelope should decay from a higher pitch"
        );

        let mut sweep = chip_preset();
        sweep.bit_depth = 24;
        sweep.sample_hold = 1;
        sweep.oscillators = vec![OscillatorSpec {
            waveform: Waveform::Saw,
            ..Default::default()
        }];
        sweep.envelope = Envelope {
            attack: 0.0,
            decay: 0.0,
            sustain: 1.0,
            release: 0.05,
        };
        sweep.filter_cutoff = 4_000.0;
        sweep.filter_envelope = Some(FilterEnvelope {
            amount_octaves: -4.0,
            decay: 0.25,
        });
        let sweep_audio = render(preset_sound(sweep.clone()), 48, 1.3, 1.25);
        // Fixed references bracket the sweep: `dark` sits at the envelope's
        // starting cutoff (4000 * 2^-4 = 250 Hz), `bright` at its settled base.
        let mut fixed_dark = sweep.clone();
        fixed_dark.filter_envelope = None;
        fixed_dark.filter_cutoff = 250.0;
        let dark_audio = render(preset_sound(fixed_dark), 48, 1.3, 1.25);
        let mut fixed_bright = sweep.clone();
        fixed_bright.filter_envelope = None;
        let bright_audio = render(preset_sound(fixed_bright), 48, 1.3, 1.25);
        let early = (0.005 * SR) as usize..(0.05 * SR) as usize;
        let late = (0.9 * SR) as usize..(1.2 * SR) as usize;
        // The opening filter is darker early and brighter late.
        assert!(
            brightness(&sweep_audio[early.clone()]) < brightness(&sweep_audio[late.clone()]),
            "negative filter envelope should open the cutoff over time"
        );
        // Early in the note the sweep is spectrally nearer the dark reference
        // than the bright one.
        let bs = brightness(&sweep_audio[early.clone()]);
        let bd = brightness(&dark_audio[early.clone()]);
        let bb = brightness(&bright_audio[early.clone()]);
        assert!(
            (bs - bd).abs() < (bs - bb).abs(),
            "early sweep brightness {bs} should be nearer dark {bd} than bright {bb}"
        );
        // Once the T60 envelope has decayed, the output is the unmodulated
        // preset at its base cutoff — bit for bit.
        assert_eq!(
            &sweep_audio[late.clone()],
            &bright_audio[late.clone()],
            "settled filter envelope must leave the base-cutoff signal untouched"
        );

        let mut resonant = sweep.clone();
        resonant.filter_envelope = None;
        let plain = render(preset_sound(resonant.clone()), 48, 0.4, 0.35);
        resonant.filter_resonance = 0.9;
        let res = render(preset_sound(resonant), 48, 0.4, 0.35);
        assert!(
            mean_abs_diff(&plain, &res) > 0.003,
            "resonance should change the tone"
        );
        assert!(res.iter().all(|s| s.is_finite() && s.abs() < 1.5));

        let mut decaying_layer = chip_preset();
        decaying_layer.bit_depth = 24;
        decaying_layer.sample_hold = 1;
        decaying_layer.oscillators = vec![OscillatorSpec {
            waveform: Waveform::Sine,
            decay: Some(0.035),
            ..Default::default()
        }];
        decaying_layer.envelope = Envelope {
            attack: 0.0,
            decay: 0.0,
            sustain: 1.0,
            release: 0.05,
        };
        let decayed = render(preset_sound(decaying_layer), 0, 0.35, 0.3);
        assert!(
            rms(&decayed[(0.01 * SR) as usize..(0.04 * SR) as usize])
                > rms(&decayed[(0.25 * SR) as usize..(0.34 * SR) as usize]) * 50.0,
            "oscillator layer decay should be an independent T60 fade"
        );

        let mut lfo = chip_preset();
        lfo.bit_depth = 24;
        lfo.sample_hold = 1;
        lfo.oscillators = vec![OscillatorSpec {
            waveform: Waveform::Sine,
            ..Default::default()
        }];
        lfo.envelope = Envelope {
            attack: 0.0,
            decay: 0.0,
            sustain: 1.0,
            release: 0.05,
        };
        let base = render(preset_sound(lfo.clone()), 60, 0.8, 0.75);
        lfo.lfo = Some(LfoSpec {
            rate_hz: 2.0,
            pitch_semitones: 1.0,
            amplitude: 0.7,
            ..Default::default()
        });
        let moved = render(preset_sound(lfo), 60, 0.8, 0.75);
        assert!(
            mean_abs_diff(&base, &moved) > 0.01,
            "LFO should modulate pitch/amplitude"
        );
        assert!(moved.iter().all(|s| s.is_finite() && s.abs() < 1.5));
    }

    #[test]
    fn preset_resonant_low_cutoff_stays_finite_and_sustains() {
        // A held saw through a resonant low cutoff: the SVF must remain stable
        // for the whole note — finite, bounded, and still audible late.
        let mut p = chip_preset();
        p.bit_depth = 24;
        p.sample_hold = 1;
        p.oscillators = vec![OscillatorSpec {
            waveform: Waveform::Saw,
            ..Default::default()
        }];
        p.envelope = Envelope {
            attack: 0.0,
            decay: 0.0,
            sustain: 1.0,
            release: 0.05,
        };
        p.filter_cutoff = 220.0;
        p.filter_resonance = 0.95;
        let x = render(preset_sound(p.clone()), 36, 2.0, 1.9);
        assert!(x.iter().all(|s| s.is_finite()));
        assert!(
            x.iter().all(|s| s.abs() < 1.5),
            "resonance must not run away"
        );
        let late = rms(&x[(1.5 * SR) as usize..(1.9 * SR) as usize]);
        assert!(
            late > 0.001,
            "resonant low voice should keep ringing ({late})"
        );
        // Resonance actually changes the sustained tone vs the same voice flat.
        let mut flat = p;
        flat.filter_resonance = 0.0;
        let y = render(preset_sound(flat), 36, 2.0, 1.9);
        assert!(
            mean_abs_diff(
                &x[(1.5 * SR) as usize..(1.9 * SR) as usize],
                &y[(1.5 * SR) as usize..(1.9 * SR) as usize]
            ) > 1e-4,
            "resonance should be audible in the sustained tail"
        );
    }

    #[test]
    fn preset_long_release_tail_decays_and_ends() {
        let mut p = chip_preset();
        p.bit_depth = 24;
        p.sample_hold = 1;
        p.oscillators = vec![OscillatorSpec {
            waveform: Waveform::Sine,
            ..Default::default()
        }];
        p.envelope = Envelope {
            attack: 0.0,
            decay: 0.0,
            sustain: 0.8,
            release: 4.0,
        };
        // Keep the modulated path active so the tail exercises LFO motion too.
        p.lfo = Some(LfoSpec {
            rate_hz: 0.2,
            amplitude: 0.3,
            ..Default::default()
        });
        let x = render(preset_sound(p), 60, 6.0, 0.5);
        assert!(x.iter().all(|s| s.is_finite()));
        let mid = rms(&x[(2.0 * SR) as usize..(3.0 * SR) as usize]);
        let end = rms(&x[(5.5 * SR) as usize..(5.9 * SR) as usize]);
        assert!(mid > 1e-4, "long release should still ring at 2-3s ({mid})");
        assert!(
            end < mid * 0.2,
            "release tail must keep decaying ({end} vs {mid})"
        );
        assert!(
            x[(5.8 * SR) as usize..].iter().all(|s| s.abs() < 1e-3),
            "long release must still end"
        );
    }

    #[test]
    fn preset_modulation_survives_hostile_params() {
        // A hand-built PresetParams bypassing from_preset must still render
        // finite, bounded audio: every modulation input is defensively clamped.
        let mut p = PresetParams::from_preset(&chip_preset());
        p.pitch_env_amount = f32::NAN;
        p.pitch_env_decay = f32::INFINITY;
        p.filter_env_amount = f32::NEG_INFINITY;
        p.filter_env_decay = f32::NAN;
        p.cutoff = f32::NAN;
        p.resonance = f32::INFINITY;
        p.lfo_rate = f32::NAN;
        p.lfo_pitch = f32::INFINITY;
        p.lfo_filter = f32::NEG_INFINITY;
        p.lfo_amp = f32::NAN;
        p.attack = f32::NAN;
        p.sustain = f32::NAN;
        p.release = f32::INFINITY;
        for o in &mut p.oscs {
            o.level = f32::NAN;
            o.ratio = f32::INFINITY;
            o.width = f32::NAN;
            o.decay = f32::NAN;
        }
        let x = render(Sound::Preset(p), 60, 0.5, 0.4);
        assert!(
            x.iter().all(|s| s.is_finite()),
            "hostile preset params produced non-finite audio"
        );
        assert!(x.iter().all(|s| s.abs() < 1.5));
    }

    #[test]
    fn bundled_8bit_presets_all_take_the_legacy_path() {
        // Every preset shipped before the modulation controls existed must
        // keep the exact legacy signal path: no envelopes, no resonance, no
        // LFO and no per-oscillator decay.
        let pack = crate::packs::parse_pack(include_str!("../../packs/8-bit/pack.json")).unwrap();
        assert!(!pack.instruments.is_empty());
        for preset in &pack.instruments {
            let params = PresetParams::from_preset(preset);
            assert!(
                params.uses_legacy_preset_path(),
                "{} unexpectedly left the legacy preset path",
                preset.id
            );
        }
    }

    #[test]
    fn preset_modulation_is_block_size_invariant() {
        let mut preset = chip_preset();
        preset.bit_depth = 24;
        preset.sample_hold = 1;
        preset.pitch_envelope = Some(PitchEnvelope {
            amount_semitones: 7.0,
            decay: 0.2,
        });
        preset.filter_envelope = Some(FilterEnvelope {
            amount_octaves: -2.0,
            decay: 0.4,
        });
        preset.filter_resonance = 0.4;
        preset.lfo = Some(LfoSpec {
            rate_hz: 3.0,
            pitch_semitones: 0.5,
            filter_octaves: 0.5,
            amplitude: 0.25,
            ..Default::default()
        });
        preset.oscillators[0].decay = Some(0.3);
        let sound = preset_sound(preset);
        assert_eq!(
            render_chunked(sound, 60, 0.6, 0.5, 1),
            render_chunked(sound, 60, 0.6, 0.5, 257),
            "voice modulation must not depend on host block size"
        );
    }

    #[test]
    fn preset_bit_depth_and_hold_change_the_signal() {
        let mut clean = chip_preset();
        clean.bit_depth = 24;
        clean.sample_hold = 1;
        let crushed = chip_preset();
        let a = render(
            Sound::Preset(PresetParams::from_preset(&clean)),
            60,
            0.5,
            0.4,
        );
        let b = render(
            Sound::Preset(PresetParams::from_preset(&crushed)),
            60,
            0.5,
            0.4,
        );
        let diff: f32 = a.iter().zip(&b).map(|(x, y)| (x - y).abs()).sum::<f32>() / a.len() as f32;
        assert!(diff > 0.002, "bit crush/sample hold had no effect ({diff})");
        let noise = InstrumentPreset {
            oscillators: vec![OscillatorSpec {
                waveform: Waveform::Noise,
                ..Default::default()
            }],
            envelope: Envelope {
                attack: 0.0,
                decay: 0.15,
                sustain: 0.0,
                release: 0.05,
            },
            ..chip_preset()
        };
        let n1 = render(
            Sound::Preset(PresetParams::from_preset(&noise)),
            60,
            0.5,
            0.05,
        );
        let n2 = render(
            Sound::Preset(PresetParams::from_preset(&noise)),
            60,
            0.5,
            0.05,
        );
        assert_eq!(n1, n2, "seeded noise is deterministic");
        assert!(n1.iter().any(|s| s.abs() > 0.01));
        assert!(
            n1[(0.45 * SR) as usize..].iter().all(|s| s.abs() < 1e-4),
            "percussive preset decays"
        );
    }

    /// A two-piece kit: a deep sine kick at 36 and a bright noise hat at 42.
    fn kit_preset() -> InstrumentPreset {
        serde_json::from_str(
            r#"{
                "id": "test.kit", "name": "Kit",
                "kit": { "pieces": [
                    { "name": "Kick", "pitch": 36, "voice": {
                        "oscillators": [{"waveform":"sine","frequency_ratio":0.5}],
                        "envelope": {"attack":0.001,"decay":0.3,"sustain":0.0,"release":0.05},
                        "pitch_envelope": {"amount_semitones":-24.0,"decay":0.08},
                        "filter_cutoff": 4000.0 } },
                    { "name": "Hat", "pitch": 42, "voice": {
                        "oscillators": [{"waveform":"noise","level":0.8}],
                        "envelope": {"attack":0.001,"decay":0.06,"sustain":0.0,"release":0.02},
                        "filter_cutoff": 9000.0 } }
                ] } }"#,
        )
        .unwrap()
    }

    fn kit_sound() -> Sound {
        let preset = kit_preset();
        crate::packs::validate_preset(&preset).unwrap();
        Sound::Kit(KitParams::from_kit(&preset))
    }

    #[test]
    fn kit_params_piece_for_exact_nearest_and_empty() {
        let kit = KitParams::from_kit(&kit_preset());
        assert_eq!(kit.count, 2);
        // Sorted by pitch regardless of JSON order (36 then 42 here anyway).
        assert_eq!(kit.pieces[0].0, 36);
        assert_eq!(kit.pieces[1].0, 42);
        // Exact matches win; cutoff distinguishes the two voices.
        assert_eq!(kit.piece_for(36).cutoff, 4_000.0);
        assert_eq!(kit.piece_for(42).cutoff, 9_000.0);
        // Nearest below/above.
        assert_eq!(kit.piece_for(37).cutoff, 4_000.0);
        assert_eq!(kit.piece_for(41).cutoff, 9_000.0);
        // Tie (39 is 3 from each) resolves to the lower pitch.
        assert_eq!(kit.piece_for(39).cutoff, 4_000.0);
        // Out-of-range pitches clamp to the nearest end piece.
        assert_eq!(kit.piece_for(0).cutoff, 4_000.0);
        assert_eq!(kit.piece_for(127).cutoff, 9_000.0);
        // An empty kit is defensive SINE, never a panic.
        let empty = KitParams {
            pieces: kit.pieces,
            count: 0,
        };
        assert_eq!(empty.piece_for(60), PresetParams::SINE);
        // A kit-less preset yields an empty KitParams too.
        assert_eq!(KitParams::from_kit(&chip_preset()).count, 0);
    }

    #[test]
    fn kit_voices_pick_piece_by_pitch_and_sound_distinct() {
        let sound = kit_sound();
        assert!(sound.is_drums());
        assert!(!sound.is_mono());
        // 1.3 s so `features` can score the full envelope window; the
        // percussive pieces finish well inside it.
        let kick = render(sound, 36, 1.3, 0.4);
        let hat = render(sound, 42, 1.3, 0.4);
        assert!(kick.iter().all(|s| s.is_finite()));
        assert!(hat.iter().all(|s| s.is_finite()));
        assert!(
            rms(&kick) > 0.005 && rms(&hat) > 0.005,
            "kit pieces audible"
        );
        // Sine pitch-drop kick vs noise hat: clearly different timbres.
        assert!(
            distance(&features(&kick), &features(&hat)) > 0.25,
            "kit pieces should sound distinct"
        );
        // Percussive pieces decay to silence.
        assert!(kick[(1.0 * SR) as usize..]
            .iter()
            .chain(&hat[(1.0 * SR) as usize..])
            .all(|s| s.abs() < 1e-3));
    }

    #[test]
    fn kit_unmapped_pitch_plays_nearest_piece_bit_exact() {
        let sound = kit_sound();
        let kit = match sound {
            Sound::Kit(k) => k,
            _ => unreachable!(),
        };
        // 40 sits between Kick(36) and Hat(42) but nearer Kick; the rendered
        // signal must equal the Kick piece's PresetParams voice at pitch 40.
        for pitch in [36u8, 39, 40, 41, 42] {
            let piece = kit.piece_for(pitch);
            let via_kit = render(sound, pitch, 0.4, 0.3);
            let via_preset = render(Sound::Preset(piece), pitch, 0.4, 0.3);
            assert_eq!(
                via_kit, via_preset,
                "pitch {pitch} must play its nearest piece exactly"
            );
        }
        // And the mapped pitches really do pick different pieces.
        assert_ne!(kit.piece_for(36), kit.piece_for(42));
    }

    /// `render` with an explicit velocity (the shared helper fixes 0.8).
    fn render_vel(
        sound: Sound,
        pitch: u8,
        velocity: f32,
        seconds: f32,
        gate_seconds: f32,
    ) -> Vec<f32> {
        let mut v = Voice::new(SR);
        v.start(&NoteStart {
            sound,
            strip: 0,
            pitch,
            velocity,
            gate: (gate_seconds * SR) as u64,
            from_song: true,
            age: 1,
        });
        let n = (seconds * SR) as usize;
        let mut l = vec![0.0; n];
        let mut r = vec![0.0; n];
        for (cl, cr) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
            if v.active {
                v.render(cl, cr);
            }
        }
        l.iter().zip(&r).map(|(a, b)| (a + b) * 0.5).collect()
    }

    #[test]
    fn guitar_is_in_tune() {
        // Pitches in the guitar compass (E2 up) need no octave fold, so the
        // expected period is just SR / pitch_hz. Autocorrelation must peak
        // within ±10% of it, like the pluck test.
        for pitch in [40u8, 52, 64, 76, 83] {
            let x = render_vel(Sound::Builtin(Instrument::Guitar), pitch, 0.8, 0.6, 0.6);
            let seg = &x[(0.1 * SR) as usize..(0.5 * SR) as usize];
            let expected = SR / pitch_hz(pitch as f32);
            let lo = (expected * 0.9) as usize;
            let hi = (expected * 1.1) as usize;
            let best = (lo..=hi)
                .max_by(|&a, &b| {
                    let ca: f32 = seg.iter().zip(&seg[a..]).map(|(p, q)| p * q).sum();
                    let cb: f32 = seg.iter().zip(&seg[b..]).map(|(p, q)| p * q).sum();
                    ca.total_cmp(&cb)
                })
                .unwrap();
            assert!(
                (best as f32 - expected).abs() <= 1.5,
                "guitar {pitch}: period {best} vs {expected}"
            );
        }
    }

    #[test]
    fn guitar_note_decays_over_seconds() {
        // Held note: the KS loop rings for seconds then dies — the acoustic
        // ring-out, with low/mid strings sustaining longest.
        let x = render_vel(Sound::Builtin(Instrument::Guitar), 52, 0.9, 6.0, 6.0);
        assert!(x.iter().all(|s| s.is_finite()), "guitar not finite");
        let peak = x.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak > 0.05 && peak < 1.2, "guitar peak {peak}");
        let w = |a: f32, b: f32| rms(&x[(a * SR) as usize..(b * SR) as usize]);
        assert!(w(0.2, 0.8) > w(1.5, 2.5) * 1.5, "guitar not decaying");
        assert!(w(1.5, 2.5) > 1e-4, "guitar died too early");
        assert!(w(4.5, 5.5) < w(1.5, 2.5) * 0.4, "guitar never decays");
    }

    #[test]
    fn guitar_velocity_changes_level() {
        let soft = render_vel(Sound::Builtin(Instrument::Guitar), 60, 0.25, 0.6, 0.6);
        let hard = render_vel(Sound::Builtin(Instrument::Guitar), 60, 0.95, 0.6, 0.6);
        let seg = (0.4 * SR) as usize;
        let rs = rms(&soft[..seg]);
        let rh = rms(&hard[..seg]);
        assert!(rh > rs * 1.4, "guitar velocity level {rs} vs {rh}");
    }

    #[test]
    fn guitar_all_midi_pitches_stay_finite() {
        // Every MIDI pitch 0–127 must render bounded, non-silent audio — the
        // octave fold and clamped delay keep the extremes stable.
        for pitch in 0..=127u8 {
            let x = render_vel(Sound::Builtin(Instrument::Guitar), pitch, 0.7, 0.4, 0.4);
            assert!(x.iter().all(|s| s.is_finite()), "guitar {pitch} NaN/inf");
            let peak = x.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            assert!(peak > 0.005, "guitar {pitch} silent (peak {peak})");
            assert!(peak < 1.2, "guitar {pitch} too hot (peak {peak})");
        }
    }

    #[test]
    fn choke_and_release_end_voices() {
        let mut v = Voice::new(SR);
        v.start(&NoteStart {
            sound: Sound::Builtin(Instrument::Pad),
            strip: 0,
            pitch: 60,
            velocity: 1.0,
            gate: u64::MAX,
            from_song: false,
            age: 3,
        });
        let mut l = vec![0.0; 512];
        let mut r = vec![0.0; 512];
        v.render(&mut l, &mut r);
        assert!(v.active && v.held);
        v.choke(0.005);
        for _ in 0..2 {
            v.render(&mut l, &mut r);
        }
        assert!(!v.active);
    }

    // ------------------------------------------------------------------
    // Embedded sample clips
    // ------------------------------------------------------------------

    /// A clip-only preset: 0.25 s of 220 Hz sine at 22.05 kHz, root 60. The
    /// sustain-forever envelope mirrors the GUI import so the clip itself is
    /// the note's shape.
    fn sample_preset(looped: bool) -> InstrumentPreset {
        let rate = 22_050u32;
        let frames: Vec<f32> = (0..rate as usize / 4)
            .map(|i| (i as f32 * 220.0 * std::f32::consts::TAU / rate as f32).sin() * 0.8)
            .collect();
        InstrumentPreset {
            id: "test.sample".into(),
            name: "Sample".into(),
            oscillators: Vec::new(),
            envelope: Envelope {
                attack: 0.002,
                decay: 0.0,
                sustain: 1.0,
                release: 0.05,
            },
            bit_depth: 24,
            sample_hold: 1,
            filter_cutoff: 20_000.0,
            sample: Some(crate::packs::SampleSpec::from_frames(
                &frames, rate, 60, looped,
            )),
            ..chip_preset()
        }
    }

    fn sample_sound(looped: bool) -> Sound {
        preset_sound(sample_preset(looped))
    }

    #[test]
    fn sample_preset_params_and_bank_registration() {
        let p = PresetParams::from_preset(&sample_preset(false));
        assert_ne!(p.sample_slot, NO_SAMPLE_SLOT);
        assert_eq!(p.sample_rate, 22_050.0);
        assert_eq!(p.sample_root, 60);
        assert!(!p.sample_loop);
        // Rebuilding the params for the same clip is a bank hit, not a re-decode.
        let p2 = PresetParams::from_preset(&sample_preset(false));
        assert_eq!(p2.sample_slot, p.sample_slot, "same clip registered twice");
        // The clip itself is shared, decoded, bounded.
        let clip = sample_clip(p.sample_slot).unwrap();
        assert_eq!(clip.len(), 22_050 / 4);
        assert!(clip.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
        // A preset without a clip stays key-less.
        assert_eq!(
            PresetParams::from_preset(&chip_preset()).sample_slot,
            NO_SAMPLE_SLOT
        );
        // A hand-built key that misses the bank resolves to None, not a panic.
        assert!(sample_clip(0xDEAD_BEEF).is_none());
    }

    #[test]
    fn sample_voice_is_finite_audible_and_ends() {
        let x = render(sample_sound(false), 60, 2.0, 1.5);
        assert!(x.iter().all(|s| s.is_finite()), "sample not finite");
        let peak = x.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak > 0.01, "sample silent (peak {peak})");
        assert!(peak < 1.5, "sample too hot (peak {peak})");
        // One-shot: the 0.25 s clip plus the short release leave silence well
        // before the (still-held) note gate would have ended.
        assert!(x[(1.0 * SR) as usize..].iter().all(|s| s.abs() < 1e-3));
    }

    #[test]
    fn sample_voice_transposes_by_semitone() {
        // Root 60 at 0.5x clip rate ≈ 110 Hz out; an octave up doubles it.
        let lo = zc_rate(
            &render(sample_sound(false), 60, 0.2, 0.18)[(0.01 * SR) as usize..(0.1 * SR) as usize],
        );
        let hi = zc_rate(
            &render(sample_sound(false), 72, 0.2, 0.18)[(0.01 * SR) as usize..(0.1 * SR) as usize],
        );
        assert!(
            (hi / lo - 2.0).abs() < 0.12,
            "octave up should double the rate ({lo} vs {hi})"
        );
        // A tritone (ratio sqrt(2) ≈ 1.414) sits in between.
        let mid = zc_rate(
            &render(sample_sound(false), 66, 0.2, 0.18)[(0.01 * SR) as usize..(0.1 * SR) as usize],
        );
        assert!(
            mid > lo * 1.3 && mid < hi * 0.9,
            "tritone rate {mid} vs {lo}..{hi}"
        );
    }

    #[test]
    fn sample_voice_loops_while_held_and_releases() {
        let x = render(sample_sound(true), 60, 2.0, 1.2);
        assert!(x.iter().all(|s| s.is_finite()));
        // Looping keeps the voice sounding well past the 0.25 s clip length.
        assert!(rms(&x[(0.5 * SR) as usize..(0.9 * SR) as usize]) > 0.01);
        // After the gate ends the amp release fades it out.
        assert!(x[(1.8 * SR) as usize..].iter().all(|s| s.abs() < 1e-3));
    }

    #[test]
    fn sample_voice_missing_clip_falls_back_to_oscillators() {
        // A preset whose clip never registered (undecodable payload, or a
        // hand-built key that misses the bank) keeps the armed oscillator
        // path — bounded, finite audio, never a panic.
        let mut params = PresetParams::from_preset(&sample_preset(false));
        params.sample_slot = 0xDEAD_BEEF;
        let x = render(Sound::Preset(params), 60, 0.4, 0.3);
        assert!(x.iter().all(|s| s.is_finite() && s.abs() < 1.5));
        // And a clip-free preset ignores the sample fields entirely.
        let x = render(preset_sound(chip_preset()), 60, 0.4, 0.3);
        assert!(x.iter().all(|s| s.is_finite() && s.abs() < 1.5));
    }

    #[test]
    fn sample_kit_piece_plays_the_clip() {
        // A kit piece whose voice is a bare clip: note-on picks the piece,
        // start_preset upgrades it to Kind::Sample.
        let mut kit = kit_preset();
        let voice = &mut kit.kit.as_mut().unwrap().pieces[0].voice;
        voice.oscillators.clear();
        voice.sample = Some(crate::packs::SampleSpec::from_frames(
            &(0..4_410)
                .map(|i| (i as f32 * 220.0 * std::f32::consts::TAU / 44_100.0).sin() * 0.8)
                .collect::<Vec<_>>(),
            44_100,
            60,
            false,
        ));
        crate::packs::validate_preset(&kit).unwrap();
        let x = render(Sound::Kit(KitParams::from_kit(&kit)), 36, 0.5, 0.4);
        assert!(x.iter().all(|s| s.is_finite()));
        assert!(rms(&x[(0.02 * SR) as usize..(0.08 * SR) as usize]) > 0.01);
    }

    /// Renders one note at `velocity`, returning the separate L/R channels.
    fn render_stereo(
        sound: Sound,
        pitch: u8,
        velocity: f32,
        seconds: f32,
        gate_seconds: f32,
    ) -> (Vec<f32>, Vec<f32>) {
        let mut v = Voice::new(SR);
        v.start(&NoteStart {
            sound,
            strip: 0,
            pitch,
            velocity,
            gate: (gate_seconds * SR) as u64,
            from_song: true,
            age: 1,
        });
        let n = (seconds * SR) as usize;
        let mut l = vec![0.0; n];
        let mut r = vec![0.0; n];
        for (cl, cr) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
            if v.active {
                v.render(cl, cr);
            }
        }
        (l, r)
    }

    /// Energy above ~4 kHz relative to total energy: a one-pole high-pass
    /// output's RMS over the signal's RMS (level-independent).
    fn high_band_ratio(x: &[f32]) -> f32 {
        let a = (-std::f32::consts::TAU * 4_000.0 / SR).exp();
        let (mut lp, mut hi) = (0.0f32, Vec::with_capacity(x.len()));
        for &s in x {
            lp = s + (lp - s) * a;
            hi.push(s - lp);
        }
        rms(&hi) / rms(x).max(1e-9)
    }

    fn filtered_saw() -> InstrumentPreset {
        InstrumentPreset {
            id: "test.dark-saw".into(),
            name: "Dark Saw".into(),
            oscillators: vec![OscillatorSpec {
                waveform: Waveform::Saw,
                ..Default::default()
            }],
            envelope: Envelope {
                attack: 0.002,
                decay: 0.2,
                sustain: 0.8,
                release: 0.1,
            },
            filter_cutoff: 1_500.0,
            bit_depth: 24,
            sample_hold: 1,
            ..chip_preset()
        }
    }

    #[test]
    fn velocity_cutoff_pivots_at_point_eight() {
        assert_eq!(velocity_cutoff(1_500.0, 2.0, 0.8), 1_500.0);
        assert!((velocity_cutoff(1_500.0, 2.0, 0.3) - 750.0).abs() < 0.5);
        assert!(velocity_cutoff(1_500.0, 2.0, 1.0) > 1_500.0 * 1.3);
        // Off, or a fully open filter: never scaled.
        assert_eq!(velocity_cutoff(1_500.0, 0.0, 0.1), 1_500.0);
        assert_eq!(velocity_cutoff(20_000.0, 6.0, 0.0), 20_000.0);
        assert_eq!(velocity_cutoff(19_000.0, 6.0, 1.0), 20_000.0);
        assert!(velocity_cutoff(f32::NAN, f32::NAN, f32::NAN).is_finite());
    }

    #[test]
    fn higher_velocity_opens_the_preset_filter() {
        let sound = preset_sound(filtered_saw());
        let soft = render_stereo(sound, 48, 0.4, 0.6, 0.5).0;
        let hard = render_stereo(sound, 48, 1.0, 0.6, 0.5).0;
        let range = (0.05 * SR) as usize..(0.45 * SR) as usize;
        let (soft_hi, hard_hi) = (
            high_band_ratio(&soft[range.clone()]),
            high_band_ratio(&hard[range.clone()]),
        );
        assert!(
            hard_hi > soft_hi * 1.25,
            "velocity 1.0 should be brighter: high band {hard_hi} vs {soft_hi}"
        );
        // The modulated path (filter envelope armed) follows velocity too.
        let mut modulated = filtered_saw();
        modulated.filter_envelope = Some(FilterEnvelope {
            amount_octaves: 1.0,
            decay: 0.3,
        });
        let sound = preset_sound(modulated);
        let soft = render_stereo(sound, 48, 0.4, 0.6, 0.5).0;
        let hard = render_stereo(sound, 48, 1.0, 0.6, 0.5).0;
        assert!(high_band_ratio(&hard[range.clone()]) > high_band_ratio(&soft[range]) * 1.25);
    }

    #[test]
    fn velocity_to_cutoff_zero_keeps_timbre_fixed() {
        let mut fixed = filtered_saw();
        fixed.velocity_to_cutoff = 0.0;
        let sound = preset_sound(fixed);
        let soft = render_stereo(sound, 48, 0.4, 0.4, 0.3).0;
        let hard = render_stereo(sound, 48, 1.0, 0.4, 0.3).0;
        // Only the level differs: normalised waveforms match.
        let (gs, gh) = (velocity_gain(0.4), velocity_gain(1.0));
        let err = soft
            .iter()
            .zip(&hard)
            .map(|(a, b)| (a / gs - b / gh).abs())
            .fold(0.0f32, f32::max);
        assert!(err < 1e-4, "timbre changed with velocity: {err}");
        // And velocity 0.8 renders exactly the unscaled cutoff.
        let mut v = Voice::new(SR);
        v.start(&NoteStart {
            sound: preset_sound(filtered_saw()),
            strip: 0,
            pitch: 48,
            velocity: 0.8,
            gate: 100,
            from_song: true,
            age: 1,
        });
        assert_eq!(v.cutoff, 1_500.0);
    }
}
