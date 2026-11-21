//! Deterministic speech-like test-signal synthesis.
//!
//! The signal mimics voiced speech: a slowly wandering pitch contour
//! with harmonics, amplitude-modulated syllable envelopes and pauses,
//! plus a little noise. Deterministic (seeded) so evaluations reproduce.

pub struct Synth {
    seed: u64,
}

impl Synth {
    pub fn new(seed: u64) -> Self {
        Synth { seed }
    }

    fn next_noise(&mut self) -> f32 {
        // xorshift64*
        self.seed ^= self.seed >> 12;
        self.seed ^= self.seed << 25;
        self.seed ^= self.seed >> 27;
        let v = self.seed.wrapping_mul(0x2545F4914F6CDD1D);
        ((v >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0
    }

    /// One talker's channel: `duration_s` seconds at `rate` Hz.
    /// `base_pitch` differentiates near/far end talkers.
    pub fn voice(&mut self, duration_s: f32, rate: u32, base_pitch: f32) -> Vec<f32> {
        let n = (duration_s * rate as f32) as usize;
        let mut out = vec![0.0f32; n];
        let mut phase = [0.0f32; 4]; // harmonic phases
        let harmonic_amp = [1.0f32, 0.5, 0.28, 0.12];
        let mut jitter = 0.0f32; // smoothed pitch random walk
        for i in 0..n {
            let t = i as f32 / rate as f32;
            // syllable gating: ~3.5 Hz syllable rate with pauses
            let syll = (t * 3.5).fract();
            let word = (t * 0.9).fract();
            let gate = if word > 0.82 {
                0.0 // pause between words
            } else if syll < 0.62 {
                (std::f32::consts::PI * syll / 0.62).sin()
            } else {
                0.0
            };
            // wandering pitch + smoothed random jitter (decorrelates the
            // signal from its distant context, like real speech)
            jitter = (jitter + 0.0012 * self.next_noise()).clamp(-0.05, 0.05);
            let pitch =
                base_pitch * (1.0 + 0.06 * (2.0 * std::f32::consts::PI * 0.7 * t).sin() + jitter);
            let mut s = 0.0;
            for h in 0..4 {
                phase[h] += 2.0 * std::f32::consts::PI * pitch * (h + 1) as f32 / rate as f32;
                s += harmonic_amp[h] * phase[h].sin();
            }
            s = s / 1.9 + 0.04 * self.next_noise();
            out[i] = 0.55 * gate * s;
        }
        out
    }
}
