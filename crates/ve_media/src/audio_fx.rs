//! Clip audio effects: the DSP behind the built-in `ve.audio.*` effects.
//!
//! A clip's effects run on its stereo signal in order, before its Volume
//! and Panner (as in Premiere, where those fixed effects come last). Each
//! effect keeps its state (filter memories, envelopes, delay lines) from
//! block to block while the clip plays on, and starts clean after a jump.
//! Parameters are read once per block at the block's start (blocks are a
//! few milliseconds), so they can be keyframed.
//!
//! The parameter lists (names, ranges, defaults) are declared next to the
//! other built-ins in `ve_plugin_host::intrinsic`; `ve_engine`'s tests check
//! the two agree.

use std::f32::consts::PI;

use ve_model::{Clip, Effect, EffectId, Value};
use ve_time::Time;

pub const GAIN: &str = "ve.audio.gain";
pub const EQ: &str = "ve.audio.eq";
pub const HIGH_PASS: &str = "ve.audio.highpass";
pub const LOW_PASS: &str = "ve.audio.lowpass";
pub const COMPRESSOR: &str = "ve.audio.compressor";
pub const LIMITER: &str = "ve.audio.limiter";
pub const GATE: &str = "ve.audio.gate";
pub const DELAY: &str = "ve.audio.delay";
pub const REVERB: &str = "ve.audio.reverb";
pub const CHANNELS: &str = "ve.audio.channels";

/// Every built-in audio effect's id.
pub const ALL: [&str; 10] = [GAIN, EQ, HIGH_PASS, LOW_PASS, COMPRESSOR, LIMITER, GATE, DELAY, REVERB, CHANNELS];

pub fn is_audio_effect(id: &str) -> bool {
    ALL.contains(&id)
}

fn db(x: f32) -> f32 {
    10f32.powf(x / 20.0)
}

fn to_db(x: f32) -> f32 {
    20.0 * x.max(1e-9).log10()
}

/// A one-pole smoothing coefficient for a time constant of `ms`.
fn coef(ms: f32, rate: f32) -> f32 {
    if ms <= 0.0 {
        0.0
    } else {
        (-1.0 / (ms * 0.001 * rate)).exp()
    }
}

/// The value of `name` on `e` at clip time `t`, or `default`.
fn get(e: &Effect, name: &str, t: Time, default: f32) -> f32 {
    match e.params.get(name).map(|p| p.value_at(t)) {
        Some(Value::Float(v)) => v as f32,
        Some(Value::Int(v)) => v as f32,
        Some(Value::Choice(v)) => v as f32,
        Some(Value::Bool(b)) => b as u8 as f32,
        _ => default,
    }
}

/// A biquad (RBJ cookbook), direct form I, one per channel.
#[derive(Clone, Copy, Default)]
struct Biquad {
    b: [f32; 3],
    a: [f32; 2],
    x: [[f32; 2]; 2],
    y: [[f32; 2]; 2],
}

#[derive(Clone, Copy)]
enum Shape {
    LowPass,
    HighPass,
    Peak(f32),
    LowShelf(f32),
    HighShelf(f32),
}

impl Biquad {
    /// Set the coefficients, keeping the memories (a keyframed cutoff glides).
    fn set(&mut self, shape: Shape, freq: f32, q: f32, rate: f32) {
        let f = freq.clamp(10.0, rate * 0.49);
        let w = 2.0 * PI * f / rate;
        let (sin, cos) = w.sin_cos();
        let alpha = sin / (2.0 * q.max(0.05));
        let (b, a) = match shape {
            Shape::LowPass => ([(1.0 - cos) / 2.0, 1.0 - cos, (1.0 - cos) / 2.0], [1.0 + alpha, -2.0 * cos, 1.0 - alpha]),
            Shape::HighPass => ([(1.0 + cos) / 2.0, -(1.0 + cos), (1.0 + cos) / 2.0], [1.0 + alpha, -2.0 * cos, 1.0 - alpha]),
            Shape::Peak(gain) => {
                let a = db(gain / 2.0);
                ([1.0 + alpha * a, -2.0 * cos, 1.0 - alpha * a], [1.0 + alpha / a, -2.0 * cos, 1.0 - alpha / a])
            }
            Shape::LowShelf(gain) | Shape::HighShelf(gain) => {
                let a = db(gain / 2.0);
                let s = 2.0 * a.sqrt() * alpha;
                if matches!(shape, Shape::LowShelf(_)) {
                    (
                        [a * ((a + 1.0) - (a - 1.0) * cos + s), 2.0 * a * ((a - 1.0) - (a + 1.0) * cos), a * ((a + 1.0) - (a - 1.0) * cos - s)],
                        [(a + 1.0) + (a - 1.0) * cos + s, -2.0 * ((a - 1.0) + (a + 1.0) * cos), (a + 1.0) + (a - 1.0) * cos - s],
                    )
                } else {
                    (
                        [a * ((a + 1.0) + (a - 1.0) * cos + s), -2.0 * a * ((a - 1.0) + (a + 1.0) * cos), a * ((a + 1.0) + (a - 1.0) * cos - s)],
                        [(a + 1.0) - (a - 1.0) * cos + s, 2.0 * ((a - 1.0) - (a + 1.0) * cos), (a + 1.0) - (a - 1.0) * cos - s],
                    )
                }
            }
        };
        self.b = [b[0] / a[0], b[1] / a[0], b[2] / a[0]];
        self.a = [a[1] / a[0], a[2] / a[0]];
    }

