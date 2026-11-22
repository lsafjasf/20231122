use serde::{Deserialize, Serialize};

pub const EXPERIMENT_FORMAT: &str = "audio-undo-experiment/v1";
pub const UNKNOWN_EXTENSION: &str = ".undoexp";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceSpec {
    pub frequency_hz: u32,
    pub duration_ms: u32,
    pub sample_rate: u32,
    pub channels: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClipState {
    pub clip_id: String,
    pub source: SourceSpec,
    pub trim_start_ms: u32,
    pub trim_end_ms: u32,
    pub gain_milli: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Snapshot {
    pub name: String,
    pub state_sha256: String,
    pub clip: ClipState,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExperimentSpec {
    pub format: String,
    pub experiment_id: String,
    pub extension: ExtensionSpec,
    pub controls: Controls,
    pub objects: Vec<ExperimentObject>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExtensionSpec {
    pub value: String,
    pub registered: bool,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Controls {
    pub snapshot_v1: Snapshot,
    pub snapshot_v2: Snapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExperimentObject {
    pub id: String,
    pub description: String,
    pub expect: ObjectExpectation,
    pub steps: Vec<StepSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectExpectation {
    #[serde(default)]
    pub rejected_step_indices: Vec<usize>,
    pub final_control: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StepSpec {
    pub request_id: String,
    #[serde(default)]
    pub base_revision: Option<u64>,
    pub command: Command,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Command {
    Trim { start_ms: u32, end_ms: u32 },
    Gain { gain_milli: u32 },
    Undo,
    Redo,
}
