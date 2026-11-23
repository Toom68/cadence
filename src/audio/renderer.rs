//! The song renderer shared by real-time playback and offline export: sample-accurate
//! transport with swing, voice allocation, per-track drive/filter/fader/pan strips, shared
//! reverb and tempo-synced delay sends, master gain, DC blocking and a look-ahead limiter.
//!
//! After construction nothing here allocates; commands only swap pointers.

use super::dsp::{
    soft_clip, time_coef, DcBlock, Limiter, PingPongDelay, Reverb, Smoother, Svf, SvfCoefs,
};
use super::font::{FontBank, FontSynth};
use super::meter::BlockPeaks;
use super::song::{BusParams, SongData, TrackParams};
use super::voice::{DrumKind, NoteStart, Sound, Voice, LEAD_GLIDE_SECONDS};
use crate::font::FontLibrary;
use crate::model::{
    Automation, Instrument, MAX_AUTOMATION_GAIN, MAX_CUTOFF_HZ, MAX_GLIDE_SECONDS, MAX_TRACKS,
    NUM_BUSES, STEPS_PER_BAR, STEPS_PER_BEAT,
};
use std::sync::Arc;

/// Largest internal processing block in frames.
pub const MAX_BLOCK: usize = 256;
/// Voice pool size; the oldest/quietest voice is stolen beyond this.
pub const MAX_VOICES: usize = 96;
/// Strip used for auditioning notes from the editor.
pub const PREVIEW_STRIP: usize = MAX_TRACKS;
pub(crate) const STRIPS: usize = MAX_TRACKS + 1;
/// Transport timing tolerance in steps.
const EPS: f64 = 1e-9;
/// Overall mix trim. At the default master volume (0.8) this is ~unity, so a full
/// arrangement peaks around -6 dBFS: loud, with the limiter kept as a safety net.
const MASTER_TRIM: f32 = 1.6;
const REVERB_SEND: f32 = 0.75;
const DELAY_SEND: f32 = 0.7;
/// Delay echo spacing: a dotted eighth.
const DELAY_STEPS: f64 = 3.0;
const PANIC_SECONDS: f32 = 0.012;

/// Messages from the control thread. `Box` payloads are built off the audio thread.
// `Preview` carries a `TrackParams` whose `Sound::Kit` variant is ~5 KB; the
// command is built and sent once per user action, never copied per note.
#[allow(clippy::large_enum_variant)]
pub enum Command {
    Load(Box<SongData>),
    Play {
        song: Box<SongData>,
        start_step: f64,
        looping: bool,
        /// Repeat bounds in steps while `looping` (`None` wraps at song end).
        loop_region: Option<(f64, f64)>,
    },
    Pause,
    Stop,
    /// Moves the repeat bounds without restarting the transport (`None` wraps
    /// at the song end). Only matters while `looping` is on.
    SetLoopRegion(Option<(f64, f64)>),
    Preview {
        params: TrackParams,
        pitch: u8,
        velocity: f32,
        seconds: f64,
    },
    Metronome(bool),
}

/// One mixer channel.
#[derive(Clone, Copy, Debug)]
struct Strip {
    gain_l: Smoother,
    gain_r: Smoother,
    reverb: Smoother,
    delay: Smoother,
    drive: Smoother,
    cutoff: f32,
    cutoff_target: f32,
    coefs: SvfCoefs,
    filt_l: Svf,
    filt_r: Svf,
    /// Still producing output (tails through the filter).
    live: bool,
    /// Where the strip's post-fader output sums: 0 = straight into the
    /// master mix, `1..=NUM_BUSES` = into that bus's input buffer.
    bus: u8,
}