    fn run(&mut self, c: usize, x: f32) -> f32 {
        let [x1, x2] = self.x[c];
        let [y1, y2] = self.y[c];
        let y = self.b[0] * x + self.b[1] * x1 + self.b[2] * x2 - self.a[0] * y1 - self.a[1] * y2;
        // Denormals stall some CPUs on the quiet tail of a filter.
        let y = if y.abs() < 1e-20 { 0.0 } else { y };
        self.x[c] = [x, x1];
        self.y[c] = [y, y1];
        y
    }

    fn block(&mut self, l: &mut [f32], r: &mut [f32]) {
        for (a, b) in l.iter_mut().zip(r.iter_mut()) {
            *a = self.run(0, *a);
            *b = self.run(1, *b);
        }
    }
}

/// A comb filter with damping in its loop, and an allpass (Freeverb).
struct Comb {
    buf: Vec<f32>,
    i: usize,
    store: f32,
}

impl Comb {
    fn run(&mut self, x: f32, feedback: f32, damp: f32) -> f32 {
        let y = self.buf[self.i];
        self.store = y * (1.0 - damp) + self.store * damp;
        self.buf[self.i] = x + self.store * feedback;
        self.i = (self.i + 1) % self.buf.len();
        y
    }
}

struct Allpass {
    buf: Vec<f32>,
    i: usize,
}

impl Allpass {
    fn run(&mut self, x: f32) -> f32 {
        let b = self.buf[self.i];
        self.buf[self.i] = x + b * 0.5;
        self.i = (self.i + 1) % self.buf.len();
        b - x
    }
}

/// Freeverb's tunings (at 44.1 kHz), scaled to the mix rate; the right
/// channel's lines are 23 samples longer, for width.
struct Reverb {
    combs: [Vec<Comb>; 2],
    allpasses: [Vec<Allpass>; 2],
}

impl Reverb {
    fn new(rate: f32) -> Self {
        let scale = rate / 44_100.0;
        let len = |n: usize, side: usize| (((n + side * 23) as f32 * scale) as usize).max(1);
        let combs = |side| [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617].map(|n| Comb { buf: vec![0.0; len(n, side)], i: 0, store: 0.0 }).into();
        let allpasses = |side| [556, 441, 341, 225].map(|n| Allpass { buf: vec![0.0; len(n, side)], i: 0 }).into();
        Reverb { combs: [combs(0), combs(1)], allpasses: [allpasses(0), allpasses(1)] }
    }
}

/// One effect's state.
enum Fx {
    Gain,
    Eq([Biquad; 3]),
    Filter(Biquad),
    /// Dynamics: the envelope (linear, smoothed gain reduction for the
    /// gate).
    Compressor(f32),
    Limiter(f32),
    Gate(f32),
    Delay { buf: [Vec<f32>; 2], i: usize },
    Reverb(Box<Reverb>),
    Channels,
}

impl Fx {
    fn new(id: &str, rate: f32) -> Option<Fx> {
        Some(match id {
            GAIN => Fx::Gain,
            EQ => Fx::Eq([Biquad::default(); 3]),
            HIGH_PASS | LOW_PASS => Fx::Filter(Biquad::default()),
            COMPRESSOR => Fx::Compressor(0.0),
            LIMITER => Fx::Limiter(1.0),
            GATE => Fx::Gate(0.0),
            // Up to two seconds.
            DELAY => Fx::Delay { buf: [vec![0.0; (rate * 2.0) as usize + 1], vec![0.0; (rate * 2.0) as usize + 1]], i: 0 },
            REVERB => Fx::Reverb(Box::new(Reverb::new(rate))),
            CHANNELS => Fx::Channels,
            _ => return None,
        })
    }

