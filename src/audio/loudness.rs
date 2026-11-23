//! Integrated loudness per ITU-R BS.1770-4 / EBU R 128: K-weighting (a
//! high-shelf "head" filter plus the RLB high-pass), 400 ms blocks with 75 %
//! overlap, an absolute gate at -70 LUFS and a relative gate 10 LU below the
//! ungated mean. Used offline by WAV export normalization; pure Rust, f64
//! filter state.
//!
//! Memory grows by one `f64` per 100 ms of audio (a 10-minute song keeps
//! ~6000 values), so a whole song can be measured while it streams.

use std::f64::consts::PI;

/// Default target of the export normalizer (the common streaming level).
pub const DEFAULT_TARGET_LUFS: f32 = -14.0;
/// Absolute gate.
const ABSOLUTE_GATE_LUFS: f64 = -70.0;
/// Relative gate below the absolute-gated mean.
const RELATIVE_GATE_LU: f64 = 10.0;
/// Hop between gating blocks (blocks are four hops = 400 ms long).
const HOP_SECONDS: f64 = 0.1;
const HOPS_PER_BLOCK: usize = 4;

/// Transposed direct-form II biquad (normalized, `a0 = 1`).
#[derive(Clone, Copy, Debug, Default)]
struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}

impl Biquad {
    #[inline]
    fn process(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }

    /// Stage 1: the spherical-head high shelf (+4 dB above ~1.7 kHz).
    /// Coefficients from the BS.1770 analog prototype, re-derived for any
    /// sample rate (identical to the published 48 kHz values).
    fn head_shelf(sample_rate: f64) -> Self {
        let f0 = 1681.974450955533;
        let gain_db = 3.999843853973347;
        let q = 0.7071752369554196;
        let k = (PI * f0 / sample_rate).tan();
        let vh = 10f64.powf(gain_db / 20.0);
        let vb = vh.powf(0.4996667741545416);
        let a0 = 1.0 + k / q + k * k;
        Biquad {
            b0: (vh + vb * k / q + k * k) / a0,
            b1: 2.0 * (k * k - vh) / a0,
            b2: (vh - vb * k / q + k * k) / a0,
            a1: 2.0 * (k * k - 1.0) / a0,
            a2: (1.0 - k / q + k * k) / a0,
            ..Biquad::default()
        }
    }

    /// Stage 2: the revised low-frequency B-weighting (RLB) high-pass.
    fn rlb_highpass(sample_rate: f64) -> Self {
        let f0 = 38.13547087602444;
        let q = 0.5003270373238773;
        let k = (PI * f0 / sample_rate).tan();
        let a0 = 1.0 + k / q + k * k;
        Biquad {
            b0: 1.0,
            b1: -2.0,
            b2: 1.0,
            a1: 2.0 * (k * k - 1.0) / a0,
            a2: (1.0 - k / q + k * k) / a0,
            ..Biquad::default()
        }
    }
}

/// Streaming stereo integrated-loudness meter.
#[derive(Clone, Debug)]
pub struct LoudnessMeter {
    filters: [[Biquad; 2]; 2],
    hop_len: usize,
    hop_pos: usize,
    hop_sum: f64,
    /// Channel-summed mean square of every complete 100 ms hop.
    hops: Vec<f64>,
    /// Frames seen, for songs shorter than one gating block.
    frames: u64,
    total_sum: f64,
}

impl LoudnessMeter {
    pub fn new(sample_rate: u32) -> Self {
        let sr = f64::from(sample_rate.max(1));
        let chain = [Biquad::head_shelf(sr), Biquad::rlb_highpass(sr)];
        LoudnessMeter {
            filters: [chain, chain],
            hop_len: ((HOP_SECONDS * sr).round() as usize).max(1),
            hop_pos: 0,
            hop_sum: 0.0,
            hops: Vec::new(),
            frames: 0,
            total_sum: 0.0,
        }
    }

    /// Adds a block of stereo audio.
    pub fn push(&mut self, left: &[f32], right: &[f32]) {
        for (&l, &r) in left.iter().zip(right) {
            let mut energy = 0.0;
            for (ch, x) in [l, r].into_iter().enumerate() {
                let [shelf, hp] = &mut self.filters[ch];
                let y = hp.process(shelf.process(f64::from(x)));
                // L and R both carry weight 1.0 in BS.1770.
                energy += y * y;
            }
            self.hop_sum += energy;
            self.total_sum += energy;
            self.frames += 1;
            self.hop_pos += 1;
            if self.hop_pos == self.hop_len {
                self.hops.push(self.hop_sum / self.hop_len as f64);
                self.hop_pos = 0;
                self.hop_sum = 0.0;
            }
        }
    }

