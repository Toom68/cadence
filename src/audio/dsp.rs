//! Allocation-free DSP building blocks. Every buffer is sized at construction or on
//! [`set_sample_rate`](Reverb::set_sample_rate); per-sample processing never allocates.

use std::f32::consts::{PI, TAU};

/// Flushes tiny values to zero so decaying feedback loops never fall into denormals.
#[inline]
pub fn flush(x: f32) -> f32 {
    if x.abs() < 1e-15 {
        0.0
    } else {
        x
    }
}

/// Smooth saturating curve close to `tanh`, exact limit ±1 for |x| ≥ 3.
#[inline]
pub fn soft_clip(x: f32) -> f32 {
    if x >= 3.0 {
        1.0
    } else if x <= -3.0 {
        -1.0
    } else {
        let x2 = x * x;
        x * (27.0 + x2) / (27.0 + 9.0 * x2)
    }
}

/// Coefficient for a one-pole smoother with time constant `seconds`.
#[inline]
pub fn time_coef(seconds: f32, sample_rate: f32) -> f32 {
    if seconds <= 0.0 {
        0.0
    } else {
        (-1.0 / (seconds * sample_rate)).exp()
    }
}

/// Per-sample multiplier that decays by 60 dB over `t60` seconds.
#[inline]
pub fn t60_coef(t60: f32, sample_rate: f32) -> f32 {
    time_coef(t60 / 6.9078, sample_rate)
}

/// Deterministic xorshift noise; seeding keeps offline renders reproducible.
#[derive(Clone, Copy, Debug)]
pub struct Rng(u32);

impl Rng {
    pub fn new(seed: u32) -> Self {
        let mut r = Rng(seed.wrapping_mul(0x9E37_79B9) ^ 0x2545_F491 | 1);
        r.next_u32();
        r
    }

    #[inline]
    pub fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }

    /// Uniform in [-1, 1).
    #[inline]
    pub fn noise(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 * (2.0 / 16_777_216.0) - 1.0
    }

    /// Uniform in [0, 1).
    #[inline]
    pub fn unit(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / 16_777_216.0
    }
}

/// PolyBLEP residual for band-limited discontinuities; `t` is phase in [0,1), `dt` the increment.
#[inline]
pub fn poly_blep(t: f32, dt: f32) -> f32 {
    if t < dt {
        let t = t / dt;
        t + t - t * t - 1.0
    } else if t > 1.0 - dt {
        let t = (t - 1.0) / dt;
        t * t + t + t + 1.0
    } else {
        0.0
    }
}

#[inline]
pub fn saw(phase: f32, dt: f32) -> f32 {
    2.0 * phase - 1.0 - poly_blep(phase, dt)
}

#[inline]
pub fn square(phase: f32, dt: f32) -> f32 {
    let mut v = if phase < 0.5 { 1.0 } else { -1.0 };
    v += poly_blep(phase, dt);
    let shifted = phase + 0.5;
    v -= poly_blep(
        if shifted >= 1.0 {
            shifted - 1.0
        } else {
            shifted
        },
        dt,
    );
    v
}

#[inline]
pub fn advance(phase: &mut f32, inc: f32) {
    *phase += inc;
    if *phase >= 1.0 {
        *phase -= phase.floor();
    }
}

#[inline]
pub fn sine(phase: f32) -> f32 {
    (phase * TAU).sin()
}

/// Wrap a delay-line position known to be in `[0, 2 * len)`.
#[inline]
fn wrap_once(i: usize, len: usize) -> usize {
    if i >= len {
        i - len
    } else {
        i
    }
}

/// Topology-preserving state-variable filter coefficients (Cytomic / Simper form).
#[derive(Clone, Copy, Debug)]
pub struct SvfCoefs {
    k: f32,
    a1: f32,
    a2: f32,
    a3: f32,
}

impl SvfCoefs {
    pub fn new(cutoff: f32, q: f32, sample_rate: f32) -> Self {
        let fc = cutoff.clamp(10.0, sample_rate * 0.47);
        let g = (PI * fc / sample_rate).tan();
        let k = 1.0 / q.max(0.05);
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        let a3 = g * a2;
        SvfCoefs { k, a1, a2, a3 }
    }
}

