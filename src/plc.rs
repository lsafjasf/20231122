//! Concealment strategies applied to a single gap.
//!
//! All strategies operate on one channel of an interleaved signal and
//! fill `[start, end)` (frame indices on that channel's timeline).

use crate::loss::{Gap, LossManifest};
use crate::wav::Wav;
use crate::Strategy;

/// Fill `gap` in `wav` (in place) using `strategy`.
///
/// For stereo one-sided gaps, `Interpolate` falls back to copying the
/// intact sibling channel (level-matched) because that is far more
/// reliable than temporal interpolation.
pub fn conceal(wav: &mut Wav, manifest: &LossManifest, gap: &Gap, strategy: Strategy) {
    let ch = wav.spec.channels;
    let rate = wav.spec.sample_rate as usize;
    let frame_len = rate * manifest.frame_ms / 1000;
    let start = gap.start_frame * frame_len;
    let end = ((gap.start_frame + gap.frames) * frame_len).min(wav.frames());
    if start >= end {
        return;
    }
    let channels = manifest.affected_channels(gap, ch);
    // Decide per channel up front: a sibling is only "intact" if it has
    // energy in the *degraded* signal, before we conceal anything.
    let use_sibling: Vec<bool> = channels
        .iter()
        .map(|&c| {
            strategy == Strategy::Interpolate
                && ch > 1
                && has_intact_sibling(wav, gap, c, start, end)
        })
        .collect();
    for (c, &sibling) in channels.iter().zip(&use_sibling) {
        let c = *c;
        match strategy {
            Strategy::Silence => {
                for f in start..end {
                    wav.samples[f * ch + c] = 0.0;
                }
            }
            Strategy::RepeatLastFrame => repeat_last_frame(wav, c, start, end, frame_len),
            Strategy::Interpolate => {
                if sibling {
                    copy_sibling(wav, c, start, end);
                } else {
                    temporal_interpolate(wav, c, start, end);
                }
            }
        }
    }
}