fn pan_gains(pan: f32) -> (f32, f32) {
    let theta = (pan.clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
    let s = std::f32::consts::SQRT_2;
    ((s * theta.cos()).min(1.0), (s * theta.sin()).min(1.0))
}

impl Strip {
    fn new(sr: f32) -> Self {
        Strip {
            gain_l: Smoother::new(0.0, 0.012, sr),
            gain_r: Smoother::new(0.0, 0.012, sr),
            reverb: Smoother::new(0.0, 0.02, sr),
            delay: Smoother::new(0.0, 0.02, sr),
            drive: Smoother::new(0.0, 0.03, sr),
            cutoff: MAX_CUTOFF_HZ,
            cutoff_target: MAX_CUTOFF_HZ,
            coefs: SvfCoefs::new(MAX_CUTOFF_HZ, 0.707, sr),
            filt_l: Svf::default(),
            filt_r: Svf::default(),
            live: false,
            bus: 0,
        }
    }

    fn set(&mut self, p: &TrackParams, sr: f32, snap: bool) {
        let g = if p.audible { p.gain } else { 0.0 };
        let (pl, pr) = pan_gains(p.pan);
        let targets = [
            g * pl,
            g * pr,
            p.reverb * REVERB_SEND,
            p.delay * DELAY_SEND,
            p.drive,
        ];
        for (s, t) in [
            &mut self.gain_l,
            &mut self.gain_r,
            &mut self.reverb,
            &mut self.delay,
            &mut self.drive,
        ]
        .into_iter()
        .zip(targets)
        {
            if snap {
                s.snap(t);
            } else {
                s.target = t;
            }
        }
        self.cutoff_target = p.cutoff;
        if snap {
            self.cutoff = p.cutoff;
            self.coefs = SvfCoefs::new(self.cutoff, 0.707, sr);
        }
        // Routing switches instantly; the `min` keeps unvalidated params
        // from indexing past the bus buffers.
        self.bus = p.bus.min(NUM_BUSES as u8);
    }

    /// Bus-level trim: the same fader/pan stage a track strip runs, but the
    /// group itself adds no drive, filter or sends — those stay on the
    /// track strips, so a muted bus also cannot leak into the shared
    /// reverb/delay (its tracks gate their sends through
    /// [`TrackParams::audible`]).
    fn set_bus(&mut self, p: &BusParams, sr: f32, snap: bool) {
        let g = if p.audible { p.gain } else { 0.0 };
        let (pl, pr) = pan_gains(p.pan);
        let targets = [g * pl, g * pr, 0.0, 0.0, 0.0];
        for (s, t) in [
            &mut self.gain_l,
            &mut self.gain_r,
            &mut self.reverb,
            &mut self.delay,
            &mut self.drive,
        ]
        .into_iter()
        .zip(targets)
        {
            if snap {
                s.snap(t);
            } else {
                s.target = t;
            }
        }
        self.cutoff_target = MAX_CUTOFF_HZ;
        if snap {
            self.cutoff = MAX_CUTOFF_HZ;
            self.coefs = SvfCoefs::new(self.cutoff, 0.707, sr);
        }
        self.bus = 0;
    }

    /// Retargets only the fader/pan smoothers to the automated mix at `step`
    /// — the volume lane multiplies `p.gain`, the pan lane offsets `p.pan` —
    /// leaving every other parameter wherever `set` put it. Called once per
    /// render block for tracks that carry automation; the ~12 ms ramp turns
    /// the piecewise-linear curve into click-free gain movement, and
    /// non-finite lane data (only reachable unvalidated) degrades to neutral
    /// rather than poisoning the smoothers.
    fn set_automation(&mut self, p: &TrackParams, auto: &Automation, step: f64) {
        let mul = auto.volume_at(step);
        let mul = if mul.is_finite() {
            mul.clamp(0.0, MAX_AUTOMATION_GAIN)
        } else {
            1.0
        };
        let off = auto.pan_at(step);
        let off = if off.is_finite() { off } else { 0.0 };
        let g = if p.audible { p.gain * mul } else { 0.0 };
        let (pl, pr) = pan_gains(p.pan + off);
        self.gain_l.target = g * pl;
        self.gain_r.target = g * pr;
    }

    fn silence(&mut self) {
        for s in [
            &mut self.gain_l,
            &mut self.gain_r,
            &mut self.reverb,
            &mut self.delay,
        ] {
            s.target = 0.0;
        }
    }

    fn reset(&mut self) {
        self.filt_l.reset();
        self.filt_r.reset();
        self.live = false;
        for s in [
            &mut self.gain_l,
            &mut self.gain_r,
            &mut self.reverb,
            &mut self.delay,
            &mut self.drive,
        ] {
            s.snap(s.target);
        }
        self.cutoff = self.cutoff_target;
    }

    /// Settles parameters while the strip is idle so it resumes at its targets.
    fn idle(&mut self, sr: f32) {
        self.reset();
        self.coefs = SvfCoefs::new(self.cutoff, 0.707, sr);
    }

    #[allow(clippy::too_many_arguments)]
    fn process<const METER: bool>(
        &mut self,
        sr: f32,
        l: &[f32],
        r: &[f32],
        out: (&mut [f32], &mut [f32]),
        rev: (&mut [f32], &mut [f32]),
        dly: (&mut [f32], &mut [f32]),
    ) -> f32 {
        let n = l.len();
        let drive = self.drive.skip(n);
        let (pre, makeup) = if drive > 1e-4 {
            let pre = 1.0 + 11.0 * drive;
            (pre, 0.25 / soft_clip(0.25 * pre))
        } else {
            (1.0, 1.0)
        };
        if (self.cutoff - self.cutoff_target).abs() > 0.5 {
            // Glide in the log domain over ~30 ms.
            let k = 1.0 - time_coef(0.03, sr).powi(n as i32);
            self.cutoff =
                (self.cutoff.ln() + (self.cutoff_target.ln() - self.cutoff.ln()) * k).exp();
            self.coefs = SvfCoefs::new(self.cutoff, 0.707, sr);
        }
        let (out_l, out_r) = out;
        let (rev_l, rev_r) = rev;
        let (dly_l, dly_r) = dly;
        let mut peak = 0.0f32;
        let mut output_peak = 0.0f32;
        for (i, (&x0, &y0)) in l.iter().zip(r.iter()).enumerate() {
            let (mut x, mut y) = (x0, y0);
            if pre > 1.0 {
                x = soft_clip(x * pre) * makeup;
                y = soft_clip(y * pre) * makeup;
            }
            x = self.filt_l.low(&self.coefs, x);
            y = self.filt_r.low(&self.coefs, y);
            let ol = x * self.gain_l.next();
            let or = y * self.gain_r.next();
            let rv = self.reverb.next();
            let dv = self.delay.next();
            out_l[i] += ol;
            out_r[i] += or;
            rev_l[i] += ol * rv;
            rev_r[i] += or * rv;
            dly_l[i] += ol * dv;
            dly_r[i] += or * dv;
            peak = peak.max(x.abs()).max(y.abs());
            if METER {
                output_peak = output_peak.max(ol.abs()).max(or.abs());
            }
        }
        self.live = peak > 1e-7 || !self.filt_l.is_quiet() || !self.filt_r.is_quiet();
        output_peak
    }
}

/// Metronome click generator.
#[derive(Clone, Copy, Debug)]
struct Click {
    phase: f32,
    inc: f32,
    env: f32,
    coef: f32,
}

impl Click {
    fn new(sr: f32) -> Self {
        Click {
            phase: 0.0,
            inc: 0.0,
            env: 0.0,
            coef: time_coef(0.012, sr),
        }
    }

    fn trigger(&mut self, accent: bool, sr: f32) {
        self.phase = 0.0;
        self.inc = if accent { 1760.0 } else { 1320.0 } / sr;
        self.env = if accent { 0.32 } else { 0.22 };
    }

    fn render(&mut self, out: &mut [f32]) {
        if self.env < 1e-5 {
            return;
        }
        for s in out {
            *s += super::dsp::sine(self.phase) * self.env;
            super::dsp::advance(&mut self.phase, self.inc);
            self.env *= self.coef;
        }
    }
}

pub struct Renderer {
    sr: f32,
    song: Box<SongData>,
    playing: bool,
    looping: bool,
    /// Repeat bounds in steps while `looping`; `None` wraps at the song end.
    /// Session state only — never part of a song snapshot.
    loop_region: Option<(f64, f64)>,
    /// Transport position in swung steps.
    clock: f64,
    next_event: usize,
    next_beat: f64,
    metronome: bool,
    voices: Vec<Voice>,
    age: u64,
    strips: Vec<Strip>,
    scratch: Vec<Strip>,
    strip_l: Vec<f32>,
    strip_r: Vec<f32>,
    strip_used: [bool; STRIPS],
    /// The fixed subgroup strips: track strips with `bus > 0` sum into
    /// `bus_l`/`bus_r`, then these run fader/pan into `mix_*`.
    bus_strips: [Strip; NUM_BUSES],
    bus_l: Vec<f32>,
    bus_r: Vec<f32>,
    bus_used: [bool; NUM_BUSES],
    mix_l: Vec<f32>,
    mix_r: Vec<f32>,
    rev_l: Vec<f32>,
    rev_r: Vec<f32>,
    dly_l: Vec<f32>,
    dly_r: Vec<f32>,
    click_buf: Vec<f32>,
    delay: PingPongDelay,
    reverb: Reverb,
    limiter: Limiter,
    dc_l: DcBlock,
    dc_r: DcBlock,
    master: Smoother,
    click: Click,
    panic_left: usize,
    panic_len: usize,
    peak_l: f32,
    peak_r: f32,
    /// SoundFont catalog + per-track synthesizers for `Sound::Font` notes.
    font_bank: FontBank,
    /// Stereo scratch for rustysynth output (its `render` needs separate L/R).
    font_scratch_l: Vec<f32>,
    font_scratch_r: Vec<f32>,
}

impl Renderer {
    pub fn new(sample_rate: f32) -> Self {
        let sr = sample_rate.clamp(8_000.0, 384_000.0);
        let song = Box::new(SongData::empty());
        Renderer {
            sr,
            master: Smoother::new(Self::master_gain(&song), 0.02, sr),
            song,
            playing: false,
            looping: false,
            loop_region: None,
            clock: 0.0,
            next_event: 0,
            next_beat: 0.0,
            metronome: false,
            voices: (0..MAX_VOICES).map(|_| Voice::new(sr)).collect(),
            age: 0,
            strips: vec![Strip::new(sr); STRIPS],
            scratch: vec![Strip::new(sr); STRIPS],
            strip_l: vec![0.0; STRIPS * MAX_BLOCK],
            strip_r: vec![0.0; STRIPS * MAX_BLOCK],
            strip_used: [false; STRIPS],
            bus_strips: [Strip::new(sr); NUM_BUSES],
            bus_l: vec![0.0; NUM_BUSES * MAX_BLOCK],
            bus_r: vec![0.0; NUM_BUSES * MAX_BLOCK],
            bus_used: [false; NUM_BUSES],
            mix_l: vec![0.0; MAX_BLOCK],
            mix_r: vec![0.0; MAX_BLOCK],
            rev_l: vec![0.0; MAX_BLOCK],
            rev_r: vec![0.0; MAX_BLOCK],
            dly_l: vec![0.0; MAX_BLOCK],
            dly_r: vec![0.0; MAX_BLOCK],
            click_buf: vec![0.0; MAX_BLOCK],
            delay: PingPongDelay::new(sr),
            reverb: Reverb::new(sr),
            limiter: Limiter::new(sr),
            dc_l: DcBlock::new(sr),
            dc_r: DcBlock::new(sr),
            click: Click::new(sr),
            panic_left: 0,
            panic_len: ((PANIC_SECONDS * sr) as usize).max(1),
            peak_l: 0.0,
            peak_r: 0.0,
            font_bank: FontBank::new(sr, Arc::new(FontLibrary::empty())),
            font_scratch_l: vec![0.0; MAX_BLOCK],
            font_scratch_r: vec![0.0; MAX_BLOCK],
        }
    }

    fn master_gain(song: &SongData) -> f32 {
        let m = song.master_volume.clamp(0.0, 1.0);
        m * m * MASTER_TRIM
    }

    /// Changes the sample rate, reallocating delay lines. Never call on the audio thread.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        if (sample_rate - self.sr).abs() < 0.5 {
            return;
        }
        let mut fresh = Renderer::new(sample_rate);
        // Synthesizers are sample-rate specific: keep the catalog, drop the synths.
        std::mem::swap(&mut fresh.song, &mut self.song);
        let library = Arc::clone(self.font_bank.library());
        fresh.font_bank = if self.font_bank.is_prepared() {
            FontBank::prepare(
                fresh.sr,
                library,
                FontBank::track_bindings(&fresh.song.tracks),
            )
        } else {
            FontBank::new(fresh.sr, library)
        };
        fresh.metronome = self.metronome;
        fresh.loop_region = self.loop_region;
        fresh.master.snap(Self::master_gain(&fresh.song));
        for (strip, p) in fresh.strips.iter_mut().zip(fresh.song.tracks.iter()) {
            strip.set(p, fresh.sr, true);
        }
        for (strip, p) in fresh.bus_strips.iter_mut().zip(fresh.song.buses.iter()) {
            strip.set_bus(p, fresh.sr, true);
        }
        *self = fresh;
    }

    /// Installs the SoundFont catalog for `Sound::Font` tracks. Call before
    /// `load`/`play` in offline renders.
    pub fn set_fonts(&mut self, library: Arc<FontLibrary>) {
        self.font_bank.set_library(library);
    }

    pub fn swap_prepared_fonts(&mut self, bank: FontBank) -> FontBank {
        self.font_bank.swap_prepared(bank)
    }

    pub fn swap_prepared_preview(&mut self, synth: Box<FontSynth>) -> Option<Box<FontSynth>> {
        self.font_bank.swap_preview(synth)
    }

    pub fn sample_rate(&self) -> f32 {
        self.sr
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }

    /// Transport position in (swung) steps.
    pub fn position(&self) -> f64 {
        self.clock
    }

    pub fn active_voices(&self) -> usize {
        self.voices.iter().filter(|v| v.active).count() + self.font_bank.active_notes()
    }

    /// Peak of the most recent rendered block, per channel.
    pub fn peaks(&self) -> (f32, f32) {
        (self.peak_l, self.peak_r)
    }

    pub fn tracks(&self) -> &[TrackParams] {
        &self.song.tracks
    }

    pub fn latency(&self) -> usize {
        self.limiter.latency()
    }

    /// No voices and no effect tails left.
    pub fn is_quiet(&self) -> bool {
        self.active_voices() == 0
            && self.delay.is_quiet()
            && self.reverb.is_quiet()
            && self.strips.iter().all(|s| !s.live)
            && self.bus_strips.iter().all(|s| !s.live)
    }

    fn samples_per_step(&self) -> f64 {
        self.sr as f64 * self.song.seconds_per_step()
    }

    /// Applies a control command; returns a replaced song for disposal off the audio thread.
    pub fn apply(&mut self, cmd: Command) -> Option<Box<SongData>> {
        match cmd {
            Command::Load(song) => Some(self.load(song)),
            Command::Play {
                song,
                start_step,
                looping,
                loop_region,
            } => {
                let old = self.load(song);
                self.set_loop_region(loop_region);
                self.play(start_step, looping);
                Some(old)
            }
            Command::Pause => {
                self.pause();
                None
            }
            Command::Stop => {
                self.stop();
                None
            }
            Command::SetLoopRegion(region) => {
                self.set_loop_region(region);
                None
            }
            Command::Preview {
                params,
                pitch,
                velocity,
                seconds,
            } => {
                self.preview(params, pitch, velocity, seconds);
                None
            }
            Command::Metronome(on) => {
                self.metronome = on;
                None
            }
        }
    }

    /// Installs a new song snapshot, keeping sounding notes attached to their tracks even if
    /// tracks were reordered; voices of deleted tracks are cut.
    pub fn load(&mut self, song: Box<SongData>) -> Box<SongData> {
        let same_layout = self.song.tracks.len() == song.tracks.len()
            && self
                .song
                .tracks
                .iter()
                .zip(&song.tracks)
                .all(|(a, b)| a.id == b.id);
        let was_known = if same_layout {
            [true; MAX_TRACKS]
        } else {
            self.remap_strips(&song.tracks)
        };
        for (j, p) in song.tracks.iter().enumerate() {
            self.strips[j].set(p, self.sr, !was_known[j]);
        }
        // Buses occupy fixed slots. Snap while idle, like the master trim.
        let snap = !self.playing && self.is_quiet();
        for (strip, p) in self.bus_strips.iter_mut().zip(song.buses.iter()) {
            strip.set_bus(p, self.sr, snap);
        }
        if snap {
            self.master.snap(Self::master_gain(&song));
        } else {
            self.master.target = Self::master_gain(&song);
        }
        let old = std::mem::replace(&mut self.song, song);
        self.font_bank.retain_tracks(&self.song.tracks);
        self.resync();
        old
    }

    fn remap_strips(&mut self, tracks: &[TrackParams]) -> [bool; MAX_TRACKS] {
        let mut map = [None::<usize>; MAX_TRACKS];
        for (i, old) in self.song.tracks.iter().enumerate() {
            map[i] = tracks.iter().position(|t| t.id == old.id);
        }
        for v in self
            .voices
            .iter_mut()
            .filter(|v| v.active && v.strip < MAX_TRACKS)
        {
            match map[v.strip] {
                Some(j) => v.strip = j,
                None => v.active = false,
            }
        }
        for s in self.scratch[..MAX_TRACKS].iter_mut() {
            *s = Strip::new(self.sr);
        }
        for (i, target) in map.iter().enumerate().take(self.song.tracks.len()) {
            if let Some(j) = *target {
                self.scratch[j] = self.strips[i];
            }
        }
        for (strip, scratch) in self.strips[..MAX_TRACKS]
            .iter_mut()
            .zip(self.scratch[..MAX_TRACKS].iter_mut())
        {
            std::mem::swap(strip, scratch);
        }
        for s in &mut self.strips[tracks.len()..MAX_TRACKS] {
            s.silence();
        }
        let mut was_known = [false; MAX_TRACKS];
        for j in map.into_iter().flatten() {
            was_known[j] = true;
        }
        was_known
    }

    /// Recomputes the next event/beat for the current clock (after load or seek).
    fn resync(&mut self) {
        let clock = self.clock;
        self.next_event = self.song.events.partition_point(|e| e.clock < clock - EPS);
        self.next_beat =
            (clock / STEPS_PER_BEAT as f64 - EPS).ceil().max(0.0) * STEPS_PER_BEAT as f64;
    }

    fn hard_reset(&mut self) {
        for v in &mut self.voices {
            v.active = false;
            v.held = false;
        }
        self.font_bank.hard_reset();
        for s in &mut self.strips {
            s.reset();
        }
        for s in &mut self.bus_strips {
            s.reset();
        }
        self.delay.clear();
        self.reverb.clear();
        self.limiter.clear();
        self.click.env = 0.0;
        self.dc_l = DcBlock::new(self.sr);
        self.dc_r = DcBlock::new(self.sr);
        self.panic_left = 0;
    }

    /// Starts the transport at `start_step` (clamped into the song). Notes already sounding
    /// at that point are chased so held chords and pads are heard immediately.
    pub fn play(&mut self, start_step: f64, looping: bool) {
        if self.panic_left > 0 {
            self.hard_reset();
        }
        self.release_song_voices();
        let total = self.song.total_steps;
        let start = if start_step.is_finite() && start_step >= 0.0 && start_step < total {
            start_step
        } else {
            0.0
        };
        self.clock = start;
        self.looping = looping;
        self.playing = true;
        self.resync();
        if start > 0.0 {
            let sps = self.samples_per_step();
            for i in 0..self.next_event {
                let ev = self.song.events[i];
                if ev.end > start + 0.25 {
                    if let Some(tp) = self.song.tracks.get(ev.track).copied() {
                        if tp.audible && !tp.sound.is_drums() {
                            let gate = ((ev.end - start) * sps) as u64;
                            self.note_on(ev.track, &tp, ev.pitch, ev.velocity, gate, true);
                        }
                    }
                }
            }
        }
    }

    /// Sets the repeat bounds used while `looping`, in steps (`None` wraps at
    /// the song end). Reversed or degenerate spans are ignored. Changing the
    /// region takes effect on the next transport step, mid-playback included.
    pub fn set_loop_region(&mut self, region: Option<(f64, f64)>) {
        self.loop_region = region.and_then(|(a, b)| {
            let (a, b) = (a.min(b).max(0.0), a.max(b));
            (a.is_finite() && b.is_finite() && b - a > EPS).then_some((a, b))
        });
    }

    /// The active repeat bounds `(start, end)` in steps: the region while
    /// `looping` is on and it still overlaps the song, else the whole song.
    fn loop_points(&self) -> (f64, f64) {
        let total = self.song.total_steps;
        if self.looping {
            if let Some((a, b)) = self.loop_region {
                let (a, b) = (a.clamp(0.0, total), b.clamp(0.0, total));
                if b - a > EPS {
                    return (a, b);
                }
            }
        }
        (0.0, total)
    }

    /// Stops the transport where it is; notes release naturally and effect tails ring out.
    pub fn pause(&mut self) {
        self.playing = false;
        self.release_song_voices();
    }

    pub fn device_lost(&mut self) {
        self.playing = false;
        self.hard_reset();
    }

    /// Stops, rewinds and silences everything (voices and effect tails) after a short fade.
    pub fn stop(&mut self) {
        self.playing = false;
        self.clock = 0.0;
        self.next_event = 0;
        self.next_beat = 0.0;
        for v in &mut self.voices {
            v.held = false;
        }
        if self.is_quiet() {
            self.hard_reset();
        } else {
            self.panic_left = self.panic_len;
        }
    }

    /// Auditions a note on the dedicated preview strip with the given track's sound and mix.
    pub fn preview(&mut self, params: TrackParams, pitch: u8, velocity: f32, seconds: f64) {
        if self.panic_left > 0 {
            self.hard_reset();
        }
        let snap = self
            .voices
            .iter()
            .all(|v| !v.active || v.strip != PREVIEW_STRIP)
            && !self.font_bank.preview_sounding();
        // Auditions always feed the master directly — never the track's
        // subgroup, which might be muted or solo-gated.
        let preview = TrackParams {
            audible: true,
            bus: 0,
            ..params
        };
        self.strips[PREVIEW_STRIP].set(&preview, self.sr, snap);
        let secs = if seconds.is_finite() {
            seconds.clamp(0.02, 10.0)
        } else {
            0.5
        };
        let gate = (secs * self.sr as f64) as u64;
        self.note_on(
            PREVIEW_STRIP,
            &params,
            pitch.min(127),
            velocity.clamp(0.0, 1.0),
            gate,
            false,
        );
    }

    fn release_song_voices(&mut self) {
        for v in self.voices.iter_mut().filter(|v| v.active && v.from_song) {
            v.release();
        }
        self.font_bank.release_song_voices();
    }

    fn note_on(
        &mut self,
        strip: usize,
        params: &TrackParams,
        pitch: u8,
        velocity: f32,
        gate: u64,
        from_song: bool,
    ) {
        self.age += 1;
        let gate = gate.max((self.sr * 0.001) as u64);
        let mut sound = params.sound;
        if let Sound::Font(key) = sound {
            // Sampled notes go to the per-track rustysynth instance (keyed by
            // track id, so reordering needs no remap; the preview strip uses
            // its own slot). When the font file is not installed — or cannot
            // run at this rate — the track's fallback sound plays instead so
            // the song still sounds.
            if self.font_bank.note_on(
                strip == PREVIEW_STRIP,
                params.id,
                key,
                pitch,
                velocity,
                gate,
                from_song,
            ) {
                return;
            }
            sound = params.fallback;
        }
        let start = NoteStart {
            sound,
            strip,
            pitch,
            velocity,
            gate,
            from_song,
            age: self.age,
        };
        // Per-track glide (seconds, 0 = off): on melodic sounds the newest
        // same-strip voice retargets legato-style — bending to the new pitch
        // instead of stacking a second voice — so consecutive notes connect
        // rather than re-attack, exactly like a mono sound. Drums and kits
        // never glide; `Sound::Font` already returned above (rustysynth
        // voices have no pitch slew).
        // `clamp` keeps a NaN NaN (still `> 0.0` == false → off); `legato`
        // only sees a finite positive value.
        let glide_secs = params.glide.clamp(0.0, MAX_GLIDE_SECONDS);
        let glide = glide_secs > 0.0 && !sound.is_drums();
        if sound.is_mono() || glide {
            let current = self
                .voices
                .iter_mut()
                .filter(|v| v.active && v.strip == strip && !v.is_fading())
                .max_by_key(|v| v.age);
            if let Some(v) = current {
                // Karplus-Strong voices (pluck/guitar) can't bend mid-ring —
                // the pitch lives in the string's delay line — so on a glide
                // track they re-pluck at the new pitch instead.
                let bends = glide
                    && !matches!(
                        sound,
                        Sound::Builtin(Instrument::Pluck | Instrument::Guitar)
                    );
                if v.held && bends {
                    v.legato(pitch, velocity, start.gate, glide_secs);
                } else if v.held && !glide {
                    // Mono sounds keep their own slide feel: the lead's
                    // fixed constant, none for mono pack presets.
                    let slew = if matches!(sound, Sound::Builtin(_)) {
                        LEAD_GLIDE_SECONDS
                    } else {
                        0.0
                    };
                    v.legato(pitch, velocity, start.gate, slew);
                } else {
                    // Released-but-ringing (or a held plucked-string voice):
                    // restart from the current envelope level — still
                    // connected, still one voice per strip.
                    v.retrigger(&start);
                }
                v.age = self.age;
                v.from_song = from_song;
                for other in self
                    .voices
                    .iter_mut()
                    .filter(|o| o.active && o.strip == strip && o.age != self.age)
                {
                    other.choke(0.01);
                }
                return;
            }
        } else if matches!(sound, Sound::Builtin(Instrument::Drums)) {
            // Same-kind choking for the built-in eight-voice kit keys on
            // `Voice::drum`/`drum_kind`; pack kits use choke groups below.
            let kind = super::voice::drum_kind(pitch);
            for v in self
                .voices
                .iter_mut()
                .filter(|v| v.active && v.strip == strip && !v.is_fading())
            {
                if v.drum == kind {
                    v.choke(0.015);
                } else if kind == DrumKind::ClosedHat && v.drum == DrumKind::OpenHat {
                    v.choke(0.006);
                }
            }
        } else if let Sound::Kit(kit) = &sound {
            // Pack kits: a piece with a `choke_group` fades every sounding
            // voice of the same group on this strip (closed hat cuts the
            // open hat); otherwise the generic same-pitch release applies.
            let group = kit.piece_for(pitch).choke_group;
            for v in self
                .voices
                .iter_mut()
                .filter(|v| v.active && v.strip == strip && !v.is_fading())
            {
                if group != 0 && v.choke_group == group {
                    v.choke(0.008);
                } else if v.held && v.pitch == pitch {
                    v.release();
                }
            }
        } else {
            for v in self
                .voices
                .iter_mut()
                .filter(|v| v.active && v.held && v.strip == strip && v.pitch == pitch)
            {
                v.release();
            }
        }
        let idx = self.allocate();
        self.voices[idx].start(&start);
    }

    fn allocate(&mut self) -> usize {
        let mut active = 0;
        let mut free = None;
        for (i, v) in self.voices.iter().enumerate() {
            if v.active {
                active += 1;
            } else if free.is_none() {
                free = Some(i);
            }
        }
        // Keep a few slots free by fading the quietest released voice early.
        if active > MAX_VOICES - 8 {
            if let Some(v) = self
                .voices
                .iter_mut()
                .filter(|v| v.active && !v.held && !v.is_fading())
                .min_by(|a, b| a.level.total_cmp(&b.level))
            {
                v.choke(0.005);
            }
        }
        if let Some(i) = free {
            return i;
        }
        // Steal: prefer released voices (quietest first), else the oldest.
        let mut best = 0;
        let mut best_key = (true, f32::MAX, u64::MAX);
        for (i, v) in self.voices.iter().enumerate() {
            let key = (v.held, v.level, v.age);
            if (key.0 as u8, key.1, key.2) < (best_key.0 as u8, best_key.1, best_key.2) {
                best = i;
                best_key = key;
            }
        }
        best
    }

    /// Fires events and beats due at the current clock, then advances the clock to the next
    /// due point (at most `max` frames). Returns the frames to render before the next call.
    fn transport_step(&mut self, max: usize) -> usize {
        let sps = self.samples_per_step();
        // `end` plays the role `total_steps` has for whole-song playback: the
        // clock never crosses it, events and beats stop there, and a looping
        // transport wraps to `loop_start` on arrival.
        let (loop_start, end) = self.loop_points();
        loop {
            while let Some(ev) = self.song.events.get(self.next_event).copied() {
                if ev.clock > self.clock + EPS || ev.clock >= end - EPS {
                    break;
                }
                self.next_event += 1;
                if let Some(tp) = self.song.tracks.get(ev.track).copied() {
                    if tp.audible {
                        let gate = ((ev.end - ev.clock).max(0.0) * sps) as u64;
                        self.note_on(ev.track, &tp, ev.pitch, ev.velocity, gate, true);
                    }
                }
            }
            if self.next_beat <= self.clock + EPS {
                if self.metronome && self.next_beat < end - EPS {
                    let accent = (self.next_beat / STEPS_PER_BAR as f64).fract().abs() < 1e-6;
                    self.click.trigger(accent, self.sr);
                }
                self.next_beat += STEPS_PER_BEAT as f64;
                continue;
            }
            if self.clock >= end - EPS {
                if self.looping {
                    // `rem_euclid` lands back inside the span in one step even
                    // when a seek jumped whole passes past `end`.
                    let overshoot = (self.clock - end).max(0.0);
                    self.clock = loop_start + overshoot.rem_euclid(end - loop_start);
                    self.resync();
                    continue;
                }
                self.playing = false;
                self.clock = 0.0;
                self.resync();
                return max;
            }
            break;
        }
        let mut next = end.min(self.next_beat);
        if let Some(ev) = self.song.events.get(self.next_event) {
            next = next.min(ev.clock);
        }
        let frames = (((next - self.clock) * sps) - 1e-6).ceil().max(1.0);
        let frames = (frames as usize).min(max);
        self.clock += frames as f64 / sps;
        frames
    }

    fn render_voices(&mut self, from: usize, to: usize) {
        for v in self.voices.iter_mut().filter(|v| v.active) {
            let s = v.strip.min(STRIPS - 1);
            self.strip_used[s] = true;
            let base = s * MAX_BLOCK;
            v.render(
                &mut self.strip_l[base + from..base + to],
                &mut self.strip_r[base + from..base + to],
            );
        }
        // SoundFont synths accumulate into the same per-strip buffers, so font
        // audio gets the identical fader/pan/drive/filter/FX treatment as voices.
        self.font_bank.render(
            &self.song.tracks,
            &mut self.strip_l,
            &mut self.strip_r,
            &mut self.strip_used,
            &mut self.font_scratch_l,
            &mut self.font_scratch_r,
            from,
            to,
        );
        self.click.render(&mut self.click_buf[from..to]);
    }

    /// Mixes imported-WAV clips into the per-strip buffers for output frames
    /// `from..to`, which cover transport steps `[seg_start, seg_start + len/sps)`.
    ///
    /// Clips are stateless — the transport clock maps straight onto a source
    /// position — so seeking into the middle of a clip and transport loop
    /// wraps (which only ever happen at segment boundaries) need no chase
    /// bookkeeping at all: the first post-seek block simply lands mid-clip.
    /// Everything runs through the track's strip, so fader/pan/drive/filter,
    /// sends and bus routing behave exactly as they do for synthesized audio.
    fn render_audio(&mut self, seg_start: f64, from: usize, to: usize) {
        if self.song.audio_clips.is_empty() {
            return;
        }
        let sps = self.samples_per_step();
        let secs_per_step = self.song.seconds_per_step();
        let inv_sps = 1.0 / sps;
        let seg_end = seg_start + (to - from) as f64 * inv_sps;
        for ci in 0..self.song.audio_clips.len() {
            let clip = self.song.audio_clips[ci];
            let span = clip.span();
            let clip_end = clip.start + span;
            if span <= 0.0 || clip.start >= seg_end - EPS || clip_end <= seg_start + EPS {
                continue;
            }
            let Some(src) = self.song.audio.get(clip.source) else {
                continue;
            };
            let rate = src.sample_rate as f64;
            if !rate.is_finite()
                || rate <= 0.0
                || src.left.len() < 2
                || src.right.len() != src.left.len()
            {
                continue;
            }
            let strip = clip.track.min(STRIPS - 1);
            let base = strip * MAX_BLOCK;
            // Output-frame bounds of the overlap inside [from, to).
            let i0 = from
                + (((clip.start - seg_start) * sps - 1e-9).ceil().max(0.0) as usize).min(to - from);
            let i1 = (from + ((clip_end - seg_start) * sps + 1e-9).ceil() as usize).min(to);
            if i0 >= i1 {
                continue;
            }
            self.strip_used[strip] = true;
            // Walk clip-local steps and the fractional source position
            // together, resetting the source pointer at each repetition.
            let ratio = rate / self.sr as f64;
            let fade_in = clip.fade_in.max(0.0);
            let fade_out = clip.fade_out.max(0.0);
            let mut local = seg_start + (i0 - from) as f64 * inv_sps - clip.start;
            let mut loop_pos = local.rem_euclid(clip.length);
            let mut src_pos = (clip.offset + loop_pos * secs_per_step) * rate;
            let src_frames = src.left.len();
            for i in i0..i1 {
                // Linear fades over the clip span, both in steps.
                let mut g = 1.0f64;
                if fade_in > EPS && local < fade_in {
                    g = (local / fade_in).max(0.0);
                }
                if fade_out > EPS && span - local < fade_out {
                    g = g.min(((span - local) / fade_out).max(0.0));
                }
                let idx = src_pos as usize;
                if idx + 1 < src_frames {
                    // 4-point cubic interpolation; the outer taps clamp to
                    // the source's ends (clip loops restart from `offset`,
                    // not by wrapping the source).
                    let fr = (src_pos - idx as f64) as f32;
                    let (im1, i2) = (idx.saturating_sub(1), (idx + 2).min(src_frames - 1));
                    let (l, r) = (&src.left, &src.right);
                    let hermite = super::voice::hermite;
                    let x = hermite(l[im1], l[idx], l[idx + 1], l[i2], fr);
                    let y = hermite(r[im1], r[idx], r[idx + 1], r[i2], fr);
                    let gv = g as f32;
                    self.strip_l[base + i] += x * gv;
                    self.strip_r[base + i] += y * gv;
                }
                local += inv_sps;
                loop_pos += inv_sps;
                src_pos += ratio;
                if loop_pos >= clip.length {
                    loop_pos -= clip.length;
                    src_pos = (clip.offset + loop_pos * secs_per_step) * rate;
                }
            }
        }
    }

    fn render_block<const METER: bool>(&mut self, n: usize, peaks: &mut BlockPeaks) {
        let mut pos = 0;
        let sps = self.samples_per_step();
        while pos < n {
            let was_playing = self.playing;
            let seg = if was_playing {
                self.transport_step(n - pos)
            } else {
                n - pos
            };
            if was_playing {
                // `transport_step` already advanced the clock past this
                // segment; the segment's start is `seg` frames back. When the
                // song ended mid-call the clock rewound to 0, so the (dead
                // tail) segment lands before step 0 and clips stay silent.
                let seg_start = self.clock - seg as f64 / sps;
                self.render_audio(seg_start, pos, pos + seg);
            }
            self.render_voices(pos, pos + seg);
            pos += seg;
        }

        let sr = self.sr;
        for s in 0..STRIPS {
            // Mixer automation chases the transport clock: retarget the
            // strip's fader/pan smoothers once per block and let their ramp
            // ride the curve. `None` entries — the common case — skip
            // entirely, so an un-automated mix costs nothing here.
            if let (Some(auto), Some(p)) = (
                self.song.automation.get(s).and_then(Option::as_ref),
                self.song.tracks.get(s),
            ) {
                self.strips[s].set_automation(p, auto, self.clock);
            }
            let used = std::mem::take(&mut self.strip_used[s]);
            if !used && !self.strips[s].live {
                if !self.strips[s].gain_l.settled()
                    || self.strips[s].cutoff != self.strips[s].cutoff_target
                {
                    self.strips[s].idle(sr);
                }
                continue;
            }
            let base = s * MAX_BLOCK;
            // Post-fader output goes to the strip's subgroup buffer (bus > 0)
            // or straight to the master mix (bus 0 — including the preview
            // strip, which is always master-direct).
            let b = self.strips[s].bus.min(NUM_BUSES as u8) as usize;
            let (out_l, out_r) = if b == 0 {
                (&mut self.mix_l[..n], &mut self.mix_r[..n])
            } else {
                let bb = (b - 1) * MAX_BLOCK;
                self.bus_used[b - 1] = true;
                (&mut self.bus_l[bb..bb + n], &mut self.bus_r[bb..bb + n])
            };
            let peak = self.strips[s].process::<METER>(
                sr,
                &self.strip_l[base..base + n],
                &self.strip_r[base..base + n],
                (out_l, out_r),
                (&mut self.rev_l[..n], &mut self.rev_r[..n]),
                (&mut self.dly_l[..n], &mut self.dly_r[..n]),
            );
            if METER && s < MAX_TRACKS {
                peaks.tracks[s] = peaks.tracks[s].max(peak);
            }
            self.strip_l[base..base + n].fill(0.0);
            self.strip_r[base..base + n].fill(0.0);
        }

        // Second pass: each subgroup's collected audio runs through its own
        // fader/pan strip into the master. Track sends already reached the
        // shared effects at source, so a gated group also cannot leak echo
        // or reverb into the mix.
        for b in 0..NUM_BUSES {
            let used = std::mem::take(&mut self.bus_used[b]);
            if !used && !self.bus_strips[b].live {
                if !self.bus_strips[b].gain_l.settled()
                    || self.bus_strips[b].cutoff != self.bus_strips[b].cutoff_target
                {
                    self.bus_strips[b].idle(sr);
                }
                continue;
            }
            let base = b * MAX_BLOCK;
            let peak = self.bus_strips[b].process::<METER>(
                sr,
                &self.bus_l[base..base + n],
                &self.bus_r[base..base + n],
                (&mut self.mix_l[..n], &mut self.mix_r[..n]),
                (&mut self.rev_l[..n], &mut self.rev_r[..n]),
                (&mut self.dly_l[..n], &mut self.dly_r[..n]),
            );
            if METER {
                peaks.buses[b] = peaks.buses[b].max(peak);
            }
            self.bus_l[base..base + n].fill(0.0);
            self.bus_r[base..base + n].fill(0.0);
        }

        let has = |b: &[f32]| b.iter().any(|x| *x != 0.0);
        let echo = (DELAY_STEPS * self.song.seconds_per_step()) as f32;
        self.delay.set_time(echo, self.delay.is_quiet());
        if has(&self.dly_l[..n]) || has(&self.dly_r[..n]) || !self.delay.is_quiet() {
            self.delay
                .process(&mut self.dly_l[..n], &mut self.dly_r[..n]);
            for (i, (&dl, &dr)) in self.dly_l[..n]
                .iter()
                .zip(self.dly_r[..n].iter())
                .enumerate()
            {
                self.mix_l[i] += dl;
                self.mix_r[i] += dr;
                self.rev_l[i] += dl * 0.2;
                self.rev_r[i] += dr * 0.2;
            }
        }
        if has(&self.rev_l[..n]) || has(&self.rev_r[..n]) || !self.reverb.is_quiet() {
            self.reverb
                .process(&mut self.rev_l[..n], &mut self.rev_r[..n]);
            for (i, (&rv, &rw)) in self.rev_l[..n]
                .iter()
                .zip(self.rev_r[..n].iter())
                .enumerate()
            {
                self.mix_l[i] += rv;
                self.mix_r[i] += rw;
            }
        }

        for (i, (ml, mr)) in self.mix_l[..n]
            .iter_mut()
            .zip(self.mix_r[..n].iter_mut())
            .enumerate()
        {
            let g = self.master.next();
            let c = self.click_buf[i];
            *ml = self.dc_l.process(*ml * g + c);
            *mr = self.dc_r.process(*mr * g + c);
        }
        self.limiter
            .process(&mut self.mix_l[..n], &mut self.mix_r[..n]);

        if self.panic_left > 0 {
            for (ml, mr) in self.mix_l[..n].iter_mut().zip(self.mix_r[..n].iter_mut()) {
                let g = self.panic_left as f32 / self.panic_len as f32;
                *ml *= g;
                *mr *= g;
                self.panic_left = self.panic_left.saturating_sub(1);
            }
            if self.panic_left == 0 {
                self.hard_reset();
            }
        }

        let (pl, pr) = self.mix_l[..n]
            .iter()
            .zip(self.mix_r[..n].iter())
            .fold((0.0f32, 0.0f32), |(a, b), (&x, &y)| {
                (a.max(x.abs()), b.max(y.abs()))
            });
        self.peak_l = pl;
        self.peak_r = pr;
    }

    /// Renders `out_l.len()` frames (any length) into the given buffers, overwriting them.
    pub fn render(&mut self, out_l: &mut [f32], out_r: &mut [f32]) {
        self.render_with_metering::<false>(out_l, out_r, &mut BlockPeaks::default());
    }

    pub fn render_with_metering<const METER: bool>(
        &mut self,
        out_l: &mut [f32],
        out_r: &mut [f32],
        peaks: &mut BlockPeaks,
    ) {
        let len = out_l.len().min(out_r.len());
        let mut done = 0;
        let (mut peak_l, mut peak_r) = (0.0f32, 0.0f32);
        while done < len {
            let n = (len - done).min(MAX_BLOCK);
            for b in [
                &mut self.mix_l,
                &mut self.mix_r,
                &mut self.rev_l,
                &mut self.rev_r,
                &mut self.dly_l,
                &mut self.dly_r,
                &mut self.click_buf,
            ] {
                b[..n].fill(0.0);
            }
            self.render_block::<METER>(n, peaks);
            out_l[done..done + n].copy_from_slice(&self.mix_l[..n]);
            out_r[done..done + n].copy_from_slice(&self.mix_r[..n]);
            peak_l = peak_l.max(self.peak_l);
            peak_r = peak_r.max(self.peak_r);
            done += n;
        }
        self.peak_l = peak_l;
        self.peak_r = peak_r;
    }
}