impl Default for SvfCoefs {
    fn default() -> Self {
        SvfCoefs::new(1000.0, 0.707, 48_000.0)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Svf {
    ic1: f32,
    ic2: f32,
}

/// Simultaneous low-, band- and high-pass outputs.
#[derive(Clone, Copy, Debug)]
pub struct SvfOut {
    pub low: f32,
    pub band: f32,
    pub high: f32,
}

impl Svf {
    #[inline]
    pub fn process(&mut self, c: &SvfCoefs, x: f32) -> SvfOut {
        let v3 = x - self.ic2;
        let v1 = c.a1 * self.ic1 + c.a2 * v3;
        let v2 = self.ic2 + c.a2 * self.ic1 + c.a3 * v3;
        self.ic1 = flush(2.0 * v1 - self.ic1);
        self.ic2 = flush(2.0 * v2 - self.ic2);
        SvfOut {
            low: v2,
            band: v1,
            high: x - c.k * v1 - v2,
        }
    }

    #[inline]
    pub fn low(&mut self, c: &SvfCoefs, x: f32) -> f32 {
        self.process(c, x).low
    }

    pub fn reset(&mut self) {
        *self = Svf::default();
    }

    pub fn is_quiet(&self) -> bool {
        self.ic1.abs() < 1e-7 && self.ic2.abs() < 1e-7
    }
}

/// One-pole low-pass; the high-pass is `x - lowpass(x)`.
#[derive(Clone, Copy, Debug, Default)]
pub struct OnePole {
    pub a: f32,
    pub z: f32,
}

impl OnePole {
    pub fn new(cutoff: f32, sample_rate: f32) -> Self {
        let mut f = OnePole::default();
        f.set(cutoff, sample_rate);
        f
    }

    pub fn set(&mut self, cutoff: f32, sample_rate: f32) {
        self.a = 1.0 - (-TAU * cutoff.clamp(1.0, sample_rate * 0.49) / sample_rate).exp();
    }

    #[inline]
    pub fn low(&mut self, x: f32) -> f32 {
        self.z = flush(self.z + self.a * (x - self.z));
        self.z
    }

    #[inline]
    pub fn high(&mut self, x: f32) -> f32 {
        x - self.low(x)
    }
}

/// Exponential parameter smoother for zipper-free automation.
#[derive(Clone, Copy, Debug)]
pub struct Smoother {
    pub value: f32,
    pub target: f32,
    coef: f32,
}

impl Smoother {
    pub fn new(value: f32, seconds: f32, sample_rate: f32) -> Self {
        Smoother {
            value,
            target: value,
            coef: time_coef(seconds, sample_rate),
        }
    }

    #[inline]
    pub fn next(&mut self) -> f32 {
        self.value = self.target + (self.value - self.target) * self.coef;
        if (self.value - self.target).abs() < 1e-6 {
            self.value = self.target;
        }
        self.value
    }

    /// Advances `frames` samples at once (for block-rate parameters).
    pub fn skip(&mut self, frames: usize) -> f32 {
        self.value = self.target + (self.value - self.target) * self.coef.powi(frames as i32);
        if (self.value - self.target).abs() < 1e-6 {
            self.value = self.target;
        }
        self.value
    }

    pub fn snap(&mut self, value: f32) {
        self.value = value;
        self.target = value;
    }

    pub fn settled(&self) -> bool {
        self.value == self.target
    }
}

/// DC blocking high-pass (~5 Hz).
#[derive(Clone, Copy, Debug, Default)]
pub struct DcBlock {
    x1: f32,
    y1: f32,
    r: f32,
}

impl DcBlock {
    pub fn new(sample_rate: f32) -> Self {
        DcBlock {
            x1: 0.0,
            y1: 0.0,
            r: 1.0 - TAU * 5.0 / sample_rate,
        }
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let y = x - self.x1 + self.r * self.y1;
        self.x1 = x;
        self.y1 = flush(y);
        y
    }
}

/// Schroeder all-pass diffuser on a fixed buffer.
#[derive(Clone, Debug, Default)]
struct AllPass {
    buf: Vec<f32>,
    pos: usize,
    gain: f32,
}

impl AllPass {
    fn new(len: usize, gain: f32) -> Self {
        AllPass {
            buf: vec![0.0; len.max(1)],
            pos: 0,
            gain,
        }
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let delayed = self.buf[self.pos];
        let v = x + self.gain * delayed;
        self.buf[self.pos] = flush(v);
        self.pos += 1;
        if self.pos == self.buf.len() {
            self.pos = 0;
        }
        delayed - self.gain * v
    }

    fn clear(&mut self) {
        self.buf.fill(0.0);
    }
}

const FDN_LINES: usize = 8;
const FDN_LENGTHS_48K: [usize; FDN_LINES] = [1433, 1601, 1867, 2053, 2251, 2399, 2617, 2797];
const DIFFUSER_LENGTHS_48K: [usize; 4] = [229, 163, 547, 397];

/// Stereo 8-line feedback-delay-network reverb with input diffusion, pre-delay, frequency
/// dependent decay and slow delay modulation to avoid metallic ringing.
#[derive(Clone, Debug)]
pub struct Reverb {
    sample_rate: f32,
    lines: Vec<Vec<f32>>,
    lens: [usize; FDN_LINES],
    pos: [usize; FDN_LINES],
    damp: [OnePole; FDN_LINES],
    gains: [f32; FDN_LINES],
    diffusers: Vec<AllPass>,
    pre: Vec<f32>,
    pre_pos: usize,
    input_hp: OnePole,
    input_lp: OnePole,
    lfo: f32,
    lfo_inc: f32,
    energy: f32,
}

impl Reverb {
    pub const DECAY_SECONDS: f32 = 3.2;
    const PRE_DELAY: f32 = 0.018;
    const MOD_DEPTH: f32 = 6.0;

    pub fn new(sample_rate: f32) -> Self {
        let mut r = Reverb {
            sample_rate: 0.0,
            lines: Vec::new(),
            lens: [1; FDN_LINES],
            pos: [0; FDN_LINES],
            damp: [OnePole::default(); FDN_LINES],
            gains: [0.0; FDN_LINES],
            diffusers: Vec::new(),
            pre: Vec::new(),
            pre_pos: 0,
            input_hp: OnePole::default(),
            input_lp: OnePole::default(),
            lfo: 0.0,
            lfo_inc: 0.0,
            energy: 0.0,
        };
        r.set_sample_rate(sample_rate);
        r
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        let scale = sample_rate / 48_000.0;
        self.sample_rate = sample_rate;
        for (i, &base) in FDN_LENGTHS_48K.iter().enumerate() {
            self.lens[i] = ((base as f32 * scale) as usize).max(8);
            // Headroom for the modulated read position.
            let size = self.lens[i] + Self::MOD_DEPTH as usize * 2 + 4;
            self.damp[i] = OnePole::new(5200.0, sample_rate);
            self.gains[i] =
                10f32.powf(-3.0 * self.lens[i] as f32 / (sample_rate * Self::DECAY_SECONDS));
            if self.lines.len() <= i {
                self.lines.push(vec![0.0; size]);
            } else {
                self.lines[i] = vec![0.0; size];
            }
        }
        self.diffusers = DIFFUSER_LENGTHS_48K
            .iter()
            .map(|&l| AllPass::new(((l as f32 * scale) as usize).max(4), 0.62))
            .collect();
        self.pre = vec![0.0; ((Self::PRE_DELAY * sample_rate) as usize).max(1)];
        self.input_hp = OnePole::new(180.0, sample_rate);
        self.input_lp = OnePole::new(9000.0, sample_rate);
        self.lfo_inc = 0.35 / sample_rate;
        self.clear();
    }

    pub fn clear(&mut self) {
        for l in &mut self.lines {
            l.fill(0.0);
        }
        self.pos = [0; FDN_LINES];
        for d in &mut self.diffusers {
            d.clear();
        }
        for d in &mut self.damp {
            d.z = 0.0;
        }
        self.pre.fill(0.0);
        self.pre_pos = 0;
        self.input_hp.z = 0.0;
        self.input_lp.z = 0.0;
        self.energy = 0.0;
    }

    /// True once both input and internal state have decayed below audibility.
    pub fn is_quiet(&self) -> bool {
        self.energy < 1e-6
    }

    /// Processes a block in place: `l`/`r` hold the send input and receive the wet output.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let mut peak = 0.0f32;
        for (sl, sr) in l.iter_mut().zip(r.iter_mut()) {
            let input = (*sl + *sr) * 0.5;
            peak = peak.max(input.abs());
            let pre_out = self.pre[self.pre_pos];
            self.pre[self.pre_pos] = input;
            self.pre_pos = wrap_once(self.pre_pos + 1, self.pre.len());
            let mut x = self.input_lp.low(self.input_hp.high(pre_out));
            for d in &mut self.diffusers {
                x = d.process(x);
            }

            self.lfo += self.lfo_inc;
            if self.lfo >= 1.0 {
                self.lfo -= 1.0;
            }
            let m = sine(self.lfo) * Self::MOD_DEPTH;

            let mut outs = [0.0f32; FDN_LINES];
            for (k, out) in outs.iter_mut().enumerate() {
                let buf = &self.lines[k];
                let size = buf.len();
                // Modulate two lines in opposite directions; the rest read straight.
                let offset = match k {
                    1 => m,
                    6 => -m,
                    _ => 0.0,
                };
                let delay = self.lens[k] as f32 + offset;
                let read = self.pos[k] as f32 + size as f32 - delay;
                let i0 = read.floor();
                let frac = read - i0;
                let index = wrap_once(i0 as usize, size);
                let a = buf[index];
                let b = buf[wrap_once(index + 1, size)];
                *out = self.damp[k].low(a + (b - a) * frac) * self.gains[k];
            }
            // Householder mixing: y = x - (2/N) * sum(x).
            let sum: f32 = outs.iter().sum::<f32>() * (2.0 / FDN_LINES as f32);
            let mut energy = 0.0f32;
            for (k, out) in outs.iter().enumerate() {
                let size = self.lines[k].len();
                let sign = if k % 2 == 0 { 1.0 } else { -1.0 };
                let v = flush(*out - sum + x * sign * 0.35);
                self.lines[k][self.pos[k]] = v;
                self.pos[k] = wrap_once(self.pos[k] + 1, size);
                energy = energy.max(v.abs());
            }
            *sl = (outs[0] - outs[2] + outs[4] + outs[6] - outs[1] * 0.3) * 0.9;
            *sr = (outs[1] - outs[3] + outs[5] + outs[7] - outs[0] * 0.3) * 0.9;
            self.energy = energy.max(peak);
        }
    }
}

/// Tempo-synced stereo ping-pong delay with filtered feedback and tape-style time smoothing.
#[derive(Clone, Debug)]
pub struct PingPongDelay {
    sample_rate: f32,
    buf_l: Vec<f32>,
    buf_r: Vec<f32>,
    write: usize,
    time: f32,
    target_time: f32,
    time_coef: f32,
    feedback: f32,
    lp_l: OnePole,
    lp_r: OnePole,
    hp_l: OnePole,
    hp_r: OnePole,
    energy: f32,
}

impl PingPongDelay {
    pub const MAX_SECONDS: f32 = 2.5;
    pub const FEEDBACK: f32 = 0.48;

