use serde::{Deserialize, Serialize};
use std::path::Path;

pub const SCHEMA_VERSION: u32 = 1;
pub const ANCHOR_COUNT: usize = 3;
pub const SEGMENT_SECONDS: f64 = 5.0;
pub const DEFAULT_TARGET_RMS_DBFS: f64 = -20.0;
pub const DEFAULT_MAX_GAIN_DB: f64 = 20.0;
pub const DEFAULT_MIN_GAIN_DB: f64 = -20.0;
pub const DEFAULT_PEAK_CEILING_DBFS: f64 = -1.0;
pub const DEFAULT_ANALYSIS_RATE: u32 = 48_000;
pub const DEGRADED_RATE: u32 = 24_000;

pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

pub fn db_from_power(power: f64) -> f64 {
    if power <= 1.0e-12 {
        -120.0
    } else {
        10.0 * power.log10()
    }
}

pub fn power_from_db(db: f64) -> f64 {
    10.0_f64.powf(db / 10.0)
}

pub fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceInfo {
    pub path: String,
    pub sha256: String,
    pub file_size: u64,
    pub data_offset: u64,
    pub data_size: u64,
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
    pub sample_format: String,
    pub frame_count: u64,
    pub duration_ns: u64,
    pub original_copy_path: String,
    pub original_copy_sha256: String,
    pub original_mode_readonly: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnchorSpec {
    pub id: String,
    pub segment_index: usize,
    pub segment_start_ns: u64,
    pub segment_end_ns: u64,
    pub anchor_ns: u64,
    pub segment_start_frame: u64,
    pub segment_end_frame: u64,
    pub anchor_frame: u64,
    pub payload_start_byte: u64,
    pub payload_end_byte: u64,
    pub file_start_byte: u64,
    pub file_end_byte: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalibrationConfig {
    pub target_rms_dbfs: f64,
    pub max_gain_db: f64,
    pub min_gain_db: f64,
    pub peak_ceiling_dbfs: f64,
    pub force_degrade: bool,
    pub memory_floor_mb: u64,
    pub disk_floor_mb: u64,
}

impl Default for CalibrationConfig {
    fn default() -> Self {
        Self {
            target_rms_dbfs: DEFAULT_TARGET_RMS_DBFS,
            max_gain_db: DEFAULT_MAX_GAIN_DB,
            min_gain_db: DEFAULT_MIN_GAIN_DB,
            peak_ceiling_dbfs: DEFAULT_PEAK_CEILING_DBFS,
            force_degrade: false,
            memory_floor_mb: 256,
            disk_floor_mb: 512,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoudnessStats {
    pub rms_dbfs: f64,
    pub peak_dbfs: f64,
    pub sample_count: u64,
    pub sample_rate: u32,
    pub channels: u16,
    pub measurement: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GainDecision {
    pub requested_gain_db: f64,
    pub applied_gain_db: f64,
    pub gain_linear: f64,
    pub cap: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceSnapshot {
    pub captured_at: String,
    pub memory_available_mb: u64,
    pub disk_available_mb: u64,
    pub cpu_count: usize,
    pub memory_required_mb: u64,
    pub disk_required_mb: u64,
    pub mode: String,
    pub reasons: Vec<String>,
    pub forced: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DegradationRecord {
    pub entered_at: String,
    pub mode: String,
    pub reasons: Vec<String>,
    pub strategy: String,
    pub output_sample_rate: u32,
    pub anchor_probe_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnchorCheckpoint {
    pub anchor_id: String,
    pub segment_index: usize,
    pub status: String,
    pub anchor_ns: u64,
    pub anchor_seconds: f64,
    pub anchor_frame: u64,
    pub original_anchor_file_byte: u64,
    pub segment_start_ns: u64,
    pub segment_end_ns: u64,
    pub segment_start_frame: u64,
    pub segment_end_frame: u64,
    pub payload_start_byte: u64,
    pub payload_end_byte: u64,
    pub file_start_byte: u64,
    pub file_end_byte: u64,
    pub analysis_pipeline: String,
    pub render_pipeline: String,
    pub before: LoudnessStats,
    pub gain: GainDecision,
    pub after: LoudnessStats,
    pub delta_db: f64,
    pub target_error_db: f64,
    pub output_wav: String,
    pub output_sha256: String,
    pub output_bytes: u64,
    pub output_sample_rate: u32,
    pub raw_probe_file: String,
    pub raw_probe_start_byte: u64,
    pub raw_probe_end_byte: u64,
    pub raw_probe_sha256: String,
    pub raw_probe_bytes: u64,
    pub checkpoint_path: String,
    pub checkpoint_sha256: String,
    pub completed_at: String,
    pub pause_after: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowEvent {
    pub at: String,
    pub kind: String,
    pub anchor: Option<String>,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FinalArtifact {
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
    pub sample_rate: u32,
    pub frame_count: u64,
    pub duration_ns: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunState {
    pub schema_version: u32,
    pub run_id: String,
    pub created_at: String,
    pub updated_at: String,
    pub status: String,
    pub source: SourceInfo,
    pub config: CalibrationConfig,
    pub anchors: Vec<AnchorSpec>,
    pub resource: ResourceSnapshot,
    pub degradation: Option<DegradationRecord>,
    pub next_segment: usize,
    pub completed: Vec<AnchorCheckpoint>,
    pub resume_count: u32,
    pub last_pause_reason: Option<String>,
    pub final_output: Option<FinalArtifact>,
    pub events: Vec<FlowEvent>,
}

impl RunState {
    pub fn push_event(&mut self, kind: &str, anchor: Option<&str>, detail: impl Into<String>) {
        self.events.push(FlowEvent {
            at: now_rfc3339(),
            kind: kind.to_string(),
            anchor: anchor.map(str::to_string),
            detail: detail.into(),
        });
        self.updated_at = now_rfc3339();
    }

    pub fn is_complete(&self) -> bool {
        self.next_segment >= self.anchors.len()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResumePreflight {
    pub checked_at: String,
    pub action: String,
    pub reasons: Vec<String>,
    pub source_hash_ok: bool,
    pub completed_output_hashes_ok: bool,
    pub checkpoint_chain_ok: bool,
    pub state_consistent: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SegmentComparison {
    pub anchor_id: String,
    pub before_rms_dbfs: f64,
    pub after_rms_dbfs: f64,
    pub gain_db: f64,
    pub delta_db: f64,
    pub target_error_db: f64,
    pub output_sha256: String,
    pub within_tolerance: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComparisonReport {
    pub generated_at: String,
    pub run_id: String,
    pub tolerance_db: f64,
    pub segments: Vec<SegmentComparison>,
    pub resume_count: u32,
    pub paused_anchors: Vec<String>,
    pub final_output_sha256: Option<String>,
    pub complete: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyCheck {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyReport {
    pub checked_at: String,
    pub ok: bool,
    pub checks: Vec<VerifyCheck>,
    pub acceptance_lines: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EquivalenceReport {
    pub generated_at: String,
    pub a_run: String,
    pub b_run: String,
    pub a_output_sha256: Option<String>,
    pub b_output_sha256: Option<String>,
    pub outputs_identical: bool,
    pub anchors: Vec<AnchorEquivalence>,
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnchorEquivalence {
    pub anchor_id: String,
    pub same_before_rms: bool,
    pub same_after_rms: bool,
    pub same_gain: bool,
    pub same_output_sha256: bool,
    pub a_after_rms_dbfs: Option<f64>,
    pub b_after_rms_dbfs: Option<f64>,
}

pub fn source_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "source.wav".to_string())
}
