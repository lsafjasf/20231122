use sha2::{Digest, Sha256};
use std::path::Path;

pub const SAMPLE_RATE: u32 = 48_000;

pub fn sha256_file(path: &Path) -> anyhow::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    std::io::copy(&mut f, &mut h)?;
    Ok(hex(&h.finalize()))
}

pub fn sha256_bytes(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn f32_to_bytes(samples: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * 4);
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

pub fn bytes_to_f32(data: &[u8]) -> Vec<f32> {
    data.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// Find the sample in `samples[center-radius..=center+radius]` whose value and
/// slope best match the value/slope at `anchor`. Deterministic: ties break to
/// the lowest index. `stride` > 1 is the degraded (coarse) search mode.
pub fn best_join_point(samples: &[f32], anchor: usize, center: usize, radius: usize, stride: usize) -> usize {
    let a_val = samples[anchor] as f64;
    let a_slope = samples[anchor + 1] as f64 - samples[anchor] as f64;
    let lo = center.saturating_sub(radius).max(1);
    let hi = (center + radius).min(samples.len() - 2);
    let mut best = lo;
    let mut best_err = f64::MAX;
    let mut i = lo;
    while i <= hi {
        let val = samples[i] as f64;
        let slope = samples[i + 1] as f64 - samples[i] as f64;
        let err = (val - a_val).abs() + 0.5 * (slope - a_slope).abs();
        if err < best_err {
            best_err = err;
            best = i;
        }
        i += stride.max(1);
    }
    best
}

/// Equal-power crossfade of the region [end-xf, end) with [start, start+xf).
/// Writes the blended result into `out`, which must have length `end - start`.
pub fn render_loop(samples: &[f32], start: usize, end: usize, xf: usize) -> Vec<f32> {
    let len = end - start;
    let mut out = Vec::with_capacity(len);
    out.extend_from_slice(&samples[start..end]);
    for i in 0..xf {
        let t = (i + 1) as f64 / (xf + 1) as f64;
        let angle = t * std::f64::consts::FRAC_PI_2;
        let (g_in, g_out) = (angle.sin() as f32, angle.cos() as f32);
        let tail_idx = len - xf + i;
        let head = samples[start + i];
        out[tail_idx] = out[tail_idx] * g_out + head * g_in;
    }
    out
}

/// Worst-case first-difference jump across the wrap boundary of a loop.
/// Compares the step out[len-1] -> out[0] against the local step statistics.
pub fn seam_metrics(out: &[f32]) -> (f64, f64) {
    if out.len() < 3 {
        return (0.0, 0.0);
    }
    let wrap_step = (out[0] as f64 - out[out.len() - 1] as f64).abs();
    let mut max_inner = 0.0f64;
    for w in out.windows(2) {
        let d = (w[1] as f64 - w[0] as f64).abs();
        if d > max_inner {
            max_inner = d;
        }
    }
    (wrap_step, max_inner)
}

/// Pre-render estimate of the residual seam error for a candidate join, used
/// as evidence in the A2 plan. Deterministic.
pub fn seam_error_estimate(samples: &[f32], start: usize, end: usize) -> f64 {
    let a = samples[end - 1] as f64;
    let b = samples[start] as f64;
    (b - a).abs()
}