    pub fn new(sample_rate: f32) -> Self {
        let mut d = PingPongDelay {
            sample_rate,
            buf_l: Vec::new(),
            buf_r: Vec::new(),
            write: 0,
            time: 0.0,
            target_time: 0.0,
            time_coef: 0.0,
            feedback: Self::FEEDBACK,
            lp_l: OnePole::default(),
            lp_r: OnePole::default(),
            hp_l: OnePole::default(),
            hp_r: OnePole::default(),
            energy: 0.0,
        };
        d.set_sample_rate(sample_rate);
        d
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        let len = (Self::MAX_SECONDS * sample_rate) as usize + 4;
        self.buf_l = vec![0.0; len];
        self.buf_r = vec![0.0; len];
        self.lp_l = OnePole::new(4800.0, sample_rate);
        self.lp_r = OnePole::new(4800.0, sample_rate);
        self.hp_l = OnePole::new(140.0, sample_rate);
        self.hp_r = OnePole::new(140.0, sample_rate);
        self.time_coef = time_coef(0.06, sample_rate);
        self.time = 0.3 * sample_rate;
        self.target_time = self.time;
        self.clear();
    }

    /// Sets the echo spacing in seconds (smoothed to avoid clicks when the tempo changes).
    pub fn set_time(&mut self, seconds: f32, immediate: bool) {
        let max = (self.buf_l.len() - 4) as f32;
        self.target_time = (seconds * self.sample_rate).clamp(1.0, max);
        if immediate {
            self.time = self.target_time;
        }
    }