    fn run(&mut self, e: &Effect, t: Time, rate: f32, l: &mut [f32], r: &mut [f32]) {
        let p = |name: &str, d: f32| get(e, name, t, d);
        match self {
            Fx::Gain => {
                let g = db(p("gain", 0.0));
                l.iter_mut().chain(r.iter_mut()).for_each(|x| *x *= g);
            }
            Fx::Eq(bands) => {
                bands[0].set(Shape::LowShelf(p("low_gain", 0.0)), p("low_freq", 100.0), 0.707, rate);
                bands[1].set(Shape::Peak(p("mid_gain", 0.0)), p("mid_freq", 1000.0), p("mid_q", 1.0), rate);
                bands[2].set(Shape::HighShelf(p("high_gain", 0.0)), p("high_freq", 8000.0), 0.707, rate);
                for b in bands.iter_mut() {
                    b.block(l, r);
                }
            }
            Fx::Filter(b) => {
                let shape = if e.plugin.id == HIGH_PASS { Shape::HighPass } else { Shape::LowPass };
                b.set(shape, p("cutoff", 1000.0), 0.707, rate);
                b.block(l, r);
            }
            Fx::Compressor(env) => {
                // Feed-forward, on the louder channel, in dB.
                let (threshold, ratio) = (p("threshold", -18.0), p("ratio", 4.0).max(1.0));
                let (att, rel, makeup) = (coef(p("attack", 10.0), rate), coef(p("release", 150.0), rate), db(p("makeup", 0.0)));
                for (a, b) in l.iter_mut().zip(r.iter_mut()) {
                    let level = to_db(a.abs().max(b.abs()));
                    let over = (level - threshold).max(0.0);
                    let want = over * (1.0 - 1.0 / ratio); // dB of reduction
                    let k = if want > *env { att } else { rel };
                    *env = want + (*env - want) * k;
                    let g = db(-*env) * makeup;
                    *a *= g;
                    *b *= g;
                }
            }
            Fx::Limiter(gain) => {
                // Instant attack (never over the ceiling), smooth release.
                let ceiling = db(p("ceiling", -1.0));
                let rel = coef(p("release", 50.0), rate);
                for (a, b) in l.iter_mut().zip(r.iter_mut()) {
                    let peak = a.abs().max(b.abs());
                    let want = if peak > ceiling { ceiling / peak } else { 1.0 };
                    *gain = if want < *gain { want } else { want + (*gain - want) * rel };
                    *a *= *gain;
                    *b *= *gain;
                }
            }
            Fx::Gate(open) => {
                let threshold = db(p("threshold", -50.0));
                let (att, rel) = (coef(p("attack", 1.0), rate), coef(p("release", 100.0), rate));
                for (a, b) in l.iter_mut().zip(r.iter_mut()) {
                    let want = if a.abs().max(b.abs()) >= threshold { 1.0 } else { 0.0 };
                    let k = if want > *open { att } else { rel };
                    *open = want + (*open - want) * k;
                    *a *= *open;
                    *b *= *open;
                }
            }
            Fx::Delay { buf, i } => {
                let n = buf[0].len();
                let d = ((p("time", 250.0) * 0.001 * rate) as usize).clamp(1, n - 1);
                let (fb, mix) = ((p("feedback", 30.0) / 100.0).clamp(0.0, 0.95), (p("mix", 25.0) / 100.0).clamp(0.0, 1.0));
                for (a, b) in l.iter_mut().zip(r.iter_mut()) {
                    let at = (*i + n - d) % n;
                    for (c, x) in [a, b].into_iter().enumerate() {
                        let wet = buf[c][at];
                        buf[c][*i] = *x + wet * fb;
                        *x = *x * (1.0 - mix) + wet * mix;
                    }
                    *i = (*i + 1) % n;
                }
            }
            Fx::Reverb(rv) => {
                let room = 0.7 + 0.28 * (p("size", 50.0) / 100.0).clamp(0.0, 1.0);
                let damp = 0.4 * (p("damping", 50.0) / 100.0).clamp(0.0, 1.0);
                let mix = (p("mix", 25.0) / 100.0).clamp(0.0, 1.0);
                for (a, b) in l.iter_mut().zip(r.iter_mut()) {
                    let input = (*a + *b) * 0.015;
                    let mut wet = [0.0f32; 2];
                    for (c, w) in wet.iter_mut().enumerate() {
                        let mut s: f32 = rv.combs[c].iter_mut().map(|cb| cb.run(input, room, damp)).sum();
                        for ap in rv.allpasses[c].iter_mut() {
                            s = ap.run(s);
                        }
                        *w = s;
                    }
                    *a = *a * (1.0 - mix) + wet[0] * mix * 3.0;
                    *b = *b * (1.0 - mix) + wet[1] * mix * 3.0;
                }
            }
            Fx::Channels => match p("mode", 0.0) as u32 {
                0 => l.swap_with_slice(r),
                1 => l.copy_from_slice(r),
                2 => r.copy_from_slice(l),
                3 => l.iter_mut().chain(r.iter_mut()).for_each(|x| *x = -*x),
                _ => {
                    for (a, b) in l.iter_mut().zip(r.iter_mut()) {
                        let m = (*a + *b) * 0.5;
                        (*a, *b) = (m, m);
                    }
                }
            },
        }
    }
}

