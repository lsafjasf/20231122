use serde::Serialize;

use crate::model::SampleResult;

#[derive(Serialize, Clone, Debug)]
pub struct Mismatch {
    pub seq: usize,
    pub before: SampleResult,
    pub after: SampleResult,
}

#[derive(Serialize, Clone, Debug)]
pub struct ComparisonReport {
    pub prefix_preserved: bool,
    pub pause_resume_equivalent: bool,
    pub compared_samples: usize,
    pub mismatch_count: usize,
    pub mismatches: Vec<Mismatch>,
}

pub fn compare_results(before: &[SampleResult], after: &[SampleResult]) -> ComparisonReport {
    let limit = before.len().min(after.len());
    let mut mismatches = Vec::new();
    for index in 0..limit {
        if before[index] != after[index] {
            mismatches.push(Mismatch {
                seq: index,
                before: before[index].clone(),
                after: after[index].clone(),
            });
        }
    }
    if before.len() != after.len() && before.len() > limit {
        for (index, sample) in before.iter().enumerate().skip(limit) {
            mismatches.push(Mismatch {
                seq: index,
                before: sample.clone(),
                after: SampleResult {
                    seq: index,
                    timestamp_ms: 0,
                    landed_timestamp_ms: 0,
                    x: 0,
                    y: 0,
                    raw_offset: 0,
                    effective_x: 0,
                    effective_y: 0,
                    scale_percent: 0,
                    effective_offset: 0,
                    rgb: [0, 0, 0],
                    frame_sha256: "missing-after".into(),
                    degraded_reason: None,
                },
            });
        }
    }
    if after.len() != before.len() && after.len() > limit {
        for (index, sample) in after.iter().enumerate().skip(limit) {
            mismatches.push(Mismatch {
                seq: index,
                before: SampleResult {
                    seq: index,
                    timestamp_ms: 0,
                    landed_timestamp_ms: 0,
                    x: 0,
                    y: 0,
                    raw_offset: 0,
                    effective_x: 0,
                    effective_y: 0,
                    scale_percent: 0,
                    effective_offset: 0,
                    rgb: [0, 0, 0],
                    frame_sha256: "missing-before".into(),
                    degraded_reason: None,
                },
                after: sample.clone(),
            });
        }
    }
    ComparisonReport {
        prefix_preserved: mismatches.is_empty(),
        pause_resume_equivalent: mismatches.is_empty() && before.len() == after.len(),
        compared_samples: limit,
        mismatch_count: mismatches.len(),
        mismatches,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(seq: usize, rgb: [u8; 3]) -> SampleResult {
        SampleResult {
            seq,
            timestamp_ms: seq as u64 * 100,
            landed_timestamp_ms: seq as u64 * 100,
            x: 0,
            y: 0,
            raw_offset: 0,
            effective_x: 0,
            effective_y: 0,
            scale_percent: 25,
            effective_offset: 0,
            rgb,
            frame_sha256: "frame".into(),
            degraded_reason: None,
        }
    }

    #[test]
    fn comparison_reports_the_first_changed_sample() {
        let report = compare_results(&[sample(0, [1, 2, 3])], &[sample(0, [1, 2, 4])]);
        assert!(!report.pause_resume_equivalent);
        assert_eq!(report.mismatches[0].seq, 0);
    }
}