    pub fn clear(&mut self) {
        self.buf_l.fill(0.0);
        self.buf_r.fill(0.0);
        for f in [
            &mut self.lp_l,
            &mut self.lp_r,
            &mut self.hp_l,
            &mut self.hp_r,
        ] {
            f.z = 0.0;
        }
        self.energy = 0.0;
    }

    pub fn is_quiet(&self) -> bool {
        self.energy < 1e-6
    }

    /// Processes a block in place: input is the send, output the wet echoes.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let size = self.buf_l.len();
        let mut energy = 0.0f32;
        for (sl, sr) in l.iter_mut().zip(r.iter_mut()) {
            self.time = self.target_time + (self.time - self.target_time) * self.time_coef;
            let pos = self.write as f32 + size as f32 - self.time;
            let i0 = pos.floor();
            let frac = pos - i0;
            // Both channels have the same read head; the delay is at least
            // one sample and shorter than the ring, so one wrap suffices.
            let a = wrap_once(i0 as usize, size);
            let b = wrap_once(a + 1, size);
            let dl = self.buf_l[a] + (self.buf_l[b] - self.buf_l[a]) * frac;
            let dr = self.buf_r[a] + (self.buf_r[b] - self.buf_r[a]) * frac;
            let input = (*sl + *sr) * 0.5;
            let side = (*sl - *sr) * 0.5;
            let fb_l = self.hp_l.high(self.lp_l.low(dr));
            let fb_r = self.hp_r.high(self.lp_r.low(dl));
            // The left line is fed by the input; each echo hops to the other side.
            self.buf_l[self.write] = flush(input + side + fb_l * self.feedback);
            self.buf_r[self.write] = flush(fb_r - side * 0.5);
            self.write = wrap_once(self.write + 1, size);
            *sl = dl;
            *sr = dr;
            energy = energy.max(dl.abs()).max(dr.abs()).max(input.abs());
        }
        self.energy = energy;
    }
}

