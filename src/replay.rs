use serde::{Deserialize, Serialize};

use crate::engine::{AddressEngine, SubmitOutcome};
use crate::journal::{JournalEntry, JournalInput, StepResult};
use crate::model::StateView;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayMismatch {
    pub seq: u64,
    pub field: String,
    pub expected: String,
    pub actual: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayCheck {
    pub checked_entries: usize,
    pub matched: bool,
    pub first_mismatch: Option<ReplayMismatch>,
}

pub fn replay_entries(entries: &[JournalEntry]) -> ReplayCheck {
    let mut engine: Option<AddressEngine> = None;
    let mut mismatch = None;

    for entry in entries {
        let before = engine
            .as_ref()
            .map(AddressEngine::state_view)
            .unwrap_or_else(StateView::empty);
        if let Some(found) = compare(
            entry.seq,
            "state_before_digest",
            &entry.state_before_digest,
            &before.digest(),
        ) {
            mismatch = Some(found);
            break;
        }

        if entry.input.digest() != entry.input_digest {
            mismatch = Some(ReplayMismatch {
                seq: entry.seq,
                field: "input_digest".to_string(),
                expected: entry.input_digest.clone(),
                actual: entry.input.digest(),
            });
            break;
        }

        let actual_result = match &entry.input {
            JournalInput::SnapshotLoaded {
                version, policy, ..
            } => {
                engine = Some(AddressEngine::new(version.clone(), *policy));
                StepResult::SnapshotLoaded {
                    version: version.clone(),
                    policy: *policy,
                }
            }
            JournalInput::FrameSubmitted { frame, .. } => {
                let Some(active) = engine.as_mut() else {
                    mismatch = Some(ReplayMismatch {
                        seq: entry.seq,
                        field: "engine".to_string(),
                        expected: "snapshot loaded".to_string(),
                        actual: "no snapshot".to_string(),
                    });
                    break;
                };
                StepResult::Record {
                    outcome: active.submit(frame.clone()),
                }
            }
            JournalInput::RawSubmitted { raw, .. } => {
                let Some(active) = engine.as_mut() else {
                    mismatch = Some(ReplayMismatch {
                        seq: entry.seq,
                        field: "engine".to_string(),
                        expected: "snapshot loaded".to_string(),
                        actual: "no snapshot".to_string(),
                    });
                    break;
                };
                StepResult::Record {
                    outcome: active.submit_raw(raw),
                }
            }
            JournalInput::Finalized => {
                let Some(active) = engine.as_mut() else {
                    mismatch = Some(ReplayMismatch {
                        seq: entry.seq,
                        field: "engine".to_string(),
                        expected: "snapshot loaded".to_string(),
                        actual: "no snapshot".to_string(),
                    });
                    break;
                };
                StepResult::Finalized {
                    outcome: active.finalize(),
                }
            }
        };

        if entry.result != actual_result {
            mismatch = Some(ReplayMismatch {
                seq: entry.seq,
                field: "result".to_string(),
                expected: compact(&entry.result),
                actual: compact(&actual_result),
            });
            break;
        }

        let after = engine
            .as_ref()
            .map(AddressEngine::state_view)
            .unwrap_or_else(StateView::empty);
        if let Some(found) = compare(
            entry.seq,
            "state_after_digest",
            &entry.state_after_digest,
            &after.digest(),
        ) {
            mismatch = Some(found);
            break;
        }
    }

    ReplayCheck {
        checked_entries: entries.len(),
        matched: mismatch.is_none(),
        first_mismatch: mismatch,
    }
}

fn compare(seq: u64, field: &str, expected: &str, actual: &str) -> Option<ReplayMismatch> {
    (expected != actual).then(|| ReplayMismatch {
        seq,
        field: field.to_string(),
        expected: expected.to_string(),
        actual: actual.to_string(),
    })
}

fn compact<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("value is serializable")
}

pub fn replay_outcome_is_duplicate(entry: &JournalEntry) -> bool {
    matches!(
        &entry.result,
        StepResult::Record {
            outcome: SubmitOutcome::Duplicate { .. }
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::serialize_journal;

    #[test]
    fn replay_detects_tampered_state_digest() {
        let mut entry = JournalEntry::new(
            1,
            "load",
            JournalInput::SnapshotLoaded {
                version: "v2".into(),
                policy: crate::model::AddressPolicy::PtsStable,
                source: "fixture".into(),
            },
            StateView::empty(),
            StepResult::SnapshotLoaded {
                version: "v2".into(),
                policy: crate::model::AddressPolicy::PtsStable,
            },
            StateView {
                version: "v2".into(),
                policy: Some(crate::model::AddressPolicy::PtsStable),
                ..StateView::empty()
            },
        );
        assert!(replay_entries(&[entry.clone()]).matched);
        entry.state_after_digest = "tampered".into();
        let check = replay_entries(&[entry]);
        assert!(!check.matched);
        assert_eq!(
            check.first_mismatch.unwrap().field,
            "state_after_digest".to_string()
        );
        assert!(serialize_journal(&[]).is_empty());
    }
}
