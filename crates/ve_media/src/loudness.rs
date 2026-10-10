//! Loudness as broadcasters measure it (ITU-R BS.1770-4, EBU R128):
//! momentary (400 ms), short-term (3 s) and gated integrated loudness in
//! LUFS, and true peak in dBTP from 4× oversampling.
//!
//! The signal is K-weighted (a high shelf for the head's effect, then a
//! high-pass), squared and averaged per channel; loudness is
//! `-0.691 + 10·log10(Σ mean squares)`. Integrated loudness averages 400 ms
//! blocks taken every 100 ms, ignoring blocks below -70 LUFS and then those
//! more than 10 LU under the mean of the rest.

use std::collections::VecDeque;

/// One reading. `-inf` where there is nothing to measure yet (silence).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Loudness {
    pub momentary: f32,
    pub short_term: f32,
    pub integrated: f32,
    /// The highest true peak since the meter was reset, dBTP.
    pub true_peak: f32,
}

impl Default for Loudness {
    fn default() -> Self {
        Loudness { momentary: f32::NEG_INFINITY, short_term: f32::NEG_INFINITY, integrated: f32::NEG_INFINITY, true_peak: f32::NEG_INFINITY }
    }
}

#[derive(Clone, Copy, Default)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    z: [f64; 2],
}