    fn lufs(power: f64) -> f64 {
        -0.691 + 10.0 * power.log10()
    }

    /// Gated integrated loudness in LUFS, `None` for silence (nothing
    /// above the absolute gate). Audio shorter than one 400 ms block is
    /// measured as a single block.
    pub fn integrated_lufs(&self) -> Option<f64> {
        let blocks: Vec<f64> = if self.hops.len() >= HOPS_PER_BLOCK {
            self.hops
                .windows(HOPS_PER_BLOCK)
                .map(|w| w.iter().sum::<f64>() / HOPS_PER_BLOCK as f64)
                .collect()
        } else if self.frames > 0 {
            vec![self.total_sum / self.frames as f64]
        } else {
            Vec::new()
        };
        let gated_mean = |floor: f64| {
            let (sum, count) = blocks
                .iter()
                .filter(|&&p| p > 0.0 && Self::lufs(p) > floor)
                .fold((0.0, 0usize), |(s, c), &p| (s + p, c + 1));
            (count > 0).then(|| sum / count as f64)
        };
        let ungated = gated_mean(ABSOLUTE_GATE_LUFS)?;
        let relative = Self::lufs(ungated) - RELATIVE_GATE_LU;
        gated_mean(relative.max(ABSOLUTE_GATE_LUFS)).map(Self::lufs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f64, amp: f64, sr: u32, seconds: f64) -> Vec<f32> {
        let n = (seconds * f64::from(sr)) as usize;
        (0..n)
            .map(|i| (amp * (2.0 * PI * freq * i as f64 / f64::from(sr)).sin()) as f32)
            .collect()
    }

    fn measure(l: &[f32], r: &[f32], sr: u32) -> Option<f64> {
        let mut m = LoudnessMeter::new(sr);
        for (a, b) in l.chunks(256).zip(r.chunks(256)) {
            m.push(a, b);
        }
        m.integrated_lufs()
    }

    #[test]
    fn k_weighting_matches_the_published_48k_coefficients() {
        let s = Biquad::head_shelf(48_000.0);
        assert!((s.b0 - 1.53512485958697).abs() < 1e-9);
        assert!((s.b1 + 2.69169618940638).abs() < 1e-9);
        assert!((s.b2 - 1.19839281085285).abs() < 1e-9);
        assert!((s.a1 + 1.69065929318241).abs() < 1e-9);
        assert!((s.a2 - 0.73248077421585).abs() < 1e-9);
        let h = Biquad::rlb_highpass(48_000.0);
        assert!((h.a1 + 1.99004745483398).abs() < 1e-9);
        assert!((h.a2 - 0.99007225036621).abs() < 1e-9);
    }

    #[test]
    fn known_sines_measure_their_reference_loudness() {
        // BS.1770: a 997 Hz sine at 0 dBFS peak in one channel reads
        // -3.01 LUFS; in both channels it reads 0.0 LUFS (+3 dB of power).
        for sr in [44_100, 48_000] {
            let full = sine(997.0, 1.0, sr, 5.0);
            let silent = vec![0.0f32; full.len()];
            let mono = measure(&full, &silent, sr).unwrap();
            assert!((mono + 3.01).abs() < 0.1, "{sr} Hz: one channel {mono}");
            let quiet = sine(997.0, 0.1, sr, 5.0);
            let both = measure(&quiet, &quiet, sr).unwrap();
            assert!((both + 20.0).abs() < 0.5, "{sr} Hz: -20 dBFS stereo {both}");
        }
    }

    #[test]
    fn gates_ignore_silence_and_quiet_passages() {
        let sr = 44_100;
        assert_eq!(measure(&[], &[], sr), None);
        let zeros = vec![0.0f32; 44_100];
        assert_eq!(measure(&zeros, &zeros, sr), None, "silence has no loudness");
        // Loud tone, then a long silence: the silence is gated out.
        let mut l = sine(997.0, 0.1, sr, 4.0);
        l.extend(std::iter::repeat_n(0.0, 8 * 44_100));
        let lufs = measure(&l, &l, sr).unwrap();
        assert!((lufs + 20.0).abs() < 0.5, "gated {lufs}");
        // A much quieter tail (> 10 LU down) is dropped by the relative gate.
        let mut l = sine(997.0, 0.1, sr, 4.0);
        l.extend(sine(997.0, 0.01, sr, 8.0));
        let lufs = measure(&l, &l, sr).unwrap();
        assert!((lufs + 20.0).abs() < 0.5, "relative gate {lufs}");
        // Shorter than one block still measures.
        let short = sine(997.0, 0.1, sr, 0.2);
        let lufs = measure(&short, &short, sr).unwrap();
        assert!((lufs + 20.0).abs() < 0.5, "short {lufs}");
    }
}