/// Stereo-linked look-ahead peak limiter followed by a soft safety clip.
#[derive(Clone, Debug)]
pub struct Limiter {
    buf_l: Vec<f32>,
    buf_r: Vec<f32>,
    pos: usize,
    hold_gain: f32,
    hold_left: usize,
    env: f32,
    attack: f32,
    release: f32,
    pub gain_reduction: f32,
}

impl Limiter {
    pub const CEILING: f32 = 0.891; // -1 dBFS
    pub const LOOKAHEAD_SECONDS: f32 = 0.0015;

    pub fn new(sample_rate: f32) -> Self {
        let look = Self::latency_for(sample_rate);
        Limiter {
            buf_l: vec![0.0; look],
            buf_r: vec![0.0; look],
            pos: 0,
            hold_gain: 1.0,
            hold_left: 0,
            env: 1.0,
            attack: 1.0 - (-5.0 / look as f32).exp(),
            release: 1.0 - time_coef(0.12, sample_rate),
            gain_reduction: 0.0,
        }
    }

    /// Delay introduced by the look-ahead, in frames.
    pub fn latency_for(sample_rate: f32) -> usize {
        ((Self::LOOKAHEAD_SECONDS * sample_rate) as usize).max(1)
    }

    pub fn latency(&self) -> usize {
        self.buf_l.len()
    }

    pub fn clear(&mut self) {
        self.buf_l.fill(0.0);
        self.buf_r.fill(0.0);
        self.hold_gain = 1.0;
        self.hold_left = 0;
        self.env = 1.0;
    }

