use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const FPS: u32 = 25;
pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: u16 = 1;
pub const BITS_PER_SAMPLE: u16 = 16;
pub const DAY_SAMPLES: u64 = 24 * 60 * 60 * SAMPLE_RATE as u64;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SampleSpec {
    pub duration_seconds: u32,
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
    pub time_reference_samples: u64,
    pub markers: Vec<MarkerSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarkerSpec {
    pub id: u32,
    pub label: String,
    pub sample_offset: u64,
    pub absolute_timecode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BwfSnapshot {
    pub path: PathBuf,
    pub file_len: u64,
    pub riff_declared_len: u64,
    pub data_payload_offset: u64,
    pub data_declared_len: u64,
    pub data_actual_len: u64,
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
    pub time_reference_samples: u64,
    pub time_reference_timecode: String,
    pub sha256: String,
    pub markers: Vec<MarkerSpec>,
    pub structural_fault: Option<ValidationFault>,
}

impl BwfSnapshot {
    pub fn frame_count(&self) -> u64 {
        let frame_bytes = u64::from(self.channels) * u64::from(self.bits_per_sample / 8);
        self.data_actual_len.checked_div(frame_bytes).unwrap_or(0)
    }

    pub fn declared_frames(&self) -> u64 {
        let frame_bytes = u64::from(self.channels) * u64::from(self.bits_per_sample / 8);
        self.data_declared_len.checked_div(frame_bytes).unwrap_or(0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ValidationFault {
    pub code: String,
    pub message: String,
    pub byte_offset: u64,
    pub expected: String,
    pub actual: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryEvidence {
    pub source: PathBuf,
    pub recovered: PathBuf,
    pub truncated_at: u64,
    pub appended_zero_bytes: u64,
    pub mode: String,
    pub source_sha256: String,
    pub recovered_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GstReplayEvidence {
    pub asset: PathBuf,
    pub pipeline: String,
    pub eos: bool,
    pub bus_messages: Vec<String>,
    pub buffer_count: u64,
    pub decoded_bytes: u64,
    pub decoded_frames: u64,
    pub first_pts_ns: Option<u64>,
    pub last_pts_ns: Option<u64>,
    pub sink_caps: Option<String>,
    pub expected_frames: u64,
    pub duration_ok: bool,
    pub passed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum RunKind {
    TruncatedProbe,
    RecoveredControl,
    OrderChanged,
    DuplicateSubmission,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum StepPhase {
    Ingest,
    Parse,
    Recover,
    GstReplay,
    SubmitMarker,
    Complete,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepResult {
    pub status: StepStatus,
    pub code: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum StepStatus {
    Accepted,
    Warning,
    Rejected,
    ObservedAfterError,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    pub name: String,
    pub accepted_markers: usize,
    pub expected_marker_index: usize,
    pub last_sample_offset: Option<u64>,
    pub first_error_step: Option<usize>,
}

impl SessionState {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            accepted_markers: 0,
            expected_marker_index: 0,
            last_sample_offset: None,
            first_error_step: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepRecord {
    pub index: usize,
    pub phase: StepPhase,
    pub input: serde_json::Value,
    pub state_before: SessionState,
    pub state_after: SessionState,
    pub result: StepResult,
    pub evidence: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FirstError {
    pub step_index: usize,
    pub phase: StepPhase,
    pub code: String,
    pub message: String,
    pub expected: String,
    pub actual: String,
    pub state_before: SessionState,
    pub error_state: SessionState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunReport {
    pub id: String,
    pub title: String,
    pub kind: RunKind,
    pub asset: PathBuf,
    pub steps: Vec<StepRecord>,
    pub first_error: Option<FirstError>,
    pub passed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceManifest {
    pub generated_at_utc: String,
    pub workspace: PathBuf,
    pub spec: SampleSpec,
    pub truncated: BwfSnapshot,
    pub recovered: BwfSnapshot,
    pub recovery: RecoveryEvidence,
    pub report_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExperimentReport {
    pub schema_version: u32,
    pub title: String,
    pub first_error: Option<FirstError>,
    pub runs: Vec<RunReport>,
    pub evidence: EvidenceManifest,
}
