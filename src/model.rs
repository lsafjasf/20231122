use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::SCHEMA;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordedMedia {
    pub uri: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub duration_ms: u64,
    pub probe_engine: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveredRecord {
    pub schema: String,
    pub record_id: String,
    pub source_take: String,
    pub recovered_at: String,
    pub media: RecordedMedia,
    pub inputs: Vec<RecordedInput>,
}

impl RecoveredRecord {
    pub fn new(
        record_id: impl Into<String>,
        source_take: impl Into<String>,
        recovered_at: impl Into<String>,
        media: RecordedMedia,
        inputs: Vec<RecordedInput>,
    ) -> Self {
        Self {
            schema: SCHEMA.to_owned(),
            record_id: record_id.into(),
            source_take: source_take.into(),
            recovered_at: recovered_at.into(),
            media,
            inputs,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordedInput {
    pub event_id: String,
    pub sequence: u64,
    pub submission_id: String,
    pub action: Action,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    Open {
        uri: String,
        asset_sha256: String,
        observed_bytes: u64,
    },
    Probe {
        fields: BTreeMap<String, String>,
        probe_sha256: String,
    },
    StageMetadata {
        field: String,
        value: String,
        revision: u64,
    },
    Submit {
        endpoint: String,
        payload_sha256: String,
    },
    Verify {
        expected_submission_id: String,
    },
}

impl Action {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Open { .. } => "open",
            Self::Probe { .. } => "probe",
            Self::StageMetadata { .. } => "stage_metadata",
            Self::Submit { .. } => "submit",
            Self::Verify { .. } => "verify",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Idle,
    Opened,
    Probed,
    Staged,
    Committed,
    Verified,
    Rejected,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetState {
    pub uri: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FirstError {
    pub step_index: usize,
    pub step_id: String,
    pub rule: String,
    pub message: String,
    pub expected: String,
    pub observed: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayState {
    pub phase: Phase,
    pub cursor: u64,
    pub revision: u64,
    pub asset: Option<AssetState>,
    pub metadata: BTreeMap<String, String>,
    pub metadata_sha256: String,
    pub submission_id: Option<String>,
    pub submitted_payload_sha256: Option<String>,
    pub seen_event_ids: BTreeSet<String>,
    pub seen_submission_ids: BTreeSet<String>,
    pub first_error: Option<FirstError>,
}

impl Default for ReplayState {
    fn default() -> Self {
        Self {
            phase: Phase::Idle,
            cursor: 0,
            revision: 0,
            asset: None,
            metadata: BTreeMap::new(),
            metadata_sha256: empty_metadata_sha256(),
            submission_id: None,
            submitted_payload_sha256: None,
            seen_event_ids: BTreeSet::new(),
            seen_submission_ids: BTreeSet::new(),
            first_error: None,
        }
    }
}

fn empty_metadata_sha256() -> String {
    crate::evidence::sha256_json(&BTreeMap::<String, String>::new())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepEvidence {
    pub input_sha256: String,
    pub state_before_sha256: String,
    pub state_after_sha256: String,
    pub result_sha256: String,
    pub previous_evidence_sha256: String,
    pub evidence_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum StepOutcome {
    Accepted {
        effect: String,
    },
    Rejected {
        rule: String,
        message: String,
        expected: String,
        observed: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepTrace {
    pub index: usize,
    pub expected_sequence: u64,
    pub input: RecordedInput,
    pub state_before: ReplayState,
    pub outcome: StepOutcome,
    pub state_after: ReplayState,
    pub evidence: StepEvidence,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayRun {
    pub record_id: String,
    pub scenario: String,
    pub traces: Vec<StepTrace>,
    pub final_state: ReplayState,
    pub first_error: Option<FirstError>,
    pub stopped_after_first_error: bool,
}