    #[inline]
    fn safety(x: f32) -> f32 {
        let c = Self::CEILING;
        let knee = c * 0.9;
        if x.abs() <= knee {
            x
        } else {
            let over = (x.abs() - knee) / (c - knee);
            x.signum() * (knee + (c - knee) * soft_clip(over).min(1.0))
        }
    }

    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let look = self.buf_l.len();
        let mut max_reduction = 0.0f32;
        for (sl, sr) in l.iter_mut().zip(r.iter_mut()) {
            let peak = sl.abs().max(sr.abs());
            let target = if peak > Self::CEILING {
                Self::CEILING / peak
            } else {
                1.0
            };
            if target <= self.hold_gain {
                self.hold_gain = target;
                self.hold_left = look;
            } else if self.hold_left == 0 {
                self.hold_gain = target;
            } else {
                self.hold_left -= 1;
            }
            let coef = if self.hold_gain < self.env {
                self.attack
            } else {
                self.release
            };
            self.env += (self.hold_gain - self.env) * coef;
            let dl = self.buf_l[self.pos];
            let dr = self.buf_r[self.pos];
            self.buf_l[self.pos] = *sl;
            self.buf_r[self.pos] = *sr;
            self.pos = (self.pos + 1) % look;
            *sl = Self::safety(dl * self.env);
            *sr = Self::safety(dr * self.env);
            max_reduction = max_reduction.max(1.0 - self.env);
        }
        self.gain_reduction = max_reduction;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(freq: f32, sr: f32, n: usize) -> Vec<f32> {
        (0..n).map(|i| (TAU * freq * i as f32 / sr).sin()).collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    #[test]
    fn svf_lowpass_attenuates_highs() {
        let sr = 48_000.0;
        let c = SvfCoefs::new(500.0, 0.707, sr);
        let mut low = Svf::default();
        let mut high = Svf::default();
        let lo: Vec<f32> = tone(100.0, sr, 9600)
            .into_iter()
            .map(|x| low.low(&c, x))
            .collect();
        let hi: Vec<f32> = tone(8000.0, sr, 9600)
            .into_iter()
            .map(|x| high.low(&c, x))
            .collect();
        assert!(rms(&lo[4800..]) > 0.65);
        assert!(rms(&hi[4800..]) < 0.01);
    }

    #[test]
    fn limiter_holds_ceiling() {
        let sr = 48_000.0;
        let mut lim = Limiter::new(sr);
        let mut l: Vec<f32> = tone(220.0, sr, 4800).iter().map(|x| x * 3.0).collect();
        let mut r = l.clone();
        lim.process(&mut l, &mut r);
        assert!(l
            .iter()
            .chain(&r)
            .all(|x| x.abs() <= Limiter::CEILING + 1e-6));
        assert!(rms(&l[2400..]) > 0.5, "limited, not muted");
        // Quiet material passes untouched apart from the look-ahead delay.
        let mut lim = Limiter::new(sr);
        let src: Vec<f32> = tone(220.0, sr, 1000).iter().map(|x| x * 0.25).collect();
        let (mut l, mut r) = (src.clone(), src.clone());
        lim.process(&mut l, &mut r);
        let lat = lim.latency();
        for (i, &x) in l.iter().enumerate().skip(lat) {
            assert!((x - src[i - lat]).abs() < 1e-6);
        }
    }

    #[test]
    fn reverb_tail_decays_and_stays_finite() {
        let sr = 44_100.0;
        let mut rev = Reverb::new(sr);
        let mut l = vec![0.0f32; 512];
        let mut r = vec![0.0f32; 512];
        l[0] = 1.0;
        r[0] = 1.0;
        rev.process(&mut l, &mut r);
        let mut early = 0.0f32;
        let mut late = 0.0f32;
        let mut late_n = 0;
        for block in 0..((sr as usize * 6) / 512) {
            if block > 0 {
                l.fill(0.0);
                r.fill(0.0);
                rev.process(&mut l, &mut r);
            }
            assert!(l.iter().chain(&r).all(|x| x.is_finite()));
            let e = rms(&l) + rms(&r);
            if block < 40 {
                early = early.max(e);
            } else if block > 400 {
                late += e;
                late_n += 1;
            }
        }
        assert!(early > 0.001, "reverb responds (peak block rms {early})");
        assert!(
            late / (late_n.max(1) as f32) < early * 0.05,
            "reverb decays (early {early}, late {late})"
        );
        rev.clear();
        assert!(rev.is_quiet());
    }

    #[test]
    fn delay_echoes_at_set_time() {
        let sr = 48_000.0;
        let mut d = PingPongDelay::new(sr);
        d.set_time(0.25, true);
        let n = 48_000;
        let mut l = vec![0.0f32; n];
        let mut r = vec![0.0f32; n];
        l[0] = 1.0;
        r[0] = 1.0;
        d.process(&mut l, &mut r);
        let first = l.iter().position(|x| x.abs() > 0.05).unwrap();
        assert!(
            (first as i32 - 12_000).abs() < 4,
            "first echo on the left at 250 ms, got {first}"
        );
        let second = r.iter().position(|x| x.abs() > 0.02).unwrap();
        assert!(
            (second as i32 - 24_000).abs() < 8,
            "second echo hops right, got {second}"
        );
    }

    #[test]
    fn oscillators_are_bounded() {
        let mut p = 0.0;
        for _ in 0..10_000 {
            advance(&mut p, 0.013);
            assert!(saw(p, 0.013).abs() <= 1.01);
            assert!(square(p, 0.013).abs() <= 1.01);
        }
        let mut rng = Rng::new(7);
        for _ in 0..10_000 {
            let v = rng.noise();
            assert!((-1.0..1.0).contains(&v));
        }
        assert!(soft_clip(10.0) == 1.0 && soft_clip(-10.0) == -1.0);
        assert!((soft_clip(0.1) - 0.1f32.tanh()).abs() < 1e-3);
    }
}