#[cfg(test)]
mod tests {
    use super::super::song::SchedAudio;
    use super::*;
    use crate::model::{create_track, Instrument, Note, Project};

    const SR: f32 = 48_000.0;

    fn one_note_project(instrument: Instrument, pitch: u8, step: f64, duration: f64) -> Project {
        let mut p = Project::empty();
        p.bpm = 120.0;
        p.bars = 1;
        p.tracks = vec![create_track(instrument)];
        let t = &mut p.tracks[0];
        t.effects.reverb = 0.0;
        t.effects.delay = 0.0;
        t.patterns[0].notes = vec![Note::new(pitch, step, duration, 1.0)];
        p.validate().unwrap();
        p
    }

    fn run(r: &mut Renderer, frames: usize) -> (Vec<f32>, Vec<f32>) {
        let mut l = vec![0.0; frames];
        let mut rr = vec![0.0; frames];
        // Odd chunk sizes exercise block splitting like a real audio callback.
        let mut done = 0;
        let mut k = 0;
        while done < frames {
            let n = [441, 128, 1000, 37][k % 4].min(frames - done);
            r.render(&mut l[done..done + n], &mut rr[done..done + n]);
            done += n;
            k += 1;
        }
        (l, rr)
    }

    fn onset(x: &[f32], threshold: f32) -> usize {
        x.iter()
            .position(|s| s.abs() > threshold)
            .unwrap_or(usize::MAX)
    }

