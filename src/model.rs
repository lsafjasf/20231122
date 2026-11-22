use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RepairInput {
    pub schema_version: u32,
    pub reference_device_id: String,
    pub tolerance_ms: f64,
    pub devices: Vec<DeviceClock>,
    pub tracks: Vec<Track>,
    pub intervals: Vec<RepairInterval>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DeviceClock {
    pub id: String,
    pub label: String,
    pub offset_ms: f64,
    pub drift_ppm: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Track {
    pub id: u32,
    pub device_id: String,
    pub sample_rate: u32,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RepairInterval {
    pub id: String,
    pub track_id: u32,
    pub device_id: String,
    pub start: SourcePoint,
    pub end: SourcePoint,
    pub processor: Processor,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SourcePoint {
    pub device_ms: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Processor {
    Gain {
        gain_db: f64,
        #[serde(default)]
        fade_ms: f64,
    },
    Mute {
        #[serde(default)]
        fade_ms: f64,
    },
    Declick,
    Normalize {
        target_peak_db: f64,
        #[serde(default)]
        fade_ms: f64,
    },
    ReplaceSilence {
        #[serde(default)]
        fade_ms: f64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct VersionManifest {
    pub schema_version: u32,
    pub versions: Vec<AudioVersion>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AudioVersion {
    pub id: String,
    pub label: String,
    pub interval_ids: Vec<String>,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RepairReport {
    pub schema_version: u32,
    pub application: ApplicationInfo,
    pub run_id: String,
    pub created_at: String,
    pub status: RunStatus,
    pub inputs: InputEvidence,
    pub media: MediaInfo,
    pub clocks: Vec<ClockEvidence>,
    pub intervals: Vec<IntervalEvidence>,
    pub versions: Vec<VersionResult>,
    pub evidence: Vec<EvidenceEntry>,
    pub integrity: Integrity,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ApplicationInfo {
    pub name: String,
    pub version: String,
    pub rust: String,
    pub gstreamer: String,
    pub gtk4: String,
    pub schema_version: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Succeeded,
    SucceededWithWarnings,
    Rejected,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InputEvidence {
    pub recording_path: String,
    pub intervals_path: String,
    pub versions_path: String,
    pub recording: FileEvidence,
    pub intervals: FileEvidence,
    pub versions: FileEvidence,
    pub limits: LimitsSnapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FileEvidence {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LimitsSnapshot {
    pub max_json_bytes: u64,
    pub max_audio_bytes: u64,
    pub max_audio_seconds: f64,
    pub max_total_output_seconds: f64,
    pub max_intervals: usize,
    pub max_versions: usize,
    pub max_interval_refs_per_version: usize,
    pub max_waveform_buckets: usize,
    pub max_evidence_entries: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MediaInfo {
    pub requested_sample_rate: u32,
    pub decoded_sample_rate: u32,
    pub channels: u32,
    pub format: String,
    pub frame_ms: f64,
    pub total_frames: u64,
    pub duration_ms: f64,
    pub gstreamer_pipeline: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ClockEvidence {
    pub device_id: String,
    pub label: String,
    pub role: String,
    pub offset_ms: f64,
    pub drift_ppm: f64,
    pub conversion_formula: String,
    pub max_observed_skew_ms: f64,
    pub max_observed_skew_interval: String,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct IntervalEvidence {
    pub interval_id: String,
    pub track_id: u32,
    pub track_label: String,
    pub device_id: String,
    pub processor: Processor,
    pub requested_start_ms: f64,
    pub requested_end_ms: f64,
    pub requested_start_frame: Option<u64>,
    pub requested_end_frame: Option<u64>,
    pub canonical_start_ms: f64,
    pub canonical_end_ms: f64,
    pub applied_start_frame: u64,
    pub applied_end_frame: u64,
    pub source_frame_delta: i64,
    pub end_source_frame_delta: i64,
    pub clock_skew_ms: f64,
    pub duration_delta_ms: f64,
    pub applied: bool,
    pub disposition: String,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct VersionResult {
    pub manifest_index: usize,
    pub version_id: String,
    pub label: String,
    pub note: String,
    pub interval_ids: Vec<String>,
    pub output_path: String,
    pub output_bytes: u64,
    pub output_sha256: String,
    pub waveform_path: String,
    pub waveform_sha256: String,
    pub waveform: WaveformSummary,
    pub applied_intervals: Vec<AppliedInterval>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AppliedInterval {
    pub manifest_order: usize,
    pub interval_id: String,
    pub start_frame: u64,
    pub end_frame: u64,
    pub processor: Processor,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WaveformSummary {
    pub bucket_ms: f64,
    pub bucket_count: usize,
    pub source_frame_count: u64,
    pub min: f32,
    pub max: f32,
    pub rms: f32,
    pub peak: f32,
    pub buckets: Vec<WaveformBucket>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WaveformBucket {
    pub index: usize,
    pub start_frame: u64,
    pub end_frame: u64,
    pub peak: f32,
    pub rms: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EvidenceEntry {
    pub sequence: usize,
    pub kind: String,
    pub severity: String,
    pub subject: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Integrity {
    pub report_path: String,
    pub artifact_manifest_path: String,
    pub artifacts: Vec<ArtifactEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ArtifactEntry {
    pub kind: String,
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ArtifactManifest {
    pub schema_version: u32,
    pub run_id: String,
    pub entries: Vec<ArtifactEntry>,
}
