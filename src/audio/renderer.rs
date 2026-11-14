//! The song renderer shared by real-time playback and offline export: sample-accurate
//! transport with swing, voice allocation, per-track drive/filter/fader/pan strips, shared
//! reverb and tempo-synced delay sends, master gain, DC blocking and a look-ahead limiter.
//!
//! After construction nothing here allocates; commands only swap pointers.

use super::dsp::{
    soft_clip, time_coef, DcBlock, Limiter, PingPongDelay, Reverb, Smoother, Svf, SvfCoefs,
};
use super::song::{SongData, TrackParams};
use super::voice::{DrumKind, NoteStart, Sound, Voice};
use crate::model::{MAX_CUTOFF_HZ, MAX_TRACKS, STEPS_PER_BAR, STEPS_PER_BEAT};

/// Largest internal processing block in frames.
pub const MAX_BLOCK: usize = 256;
/// Voice pool size; the oldest/quietest voice is stolen beyond this.
pub const MAX_VOICES: usize = 96;
/// Strip used for auditioning notes from the editor.
pub const PREVIEW_STRIP: usize = MAX_TRACKS;
const STRIPS: usize = MAX_TRACKS + 1;
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
pub enum Command {
    Load(Box<SongData>),
    Play {
        song: Box<SongData>,
        start_step: f64,
        looping: bool,
    },
    Pause,
    Stop,
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
    fn process(
        &mut self,
        sr: f32,
        l: &[f32],
        r: &[f32],
        out: (&mut [f32], &mut [f32]),
        rev: (&mut [f32], &mut [f32]),
        dly: (&mut [f32], &mut [f32]),
    ) {
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
        }
        self.live = peak > 1e-7 || !self.filt_l.is_quiet() || !self.filt_r.is_quiet();
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
        std::mem::swap(&mut fresh.song, &mut self.song);
        fresh.metronome = self.metronome;
        fresh.master.snap(Self::master_gain(&fresh.song));
        for (strip, p) in fresh.strips.iter_mut().zip(fresh.song.tracks.iter()) {
            strip.set(p, fresh.sr, true);
        }
        *self = fresh;
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
        self.voices.iter().filter(|v| v.active).count()
    }

    /// Peak of the most recent rendered block, per channel.
    pub fn peaks(&self) -> (f32, f32) {
        (self.peak_l, self.peak_r)
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
            } => {
                let old = self.load(song);
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
        let mut map = [None::<usize>; MAX_TRACKS];
        for (i, old) in self.song.tracks.iter().enumerate() {
            map[i] = song.tracks.iter().position(|t| t.id == old.id);
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
        let was_known: [bool; MAX_TRACKS] = {
            let mut k = [false; MAX_TRACKS];
            for j in map.iter().flatten() {
                k[*j] = true;
            }
            k
        };
        for (j, p) in song.tracks.iter().enumerate() {
            self.strips[j].set(p, self.sr, !was_known[j]);
        }
        for s in &mut self.strips[song.tracks.len()..MAX_TRACKS] {
            s.silence();
        }
        if !self.playing && self.is_quiet() {
            self.master.snap(Self::master_gain(&song));
        } else {
            self.master.target = Self::master_gain(&song);
        }
        let old = std::mem::replace(&mut self.song, song);
        self.resync();
        old
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
        for s in &mut self.strips {
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
                            self.note_on(ev.track, tp.sound, ev.pitch, ev.velocity, gate, true);
                        }
                    }
                }
            }
        }
    }

    /// Stops the transport where it is; notes release naturally and effect tails ring out.
    pub fn pause(&mut self) {
        self.playing = false;
        self.release_song_voices();
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
            .all(|v| !v.active || v.strip != PREVIEW_STRIP);
        let preview = TrackParams {
            audible: true,
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
            params.sound,
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
    }

    fn note_on(
        &mut self,
        strip: usize,
        sound: Sound,
        pitch: u8,
        velocity: f32,
        gate: u64,
        from_song: bool,
    ) {
        self.age += 1;
        let start = NoteStart {
            sound,
            strip,
            pitch,
            velocity,
            gate: gate.max((self.sr * 0.001) as u64),
            from_song,
            age: self.age,
        };
        if sound.is_mono() {
            let current = self
                .voices
                .iter_mut()
                .filter(|v| v.active && v.strip == strip && !v.is_fading())
                .max_by_key(|v| v.age);
            if let Some(v) = current {
                if v.held {
                    v.legato(
                        pitch,
                        velocity,
                        start.gate,
                        matches!(sound, Sound::Builtin(_)),
                    );
                    v.age = self.age;
                    v.from_song = from_song;
                } else {
                    v.retrigger(&start);
                }
                for other in self
                    .voices
                    .iter_mut()
                    .filter(|o| o.active && o.strip == strip && o.age != self.age)
                {
                    other.choke(0.01);
                }
                return;
            }
        } else if sound.is_drums() {
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
        let total = self.song.total_steps;
        loop {
            while let Some(ev) = self.song.events.get(self.next_event).copied() {
                if ev.clock > self.clock + EPS {
                    break;
                }
                self.next_event += 1;
                if let Some(tp) = self.song.tracks.get(ev.track).copied() {
                    if tp.audible {
                        let gate = ((ev.end - ev.clock).max(0.0) * sps) as u64;
                        self.note_on(ev.track, tp.sound, ev.pitch, ev.velocity, gate, true);
                    }
                }
            }
            if self.next_beat <= self.clock + EPS {
                if self.metronome && self.next_beat < total - EPS {
                    let accent = (self.next_beat / STEPS_PER_BAR as f64).fract().abs() < 1e-6;
                    self.click.trigger(accent, self.sr);
                }
                self.next_beat += STEPS_PER_BEAT as f64;
                continue;
            }
            if self.clock >= total - EPS {
                if self.looping {
                    self.clock = (self.clock - total).max(0.0);
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
        let mut next = total.min(self.next_beat);
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
        self.click.render(&mut self.click_buf[from..to]);
    }

    fn render_block(&mut self, n: usize) {
        let mut pos = 0;
        while pos < n {
            let seg = if self.playing {
                self.transport_step(n - pos)
            } else {
                n - pos
            };
            self.render_voices(pos, pos + seg);
            pos += seg;
        }

        let sr = self.sr;
        for s in 0..STRIPS {
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
            self.strips[s].process(
                sr,
                &self.strip_l[base..base + n],
                &self.strip_r[base..base + n],
                (&mut self.mix_l[..n], &mut self.mix_r[..n]),
                (&mut self.rev_l[..n], &mut self.rev_r[..n]),
                (&mut self.dly_l[..n], &mut self.dly_r[..n]),
            );
            self.strip_l[base..base + n].fill(0.0);
            self.strip_r[base..base + n].fill(0.0);
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
            self.render_block(n);
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
}
