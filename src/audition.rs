//! Builders for the three audition signals: original seam, repaired loop,
//! and the difference between the naive join and the repaired loop.

/// ±`half_ms` around the naive wrap (file end -> file start).
pub fn original_seam(mono: &[f32], sr: u32, half_ms: f64) -> Vec<f32> {
    let half = (half_ms * 0.001 * sr as f64) as usize;
    let half = half.min(mono.len() / 2);
    let mut out = Vec::with_capacity(half * 2);
    out.extend_from_slice(&mono[mono.len() - half..]);
    out.extend_from_slice(&mono[..half]);
    out
}

/// The repaired loop repeated `iters` times.
pub fn processed_loop(loop_samples: &[f32], iters: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(loop_samples.len() * iters);
    for _ in 0..iters {
        out.extend_from_slice(loop_samples);
    }
    out
}

/// Difference between the naive seam and the repaired seam, aligned at the
/// wrap and normalized so residual artefacts are clearly audible.
pub fn difference(mono: &[f32], loop_samples: &[f32], sr: u32, half_ms: f64) -> Vec<f32> {
    let half = (half_ms * 0.001 * sr as f64) as usize;
    let half = half.min(mono.len() / 2).min(loop_samples.len() / 2);
    let naive = original_seam(mono, sr, half_ms);
    let mut repaired = Vec::with_capacity(half * 2);
    repaired.extend_from_slice(&loop_samples[loop_samples.len() - half..]);
    repaired.extend_from_slice(&loop_samples[..half]);
    let mut diff: Vec<f32> = naive
        .iter()
        .zip(repaired.iter())
        .map(|(a, b)| a - b)
        .collect();
    let peak = diff.iter().fold(0.0f32, |a, &b| a.max(b.abs()));
    if peak > 1e-6 {
        let g = 0.5 / peak;
        for v in &mut diff {
            *v *= g;
        }
    }
    diff
}
