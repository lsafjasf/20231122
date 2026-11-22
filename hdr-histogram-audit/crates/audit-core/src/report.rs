//! Batch-level audit report.
//!
//! The report deliberately leads with the *first* error state rather than a
//! single aggregate score: an operator needs to know where the pipeline first
//! diverged so the offending input and step can be opened in the UI.

use crate::model::{SampleRecord, StepState};
use crate::pipeline::PipelineRun;
use serde::{Deserialize, Serialize};

/// Pointer to the exact sample and step where a problem first appeared.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FirstError {
    /// Position of the record in processing order.
    pub record_index: usize,
    pub sample_id: String,
    /// Position of the offending step inside that record.
    pub step_index: usize,
    pub step_name: String,
    pub state: StepState,
}

/// Tallies kept for context; never used as a substitute for `first_error`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScoreCard {
    pub total: usize,
    pub binned: usize,
    pub truncated: usize,
    pub indeterminate: usize,
    pub errors: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditReport {
    pub score: ScoreCard,
    /// First problematic state in processing order, if any.
    pub first_error: Option<FirstError>,
    /// Every problematic record index, so the UI can jump to each.
    pub problem_records: Vec<usize>,
}

pub fn build_report(run: &PipelineRun) -> AuditReport {
    let mut score = ScoreCard::default();
    let mut first_error: Option<FirstError> = None;
    let mut problem_records = Vec::new();

    for (record_index, record) in run.records.iter().enumerate() {
        score.total += 1;
        match record.verdict {
            crate::model::Verdict::Binned { .. } => score.binned += 1,
            crate::model::Verdict::Truncated => score.truncated += 1,
            crate::model::Verdict::Indeterminate => score.indeterminate += 1,
            crate::model::Verdict::Error => score.errors += 1,
        }

        if let Some(step_index) = record.first_problem_step() {
            problem_records.push(record_index);
            // First error wins: records are visited in processing order.
            if first_error.is_none() {
                let step = &record.steps[step_index];
                first_error = Some(FirstError {
                    record_index,
                    sample_id: record.raw.sample_id.clone(),
                    step_index,
                    step_name: step.name.clone(),
                    state: step.state,
                });
            }
        }
    }

    AuditReport { score, first_error, problem_records }
}

/// Returns the first record whose step reaches the given state.
pub fn first_record_with_state<'a>(
    run: &'a PipelineRun,
    state: StepState,
) -> Option<(usize, &'a SampleRecord)> {
    run.records
        .iter()
        .enumerate()
        .find(|(_, record)| record.steps.iter().any(|step| step.state == state))
}
