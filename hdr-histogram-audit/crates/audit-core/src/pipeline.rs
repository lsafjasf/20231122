//! Deterministic HDR luminance processing pipeline.
//!
//! The pipeline is intentionally pure: given the same configuration and raw
//! samples it always produces the same journal, which is what makes a replay
//! verifiable. Each step records its input, output, parameters and evidence so
//! the GTK4 page can reconstruct why a sample ended up where it did.

use crate::model::{
    Evidence, Histogram, ProcessingStep, RawSample, SampleRecord, StepState, Verdict,
};
use serde::{Deserialize, Serialize};

/// Configuration of the audit pipeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PipelineConfig {
    /// Reference peak luminance in nits, mapped to headroom `1.0`.
    pub headroom_nits: f64,
    /// Number of histogram bins over the closed range `[0.0, 1.0]`.
    pub bin_count: usize,
    /// Absolute tolerance in headroom units around a bin edge. Values inside
    /// this band cannot be assigned to either neighbour and are reported as
    /// [`StepState::Indeterminate`].
    pub edge_tolerance: f64,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            headroom_nits: 1000.0,
            bin_count: 16,
            edge_tolerance: 1e-4,
        }
    }
}

/// Outcome of running the full pipeline over a batch of samples.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PipelineRun {
    pub config: PipelineConfig,
    pub records: Vec<SampleRecord>,
    pub histogram: Histogram,
}

fn evidence(sample: &RawSample, observation: impl Into<String>, facts: serde_json::Value) -> Evidence {
    Evidence {
        observation: observation.into(),
        facts,
        source_sample_id: sample.sample_id.clone(),
    }
}

/// Runs the deterministic pipeline and journals every step.
pub fn run(samples: &[RawSample], config: &PipelineConfig) -> PipelineRun {
    let width = 1.0 / config.bin_count as f64;
    let bin_edges: Vec<f64> = (0..=config.bin_count).map(|i| i as f64 * width).collect();
    let mut counts = vec![0u64; config.bin_count];
    let mut records = Vec::with_capacity(samples.len());

    for sample in samples {
        let record = process_one(sample, config, &bin_edges, &mut counts);
        records.push(record);
    }

    PipelineRun {
        config: config.clone(),
        records,
        histogram: Histogram { bin_edges, counts },
    }
}

fn process_one(
    sample: &RawSample,
    config: &PipelineConfig,
    bin_edges: &[f64],
    counts: &mut [u64],
) -> SampleRecord {
    let mut steps = Vec::new();
    let mut value = sample.nits;

    // Step 0: validate the physical reading.
    if !sample.nits.is_finite() || sample.nits < 0.0 {
        steps.push(ProcessingStep {
            index: 0,
            name: "validate".to_string(),
            label: "Validate luminance reading".to_string(),
            state: StepState::Error,
            input_value: value,
            output_value: value,
            params: serde_json::json!({"requires": "finite and >= 0 nits"}),
            evidence: Some(evidence(
                sample,
                "Non-finite or negative luminance cannot enter the pipeline",
                serde_json::json!({"nits": sample.nits, "finite": sample.nits.is_finite()}),
            )),
        });
        return SampleRecord { raw: sample.clone(), steps, verdict: Verdict::Error };
    }
    steps.push(ProcessingStep {
        index: 0,
        name: "validate".to_string(),
        label: "Validate luminance reading".to_string(),
        state: StepState::Ok,
        input_value: value,
        output_value: value,
        params: serde_json::json!({"requires": "finite and >= 0 nits"}),
        evidence: None,
    });

    // Step 1: normalize nits to headroom units.
    let normalized = sample.nits / config.headroom_nits;
    steps.push(ProcessingStep {
        index: 1,
        name: "normalize".to_string(),
        label: "Normalize to headroom".to_string(),
        state: StepState::Ok,
        input_value: value,
        output_value: normalized,
        params: serde_json::json!({"headroom_nits": config.headroom_nits}),
        evidence: None,
    });
    value = normalized;

    // Step 2: clip at the headroom, journaling the truncation explicitly.
    if normalized > 1.0 {
        steps.push(ProcessingStep {
            index: 2,
            name: "clip_headroom".to_string(),
            label: "Clip at reference headroom".to_string(),
            state: StepState::Truncated,
            input_value: normalized,
            output_value: 1.0,
            params: serde_json::json!({"ceiling": 1.0}),
            evidence: Some(evidence(
                sample,
                "Luminance exceeded the reference headroom and was clipped",
                serde_json::json!({
                    "normalized": normalized,
                    "over_by": normalized - 1.0,
                    "nits": sample.nits,
                    "headroom_nits": config.headroom_nits,
                }),
            )),
        });
        return SampleRecord { raw: sample.clone(), steps, verdict: Verdict::Truncated };
    }
    steps.push(ProcessingStep {
        index: 2,
        name: "clip_headroom".to_string(),
        label: "Clip at reference headroom".to_string(),
        state: StepState::Ok,
        input_value: normalized,
        output_value: normalized,
        params: serde_json::json!({"ceiling": 1.0}),
        evidence: None,
    });

    // Step 3: detect an undecidable bin-edge boundary before assigning a bin.
    if let Some(bin) = nearest_boundary_bin(value, bin_edges, config.edge_tolerance) {
        let edge = bin_edges[bin + 1];
        steps.push(ProcessingStep {
            index: 3,
            name: "edge_guard".to_string(),
            label: "Guard ambiguous bin edge".to_string(),
            state: StepState::Indeterminate,
            input_value: value,
            output_value: value,
            params: serde_json::json!({"edge_tolerance": config.edge_tolerance, "near_edge": edge}),
            evidence: Some(evidence(
                sample,
                "Value sits inside the tolerance band of a bin edge and cannot be decided",
                serde_json::json!({
                    "normalized": value,
                    "edge": edge,
                    "distance": (value - edge).abs(),
                    "tolerance": config.edge_tolerance,
                    "left_bin": bin,
                    "right_bin": bin + 1,
                }),
            )),
        });
        return SampleRecord {
            raw: sample.clone(),
            steps,
            verdict: Verdict::Indeterminate,
        };
    }

    // Step 4: assign the bin.
    let bin = assign_bin(value, config.bin_count);
    steps.push(ProcessingStep {
        index: 4,
        name: "assign_bin".to_string(),
        label: "Assign histogram bin".to_string(),
        state: StepState::Ok,
        input_value: value,
        output_value: bin as f64,
        params: serde_json::json!({"bin_count": config.bin_count}),
        evidence: None,
    });
    counts[bin] += 1;

    SampleRecord { raw: sample.clone(), steps, verdict: Verdict::Binned { bin } }
}

/// Returns the lower of the two bins around the nearest interior edge, when
/// `value` is within `tolerance` of that edge.
fn nearest_boundary_bin(value: f64, bin_edges: &[f64], tolerance: f64) -> Option<usize> {
    let interior = &bin_edges[1..bin_edges.len() - 1];
    interior
        .iter()
        .enumerate()
        .filter(|(_, edge)| (value - *edge).abs() <= tolerance)
        .min_by(|(_, a), (_, b)| {
            (value - *a).partial_cmp(&(value - *b)).unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(index, _)| index)
}

fn assign_bin(value: f64, bin_count: usize) -> usize {
    let scaled = value * bin_count as f64;
    let bin = scaled.floor() as isize;
    bin.clamp(0, bin_count as isize - 1) as usize
}
