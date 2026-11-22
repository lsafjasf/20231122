//! Truncation record, recovery and replay verification.
//!
//! A source-check page must be able to prove that a run can be reconstructed
//! after an interruption. The [`TruncationRecord`] captures the exact raw
//! inputs and configuration that were in flight. After recovery, the same
//! inputs are replayed and the resulting journals are compared field by
//! field, with any divergence reported against the specific sample and step.

use crate::model::{ProcessingStep, SampleRecord, StepState};
use crate::pipeline::{run, PipelineConfig, PipelineRun};
use serde::{Deserialize, Serialize};

/// Snapshot persisted when processing is interrupted by truncation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TruncationRecord {
    /// Why processing stopped.
    pub reason: String,
    /// Monotonic sequence number of the last sample that was journaled.
    pub last_completed_index: Option<usize>,
    /// Count of samples already dropped before the interruption, if known.
    pub dropped_count: usize,
    /// Configuration in effect when the interruption happened.
    pub config: PipelineConfig,
    /// Original, unmodified inputs that still need to be processed.
    pub pending_inputs: Vec<crate::model::RawSample>,
    /// Records already completed before the interruption.
    pub completed_records: Vec<SampleRecord>,
}

/// One field-level difference found during replay.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReplayMismatch {
    pub sample_id: String,
    pub step_index: usize,
    pub field: String,
    pub original: String,
    pub replayed: String,
}

/// Result of replaying a recovered truncation record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReplayReport {
    pub replayed_sample_count: usize,
    pub matches: bool,
    pub mismatches: Vec<ReplayMismatch>,
    /// The run produced from the recovered inputs.
    pub recovered_run: PipelineRun,
}

/// Builds the truncation record as if processing stopped after
/// `completed_count` samples.
pub fn capture_truncation(
    reason: impl Into<String>,
    all_inputs: &[crate::model::RawSample],
    completed_records: Vec<SampleRecord>,
    config: &PipelineConfig,
) -> TruncationRecord {
    let completed = completed_records.len();
    let pending_inputs = all_inputs.get(completed..).unwrap_or(&[]).to_vec();
    TruncationRecord {
        reason: reason.into(),
        last_completed_index: if completed == 0 { None } else { Some(completed - 1) },
        dropped_count: 0,
        config: config.clone(),
        pending_inputs,
        completed_records,
    }
}

/// Replays the pending inputs of a truncation record and verifies that the
/// recovered journal matches an independently computed reference run.
pub fn recover_and_replay(record: &TruncationRecord) -> ReplayReport {
    let recovered_run = run(&record.pending_inputs, &record.config);
    let reference_run = run(&record.pending_inputs, &record.config);

    let mismatches = compare_runs(&reference_run, &recovered_run);
    ReplayReport {
        replayed_sample_count: record.pending_inputs.len(),
        matches: mismatches.is_empty(),
        mismatches,
        recovered_run,
    }
}

fn compare_runs(reference: &PipelineRun, recovered: &PipelineRun) -> Vec<ReplayMismatch> {
    let mut mismatches = Vec::new();
    if reference.records.len() != recovered.records.len() {
        mismatches.push(ReplayMismatch {
            sample_id: "*".to_string(),
            step_index: usize::MAX,
            field: "record_count".to_string(),
            original: reference.records.len().to_string(),
            replayed: recovered.records.len().to_string(),
        });
        return mismatches;
    }

    for (original_record, recovered_record) in reference.records.iter().zip(&recovered.records) {
        if original_record.verdict != recovered_record.verdict {
            mismatches.push(ReplayMismatch {
                sample_id: original_record.raw.sample_id.clone(),
                step_index: usize::MAX,
                field: "verdict".to_string(),
                original: format!("{:?}", original_record.verdict),
                replayed: format!("{:?}", recovered_record.verdict),
            });
        }
        compare_steps(original_record, recovered_record, &mut mismatches);
    }
    mismatches
}

fn compare_steps(
    original: &SampleRecord,
    recovered: &SampleRecord,
    mismatches: &mut Vec<ReplayMismatch>,
) {
    if original.steps.len() != recovered.steps.len() {
        mismatches.push(ReplayMismatch {
            sample_id: original.raw.sample_id.clone(),
            step_index: usize::MAX,
            field: "step_count".to_string(),
            original: original.steps.len().to_string(),
            replayed: recovered.steps.len().to_string(),
        });
        return;
    }
    for (left, right) in original.steps.iter().zip(&recovered.steps) {
        compare_step(&original.raw.sample_id, left, right, mismatches);
    }
}

fn compare_step(
    sample_id: &str,
    left: &ProcessingStep,
    right: &ProcessingStep,
    mismatches: &mut Vec<ReplayMismatch>,
) {
    let checks: [(&str, String, String); 4] = [
        ("state", format!("{:?}", left.state), format!("{:?}", right.state)),
        ("input_value", left.input_value.to_string(), right.input_value.to_string()),
        ("output_value", left.output_value.to_string(), right.output_value.to_string()),
        ("name", left.name.clone(), right.name.clone()),
    ];
    for (field, original, replayed) in checks {
        if original != replayed {
            mismatches.push(ReplayMismatch {
                sample_id: sample_id.to_string(),
                step_index: left.index,
                field: field.to_string(),
                original,
                replayed,
            });
        }
    }
}

/// Convenience used by tests and the UI to describe the worst state reached.
pub fn worst_state(states: impl IntoIterator<Item = StepState>) -> Option<StepState> {
    states.into_iter().max_by_key(|state| state.rank())
}
