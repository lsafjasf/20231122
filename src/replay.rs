use std::collections::BTreeMap;

use crate::evidence::{GENESIS_EVIDENCE, make_step_evidence, sha256_json};
use crate::model::{
    Action, AssetState, FirstError, Phase, RecordedInput, RecoveredRecord, ReplayRun, ReplayState,
    StepOutcome, StepTrace,
};

#[derive(Debug)]
struct Violation {
    rule: &'static str,
    message: String,
    expected: String,
    observed: String,
}

impl Violation {
    fn new(
        rule: &'static str,
        message: impl Into<String>,
        expected: impl Into<String>,
        observed: impl Into<String>,
    ) -> Self {
        Self {
            rule,
            message: message.into(),
            expected: expected.into(),
            observed: observed.into(),
        }
    }
}

pub fn replay_record(record: &RecoveredRecord, scenario: &str) -> ReplayRun {
    replay_inputs(&record.record_id, scenario, &record.inputs)
}

pub fn replay_inputs(record_id: &str, scenario: &str, inputs: &[RecordedInput]) -> ReplayRun {
    let mut state = ReplayState::default();
    let mut traces = Vec::with_capacity(inputs.len());
    let mut previous_evidence = GENESIS_EVIDENCE.to_owned();
    let mut stopped_after_first_error = false;

    for (index, input) in inputs.iter().enumerate() {
        let expected_sequence = state.cursor;
        let state_before = state.clone();
        let outcome = match apply_input(&mut state, input, index) {
            Ok(effect) => StepOutcome::Accepted { effect },
            Err(violation) => {
                let first_error = FirstError {
                    step_index: index,
                    step_id: input.event_id.clone(),
                    rule: violation.rule.to_owned(),
                    message: violation.message.clone(),
                    expected: violation.expected.clone(),
                    observed: violation.observed.clone(),
                };
                state = state_before.clone();
                state.phase = Phase::Rejected;
                state.first_error = Some(first_error);
                StepOutcome::Rejected {
                    rule: violation.rule.to_owned(),
                    message: violation.message,
                    expected: violation.expected,
                    observed: violation.observed,
                }
            }
        };

        let state_after = state.clone();
        let evidence = make_step_evidence(
            index,
            &previous_evidence,
            input,
            &state_before,
            &state_after,
            &outcome,
        );
        let rejected = matches!(outcome, StepOutcome::Rejected { .. });
        previous_evidence = evidence.evidence_sha256.clone();
        traces.push(StepTrace {
            index,
            expected_sequence,
            input: input.clone(),
            state_before,
            outcome,
            state_after,
            evidence,
        });

        if rejected {
            stopped_after_first_error = true;
            break;
        }
    }

    ReplayRun {
        record_id: record_id.to_owned(),
        scenario: scenario.to_owned(),
        final_state: state.clone(),
        first_error: state.first_error,
        stopped_after_first_error,
        traces,
    }
}

