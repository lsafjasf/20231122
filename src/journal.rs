use serde::{Deserialize, Serialize};

use crate::engine::{FinalizeOutcome, SubmitOutcome};
use crate::model::{AddressPolicy, FrameRecord, StateView, stable_json};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JournalInput {
    SnapshotLoaded {
        version: String,
        policy: AddressPolicy,
        source: String,
    },
    FrameSubmitted {
        frame: FrameRecord,
        source: String,
    },
    RawSubmitted {
        raw: String,
        source: String,
    },
    Finalized,
}

impl JournalInput {
    pub fn digest(&self) -> String {
        crate::model::sha256_hex(&serde_json::to_vec(self).expect("JournalInput is serializable"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StepResult {
    SnapshotLoaded {
        version: String,
        policy: AddressPolicy,
    },
    Record {
        outcome: SubmitOutcome,
    },
    Finalized {
        outcome: FinalizeOutcome,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalEntry {
    pub seq: u64,
    pub label: String,
    pub input: JournalInput,
    pub input_digest: String,
    pub state_before: StateView,
    pub state_before_digest: String,
    pub result: StepResult,
    pub state_after: StateView,
    pub state_after_digest: String,
}

impl JournalEntry {
    pub fn new(
        seq: u64,
        label: impl Into<String>,
        input: JournalInput,
        state_before: StateView,
        result: StepResult,
        state_after: StateView,
    ) -> Self {
        Self {
            seq,
            label: label.into(),
            input_digest: input.digest(),
            input,
            state_before_digest: state_before.digest(),
            state_before,
            result,
            state_after_digest: state_after.digest(),
            state_after,
        }
    }

    pub fn as_json(&self) -> serde_json::Value {
        stable_json(self)
    }
}

pub fn serialize_journal(entries: &[JournalEntry]) -> String {
    let mut output = String::new();
    for entry in entries {
        output.push_str(&serde_json::to_string(entry).expect("JournalEntry is serializable"));
        output.push('\n');
    }
    output
}

pub fn parse_journal(raw: &str) -> serde_json::Result<Vec<JournalEntry>> {
    raw.lines()
        .filter(|line| !line.trim().is_empty())
        .map(serde_json::from_str)
        .collect()
}