impl Biquad {
    fn run(&mut self, x: f64) -> f64 {
        // Transposed direct form II.
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

/// The two K-weighting stages for sample rate `fs` (BS.1770's filters,
/// re-derived for any rate the way libebur128 does).
fn k_weighting(fs: f64) -> [Biquad; 2] {
    use std::f64::consts::PI;
    // Stage 1: the high shelf.
    let (f0, gain_db, q) = (1681.974450955533, 3.999843853973347, 0.7071752369554196);
    let k = (PI * f0 / fs).tan();
    let vh = 10f64.powf(gain_db / 20.0);
    let vb = vh.powf(0.4996667741545416);
    let a0 = 1.0 + k / q + k * k;
    let shelf = Biquad {
        b: [(vh + vb * k / q + k * k) / a0, 2.0 * (k * k - vh) / a0, (vh - vb * k / q + k * k) / a0],
        a: [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
        z: [0.0; 2],
    };
    // Stage 2: the high-pass.
    let (f0, q) = (38.13547087602444, 0.5003270373238773);
    let k = (PI * f0 / fs).tan();
    let a0 = 1.0 + k / q + k * k;
    let hp = Biquad { b: [1.0, -2.0, 1.0], a: [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0], z: [0.0; 2] };
    [shelf, hp]
}

/// Taps per phase of the 4× true-peak interpolator.
const TP_TAPS: usize = 12;

/// A windowed-sinc 4× interpolator, as four phases of `TP_TAPS` taps.
fn true_peak_filter() -> [[f32; TP_TAPS]; 4] {
    let n = 4 * TP_TAPS;
    let mut phases = [[0f32; TP_TAPS]; 4];
    for i in 0..n {
        let x = (i as f64 - (n as f64 - 1.0) / 2.0) / 4.0;
        let sinc = if x.abs() < 1e-9 { 1.0 } else { (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x) };
        // Blackman window.
        let w = 0.42 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / (n as f64 - 1.0)).cos()
            + 0.08 * (4.0 * std::f64::consts::PI * i as f64 / (n as f64 - 1.0)).cos();
        phases[i % 4][i / 4] = (sinc * w) as f32;
    }
    // Each phase passes DC at unity, so a steady level reads as itself.
    for p in &mut phases {
        let sum: f32 = p.iter().sum();
        for t in p.iter_mut() {
            *t /= sum;
        }
    }
    phases
}

/// Measures interleaved stereo as it is pushed.
pub struct LoudnessMeter {
    filters: [[Biquad; 2]; 2],
    /// Samples per 100 ms sub-block, and the one being filled.
    sub_len: usize,
    sub_energy: f64,
    sub_count: usize,
    /// The last 30 sub-blocks' energies (3 s), newest last.
    subs: VecDeque<f64>,
    /// Every 400 ms block's energy since the reset, for integrated loudness.
    blocks: Vec<f64>,
    tp: [[f32; TP_TAPS]; 4],
    history: [[f32; TP_TAPS]; 2],
    head: usize,
    peak: f32,
}

fn lufs(energy: f64) -> f32 {
    if energy <= 0.0 {
        f32::NEG_INFINITY
    } else {
        (-0.691 + 10.0 * energy.log10()) as f32
    }
}

impl LoudnessMeter {
    pub fn new(rate: u32) -> Self {
        let k = k_weighting(rate as f64);
        LoudnessMeter {
            filters: [k, k],
            sub_len: (rate as usize / 10).max(1),
            sub_energy: 0.0,
            sub_count: 0,
            subs: VecDeque::with_capacity(30),
            blocks: Vec::new(),
            tp: true_peak_filter(),
            history: [[0.0; TP_TAPS]; 2],
            head: 0,
            peak: 0.0,
        }
    }

    /// Start a new measurement (integrated loudness and true peak too).
    pub fn reset(&mut self) {
        let rate = self.sub_len * 10;
        *self = LoudnessMeter::new(rate as u32);
    }

    pub fn push(&mut self, stereo: &[f32]) {
        for frame in stereo.as_chunks::<2>().0 {
            for (c, &x) in frame.iter().enumerate() {
                let [s1, s2] = &mut self.filters[c];
                let y = s2.run(s1.run(x as f64));
                self.sub_energy += y * y;
                self.history[c][self.head] = x;
            }
            // True peak: the four interpolated points behind this sample.
            for c in 0..2 {
                for phase in &self.tp {
                    let mut acc = 0f32;
                    for (k, h) in phase.iter().enumerate() {
                        acc += h * self.history[c][(self.head + TP_TAPS - k) % TP_TAPS];
                    }
                    self.peak = self.peak.max(acc.abs());
                }
            }
            self.head = (self.head + 1) % TP_TAPS;
            self.sub_count += 1;
            if self.sub_count == self.sub_len {
                self.close_sub_block();
            }
        }
    }

    fn close_sub_block(&mut self) {
        let e = self.sub_energy / self.sub_len as f64;
        if self.subs.len() == 30 {
            self.subs.pop_front();
        }
        self.subs.push_back(e);
        self.sub_energy = 0.0;
        self.sub_count = 0;
        if self.subs.len() >= 4 {
            let block = self.subs.iter().rev().take(4).sum::<f64>() / 4.0;
            self.blocks.push(block);
        }
    }

    fn mean_of_last(&self, n: usize) -> Option<f64> {
        (self.subs.len() >= n).then(|| self.subs.iter().rev().take(n).sum::<f64>() / n as f64)
    }

    pub fn read(&self) -> Loudness {
        let momentary = self.mean_of_last(4).map_or(f32::NEG_INFINITY, lufs);
        // Short-term over what there is until 3 s have passed.
        let short_term = self.mean_of_last(self.subs.len().clamp(4, 30)).map_or(f32::NEG_INFINITY, lufs);
        // Gated: absolute at -70 LUFS, then relative at -10 LU.
        let gate = |threshold: f64| {
            let kept: Vec<f64> = self.blocks.iter().copied().filter(|&e| e > threshold).collect();
            (!kept.is_empty()).then(|| kept.iter().sum::<f64>() / kept.len() as f64)
        };
        let to_energy = |l: f64| 10f64.powf((l + 0.691) / 10.0);
        let integrated = gate(to_energy(-70.0))
            .and_then(|mean| gate(to_energy(lufs(mean) as f64 - 10.0)))
            .map_or(f32::NEG_INFINITY, lufs);
        let true_peak = if self.peak > 0.0 { 20.0 * self.peak.log10() } else { f32::NEG_INFINITY };
        Loudness { momentary, short_term, integrated, true_peak }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, secs: f32, freq: f32, amp: f32, left_only: bool) -> Vec<f32> {
        let n = (rate as f32 * secs) as usize;
        (0..n)
            .flat_map(|i| {
                let v = amp * (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin();
                [v, if left_only { 0.0 } else { v }]
            })
            .collect()
    }

    #[test]
    fn a_1khz_sine_reads_as_bs1770_says() {
        // BS.1770's calibration: a 1 kHz sine at 0 dBFS in one channel is
        // -3.01 LUFS, so one at -20 dBFS is -23.01.
        for rate in [44_100, 48_000, 96_000] {
            let mut m = LoudnessMeter::new(rate);
            m.push(&sine(rate, 5.0, 1000.0, 0.1, true));
            let l = m.read();
            assert!((l.momentary + 23.01).abs() < 0.1, "{rate} Hz: momentary {}", l.momentary);
            assert!((l.short_term + 23.01).abs() < 0.1, "{rate} Hz: short-term {}", l.short_term);
            assert!((l.integrated + 23.01).abs() < 0.1, "{rate} Hz: integrated {}", l.integrated);
        }
    }

    #[test]
    fn both_channels_add_3_db() {
        let mut m = LoudnessMeter::new(48_000);
        m.push(&sine(48_000, 3.0, 1000.0, 0.1, false));
        assert!((m.read().momentary + 20.0).abs() < 0.1, "{}", m.read().momentary);
    }

    #[test]
    fn silence_is_gated_out_of_integrated_loudness() {
        let mut m = LoudnessMeter::new(48_000);
        m.push(&sine(48_000, 3.0, 1000.0, 0.1, true));
        m.push(&vec![0.0; 48_000 * 2 * 6]);
        let l = m.read();
        // The silent blocks are gated out; the three straddling the end
        // (3/4, 1/2 and 1/4 tone) count, as BS.1770 says: 28.5/30 of the
        // tone's energy.
        let expected = -23.01 + 10.0 * (28.5f32 / 30.0).log10();
        assert!((l.integrated - expected).abs() < 0.05, "integrated {} (expected {expected})", l.integrated);
        assert_eq!(l.momentary, f32::NEG_INFINITY);
    }

    #[test]
    fn true_peak_finds_peaks_between_samples() {
        // A sine at a quarter of the rate, phased so no sample lands on a
        // crest: samples read 0.707 of the true 1.0 peak.
        let rate = 48_000;
        let s: Vec<f32> = (0..4800)
            .flat_map(|i| {
                let v = (2.0 * std::f32::consts::PI * (i as f32 / 4.0) + std::f32::consts::FRAC_PI_4).sin();
                [v, v]
            })
            .collect();
        let sample_peak = s.iter().fold(0f32, |a, v| a.max(v.abs()));
        let mut m = LoudnessMeter::new(rate);
        m.push(&s);
        let tp = m.read().true_peak;
        assert!(sample_peak < 0.71, "{sample_peak}");
        assert!(tp > -0.5 && tp < 0.5, "true peak {tp} dBTP, sample peak {} dBFS", 20.0 * sample_peak.log10());
    }
}
