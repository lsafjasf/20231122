//! Deterministic sample loops with deliberately flawed seams, used to
//! validate the analysis: a drum pattern (hard click at the wrap), a
//! sustained pad (low-frequency phase jump) and a plucked phrase with a
//! reverb tail (abrupt decay cut).

use crate::wav::WavData;

const SR: u32 = 44100;

fn finalize(mut x: Vec<f32>) -> WavData {
    let peak = x.iter().fold(0.0f32, |a, &b| a.max(b.abs())).max(1e-6);
    for v in &mut x {
        *v = (*v / peak * 0.85).clamp(-1.0, 1.0);
    }
    WavData {
        samples: x,
        channels: 1,
        sample_rate: SR,
    }
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1);
        ((self.0 >> 33) as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

fn kick(out: &mut [f32], at: usize) {
    let len = (0.22 * SR as f64) as usize;
    for i in 0..len {
        if at + i >= out.len() {
            break;
        }
        let t = i as f64 / SR as f64;
        let f = 140.0 * (-t * 18.0).exp() + 45.0;
        let phase = 2.0 * std::f64::consts::PI * f * t;
        let env = (-t * 14.0).exp();
        out[at + i] += (phase.sin() * env * 0.9) as f32;
    }
}

fn hat(out: &mut [f32], at: usize, rng: &mut Rng, decay: f64) {
    let len = (0.06 * SR as f64) as usize;
    let mut lp = 0.0f32;
    for i in 0..len {
        if at + i >= out.len() {
            break;
        }
        let t = i as f64 / SR as f64;
        let n = rng.next();
        lp += 0.35 * (n - lp); // crude high-pass by subtraction
        let hp = n - lp;
        out[at + i] += hp * (-t / decay).exp() as f32 * 0.35;
    }
}

fn snare(out: &mut [f32], at: usize, rng: &mut Rng) {
    let len = (0.16 * SR as f64) as usize;
    for i in 0..len {
        if at + i >= out.len() {
            break;
        }
        let t = i as f64 / SR as f64;
        let env = (-t * 22.0).exp();
        let tone = (2.0 * std::f64::consts::PI * 190.0 * t).sin() as f32 * 0.4;
        out[at + i] += (tone + rng.next() * 0.6) * env as f32 * 0.5;
    }
}

/// Two bars at 120 BPM. The file is truncated a few milliseconds early, in
/// the middle of the last hat decay, and a small DC step is baked into the
/// tail so the naive wrap clicks.
pub fn drums() -> WavData {
    let bpm = 120.0;
    let beat = 60.0 / bpm;
    let bar = 4.0 * beat;
    let len = (2.0 * bar * SR as f64) as usize;
    let mut x = vec![0.0f32; len];
    let mut rng = Rng(7);
    for bar_i in 0..2 {
        let base = (bar_i as f64 * bar * SR as f64) as usize;
        kick(&mut x, base);
        kick(&mut x, base + (2.0 * beat * SR as f64) as usize);
        kick(&mut x, base + (3.5 * beat * SR as f64) as usize);
        snare(&mut x, base + (beat * SR as f64) as usize, &mut rng);
        snare(&mut x, base + (3.0 * beat * SR as f64) as usize, &mut rng);
        for eighth in 0..8 {
            let at = base + (eighth as f64 * 0.5 * beat * SR as f64) as usize;
            hat(
                &mut x,
                at,
                &mut rng,
                if eighth % 2 == 0 { 0.05 } else { 0.02 },
            );
        }
    }
    // Truncate mid-decay and add a DC step near the end.
    let cut = len - (0.011 * SR as f64) as usize;
    x.truncate(cut);
    for v in x.iter_mut().rev().take(200) {
        *v += 0.05;
    }
    finalize(x)
}

/// Sustained detuned pad with a sub oscillator. The sub's phase does not
/// wrap at the file boundary, producing a low-frequency jump; the loop
/// length is also not an integer number of LFO cycles.
pub fn pad() -> WavData {
    let dur = 3.7f64;
    let len = (dur * SR as f64) as usize;
    let mut x = vec![0.0f32; len];
    let mut ph1 = 0.0f64;
    let mut ph2 = 0.0f64;
    let mut ph_sub = 0.0f64;
    for (i, v) in x.iter_mut().enumerate() {
        let t = i as f64 / SR as f64;
        let lfo = 1.0 + 0.25 * (2.0 * std::f64::consts::PI * 0.31 * t).sin();
        ph1 += 2.0 * std::f64::consts::PI * 220.0 / SR as f64;
        ph2 += 2.0 * std::f64::consts::PI * 220.9 / SR as f64;
        ph_sub += 2.0 * std::f64::consts::PI * 55.3 / SR as f64;
        let saw1 = 2.0 * (ph1 / (2.0 * std::f64::consts::PI)).fract() - 1.0;
        let saw2 = 2.0 * (ph2 / (2.0 * std::f64::consts::PI)).fract() - 1.0;
        let sub = ph_sub.sin();
        *v = ((saw1 + saw2) * 0.18 * lfo + sub * 0.30) as f32;
    }
    // Soften the attack so the defect is only at the wrap.
    for (i, v) in x.iter_mut().enumerate().take((0.05 * SR as f64) as usize) {
        *v *= i as f32 / (0.05 * SR as f64) as f32;
    }
    finalize(x)
}

/// Plucked phrase with an exponential reverb tail that is chopped before
/// it fully decays: the wrap loses ~18 dB in one sample.
pub fn reverb_tail() -> WavData {
    let dur = 4.4f64;
    let len = (dur * SR as f64) as usize;
    let mut dry = vec![0.0f32; len];
    let notes = [261.63, 329.63, 392.0, 523.25];
    for (n, &f) in notes.iter().enumerate() {
        let at = (n as f64 * 0.9 * SR as f64) as usize;
        let plen = (1.2 * SR as f64) as usize;
        for i in 0..plen {
            if at + i >= len {
                break;
            }
            let t = i as f64 / SR as f64;
            let env = (-t * 3.0).exp();
            dry[at + i] += ((2.0 * std::f64::consts::PI * f * t).sin() * env * 0.5) as f32;
        }
    }
    // Simple reverb: sparse exponentially-decaying echoes.
    let mut x = dry.clone();
    let mut rng = Rng(11);
    for tap in 0..40 {
        let delay = (tap as f64 * 0.017 * SR as f64) as usize;
        let gain = 0.35 * (-(tap as f64) * 0.09).exp() * (0.6 + 0.4 * rng.next() as f64);
        for i in 0..len - delay {
            x[i + delay] += dry[i] * gain as f32;
        }
    }
    // Chop the tail while it is still audible.
    x.truncate(len - (0.006 * SR as f64) as usize);
    finalize(x)
}

pub fn all() -> [(&'static str, WavData); 3] {
    [
        ("drums", drums()),
        ("pad", pad()),
        ("reverb_tail", reverb_tail()),
    ]
}
