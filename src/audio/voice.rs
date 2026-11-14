//! Synthesizer voices. A [`Voice`] is a preallocated slot that can play any built-in
//! instrument or a data-driven pack preset; starting, releasing and rendering a voice never
//! allocates.

use super::dsp::{
    advance, flush, poly_blep, saw, sine, soft_clip, square, t60_coef, time_coef, OnePole, Rng,
    Svf, SvfCoefs,
};
use crate::model::{map_drum_pitch, pitch_hz, Instrument};
use crate::packs::{InstrumentPreset, Waveform};

pub const MAX_PRESET_OSCILLATORS: usize = 4;
/// Lowest pitch the plucked-string model tunes to; lower notes are folded up an octave.
const PLUCK_MIN_HZ: f32 = 30.0;
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
    pub cutoff: f32,
    pub bits: u8,
    pub hold: u32,
    pub gain: f32,
    pub mono: bool,
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
        }; MAX_PRESET_OSCILLATORS],
        count: 1,
        attack: 0.01,
        decay: 0.2,
        sustain: 0.7,
        release: 0.3,
        cutoff: 20_000.0,
        bits: 24,
        hold: 1,
        gain: 0.9,
        mono: false,
    };

    /// Converts a preset, clamping every value into its documented range so that even an
    /// unvalidated preset can never produce non-finite audio.
    pub fn from_preset(p: &InstrumentPreset) -> Self {
        let mut oscs = [OscParams {
            wave: Waveform::Sine,
            level: 0.0,
            ratio: 1.0,
            width: 0.5,
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
            };
            count += 1;
        }
        if count == 0 {
            oscs[0].level = 1.0;
            count = 1;
        }
        PresetParams {
            oscs,
            count,
            attack: finite(p.envelope.attack, 0.01).clamp(0.0, 5.0),
            decay: finite(p.envelope.decay, 0.2).clamp(0.0, 10.0),
            sustain: finite(p.envelope.sustain, 0.7).clamp(0.0, 1.0),
            release: finite(p.envelope.release, 0.3).clamp(0.005, 20.0),
            cutoff: finite(p.filter_cutoff, 20_000.0).clamp(100.0, 20_000.0),
            bits: p.bit_depth.clamp(4, 24),
            hold: p.sample_hold.clamp(1, 64),
            gain: finite(p.gain, 0.9).clamp(0.0, 1.0),
            mono: p.monophonic,
        }
    }
}

/// What a track sounds like: one of the six built-in algorithms or a pack preset.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sound {
    Builtin(Instrument),
    Preset(PresetParams),
}

impl Sound {
    pub fn is_mono(&self) -> bool {
        match self {
            Sound::Builtin(i) => i.is_monophonic(),
            Sound::Preset(p) => p.mono,
        }
    }

    pub fn is_drums(&self) -> bool {
        matches!(self, Sound::Builtin(Instrument::Drums))
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
    Pad,
    Lead,
    Preset,
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
    freq: f32,
    target_freq: f32,
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
    hold_left: u32,
    hold_value: f32,
    noise_value: [f32; MAX_PRESET_OSCILLATORS],
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
}

fn velocity_gain(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    0.12 + 0.88 * v * v
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
            hold_left: 0,
            hold_value: 0.0,
            noise_value: [0.0; MAX_PRESET_OSCILLATORS],
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
        };
        v.set_sample_rate(sample_rate);
        v
    }

    /// Reallocates the string buffer; call only off the audio thread (or while silent).
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sr = sample_rate;
        self.inv_sr = 1.0 / sample_rate;
        self.ks = vec![0.0; (sample_rate / PLUCK_MIN_HZ) as usize + 4];
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
                self.glide = time_coef(0.045, sr);
            }
            Sound::Preset(p) => {
                self.kind = Kind::Preset;
                self.preset = p;
                self.amp = Adsr::new(
                    sr,
                    p.attack.max(0.001),
                    p.decay.max(0.001) / 4.0,
                    p.sustain,
                    p.release / 5.0,
                );
                self.fc = SvfCoefs::new(p.cutoff, 0.707, sr);
                self.hold_left = 0;
                self.hold_value = 0.0;
                let rng = &mut self.rng;
                self.noise_value.iter_mut().for_each(|x| *x = rng.noise());
            }
        }
    }

    /// Mono legato: change pitch without restarting the envelopes.
    pub fn legato(&mut self, pitch: u8, velocity: f32, gate: u64, glide: bool) {
        self.pitch = pitch;
        self.target_freq = pitch_hz(pitch as f32);
        if !glide || self.glide == 0.0 {
            self.freq = self.target_freq;
        }
        self.vel = velocity.clamp(0.0, 1.0);
        self.gain = velocity_gain(self.vel);
        self.gate = gate;
        self.held = true;
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
            let (a, b) = match self.kind {
                Kind::Drum => self.drum_sample(),
                Kind::Piano => self.piano_sample(),
                Kind::Bass => self.bass_sample(),
                Kind::Pluck => self.pluck_sample(),
                Kind::Pad => self.pad_sample(),
                Kind::Lead => self.lead_sample(),
                Kind::Preset => self.preset_sample(),
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
        self.freq = self.target_freq + (self.freq - self.target_freq) * self.glide;
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
        let raw = sum / total.max(1.0) * a;
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
    use crate::packs::{Envelope, OscillatorSpec};

    const SR: f32 = 44_100.0;

    fn render(sound: Sound, pitch: u8, seconds: f32, gate_seconds: f32) -> Vec<f32> {
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
        for (cl, cr) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
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
                },
                OscillatorSpec {
                    waveform: Waveform::Triangle,
                    level: 0.4,
                    frequency_ratio: 0.5,
                    detune_cents: 3.0,
                    pulse_width: 0.5,
                },
            ],
            envelope: Envelope {
                attack: 0.002,
                decay: 0.2,
                sustain: 0.5,
                release: 0.1,
            },
            filter_cutoff: 8000.0,
            bit_depth: 6,
            sample_hold: 3,
            gain: 0.9,
            monophonic: false,
        }
    }

    #[test]
    fn every_sound_is_finite_audible_and_ends() {
        let mut sounds: Vec<Sound> = Instrument::ALL.iter().map(|i| Sound::Builtin(*i)).collect();
        sounds.push(Sound::Preset(PresetParams::from_preset(&chip_preset())));
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
}