/// A clip's audio effects, with their state.
#[derive(Default)]
pub(crate) struct Chain {
    slots: Vec<(EffectId, String, Fx)>,
}

impl Chain {
    /// Whether `clip` has any audio effect to run.
    pub(crate) fn wanted(clip: &Clip) -> bool {
        clip.effects.iter().any(|e| e.enabled && is_audio_effect(&e.plugin.id))
    }

    /// Forget all state: the clip jumped.
    pub(crate) fn reset(&mut self) {
        self.slots.clear();
    }

    /// Run `clip`'s enabled audio effects, in order, over a block starting
    /// at clip time `t`.
    pub(crate) fn process(&mut self, clip: &Clip, t: Time, rate: u32, l: &mut [f32], r: &mut [f32]) {
        let rate = rate as f32;
        let mut slots = std::mem::take(&mut self.slots);
        for e in clip.effects.iter().filter(|e| e.enabled && is_audio_effect(&e.plugin.id)) {
            // State follows the effect, wherever it moves in the stack.
            let fx = match slots.iter().position(|(id, kind, _)| *id == e.id && *kind == e.plugin.id) {
                Some(i) => slots.swap_remove(i).2,
                None => match Fx::new(&e.plugin.id, rate) {
                    Some(fx) => fx,
                    None => continue,
                },
            };
            self.slots.push((e.id, e.plugin.id.clone(), fx));
            let fx = &mut self.slots.last_mut().unwrap().2;
            fx.run(e, t, rate, l, r);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ve_model::*;

    const RATE: u32 = 48_000;

    fn effect(id: &str, params: &[(&str, f64)]) -> Effect {
        let params = params.iter().map(|(k, v)| (k.to_string(), Param::Constant(Value::Float(*v)))).collect();
        Effect { id: EffectId::new(), plugin: PluginRef { api: PluginApi::Builtin, id: id.into(), major_version: 1 }, enabled: true, params }
    }

    fn test_clip() -> Clip {
        Clip {
            id: ClipId::new(),
            name: "x".into(),
            source: ClipSource::Asset { asset: AssetId::new(), audio_stream: 0 },
            source_range: ve_time::TimeRange::new(Time::ZERO, Time::from_seconds(10)),
            timeline_start: Time::ZERO,
            enabled: true,
            link: None,
            effects: Default::default(),
            retime: Default::default(),
            transition_in: None,
            transition_out: None,
            channels: Vec::new(),
        }
    }

    fn sine(freq: f32, n: usize, amp: f32) -> Vec<f32> {
        (0..n).map(|i| (2.0 * PI * freq * i as f32 / RATE as f32).sin() * amp).collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    /// Run `fx` over a sine in blocks of 512, return the last half's RMS (settled).
    fn through(fx: &Effect, freq: f32, amp: f32) -> f32 {
        let mut clip = test_clip();
        clip.effects = vec![std::sync::Arc::new(fx.clone())].into();
        let mut chain = Chain::default();
        let (mut l, mut r) = (sine(freq, RATE as usize / 2, amp), sine(freq, RATE as usize / 2, amp));
        for (a, b) in l.chunks_mut(512).zip(r.chunks_mut(512)) {
            chain.process(&clip, Time::ZERO, RATE, a, b);
        }
        rms(&l[l.len() / 2..])
    }

    #[test]
    fn gain_scales_by_decibels() {
        let x = through(&effect(GAIN, &[("gain", -6.0)]), 1000.0, 0.5);
        assert!((x / (0.5 / 2f32.sqrt()) - 0.501).abs() < 0.01, "{x}");
    }

    #[test]
    fn filters_pass_and_stop() {
        let hp = effect(HIGH_PASS, &[("cutoff", 1000.0)]);
        assert!(through(&hp, 100.0, 1.0) < 0.02, "100 Hz is cut by a 1 kHz high-pass");
        assert!(through(&hp, 8000.0, 1.0) > 0.68, "8 kHz passes");
        let lp = effect(LOW_PASS, &[("cutoff", 1000.0)]);
        assert!(through(&lp, 8000.0, 1.0) < 0.02);
        assert!(through(&lp, 100.0, 1.0) > 0.68);
    }

    #[test]
    fn eq_boosts_its_band() {
        let eq = effect(EQ, &[("mid_freq", 1000.0), ("mid_gain", 12.0), ("mid_q", 1.0)]);
        let boosted = through(&eq, 1000.0, 0.1);
        assert!((to_db(boosted / (0.1 / 2f32.sqrt())) - 12.0).abs() < 0.5, "{}", to_db(boosted / 0.0707));
        let far = through(&eq, 60.0, 0.1);
        assert!(to_db(far / (0.1 / 2f32.sqrt())).abs() < 1.5);
    }

    #[test]
    fn the_limiter_holds_the_ceiling_and_the_compressor_reduces() {
        let lim = effect(LIMITER, &[("ceiling", -6.0)]);
        let mut clip = test_clip();
        clip.effects = vec![std::sync::Arc::new(lim)].into();
        let (mut l, mut r) = (sine(440.0, 4800, 1.0), sine(440.0, 4800, 1.0));
        Chain::default().process(&clip, Time::ZERO, RATE, &mut l, &mut r);
        let peak = l.iter().fold(0f32, |m, x| m.max(x.abs()));
        assert!(peak <= db(-6.0) + 1e-4, "{peak}");
        let comp = effect(COMPRESSOR, &[("threshold", -20.0), ("ratio", 4.0), ("attack", 1.0), ("release", 50.0)]);
        let loud = through(&comp, 440.0, 1.0);
        assert!(to_db(loud / 0.707) < -10.0, "0 dBFS through 4:1 at -20 dB: about 15 dB less ({})", to_db(loud / 0.707));
        let quiet = through(&comp, 440.0, 0.01);
        assert!((quiet / (0.01 / 2f32.sqrt()) - 1.0).abs() < 0.05, "below the threshold: untouched");
    }

    #[test]
    fn the_gate_closes_on_quiet_and_reverb_and_delay_ring_on() {
        assert!(through(&effect(GATE, &[("threshold", -30.0)]), 440.0, 0.001) < 1e-4);
        assert!(through(&effect(GATE, &[("threshold", -30.0)]), 440.0, 0.5) > 0.3);
        // A click, then silence: the tail.
        for fx in [effect(REVERB, &[("mix", 50.0)]), effect(DELAY, &[("time", 100.0), ("mix", 50.0)])] {
            let mut clip = test_clip();
            clip.effects = vec![std::sync::Arc::new(fx)].into();
            let mut chain = Chain::default();
            let (mut l, mut r) = (vec![0.0; 24_000], vec![0.0; 24_000]);
            l[0] = 1.0;
            r[0] = 1.0;
            for (a, b) in l.chunks_mut(512).zip(r.chunks_mut(512)) {
                chain.process(&clip, Time::ZERO, RATE, a, b);
            }
            assert!(rms(&l[6000..12_000]) > 1e-4, "{}: it rings on after the click", clip.effects[0].plugin.id);
        }
    }

    #[test]
    fn channels_swap_and_fill() {
        let mut clip = test_clip();
        let mut fx = effect(CHANNELS, &[]);
        fx.params.insert("mode".into(), Param::Constant(Value::Choice(0)));
        clip.effects = vec![std::sync::Arc::new(fx)].into();
        let (mut l, mut r) = (vec![1.0; 4], vec![2.0; 4]);
        Chain::default().process(&clip, Time::ZERO, RATE, &mut l, &mut r);
        assert_eq!((l[0], r[0]), (2.0, 1.0));
    }
}
