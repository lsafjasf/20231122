//! Fingerprint comparison and per-segment decisions.
//!
//! Frames are paired by their offset within the common clock window. A pair
//! is "comparable" only when neither side is silent, which is what prevents a
//! declared capture gap (silence/zero samples on one device) from being
//! mistaken for ordinary content divergence.

use crate::model::{
    AlignmentPolicy, Decision, DeviceSpec, FingerprintFrame, MissingEvidence, SegmentRecord,
    SegmentSpec,
};

#[derive(Debug, Clone)]
pub struct ComparisonInput<'a> {
    pub segment: &'a SegmentSpec,
    pub anchor_device: &'a DeviceSpec,
    pub compared_device: &'a DeviceSpec,
    pub anchor_frames: &'a [FingerprintFrame],
    pub compared_frames: &'a [FingerprintFrame],
    pub compared_clock_offset_us: i64,
}

#[derive(Debug, Clone)]
pub struct ComparisonOutcome {
    pub similarity_milli: u32,
    pub coverage_milli: u32,
    pub decision: Decision,
    pub missing: Option<MissingEvidence>,
    pub notes: Vec<String>,
}

pub fn compare(input: ComparisonInput<'_>, policy: &AlignmentPolicy) -> ComparisonOutcome {
    let total_frames = input.anchor_frames.len().max(input.compared_frames.len());
    let mut notes = Vec::new();

    let declared = find_overlapping_gap(input.compared_device, input.segment, input.compared_clock_offset_us);
    if declared.is_some() {
        notes.push("采集元数据声明缺口与比较窗口重叠，不能把静音误判为内容缺失".to_string());
    }

    let frame_count = input.anchor_frames.len().min(input.compared_frames.len());
    let mut score_sum = 0u64;
    let mut comparable = 0usize;
    for index in 0..frame_count {
        let anchor = &input.anchor_frames[index];
        let compared = &input.compared_frames[index];
        if anchor.rms == 0 || compared.rms == 0 {
            continue;
        }
        comparable += 1;
        score_sum += frame_similarity_milli(anchor, compared) as u64;
    }

    let coverage_milli = if total_frames == 0 {
        0
    } else {
        (comparable * 1000 / total_frames) as u32
    };
    let similarity_milli = if comparable == 0 {
        0
    } else {
        (score_sum / comparable as u64) as u32
    };

    let decision = if declared.is_some()
        && coverage_milli <= policy.missing_threshold_milli
    {
        notes.push(format!("可比较非静音帧覆盖率仅 {}‰", coverage_milli));
        Decision::Missing
    } else if similarity_milli >= policy.match_threshold_milli
        && coverage_milli >= policy.match_threshold_milli
    {
        notes.push("相似度与覆盖率均达到匹配阈值".to_string());
        Decision::Match
    } else if similarity_milli <= policy.missing_threshold_milli
        && coverage_milli <= policy.missing_threshold_milli
    {
        notes.push("两端均无有效可比较帧且无采集缺口声明，标记人工复核".to_string());
        Decision::Review
    } else {
        notes.push("相似度或覆盖率低于匹配阈值，判定为内容分歧".to_string());
        Decision::Divergent
    };

    let missing = if decision == Decision::Missing {
        declared
    } else {
        None
    };

    ComparisonOutcome {
        similarity_milli,
        coverage_milli,
        decision,
        missing,
        notes,
    }
}

fn frame_similarity_milli(anchor: &FingerprintFrame, compared: &FingerprintFrame) -> u32 {
    let mut dot = 0f64;
    let mut norm_a = 0f64;
    let mut norm_b = 0f64;
    for (a, b) in anchor.bands.iter().zip(compared.bands.iter()) {
        let a = f64::from(*a);
        let b = f64::from(*b);
        dot += a * b;
        norm_a += a * a;
        norm_b += b * b;
    }
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0;
    }
    ((dot / (norm_a.sqrt() * norm_b.sqrt())).clamp(0.0, 1.0) * 1000.0).round() as u32
}