    /// A two-hat pack kit: a long ringing open hat (46) and a closed hat
    /// (42), both in `group` (0 = no choke group).
    fn hat_kit_project(group: u8) -> Project {
        let kit: crate::packs::InstrumentPreset = serde_json::from_str(&format!(
            r#"{{
                "id": "test.hats", "name": "Hats",
                "kit": {{ "pieces": [
                    {{ "name": "Closed", "pitch": 42, "voice": {{
                        "oscillators": [{{"waveform": "noise"}}],
                        "envelope": {{"attack": 0.001, "decay": 0.05, "sustain": 0.0, "release": 0.02}},
                        "choke_group": {group} }} }},
                    {{ "name": "Open", "pitch": 46, "voice": {{
                        "oscillators": [{{"waveform": "noise"}}],
                        "envelope": {{"attack": 0.001, "decay": 2.0, "sustain": 0.8, "release": 1.0}},
                        "choke_group": {group} }} }}
                ] }}
            }}"#
        ))
        .unwrap();
        let mut p = Project::empty();
        p.bpm = 120.0;
        p.bars = 1;
        let mut t = crate::model::Track::from_preset(Instrument::Drums, kit);
        t.effects.reverb = 0.0;
        t.effects.delay = 0.0;
        t.patterns[0].notes = vec![Note::new(46, 0.0, 16.0, 1.0), Note::new(42, 4.0, 1.0, 1.0)];
        p.tracks = vec![t];
        p.validate().unwrap();
        p
    }

    #[test]
    fn pack_kit_choke_group_cuts_the_open_hat() {
        let open_hat_sounding = |group: u8| {
            let mut r = Renderer::new(SR);
            r.load(Box::new(SongData::from_project(&hat_kit_project(group))));
            r.play(0.0, false);
            // Step 4 is at 24 000 frames; look 40 ms after the closed hat.
            let frames = 24_000 + r.latency() + 1_920;
            run(&mut r, frames);
            r.voices.iter().any(|v| v.active && v.pitch == 46)
        };
        assert!(open_hat_sounding(0), "without a group the hats stack");
        assert!(!open_hat_sounding(1), "the closed hat chokes the open hat");
    }

    #[test]
    fn events_fire_sample_accurately() {
        // 120 BPM at 48 kHz: 6000 frames per step.
        let p = one_note_project(Instrument::Drums, 36, 4.0, 1.0);
        let mut r = Renderer::new(SR);
        r.load(Box::new(SongData::from_project(&p)));
        r.play(0.0, false);
        let (l, _) = run(&mut r, 40_000);
        let at = onset(&l, 1e-6);
        assert_eq!(at, 24_000 + r.latency(), "kick on step 4");
    }

    #[test]
    fn swing_delays_off_beats() {
        let mut p = one_note_project(Instrument::Drums, 36, 1.0, 1.0);
        p.swing = 0.5;
        let mut r = Renderer::new(SR);
        r.load(Box::new(SongData::from_project(&p)));
        r.play(0.0, false);
        let (l, _) = run(&mut r, 20_000);
        assert_eq!(
            onset(&l, 1e-6),
            8_000 + r.latency(),
            "step 1 at triplet position 4/3"
        );
    }

    #[test]
    fn seek_and_chase_held_notes() {
        let p = one_note_project(Instrument::Pad, 60, 0.0, 16.0);
        let mut r = Renderer::new(SR);
        r.load(Box::new(SongData::from_project(&p)));
        r.play(8.0, false);
        let (l, _) = run(&mut r, 24_000);
        assert!(l.iter().any(|s| s.abs() > 0.01), "chased pad is audible");
        assert!(
            (r.position() - (8.0 + 4.0)).abs() < 1e-6,
            "position advanced 4 steps: {}",
            r.position()
        );
    }

    #[test]
    fn song_end_stops_or_loops() {
        let p = one_note_project(Instrument::Drums, 38, 0.0, 1.0);
        let mut r = Renderer::new(SR);
        r.load(Box::new(SongData::from_project(&p)));
        r.play(0.0, false);
        let total = 16 * 6000;
        run(&mut r, total + 1000);
        assert!(!r.is_playing());
        let (l, _) = run(&mut r, SR as usize * 2);
        assert!(
            l.iter().all(|s| s.abs() < 0.01),
            "no retrigger without loop"
        );

        let mut r = Renderer::new(SR);
        r.load(Box::new(SongData::from_project(&p)));
        r.play(0.0, true);
        run(&mut r, total - 100);
        let (l, _) = run(&mut r, 20_000);
        assert!(r.is_playing());
        let hit = onset(&l, 1e-3);
        assert!(
            hit > 50 && hit < 400,
            "looped snare fires at the loop point (got {hit})"
        );
    }

    /// Burst onsets: a hit counts when |x| rises above `threshold` after at
    /// least `min_gap` consecutive samples below it, so a decaying tone that
    /// crosses the threshold on every cycle counts once.
    fn onsets(x: &[f32], threshold: f32, min_gap: usize) -> Vec<usize> {
        let mut hits = Vec::new();
        let mut below = min_gap;
        for (i, s) in x.iter().enumerate() {
            if s.abs() > threshold {
                if below >= min_gap {
                    hits.push(i);
                }
                below = 0;
            } else {
                below += 1;
            }
        }
        hits
    }

    /// Four-bar song: the one-bar pattern (kick on its step 4) plays at
    /// clip start 16 → hit on step 20 — and again at clip start 44 → step 48,
    /// exactly on the region end where it must never fire.
    fn region_test_project() -> Project {
        let mut p = one_note_project(Instrument::Drums, 36, 4.0, 1.0);
        p.bars = 4;
        p.tracks[0].clips[0].start = 16.0;
        let mut boundary = p.tracks[0].clips[0].clone();
        boundary.id = crate::model::next_id();
        boundary.start = 44.0;
        p.tracks[0].clips.push(boundary);
        p
    }

    #[test]
    fn loop_region_wraps_between_its_bounds() {
        let p = region_test_project();
        let mut r = Renderer::new(SR);
        r.load(Box::new(SongData::from_project(&p)));
        let _ = r.apply(Command::SetLoopRegion(Some((16.0, 48.0))));
        r.play(0.0, true);
        // 6000 frames per step: the kick fires at clock 20, then every
        // (48 - 16) = 32 steps = 192_000 frames after each wrap. The step-48
        // hit in the second clip is outside the region and stays silent.
        let (l, _) = run(&mut r, 730_000);
        let lat = r.latency();
        let hits = onsets(&l, 1e-3, 12_000);
        assert_eq!(hits.len(), 4, "one kick per region pass: {hits:?}");
        assert!(
            hits[0].abs_diff(20 * 6000 + lat) <= 200,
            "first kick on step 20: {}",
            hits[0]
        );
        for w in hits.windows(2) {
            assert_eq!(w[1] - w[0], 192_000, "each pass is the 32-step region");
        }
        assert!(
            (16.0..48.0).contains(&r.position()),
            "transport stays inside the region: {}",
            r.position()
        );
    }

    #[test]
    fn loop_region_is_inert_without_looping() {
        let p = region_test_project();
        let mut r = Renderer::new(SR);
        r.load(Box::new(SongData::from_project(&p)));
        r.set_loop_region(Some((16.0, 48.0)));
        r.play(0.0, false);
        // Not looping: the region is only a marker — playback runs to the
        // song end and the boundary hit at step 48 fires normally.
        let (l, _) = run(&mut r, 400_000);
        assert!(!r.is_playing());
        let hits = onsets(&l, 1e-3, 12_000);
        assert_eq!(hits.len(), 2, "hits at steps 20 and 48: {hits:?}");
        assert_eq!(hits[1] - hits[0], (48 - 20) * 6000);
    }

    #[test]
    fn loop_region_moves_while_playing() {
        let p = region_test_project();
        let mut r = Renderer::new(SR);
        r.load(Box::new(SongData::from_project(&p)));
        r.set_loop_region(Some((16.0, 48.0)));
        r.play(0.0, true);
        run(&mut r, 400_000);
        // Shrink the region to one bar mid-flight: the next wrap lands on 32.
        r.set_loop_region(Some((16.0, 32.0)));
        let (l, _) = run(&mut r, 300_000);
        assert!(r.is_playing());
        assert!(
            (16.0..32.0).contains(&r.position()),
            "wrap moved to the new end: {}",
            r.position()
        );
        // Passes now take 16 steps = 96_000 frames: at least two kicks.
        let hits = onsets(&l, 1e-3, 12_000);
        assert!(hits.len() >= 2, "region pass got shorter: {hits:?}");
        assert_eq!(hits[1] - hits[0], 96_000);
    }

    #[test]
    fn clearing_loop_region_restores_song_wrap() {
        let p = region_test_project();
        let mut r = Renderer::new(SR);
        r.load(Box::new(SongData::from_project(&p)));
        r.set_loop_region(Some((16.0, 48.0)));
        r.play(0.0, true);
        run(&mut r, 100_000);
        r.set_loop_region(None);
        let (l, _) = run(&mut r, 420_000);
        // Back to whole-song wrap (64 steps = 384_000 frames): the boundary
        // hit at 48 is audible again.
        assert!(r.is_playing());
        assert!(r.position() < 64.0);
        let hits = onsets(&l, 1e-3, 12_000);
        assert!(
            hits.len() >= 2,
            "song content past the region plays: {hits:?}"
        );
    }

    #[test]
    fn stop_silences_tails_and_frees_voices() {
        let mut p = Project::demo();
        for t in &mut p.tracks {
            t.effects.reverb = 1.0;
            t.effects.delay = 1.0;
        }
        let mut r = Renderer::new(SR);
        r.load(Box::new(SongData::from_project(&p)));
        r.play(0.0, true);
        let (l, _) = run(&mut r, SR as usize * 3);
        assert!(l.iter().any(|s| s.abs() > 0.05));
        assert!(r.active_voices() > 0);
        r.stop();
        let (l, rr) = run(&mut r, (SR * 0.05) as usize);
        assert_eq!(r.active_voices(), 0);
        assert!(r.is_quiet());
        let late = &l[(SR * 0.02) as usize..];
        assert!(
            late.iter()
                .chain(&rr[(SR * 0.02) as usize..])
                .all(|s| *s == 0.0),
            "silent after panic fade"
        );
        assert_eq!(r.position(), 0.0);
        // Repeated play/stop cycles never leak voices.
        for _ in 0..20 {
            r.play(0.0, false);
            run(&mut r, 4096);
            r.stop();
            run(&mut r, 2048);
            assert_eq!(r.active_voices(), 0);
        }
    }

    #[test]
    fn pause_releases_notes_but_keeps_position() {
        let p = one_note_project(Instrument::Pad, 60, 0.0, 16.0);
        let mut r = Renderer::new(SR);
        r.load(Box::new(SongData::from_project(&p)));
        r.play(0.0, false);
        run(&mut r, 30_000);
        r.pause();
        let pos = r.position();
        run(&mut r, SR as usize * 4);
        assert_eq!(r.position(), pos);
        assert_eq!(r.active_voices(), 0, "released pad finished");
    }

    #[test]
    fn voice_pool_is_bounded_under_dense_input() {
        let mut p = one_note_project(Instrument::Pad, 60, 0.0, 16.0);
        p.tracks[0].patterns[0].notes = (0..400)
            .map(|i| {
                Note::new(
                    24 + (i % 90) as u8,
                    (i % 64) as f64 * 0.25,
                    16.0 - (i % 64) as f64 * 0.25,
                    0.9,
                )
            })
            .collect();
        let mut r = Renderer::new(SR);
        r.load(Box::new(SongData::from_project(&p)));
        r.play(0.0, true);
        let (l, rr) = run(&mut r, SR as usize * 3);
        assert!(r.active_voices() <= MAX_VOICES);
        assert!(l.iter().chain(&rr).all(|s| s.is_finite() && s.abs() <= 1.0));
    }

    #[test]
    fn mute_change_while_playing_fades_track() {
        let mut p = one_note_project(Instrument::Pad, 60, 0.0, 16.0);
        let mut r = Renderer::new(SR);
        r.load(Box::new(SongData::from_project(&p)));
        r.play(0.0, true);
        let (l, _) = run(&mut r, SR as usize);
        assert!(l[24_000..].iter().any(|s| s.abs() > 0.01));
        p.tracks[0].mute = true;
        r.load(Box::new(SongData::from_project(&p)));
        let (l, _) = run(&mut r, SR as usize);
        assert!(
            l[4_800..].iter().all(|s| s.abs() < 1e-4),
            "muted within 100 ms"
        );
    }

    #[test]
    fn reordering_tracks_keeps_voices_on_their_strip() {
        let mut p = Project::demo();
        let mut r = Renderer::new(SR);
        r.load(Box::new(SongData::from_project(&p)));
        r.play(0.0, true);
        run(&mut r, 20_000);
        p.tracks.reverse();
        p.tracks.remove(0);
        r.load(Box::new(SongData::from_project(&p)));
        let (l, _) = run(&mut r, 20_000);
        assert!(l.iter().all(|s| s.is_finite()));
        assert!(r
            .voices
            .iter()
            .filter(|v| v.active && v.strip < MAX_TRACKS)
            .all(|v| v.strip < p.tracks.len()));
    }

    #[test]
    fn preview_plays_while_stopped_through_preview_strip() {
        let mut r = Renderer::new(SR);
        let t = create_track(Instrument::Pluck);
        r.preview(TrackParams::from_track(&t, false), 72, 0.9, 0.3);
        let (l, _) = run(&mut r, 9600);
        assert!(l.iter().any(|s| s.abs() > 0.01));
        assert!(!r.is_playing());
        run(&mut r, SR as usize * 4);
        assert_eq!(r.active_voices(), 0);
    }

    #[test]
    fn missing_font_falls_back_to_builtin_sound() {
        use crate::model::FontRef;
        let mut p = one_note_project(Instrument::Piano, 60, 0.0, 4.0);
        p.tracks[0].font = Some(FontRef::new("DefinitelyNotInstalled", 0));
        let mut r = Renderer::new(SR);
        // No fonts installed: the reference must fall back to the built-in.
        r.load(Box::new(SongData::from_project(&p)));
        r.play(0.0, false);
        let (l, _) = run(&mut r, 30_000);
        assert!(
            l.iter().any(|s| s.abs() > 0.01),
            "fallback piano is audible"
        );
        assert_eq!(r.font_bank.active_notes(), 0, "no synth was created");
        run(&mut r, SR as usize * 4);
        assert_eq!(r.active_voices(), 0);
    }

    #[test]
    fn font_preview_without_font_uses_fallback() {
        use crate::model::FontRef;
        let mut t = create_track(Instrument::Pluck);
        t.font = Some(FontRef::new("DefinitelyNotInstalled", 0));
        let mut r = Renderer::new(SR);
        r.preview(TrackParams::from_track(&t, true), 72, 0.9, 0.3);
        let (l, _) = run(&mut r, 9600);
        assert!(l.iter().any(|s| s.abs() > 0.01), "fallback pluck audible");
        assert_eq!(r.font_bank.active_notes(), 0);
    }

    #[test]
    fn set_fonts_swaps_catalog_without_losing_transport() {
        let mut r = Renderer::new(SR);
        let p = one_note_project(Instrument::Drums, 36, 0.0, 1.0);
        r.load(Box::new(SongData::from_project(&p)));
        r.play(0.0, true);
        run(&mut r, 4096);
        let library = Arc::new(crate::font::FontLibrary::empty());
        r.set_fonts(library.clone());
        r.set_fonts(library);
        assert!(r.is_playing(), "transport survives a catalog swap");
        let (l, _) = run(&mut r, 8192);
        assert!(l.iter().any(|s| s.abs() > 0.001), "drums still play");
    }

    #[test]
    fn metronome_clicks_on_beats() {
        let p = Project {
            tracks: vec![],
            ..one_note_project(Instrument::Drums, 36, 0.0, 1.0)
        };
        let mut r = Renderer::new(SR);
        r.load(Box::new(SongData::from_project(&p)));
        let _ = r.apply(Command::Metronome(true));
        r.play(0.0, false);
        let (l, _) = run(&mut r, 50_000);
        let lat = r.latency();
        assert!(
            l[lat..lat + 200].iter().any(|s| s.abs() > 0.05),
            "click on beat 1"
        );
        assert!(
            l[lat + 2_000..lat + 23_900].iter().all(|s| s.abs() < 0.01),
            "quiet between beats"
        );
        assert!(
            l[lat + 24_000..lat + 24_200].iter().any(|s| s.abs() > 0.05),
            "click on beat 2"
        );
    }

    #[test]
    fn metronome_is_off_by_default_and_accents_bar_one() {
        let p = Project {
            bars: 2,
            tracks: vec![],
            ..one_note_project(Instrument::Drums, 36, 0.0, 1.0)
        };
        // No `Command::Metronome`: an empty song renders pure silence, which is
        // also what the offline export path produces (it never enables it).
        let mut r = Renderer::new(SR);
        r.load(Box::new(SongData::from_project(&p)));
        r.play(0.0, false);
        let (l, _) = run(&mut r, 40_000);
        assert!(l.iter().all(|s| s.abs() < 1e-6), "metronome off by default");

        // On: the beat-1 click is louder than the other beats' clicks.
        let mut r = Renderer::new(SR);
        r.load(Box::new(SongData::from_project(&p)));
        let _ = r.apply(Command::Metronome(true));
        r.play(0.0, false);
        let (l, _) = run(&mut r, 100_000);
        let lat = r.latency();
        let peak = |from: usize, to: usize| {
            l[lat + from..lat + to]
                .iter()
                .fold(0.0f32, |m, s| m.max(s.abs()))
        };
        // 120 BPM at 48 kHz: 6000 frames per step, a beat every 4 steps.
        let accent = peak(0, 2_000);
        let beat = peak(24_000, 26_000);
        assert!(accent > 0.1, "accent click present ({accent})");
        assert!(beat > 0.05, "beat click present ({beat})");
        assert!(accent > beat * 1.2, "accent louder: {accent} vs {beat}");
    }

    #[test]
    fn mono_lead_glides_instead_of_stacking() {
        let mut p = one_note_project(Instrument::Lead, 60, 0.0, 4.0);
        p.tracks[0].patterns[0]
            .notes
            .push(Note::new(67, 2.0, 4.0, 1.0));
        let mut r = Renderer::new(SR);
        r.load(Box::new(SongData::from_project(&p)));
        r.play(0.0, false);
        run(&mut r, 18_000);
        assert_eq!(r.voices.iter().filter(|v| v.active && v.held).count(), 1);
    }

    /// Average samples between rising zero crossings — a pitch probe for
    /// single-voice renders (0 when fewer than two crossings fit).
    #[allow(dead_code)]
    fn cycle_samples(x: &[f32]) -> f32 {
        let mut first = 0usize;
        let mut last = 0usize;
        let mut n = 0usize;
        for i in 1..x.len() {
            if x[i - 1] <= 0.0 && x[i] > 0.0 {
                if n == 0 {
                    first = i;
                }
                last = i;
                n += 1;
            }
        }
        if n < 2 {
            return 0.0;
        }
        (last - first) as f32 / (n - 1) as f32
    }

    #[test]
    fn glide_track_bends_overlapping_notes_without_reattack() {
        // Two overlapping notes on a polyphonic pad with glide: the second
        // note retargets the held voice — one voice, legato, no re-attack.
        let mut p = one_note_project(Instrument::Pad, 60, 0.0, 8.0);
        p.tracks[0].glide = 0.8;
        p.tracks[0].patterns[0]
            .notes
            .push(Note::new(72, 4.0, 8.0, 1.0));
        let mut r = Renderer::new(SR);
        r.load(Box::new(SongData::from_project(&p)));
        r.play(0.0, false);
        // 6000 frames per step: the second note fires at 24_000 (+latency).
        // A re-attacked pad would still be inside its 0.55 s attack; the
        // legato voice is already at sustain level.
        let (l, _) = run(&mut r, 28_000);
        let held: Vec<_> = r.voices.iter().filter(|v| v.active && v.held).collect();
        assert_eq!(held.len(), 1, "glide retargets the held voice, no stacking");
        assert_eq!(held[0].pitch, 72, "the same voice now plays the new note");
        assert!(
            held[0].level > 0.4,
            "envelope held through the transition: {}",
            held[0].level
        );
        let lat = r.latency();
        assert!(
            l[lat + 24_000..lat + 26_000].iter().any(|s| s.abs() > 0.01),
            "output stays connected across the note change"
        );
        // The one voice keeps the new note until its own release.
        run(&mut r, 40_000);
        assert_eq!(r.voices.iter().filter(|v| v.active && v.held).count(), 1);
    }

    #[test]
    fn glide_track_sweeps_pitch_continuously() {
        // Bass 48→60 with a 1 s glide: shortly after the note change the
        // voice frequency is still near the old note, then sweeps up toward
        // the new one — portamento, not a jump.
        use crate::model::pitch_hz;
        let mut p = one_note_project(Instrument::Bass, 48, 0.0, 8.0);
        p.tracks[0].glide = 1.0;
        p.tracks[0].patterns[0]
            .notes
            .push(Note::new(60, 4.0, 8.0, 1.0));
        let mut r = Renderer::new(SR);
        r.load(Box::new(SongData::from_project(&p)));
        r.play(0.0, false);
        let old_f = pitch_hz(48.0);
        let new_f = pitch_hz(60.0);
        let span = new_f - old_f;
        // Note 2 fires at step 4 = 24_000 frames. ~0.06 s into the 1 s
        // slide the exponential approach has covered only a few percent.
        run(&mut r, 27_000);
        let early = r
            .voices
            .iter()
            .find(|v| v.active && v.held)
            .expect("one voice carries both notes");
        assert!((early.target_freq - new_f).abs() < 0.01);
        assert!(
            early.freq > old_f && early.freq < old_f + span * 0.35,
            "still near the old pitch: {}",
            early.freq
        );
        // ~1.3 s in: most of the distance to the new pitch is covered.
        run(&mut r, 60_000);
        let late = r
            .voices
            .iter()
            .find(|v| v.active)
            .expect("voice still ringing");
        assert!(
            late.freq > old_f + span * 0.65 && late.freq <= new_f,
            "swept most of the way to the new pitch: {}",
            late.freq
        );
    }

    #[test]
    fn glide_track_retriggers_sequential_notes_on_one_voice() {
        // Gapped (non-overlapping) notes on a glide track reuse the ringing
        // voice instead of stacking — still one voice per strip, restarted
        // from the current envelope level rather than from silence.
        let mut p = one_note_project(Instrument::Pad, 60, 0.0, 2.0);
        p.tracks[0].glide = 0.3;
        p.tracks[0].patterns[0]
            .notes
            .push(Note::new(64, 3.0, 2.0, 1.0));
        let mut r = Renderer::new(SR);
        r.load(Box::new(SongData::from_project(&p)));
        r.play(0.0, false);
        // Note 1 ends at step 2; note 2 starts at step 3 while the pad is
        // still mid-release, so it is retriggered, not stacked.
        run(&mut r, 25_000);
        let active: Vec<_> = r.voices.iter().filter(|v| v.active).collect();
        assert_eq!(active.len(), 1, "the ringing voice was retriggered");
        assert_eq!(active[0].pitch, 64);
        assert!(
            active[0].level > 0.3,
            "retrigger continued from the ringing level: {}",
            active[0].level
        );
    }

    #[test]
    fn no_glide_keeps_polyphonic_stacking() {
        // The same overlapping pair without glide stacks two voices, exactly
        // as before the control existed.
        let mut p = one_note_project(Instrument::Pad, 60, 0.0, 8.0);
        p.tracks[0].patterns[0]
            .notes
            .push(Note::new(72, 4.0, 8.0, 1.0));
        let mut r = Renderer::new(SR);
        r.load(Box::new(SongData::from_project(&p)));
        r.play(0.0, false);
        run(&mut r, 26_000);
        assert_eq!(
            r.voices.iter().filter(|v| v.active && v.held).count(),
            2,
            "overlapping notes still stack when glide is off"
        );
    }

    #[test]
    fn glide_track_bends_pack_preset_notes() {
        // A polyphonic pack preset retargets legato-style the same way: the
        // glide is a per-track control, not a property of the sound.
        let mut p = one_note_project(Instrument::Piano, 60, 0.0, 8.0);
        let mut preset = crate::packs::template_pack("glide-pack", "Glide")
            .unwrap()
            .instruments[0]
            .clone();
        preset.monophonic = false;
        p.tracks[0].preset = Some(preset);
        p.tracks[0].glide = 0.5;
        p.tracks[0].patterns[0]
            .notes
            .push(Note::new(67, 4.0, 8.0, 1.0));
        let mut r = Renderer::new(SR);
        r.load(Box::new(SongData::from_project(&p)));
        r.play(0.0, false);
        run(&mut r, 26_000);
        let held: Vec<_> = r.voices.iter().filter(|v| v.active && v.held).collect();
        assert_eq!(held.len(), 1, "preset voice retargeted, no stacking");
        assert_eq!(held[0].pitch, 67);
    }

    #[test]
    fn glide_is_ignored_on_drum_tracks() {
        // Glide is a melodic control: drum hits keep stacking their own
        // one-shot voices even with the knob up.
        let mut p = one_note_project(Instrument::Drums, 36, 0.0, 1.0);
        p.tracks[0].glide = 1.0;
        p.tracks[0].patterns[0]
            .notes
            .push(Note::new(49, 0.0, 1.0, 1.0));
        let mut r = Renderer::new(SR);
        r.load(Box::new(SongData::from_project(&p)));
        r.play(0.0, false);
        run(&mut r, 10_000);
        assert!(
            r.voices.iter().filter(|v| v.active).count() >= 2,
            "kick and crash got their own voices"
        );
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|s| s * s).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    #[test]
    fn volume_automation_pulls_the_fader_down() {
        use crate::model::{Automation, AutomationPoint};
        // A bar-long pad under a lane that falls 1.0 -> 0.0 across the bar:
        // the fader multiplier reads ~0.7 early on and ~0.05 near the end.
        let mut p = one_note_project(Instrument::Pad, 60, 0.0, 16.0);
        p.tracks[0].automation = Some(Automation {
            volume: vec![
                AutomationPoint {
                    step: 0.0,
                    value: 1.0,
                },
                AutomationPoint {
                    step: 16.0,
                    value: 0.0,
                },
            ],
            pan: vec![],
        });
        p.validate().unwrap();
        let mut r = Renderer::new(SR);
        r.load(Box::new(SongData::from_project(&p)));
        r.play(0.0, false);
        // 6000 frames per step at 120 BPM; the whole bar is 96_000 frames.
        let (l, _) = run(&mut r, 96_000);
        let lat = r.latency();
        // Step ~5.5 (post-attack, well into the sustain) vs step ~15.
        let early = rms(&l[lat + 33_000..lat + 39_000]);
        let late = rms(&l[lat + 88_000..lat + 94_000]);
        assert!(early > 0.005, "pad is audible mid-bar: {early}");
        assert!(
            late < early * 0.4,
            "the fade pulled the fader down: {early} vs {late}"
        );
    }

    #[test]
    fn pan_automation_moves_the_image() {
        use crate::model::{Automation, AutomationPoint};
        // Same pad; the pan lane holds centre until step 4 (past the pad's
        // attack), then slides to a hard right by step 12 and stays there.
        let mut p = one_note_project(Instrument::Pad, 60, 0.0, 16.0);
        p.tracks[0].automation = Some(Automation {
            volume: vec![],
            pan: vec![
                AutomationPoint {
                    step: 4.0,
                    value: 0.0,
                },
                AutomationPoint {
                    step: 12.0,
                    value: 1.0,
                },
            ],
        });
        p.validate().unwrap();
        let mut r = Renderer::new(SR);
        r.load(Box::new(SongData::from_project(&p)));
        r.play(0.0, false);
        let (l, rr) = run(&mut r, 96_000);
        let lat = r.latency();
        // Step ~5.5: the slide barely started, both sides carry the pad.
        // Step ~13.5: hard right, the left channel is essentially silent.
        let (el, er) = (
            rms(&l[lat + 30_000..lat + 36_000]),
            rms(&rr[lat + 30_000..lat + 36_000]),
        );
        let (ll, lr) = (
            rms(&l[lat + 78_000..lat + 84_000]),
            rms(&rr[lat + 78_000..lat + 84_000]),
        );
        assert!(el > 0.005 && er > 0.005, "centre image: {el} / {er}");
        assert!(
            lr > el * 0.5 && ll < lr * 0.2,
            "pan rode to the right: early {el}/{er}, late {ll}/{lr}"
        );
    }

    // ----- subgroup buses ----------------------------------------------------

    #[test]
    fn bus_routed_track_plays_through_its_bus_strip() {
        let mut p = one_note_project(Instrument::Drums, 36, 0.0, 1.0);
        p.tracks[0].bus = 1;
        p.validate().unwrap();
        let mut r = Renderer::new(SR);
        r.load(Box::new(SongData::from_project(&p)));
        r.play(0.0, false);
        let (l, _) = run(&mut r, 30_000);
        assert!(l.iter().any(|s| s.abs() > 0.01), "bus-routed kick sounds");
    }

    #[test]
    fn muted_bus_silences_its_group() {
        let mut p = one_note_project(Instrument::Drums, 36, 0.0, 1.0);
        p.tracks[0].bus = 1;
        p.buses[0].mute = true;
        p.validate().unwrap();
        let mut r = Renderer::new(SR);
        r.load(Box::new(SongData::from_project(&p)));
        r.play(0.0, false);
        let (l, rr) = run(&mut r, 30_000);
        assert!(
            l.iter().chain(rr.iter()).all(|s| s.abs() < 1e-6),
            "muted Bus A swallows the whole group"
        );
    }

    #[test]
    fn soloed_bus_isolates_its_group() {
        // A master-direct track goes silent while a bus is soloed.
        let mut p = one_note_project(Instrument::Drums, 36, 0.0, 1.0);
        p.buses[0].solo = true;
        p.validate().unwrap();
        let mut r = Renderer::new(SR);
        r.load(Box::new(SongData::from_project(&p)));
        r.play(0.0, false);
        let (l, rr) = run(&mut r, 30_000);
        assert!(
            l.iter().chain(rr.iter()).all(|s| s.abs() < 1e-6),
            "master-direct track is gated by a bus solo"
        );

        // The same hit on the soloed bus still sounds.
        p.tracks[0].bus = 1;
        p.validate().unwrap();
        let mut r = Renderer::new(SR);
        r.load(Box::new(SongData::from_project(&p)));
        r.play(0.0, false);
        let (l, _) = run(&mut r, 30_000);
        assert!(
            l.iter().any(|s| s.abs() > 0.01),
            "soloed Bus A carries its group"
        );

        // Soloing a *different* bus silences this group too.
        p.buses[0].solo = false;
        p.buses[1].solo = true;
        p.validate().unwrap();
        let mut r = Renderer::new(SR);
        r.load(Box::new(SongData::from_project(&p)));
        r.play(0.0, false);
        let (l, rr) = run(&mut r, 30_000);
        assert!(
            l.iter().chain(rr.iter()).all(|s| s.abs() < 1e-6),
            "a track on a non-soloed bus is gated"
        );
    }

    #[test]
    fn preview_always_feeds_master_directly() {
        let mut p = one_note_project(Instrument::Drums, 36, 0.0, 1.0);
        p.buses[2].mute = true; // Bus C muted
        p.validate().unwrap();
        let song = SongData::from_project(&p);
        let mut r = Renderer::new(SR);
        r.load(Box::new(song.clone()));
        // Params routed at the muted bus must still audition — the preview
        // strip is always master-direct.
        let mut tp = song.tracks[0];
        tp.bus = 3;
        r.preview(tp, 36, 1.0, 0.3);
        let (l, _) = run(&mut r, 10_000);
        assert!(
            l.iter().any(|s| s.abs() > 0.01),
            "preview bypasses the (muted) subgroup"
        );
    }

    // ----- imported-WAV clips ---------------------------------------------

    /// A four-bar song on one audible track; decoded clip sources are
    /// injected into `SongData` directly (staging real files belongs to
    /// persistence's tests, which already cover the copy/dedup path).
    fn audio_clip_project() -> Project {
        let mut p = Project::empty();
        p.bpm = 120.0;
        p.bars = 4;
        p.tracks = vec![create_track(Instrument::Pad)];
        let t = &mut p.tracks[0];
        t.effects.reverb = 0.0;
        t.effects.delay = 0.0;
        p.validate().unwrap();
        p
    }

    fn sched(start: f64, length: f64, loops: u32, offset: f64) -> SchedAudio {
        SchedAudio {
            start,
            length,
            loops,
            offset,
            fade_in: 0.0,
            fade_out: 0.0,
            track: 0,
            source: 0,
        }
    }

    fn wav(rate: u32, left: Vec<f32>, right: Vec<f32>) -> crate::persistence::DecodedWav {
        crate::persistence::DecodedWav {
            sample_rate: rate,
            seconds: left.len() as f64 / rate as f64,
            left,
            right,
        }
    }

    #[test]
    fn audio_clip_fires_at_its_start_step() {
        let p = audio_clip_project();
        let mut sd = SongData::from_project(&p);
        sd.audio = vec![wav(48_000, vec![0.5; 48_000], vec![0.5; 48_000])];
        sd.audio_clips = vec![sched(4.0, 32.0, 1, 0.0)];
        let mut r = Renderer::new(SR);
        r.load(Box::new(sd));
        r.play(0.0, false);
        let (l, _) = run(&mut r, 40_000);
        assert_eq!(
            onset(&l, 1e-3),
            4 * 6_000 + r.latency(),
            "clip starts on step 4"
        );
    }

    #[test]
    fn audio_clip_seek_lands_mid_source() {
        // A sine whose amplitude ramps linearly makes the source position
        // readable through local level: source frame i carries amplitude
        // 0.1 + 0.4·i/N, so the level ratio at two instants reveals exactly
        // which source samples played — the strip gain cancels. (A plain
        // ramp can't be used: the DC block in the output chain eats DC.)
        // Seeking to step 2 resumes at source frame 12000, not 0.
        let n = 48_000usize;
        let left: Vec<f32> = (0..n)
            .map(|i| {
                let env = 0.1 + 0.4 * i as f32 / n as f32;
                env * (i as f32 * 2.0 * std::f32::consts::PI / 100.0).sin()
            })
            .collect();
        let right = left.clone();
        // Local peak amplitude in a small window around an output frame.
        let level = |x: &[f32], at: usize| {
            x[at..(at + 2_000).min(x.len())]
                .iter()
                .fold(0f32, |m, s| m.max(s.abs()))
        };
        let p = audio_clip_project();
        let mut sd = SongData::from_project(&p);
        sd.audio = vec![wav(48_000, left, right)];
        // At 120 BPM a step is 0.125 s, so the 1 s source fills an 8-step
        // repetition exactly (0.125 s × 8 = 48 000 source frames).
        sd.audio_clips = vec![sched(0.0, 8.0, 8, 0.0)];
        let mut r = Renderer::new(SR);
        r.load(Box::new(sd));
        r.play(2.0, false);
        let (l, _) = run(&mut r, 36_000);
        let lat = r.latency();
        // Both probe points sit well past the strip's post-play smoother
        // ramp: lat+6000 ↔ src 18000 (env 0.25) and lat+30000 ↔ src 42000
        // (env 0.45), ratio 1.8; a wrongly restarted source would give
        // env[30000]/env[6000] ≈ 2.33.
        let a = level(&l, lat + 6_000);
        let b = level(&l, lat + 30_000);
        assert!(a > 0.01, "sought clip is audible");
        let ratio = b / a;
        assert!(
            (ratio - 1.8).abs() < 0.1,
            "playback resumed mid-source near frame 12000 (ratio {ratio})"
        );
    }

    #[test]
    fn audio_clip_restarts_the_source_each_loop() {
        // A short burst at the top of the source repeats once per
        // repetition: three 8-step loops produce three onsets 48 000
        // output frames apart.
        let mut left = vec![0.0f32; 48_000];
        for s in left.iter_mut().take(200) {
            *s = 0.5;
        }
        let right = left.clone();
        let p = audio_clip_project();
        let mut sd = SongData::from_project(&p);
        sd.audio = vec![wav(48_000, left, right)];
        sd.audio_clips = vec![sched(0.0, 8.0, 3, 0.0)];
        let mut r = Renderer::new(SR);
        r.load(Box::new(sd));
        r.play(0.0, false);
        let (l, _) = run(&mut r, 3 * 48_000 + 2_000);
        let lat = r.latency();
        let hits = onsets(&l, 1e-3, 12_000);
        assert_eq!(hits.len(), 3, "one onset per loop: {hits:?}");
        for (k, want) in [0usize, 48_000, 96_000].iter().enumerate() {
            assert!(
                hits[k].abs_diff(want + lat) <= 64,
                "repetition {k} starts near frame {}: {}",
                want + lat,
                hits[k]
            );
        }
    }

    #[test]
    fn audio_clip_missing_file_schedules_nothing() {
        // A well-formed but unstaged reference stays valid yet silent —
        // `SongData::from_project` drops it instead of failing.
        let mut p = audio_clip_project();
        p.tracks[0].audio = true;
        p.tracks[0].audio_clips.push(crate::model::AudioClip::new(
            "definitely_missing_0123456789abcdef.wav",
            "gone",
            0.0,
            16.0,
        ));
        p.validate().unwrap();
        let sd = SongData::from_project(&p);
        assert!(sd.audio_clips.is_empty() && sd.audio.is_empty());
        let mut r = Renderer::new(SR);
        r.load(Box::new(sd));
        r.play(0.0, false);
        let (l, rr) = run(&mut r, 20_000);
        assert!(
            l.iter().chain(rr.iter()).all(|s| s.abs() < 1e-6),
            "a missing staged file renders silence, not an error"
        );
    }

    #[test]
    fn audio_clip_fades_ramp_gain_over_the_span() {
        // A 32-step clip with 8-step fades at both ends: the middle reads
        // full level while windows inside the ramps read fractions of it.
        let frames = 32 * 6_000; // 4 s of source at 48 kHz, one per step
        let left: Vec<f32> = (0..frames)
            .map(|i| 0.5 * (i as f32 * 2.0 * std::f32::consts::PI / 100.0).sin())
            .collect();
        let right = left.clone();
        let p = audio_clip_project();
        let mut sd = SongData::from_project(&p);
        sd.audio = vec![wav(48_000, left, right)];
        sd.audio_clips = vec![SchedAudio {
            fade_in: 8.0,
            fade_out: 8.0,
            ..sched(0.0, 32.0, 1, 0.0)
        }];
        let mut r = Renderer::new(SR);
        r.load(Box::new(sd));
        r.play(0.0, false);
        let (l, _) = run(&mut r, frames + 4_000);
        let lat = r.latency();
        // Step windows 1..2 (fade-in), 16..17 (flat) and 31..31.7 (fade-out).
        let early = rms(&l[lat + 6_000..lat + 12_000]);
        let mid = rms(&l[lat + 96_000..lat + 102_000]);
        let end = rms(&l[lat + 186_000..lat + 190_000]);
        assert!(mid > 0.05, "flat mid-span is audible: {mid}");
        assert!(
            early < mid * 0.35,
            "fade-in holds the start down: {early} vs {mid}"
        );
        assert!(
            end < mid * 0.3,
            "fade-out pulls the end down: {end} vs {mid}"
        );
        // Past the 32-step clip the song (no notes, no effects) is silent.
        assert!(l[lat + frames..].iter().all(|s| s.abs() < 1e-3));
    }

    #[test]
    fn audio_clip_resamples_a_22050_source_to_duration() {
        // A burst at the tail of a 22_050 Hz source: through the ratio path
        // it lands near the end of each 48 000-output-frame repetition, and
        // the clip occupies exactly 3 × 8 steps = 144 000 output frames.
        let n = 22_050usize;
        let mut left = vec![0.0f32; n];
        for (i, s) in left.iter_mut().enumerate().skip(n - 2_000) {
            *s = 0.5 * ((i as f32 - (n - 2_000) as f32) * 2.0 * std::f32::consts::PI / 100.0).sin();
        }
        let right = left.clone();
        let p = audio_clip_project();
        let mut sd = SongData::from_project(&p);
        sd.audio = vec![wav(22_050, left, right)];
        sd.audio_clips = vec![sched(0.0, 8.0, 3, 0.0)];
        let mut r = Renderer::new(SR);
        r.load(Box::new(sd));
        r.play(0.0, false);
        let (l, _) = run(&mut r, 150_000);
        let lat = r.latency();
        // The 2 000-source-frame burst starts at output frame
        // 20_050 / (22_050 / 48_000) ≈ 43 647 within each repetition.
        let hits = onsets(&l, 1e-3, 12_000);
        assert_eq!(hits.len(), 3, "one burst per repetition: {hits:?}");
        for (k, want) in [43_647usize, 91_647, 139_647].iter().enumerate() {
            assert!(
                hits[k].abs_diff(want + lat) <= 300,
                "repetition {k} starts near frame {}: {}",
                want + lat,
                hits[k]
            );
        }
        // Nothing after the 24-step span — the clip really ends there. A
        // small DC-block/limiter decay tail is expected, so allow residue
        // well below the burst's 0.5 amplitude.
        assert!(l[lat + 144_000..].iter().all(|s| s.abs() < 1e-2));
    }

    #[test]
    fn audio_clip_offset_starts_mid_source() {
        // A two-second source that is silent for its first second: starting
        // the clip at offset 1.0 s lands in the loud half, so output is loud
        // from the very first repetition — an ignored offset would stay
        // silent for 48 000 output frames instead.
        let n = 96_000usize;
        let mut left = vec![0.0f32; n];
        for (i, s) in left.iter_mut().enumerate().skip(n / 2) {
            *s = 0.5 * (i as f32 * 2.0 * std::f32::consts::PI / 100.0).sin();
        }
        let right = left.clone();
        let p = audio_clip_project();
        let mut sd = SongData::from_project(&p);
        sd.audio = vec![wav(48_000, left, right)];
        sd.audio_clips = vec![sched(0.0, 8.0, 2, 1.0)];
        let mut r = Renderer::new(SR);
        r.load(Box::new(sd));
        r.play(0.0, false);
        let (l, _) = run(&mut r, 100_000);
        let lat = r.latency();
        assert!(
            onset(&l, 1e-3) <= lat + 500,
            "the loud half starts the clip immediately, not a second late"
        );
        // Both repetitions play from the offset: the whole span sounds.
        assert!(rms(&l[lat + 4_000..lat + 44_000]) > 0.05, "first loop loud");
        assert!(
            rms(&l[lat + 52_000..lat + 90_000]) > 0.05,
            "second loop loud"
        );
        // The hard cut at the clip's last frame rings through the DC-block
        // and limiter for a few hundred ms — bounded, then decayed.
        assert!(l[lat + 96_000..].iter().all(|s| s.abs() < 0.1));
        assert!(l[lat + 99_000..].iter().all(|s| s.abs() < 1e-2));
    }

    #[test]
    fn volume_automation_seek_lands_mid_ramp() {
        use crate::model::{Automation, AutomationPoint};
        // A steady clip under a volume lane that falls 1.0 -> 0.0 across a
        // bar. Seeking to step 8 must settle the fader at the lane's
        // mid-ramp value (~0.4), not at the ramp start (1.0).
        let n = 48_000usize;
        let left: Vec<f32> = (0..n)
            .map(|i| 0.5 * (i as f32 * 2.0 * std::f32::consts::PI / 100.0).sin())
            .collect();
        let right = left.clone();
        let mut p = audio_clip_project();
        p.tracks[0].automation = Some(Automation {
            volume: vec![
                AutomationPoint {
                    step: 0.0,
                    value: 1.0,
                },
                AutomationPoint {
                    step: 16.0,
                    value: 0.0,
                },
            ],
            pan: vec![],
        });
        p.validate().unwrap();
        let mut sd = SongData::from_project(&p);
        sd.audio = vec![wav(48_000, left.clone(), right.clone())];
        sd.audio_clips = vec![sched(0.0, 8.0, 3, 0.0)];
        let mut r = Renderer::new(SR);
        r.load(Box::new(sd));
        r.play(8.0, false);
        let (l, _) = run(&mut r, 30_000);
        let lat = r.latency();
        // Steps 9..10 after the seek: the lane reads ~0.44 down to ~0.38,
        // so the settled window carries ~0.4 of the un-automated level.
        let window = rms(&l[lat + 6_000..lat + 12_000]);

        // Reference: the identical seek without automation keeps the fader
        // at full gain, so the ratio isolates the automation multiplier.
        let mut plain = audio_clip_project();
        plain.tracks[0].automation = None;
        plain.validate().unwrap();
        let mut sd = SongData::from_project(&plain);
        sd.audio = vec![wav(48_000, left, right)];
        sd.audio_clips = vec![sched(0.0, 8.0, 3, 0.0)];
        let mut r = Renderer::new(SR);
        r.load(Box::new(sd));
        r.play(8.0, false);
        let (ref_l, _) = run(&mut r, 30_000);
        let lat = r.latency();
        let reference = rms(&ref_l[lat + 6_000..lat + 12_000]);

        assert!(
            reference > 0.05,
            "un-automated clip is audible: {reference}"
        );
        let ratio = window / reference;
        assert!(
            (0.25..=0.6).contains(&ratio),
            "seek landed mid-ramp (~0.4), not at the ramp start or end: {ratio}"
        );
    }
}
