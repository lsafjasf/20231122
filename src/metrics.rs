//! Per-gap quality metrics.
//!
//! * `energy_diff_db` — log-ratio of concealed-gap energy to the
//!   original-gap energy. 0 dB is a perfect energy match; large negative
//!   values mean the concealment removed too much energy (e.g. silence).
//! * `artifact` — heuristic audible-artifact flag combining the energy
//!   mismatch with boundary discontinuities at the gap edges.

use crate::loss::Gap;
use crate::wav::Wav;

/// Energy-mismatch magnitude above which the fill is flagged.
pub const ENERGY_FLAG_DB: f32 = 3.0;
/// Edge discontinuity (relative to local RMS) above which we flag.
pub const DISCONTINUITY_FLAG: f32 = 2.5;

#[derive(Debug, Clone)]
pub struct GapMetrics {
    pub energy_diff_db: f32,
    pub edge_discontinuity: f32,
    pub artifact: bool,
}

fn gap_energy(wav: &Wav, c: usize, start: usize, end: usize) -> f32 {
    let ch = wav.spec.channels;
    let mut e = 0.0f32;
    for f in start..end {
        let s = wav.samples[f * ch + c];
        e += s * s;
    }
    e
}

/// Largest sample jump at the two gap boundaries, normalized by the RMS
/// of the surrounding 10 ms context.
fn edge_discontinuity(wav: &Wav, c: usize, start: usize, end: usize) -> f32 {
    let ch = wav.spec.channels;
    let ctx = (wav.spec.sample_rate as usize / 100).min(start).max(1);
    let mut e = 0.0f32;
    for f in (start - ctx)..start {
        let s = wav.samples[f * ch + c];
        e += s * s;
    }
    let rms = (e / ctx as f32).sqrt().max(1e-4);
    let mut worst = 0.0f32;
    if start > 0 {
        let jump = (wav.samples[start * ch + c] - wav.samples[(start - 1) * ch + c]).abs();
        worst = worst.max(jump / rms);
    }
    if end < wav.frames() {
        let jump = (wav.samples[end * ch + c] - wav.samples[(end - 1) * ch + c]).abs();
        worst = worst.max(jump / rms);
    }
    worst
}

/// Score a concealed gap against the original clean recording.
pub fn score(clean: &Wav, concealed: &Wav, gap: &Gap, frame_ms: usize) -> GapMetrics {
    let ch = concealed.spec.channels;
    let c = if ch > 1 { gap.channel.min(ch - 1) } else { 0 };
    let rate = concealed.spec.sample_rate as usize;
    let frame_len = rate * frame_ms / 1000;
    let start = gap.start_frame * frame_len;
    let end = ((gap.start_frame + gap.frames) * frame_len).min(concealed.frames());

    let e_ref = gap_energy(clean, c, start, end).max(1e-12);
    // Floor the concealed energy at -60 dB relative to the reference so
    // the log-ratio stays in a perceptually meaningful range instead of
    // running off to -inf for digital silence.
    let e_out = gap_energy(concealed, c, start, end).max((e_ref * 1e-6).max(1e-12));
    let energy_diff_db = 10.0 * (e_out / e_ref).log10();
    let disc = edge_discontinuity(concealed, c, start, end);
    let artifact = energy_diff_db.abs() > ENERGY_FLAG_DB || disc > DISCONTINUITY_FLAG;
    GapMetrics {
        energy_diff_db,
        edge_discontinuity: disc,
        artifact,
    }
}