fn find_overlapping_gap(
    device: &DeviceSpec,
    segment: &SegmentSpec,
    compared_clock_offset_us: i64,
) -> Option<MissingEvidence> {
    let common_start = segment.start_us;
    let common_end = segment.start_us + segment.duration_us;
    for gap in &device.declared_gaps {
        let gap_common_start = gap.start_us - compared_clock_offset_us;
        let gap_common_end = gap.end_us - compared_clock_offset_us;
        let overlap = common_end.min(gap_common_end) - common_start.max(gap_common_start);
        if overlap > 0 {
            return Some(MissingEvidence {
                gap_id: gap.gap_id.clone(),
                device_id: device.device_id.clone(),
                source_clock_range: crate::model::MicroRange {
                    start_us: gap.start_us,
                    end_us: gap.end_us,
                },
                common_clock_range: crate::model::MicroRange {
                    start_us: common_start,
                    end_us: common_end,
                },
                overlap_us: overlap,
                reason: gap.reason.clone(),
                declared_by_capture_metadata: true,
            });
        }
    }
    None
}

pub fn segment_digest(record: &SegmentRecord) -> String {
    crate::digest::digest_canonical(&serde_json::json!({
        "segment_id": record.segment_id,
        "decision": record.decision,
        "similarity_milli": record.similarity_milli,
        "coverage_milli": record.coverage_milli,
        "evidence_sha256": record.evidence_sha256,
        "devices": record.devices.iter().map(|device| {
            serde_json::json!({
                "device_id": device.device_id,
                "pcm_sha256": device.pcm_sha256,
                "fingerprint_sha256": device.fingerprint_sha256,
            })
        }).collect::<Vec<_>>(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(value: u8, rms: u32) -> FingerprintFrame {
        FingerprintFrame {
            offset_us: 0,
            bands: vec![value; crate::fingerprint::BAND_COUNT],
            rms,
        }
    }

    fn policy() -> AlignmentPolicy {
        AlignmentPolicy {
            search_radius_us: 4_000_000,
            search_step_us: 50_000,
            expected_offset_us: 1_350_000,
            match_threshold_milli: 820,
            missing_threshold_milli: 420,
        }
    }

    #[test]
    fn identical_frames_match() {
        let frames = vec![frame(120, 2000); 5];
        let segment = SegmentSpec {
            segment_id: "s".into(),
            ordinal: 0,
            start_us: 0,
            duration_us: 1_000_000,
            label: "t".into(),
        };
        let device = DeviceSpec {
            device_id: "b".into(),
            label: "b".into(),
            media_path: "x".into(),
            clock_epoch_unix_ms: 0,
            clock_drift_ppm: 0,
            declared_gaps: vec![],
        };
        let outcome = compare(
            ComparisonInput {
                segment: &segment,
                anchor_device: &device,
                compared_device: &device,
                anchor_frames: &frames,
                compared_frames: &frames,
                compared_clock_offset_us: 0,
            },
            &policy(),
        );
        assert_eq!(outcome.decision, Decision::Match);
        assert_eq!(outcome.similarity_milli, 1000);
        assert_eq!(outcome.coverage_milli, 1000);
    }

    #[test]
    fn silent_side_with_declared_gap_is_missing() {
        let anchor = vec![frame(120, 2000); 4];
        let compared = vec![frame(0, 0); 4];
        let segment = SegmentSpec {
            segment_id: "s".into(),
            ordinal: 0,
            start_us: 8_500_000,
            duration_us: 1_750_000,
            label: "t".into(),
        };
        let device = DeviceSpec {
            device_id: "b".into(),
            label: "b".into(),
            media_path: "x".into(),
            clock_epoch_unix_ms: 0,
            clock_drift_ppm: 0,
            declared_gaps: vec![crate::model::DeclaredGap {
                gap_id: "g1".into(),
                start_us: 9_850_000,
                end_us: 11_850_000,
                reason: "queue stall".into(),
            }],
        };
        let outcome = compare(
            ComparisonInput {
                segment: &segment,
                anchor_device: &device,
                compared_device: &device,
                anchor_frames: &anchor,
                compared_frames: &compared,
                compared_clock_offset_us: 1_350_000,
            },
            &policy(),
        );
        assert_eq!(outcome.decision, Decision::Missing);
        let evidence = outcome.missing.expect("missing evidence attached");
        assert_eq!(evidence.gap_id, "g1");
        assert_eq!(evidence.overlap_us, 1_750_000);
    }
}
