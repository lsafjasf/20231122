//! Clock-skew estimation between two recording devices.
//!
//! Both media are decoded at the strict sample rate and summarized into
//! per-frame band vectors. Device B is slid against device A in
//! `search_step_us` increments over +/- `search_radius_us`; cosine similarity
//! of overlapping band vectors produces a confidence score in per mille. The
//! best offset is reported together with the number of compared frame pairs,
//! giving a reviewer enough data to challenge the estimate.

use crate::decode::{decode_window, DecodeRequest};
use crate::fingerprint::{compute, FingerprintParams};
use crate::model::{AlignmentPolicy, AlignmentResult, DeviceSpec};
use crate::FingerprintFrame as Frame;

#[derive(Debug, Clone)]
pub struct AlignmentInput {
    pub device: DeviceSpec,
    pub media_duration_us: i64,
}

pub fn estimate(
    anchor: &AlignmentInput,
    compared: &AlignmentInput,
    policy: &AlignmentPolicy,
) -> anyhow::Result<(AlignmentResult, Vec<Frame>, Vec<Frame>)> {
    let params = FingerprintParams::strict();
    let full_range = |duration_us: i64| crate::model::MicroRange {
        start_us: 0,
        end_us: duration_us,
    };
    let anchor_pcm = decode_window(&DecodeRequest {
        source_path: anchor.device.media_path.clone().into(),
        requested_range: full_range(anchor.media_duration_us),
        sample_rate_hz: params.sample_rate_hz,
    })?;
    let compared_pcm = decode_window(&DecodeRequest {
        source_path: compared.device.media_path.clone().into(),
        requested_range: full_range(compared.media_duration_us),
        sample_rate_hz: params.sample_rate_hz,
    })?;
    let anchor_frames = compute(&anchor_pcm.pcm_s16le, &params).frames;
    let compared_frames = compute(&compared_pcm.pcm_s16le, &params).frames;

    let step_frames = (policy.search_step_us / params.hop_us).max(1) as isize;
    let radius_frames = (policy.search_radius_us / params.hop_us) as isize;

    let mut best_offset_frames = 0isize;
    let mut best_score = -1f64;
    let mut best_pairs = 0u32;
    let mut offset = -radius_frames;
    while offset <= radius_frames {
        let (score, pairs) = cosine_overlap(&anchor_frames, &compared_frames, offset);
        if pairs > 0 && score > best_score {
            best_score = score;
            best_offset_frames = offset;
            best_pairs = pairs;
        }
        offset += step_frames;
    }

    let observed_offset_us = best_offset_frames as i64 * params.hop_us;
    let confidence = ((best_score.max(0.0) * 1000.0).round() as u32).min(1000);
    let status = if (observed_offset_us - policy.expected_offset_us).abs() <= policy.search_step_us {
        "estimated"
    } else {
        "estimated_unexpected"
    };

    let result = AlignmentResult {
        status: status.to_string(),
        anchor_device_id: anchor.device.device_id.clone(),
        compared_device_id: compared.device.device_id.clone(),
        expected_offset_us: policy.expected_offset_us,
        observed_offset_us,
        confidence_milli: confidence,
        search_radius_us: policy.search_radius_us,
        search_step_us: policy.search_step_us,
        samples_compared: best_pairs,
        evidence_sha256: String::new(),
    };
    Ok((result, anchor_frames, compared_frames))
}

fn cosine_overlap(anchor: &[Frame], compared: &[Frame], shift: isize) -> (f64, u32) {
    let mut dot = 0f64;
    let mut norm_a = 0f64;
    let mut norm_b = 0f64;
    let mut pairs = 0u32;
    let len_a = anchor.len() as isize;
    let len_b = compared.len() as isize;
    for index_b in 0..len_b {
        // A positive clock offset means device B is ahead: its content at
        // local index i corresponds to device A at i - offset_frames.
        let index_a = index_b - shift;
        if index_a < 0 || index_a >= len_a {
            continue;
        }
        let frame_a = &anchor[index_a as usize];
        let frame_b = &compared[index_b as usize];
        if frame_a.rms == 0 || frame_b.rms == 0 {
            continue;
        }
        for (a, b) in frame_a.bands.iter().zip(frame_b.bands.iter()) {
            let a = f64::from(*a);
            let b = f64::from(*b);
            dot += a * b;
            norm_a += a * a;
            norm_b += b * b;
        }
        pairs += 1;
    }
    if pairs == 0 || norm_a == 0.0 || norm_b == 0.0 {
        return (0.0, pairs);
    }
    (dot / (norm_a.sqrt() * norm_b.sqrt()), pairs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(value: u8, rms: u32) -> Frame {
        Frame {
            offset_us: 0,
            bands: vec![value; crate::fingerprint::BAND_COUNT],
            rms,
        }
    }

    #[test]
    fn identical_sequences_score_one_at_zero_shift() {
        let seq = vec![frame(100, 1000), frame(120, 1200), frame(140, 1400)];
        let (score, pairs) = cosine_overlap(&seq, &seq, 0);
        assert!((score - 1.0).abs() < 1e-9);
        assert_eq!(pairs, 3);
    }

    #[test]
    fn silence_frames_are_ignored() {
        let a = vec![frame(100, 1000), frame(0, 0)];
        let b = vec![frame(100, 1000), frame(0, 0)];
        let (_, pairs) = cosine_overlap(&a, &b, 0);
        assert_eq!(pairs, 1);
    }
}
