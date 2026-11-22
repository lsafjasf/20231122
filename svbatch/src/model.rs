//! Domain model for the batch sound-effect variant experiment.

use serde::{Deserialize, Serialize};

/// A single deterministic input asset for the experiment.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceSpec {
    pub source_id: String,
    /// GStreamer `audiotestsrc` wave id (0 sine, 1 square, ...).
    pub wave: u32,
    pub freq_hz: u32,
    pub duration_ms: u32,
}

/// A transformation recipe applied to a source to produce one variant.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VariantSpec {
    pub variant_id: String,
    /// Linear gain multiplier (1.0 = unchanged).
    pub gain: f64,
    /// Playback speed / tempo multiplier (1.0 = unchanged).
    pub speed: f64,
}

/// One processing job: one source x one variant.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JobSpec {
    pub job_id: String,
    pub source: SourceSpec,
    pub variant: VariantSpec,
    /// Artificial per-job latency in milliseconds so a mid-run cancel can
    /// land deterministically between jobs.
    pub work_delay_ms: u64,
}

impl JobSpec {
    /// Deterministic idempotency key: the same logical job always has the
    /// same key regardless of its position in a batch.
    pub fn idempotency_key(&self) -> String {
        format!(
            "{}|{}|g{}|s{}",
            self.source.source_id, self.variant.variant_id, self.variant.gain, self.variant.speed
        )
    }

    pub fn output_filename(&self) -> String {
        format!("{}.wav", self.job_id)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Pending,
    Running,
    Succeeded,
    Cancelled,
    Failed,
}

impl JobState {
    pub fn as_str(self) -> &'static str {
        match self {
            JobState::Pending => "pending",
            JobState::Running => "running",
            JobState::Succeeded => "succeeded",
            JobState::Cancelled => "cancelled",
            JobState::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JobResult {
    pub job_id: String,
    pub state: JobState,
    pub output_file: Option<String>,
    pub sha256: Option<String>,
    pub bytes: Option<u64>,
    pub detail: String,
}

/// A point-in-time snapshot of every job, emitted while a batch runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchSnapshot {
    pub seq: u64,
    pub elapsed_ms: u128,
    pub states: Vec<(String, JobState)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepRecord {
    pub seq: u64,
    pub elapsed_ms: u128,
    pub phase: String,
    pub job_id: Option<String>,
    pub input: serde_json::Value,
    pub state: serde_json::Value,
    pub result: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchReport {
    pub run_id: String,
    pub mode: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub outcome: String,
    pub jobs: Vec<JobResult>,
    pub steps: Vec<StepRecord>,
    pub snapshots: Vec<BatchSnapshot>,
}