fn apply_input(
    state: &mut ReplayState,
    input: &RecordedInput,
    index: usize,
) -> Result<String, Violation> {
    if state.seen_event_ids.contains(&input.event_id) {
        return Err(Violation::new(
            "duplicate_event_id",
            format!("event {} was submitted more than once", input.event_id),
            "a fresh event_id",
            input.event_id.clone(),
        ));
    }
    if state.seen_submission_ids.contains(&input.submission_id) {
        return Err(Violation::new(
            "duplicate_submission",
            format!(
                "submission {} was received again at step {index}",
                input.submission_id
            ),
            "one commit per submission_id",
            input.submission_id.clone(),
        ));
    }
    if input.sequence != state.cursor {
        return Err(Violation::new(
            "sequence_order",
            "the next input does not match the replay cursor",
            state.cursor.to_string(),
            input.sequence.to_string(),
        ));
    }

    let effect = match &input.action {
        Action::Open {
            uri,
            asset_sha256,
            observed_bytes,
        } => {
            require_phase(state, &[Phase::Idle], "open")?;
            state.asset = Some(AssetState {
                uri: uri.clone(),
                sha256: asset_sha256.clone(),
                bytes: *observed_bytes,
            });
            state.phase = Phase::Opened;
            format!("asset opened: {observed_bytes} bytes")
        }
        Action::Probe {
            fields,
            probe_sha256,
        } => {
            require_phase(state, &[Phase::Opened], "probe")?;
            if fields.is_empty() {
                return Err(Violation::new(
                    "probe_empty",
                    "the recovered GStreamer probe returned no metadata",
                    "at least one metadata field",
                    "empty field map",
                ));
            }
            let actual_digest = sha256_json(fields);
            if actual_digest != *probe_sha256 {
                return Err(Violation::new(
                    "probe_digest_mismatch",
                    "recovered probe fields do not match their evidence hash",
                    probe_sha256.clone(),
                    actual_digest,
                ));
            }
            state.metadata = fields.clone();
            state.metadata_sha256 = digest_metadata(&state.metadata);
            state.phase = Phase::Probed;
            format!("probe accepted with {} fields", fields.len())
        }
        Action::StageMetadata {
            field,
            value,
            revision,
        } => {
            require_phase(state, &[Phase::Probed, Phase::Staged], "stage_metadata")?;
            if field.trim().is_empty() {
                return Err(Violation::new(
                    "field_required",
                    "metadata field name is empty",
                    "non-empty field",
                    field.clone(),
                ));
            }
            let expected_revision = state.revision + 1;
            if *revision != expected_revision {
                return Err(Violation::new(
                    "revision_order",
                    "metadata revision is out of order",
                    expected_revision.to_string(),
                    revision.to_string(),
                ));
            }
            state.metadata.insert(field.clone(), value.clone());
            state.revision = *revision;
            state.metadata_sha256 = digest_metadata(&state.metadata);
            state.phase = Phase::Staged;
            format!("staged {field} at revision {revision}")
        }
        Action::Submit {
            endpoint,
            payload_sha256,
        } => {
            require_phase(state, &[Phase::Staged], "submit")?;
            if *payload_sha256 != state.metadata_sha256 {
                return Err(Violation::new(
                    "payload_digest_mismatch",
                    "submitted payload is not the staged metadata",
                    state.metadata_sha256.clone(),
                    payload_sha256.clone(),
                ));
            }
            if endpoint.trim().is_empty() {
                return Err(Violation::new(
                    "endpoint_required",
                    "submit endpoint is empty",
                    "non-empty endpoint",
                    endpoint.clone(),
                ));
            }
            state.submission_id = Some(input.submission_id.clone());
            state.submitted_payload_sha256 = Some(payload_sha256.clone());
            state.phase = Phase::Committed;
            format!("committed to {endpoint}")
        }
        Action::Verify {
            expected_submission_id,
        } => {
            require_phase(state, &[Phase::Committed], "verify")?;
            if state.submission_id.as_deref() != Some(expected_submission_id) {
                return Err(Violation::new(
                    "verification_mismatch",
                    "verified submission does not match the commit",
                    expected_submission_id.clone(),
                    state.submission_id.clone().unwrap_or_default(),
                ));
            }
            state.phase = Phase::Verified;
            "commit verified".to_owned()
        }
    };

    state.seen_event_ids.insert(input.event_id.clone());
    state
        .seen_submission_ids
        .insert(input.submission_id.clone());
    state.cursor += 1;
    Ok(effect)
}

fn require_phase(state: &ReplayState, allowed: &[Phase], action: &str) -> Result<(), Violation> {
    if allowed.contains(&state.phase) {
        return Ok(());
    }
    Err(Violation::new(
        "invalid_transition",
        format!("{action} is not valid while the replay is in this state"),
        format!("{allowed:?}"),
        format!("{:?}", state.phase),
    ))
}

fn digest_metadata(metadata: &BTreeMap<String, String>) -> String {
    sha256_json(metadata)
}