fn repeat_last_frame(wav: &mut Wav, c: usize, start: usize, end: usize, frame_len: usize) {
    let ch = wav.spec.channels;
    let rate = wav.spec.sample_rate as usize;
    // Context: up to 12 frames (240 ms) before the gap, so the search
    // spans a full syllable cycle including its silent tail.
    let ctx = (12 * frame_len).min(start);
    if ctx == 0 {
        for f in start..end {
            wav.samples[f * ch + c] = 0.0;
        }
        return;
    }
    let context: Vec<f32> = (start - ctx..start)
        .map(|f| wav.samples[f * ch + c])
        .collect();
    // Find the most energetic frame inside the context; repeating a
    // syllable-final pause would fill the gap with silence.
    let best_off = (0..ctx.saturating_sub(frame_len))
        .step_by(frame_len)
        .max_by(|&a, &b| {
            let e = |o: usize| -> f32 {
                context[o..(o + frame_len).min(ctx)]
                    .iter()
                    .map(|s| s * s)
                    .sum()
            };
            e(a).partial_cmp(&e(b)).unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap_or(0);
    // Use the most energetic context frame as the repeat source; the
    // samples right before the gap may be a syllable-final pause.
    let seg_end = (best_off + frame_len).min(context.len());
    let segment = &context[..seg_end];
    // Estimate the pitch period of the segment via autocorrelation so the
    // repeat tiles phase-continuously (classic PLC behaviour).
    let min_lag = rate / 400; // 400 Hz max pitch
    let max_lag = (rate / 60).min(segment.len() / 2); // 60 Hz min pitch
    let seg_energy: f32 = segment.iter().map(|s| s * s).sum();
    let period = if seg_energy > 1e-6 && max_lag > min_lag {
        (min_lag..=max_lag)
            .max_by(|&lag_a, &lag_b| {
                let corr = |lag: usize| -> f32 {
                    segment[..segment.len() - lag]
                        .iter()
                        .zip(&segment[lag..])
                        .map(|(a, b)| a * b)
                        .sum()
                };
                corr(lag_a)
                    .partial_cmp(&corr(lag_b))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap_or(frame_len)
    } else {
        frame_len
    };
    // Tile the last `period` samples of the context with a gentle decay.
    let tile: Vec<f32> = segment[segment.len() - period..].to_vec();
    let decay = 0.985f32;
    let mut gain = 1.0f32;
    let mut i = 0usize;
    let n = end - start;
    while i < n {
        for (k, &p) in tile.iter().enumerate() {
            if i + k >= n {
                break;
            }
            wav.samples[(start + i + k) * ch + c] = p * gain;
        }
        gain *= decay;
        i += period;
    }
    // Short raised-cosine fade at both edges to smooth the joins.
    edge_fade(wav, c, start, end, 32);
}

fn temporal_interpolate(wav: &mut Wav, c: usize, start: usize, end: usize) {
    let ch = wav.spec.channels;
    let n = end - start;
    // Context windows on both sides of the gap.
    let ctx = n.min(160).min(start);
    let after_end = (end + ctx).min(wav.frames());
    let after_len = after_end.saturating_sub(end);
    let before: Vec<f32> = if ctx > 0 {
        (start - ctx..start)
            .map(|f| wav.samples[f * ch + c])
            .collect()
    } else {
        Vec::new()
    };
    let after: Vec<f32> = (end..end + after_len)
        .map(|f| wav.samples[f * ch + c])
        .collect();
    for i in 0..n {
        let t = (i + 1) as f32 / (n + 1) as f32;
        // Crossfade between reversed pre-context and forward post-context.
        let a = if ctx > 0 {
            before[ctx - 1 - (i % ctx)]
        } else {
            0.0
        };
        let b = if after_len > 0 {
            after[i % after_len]
        } else {
            0.0
        };
        // Equal-power crossfade weights.
        let wa = ((1.0 - t) * std::f32::consts::FRAC_PI_2).cos();
        let wb = (t * std::f32::consts::FRAC_PI_2).sin();
        wav.samples[(start + i) * ch + c] = a * wa + b * wb;
    }
    edge_fade(wav, c, start, end, 32);
}

fn has_intact_sibling(wav: &Wav, gap: &Gap, c: usize, start: usize, end: usize) -> bool {
    let ch = wav.spec.channels;
    if ch < 2 {
        return false;
    }
    let sibling = 1 - c.min(1);
    // Sibling is intact if the loss manifest only hit channel `c`; check
    // the sibling has non-trivial energy inside the gap.
    let mut energy = 0.0f32;
    for f in start..end {
        let s = wav.samples[f * ch + sibling];
        energy += s * s;
    }
    let _ = gap;
    energy / (end - start) as f32 > 1e-6
}

fn copy_sibling(wav: &mut Wav, c: usize, start: usize, end: usize) {
    let ch = wav.spec.channels;
    let sibling = 1 - c.min(1);
    // Level-match using 10 ms of context before the gap.
    let ctx = (wav.spec.sample_rate as usize / 100).min(start);
    let rms = |chan: usize| -> f32 {
        if ctx == 0 {
            return 1.0;
        }
        let mut e = 0.0f32;
        for f in (start - ctx)..start {
            let s = wav.samples[f * ch + chan];
            e += s * s;
        }
        (e / ctx as f32).sqrt()
    };
    let target = rms(c);
    let src = rms(sibling);
    let gain = if src > 1e-6 { target / src } else { 1.0 };
    for f in start..end {
        wav.samples[f * ch + c] = wav.samples[f * ch + sibling] * gain;
    }
    edge_fade(wav, c, start, end, 32);
}

/// Raised-cosine fade over `n` samples at both gap edges (in place).
fn edge_fade(wav: &mut Wav, c: usize, start: usize, end: usize, n: usize) {
    let ch = wav.spec.channels;
    let n = n.min((end - start) / 2);
    for i in 0..n {
        let w = 0.5 - 0.5 * (std::f32::consts::PI * i as f32 / n as f32).cos();
        // blend the concealed edge sample toward the neighbouring context
        if start > 0 {
            let prev = wav.samples[(start - 1) * ch + c];
            let cur = wav.samples[(start + i) * ch + c];
            wav.samples[(start + i) * ch + c] = prev * (1.0 - w) + cur * w;
        }
        if end < wav.frames() {
            let next = wav.samples[end * ch + c];
            let idx = end - 1 - i;
            let cur = wav.samples[idx * ch + c];
            wav.samples[idx * ch + c] = next * (1.0 - w) + cur * w;
        }
    }
}
