//! On-disk data model.
//!
//! Every serializable type uses `serde_json::Value`-compatible structs with a
//! fixed field declaration order. Combined with the `preserve_order` feature
//! this guarantees that reloading a record and writing it back again produces
//! byte-identical output (see `pipeline::roundtrip_check`).

use serde::{Deserialize, Serialize};

pub const CASE_SCHEMA: &str = "segment-fingerprint.case/v1";
pub const RUN_SCHEMA: &str = "segment-fingerprint.run/v1";
pub const EVIDENCE_SCHEMA: &str = "segment-fingerprint.evidence/v1";
pub const ALIGNMENT_SCHEMA: &str = "segment-fingerprint.alignment/v1";
pub const BUNDLE_SCHEMA: &str = "segment-fingerprint.bundle/v1";
pub const VERIFICATION_SCHEMA: &str = "segment-fingerprint.verification/v1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MicroRange {
    pub start_us: i64,
    pub end_us: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeclaredGap {
    pub gap_id: String,
    pub start_us: i64,
    pub end_us: i64,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeviceSpec {
    pub device_id: String,
    pub label: String,
    pub media_path: String,
    pub clock_epoch_unix_ms: i64,
    pub clock_drift_ppm: i64,
    #[serde(default)]
    pub declared_gaps: Vec<DeclaredGap>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SegmentSpec {
    pub segment_id: String,
    pub ordinal: usize,
    pub start_us: i64,
    pub duration_us: i64,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AlignmentPolicy {
    pub search_radius_us: i64,
    pub search_step_us: i64,
    pub expected_offset_us: i64,
    pub match_threshold_milli: u32,
    pub missing_threshold_milli: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResourcePolicy {
    pub min_free_disk_mb: u64,
    pub degrade_rss_mb: u64,
    pub pause_rss_mb: u64,
    pub reduced_sample_rate: u32,
    pub reduced_frame_us: i64,
    pub allow_wav_fallback: bool,
    pub fallback_to_file_window: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CaseFile {
    pub schema: String,
    pub case_id: String,
    pub title: String,
    pub description: String,
    pub devices: Vec<DeviceSpec>,
    pub segments: Vec<SegmentSpec>,
    pub alignment: AlignmentPolicy,
    pub resource_policy: ResourcePolicy,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ProcessMode {
    Strict,
    Reduced,
}

impl ProcessMode {
    pub fn as_str(self) -> &'static str {
        match self {
            ProcessMode::Strict => "strict",
            ProcessMode::Reduced => "reduced",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Paused,
    Complete,
    Failed,
}

impl RunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            RunStatus::Paused => "paused",
            RunStatus::Complete => "complete",
            RunStatus::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SegmentStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    Match,
    Divergent,
    Missing,
    Review,
}

impl Decision {
    pub fn as_str(self) -> &'static str {
        match self {
            Decision::Match => "match",
            Decision::Divergent => "divergent",
            Decision::Missing => "missing",
            Decision::Review => "review",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MissingEvidence {
    pub gap_id: String,
    pub device_id: String,
    pub source_clock_range: MicroRange,
    pub common_clock_range: MicroRange,
    pub overlap_us: i64,
    pub reason: String,
    pub declared_by_capture_metadata: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeviceFingerprintResult {
    pub device_id: String,
    pub requested_range: MicroRange,
    pub decoded_range: MicroRange,
    pub clock_offset_us: i64,
    pub source_path: String,
    pub source_size_bytes: u64,
    pub source_sha256: String,
    pub decode_backend: String,
    pub pipeline: String,
    pub used_fallback: bool,
    pub sample_rate_hz: u32,
    pub frame_us: i64,
    pub hop_us: i64,
    pub sample_count: usize,
    pub pcm_sha256: String,
    pub fingerprint_algorithm: String,
    pub fingerprint_sha256: String,
    pub feature_count: usize,
    pub evidence_file: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SegmentRecord {
    pub ordinal: usize,
    pub segment_id: String,
    pub label: String,
    pub common_range: MicroRange,
    pub status: SegmentStatus,
    pub decision: Decision,
    pub similarity_milli: u32,
    pub coverage_milli: u32,
    pub devices: Vec<DeviceFingerprintResult>,
    pub missing: Option<MissingEvidence>,
    pub degradations: Vec<DegradationEvent>,
    pub notes: Vec<String>,
    pub evidence_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DegradationEvent {
    pub sequence: u32,
    pub stage: String,
    pub segment_id: Option<String>,
    pub reason_code: String,
    pub detail: String,
    pub from_mode: String,
    pub to_mode: String,
    pub observed_rss_mb: u64,
    pub observed_free_disk_mb: u64,
    pub evidence_preserved: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AlignmentResult {
    pub status: String,
    pub anchor_device_id: String,
    pub compared_device_id: String,
    pub expected_offset_us: i64,
    pub observed_offset_us: i64,
    pub confidence_milli: u32,
    pub search_radius_us: i64,
    pub search_step_us: i64,
    pub samples_compared: u32,
    pub evidence_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RunMeta {
    pub run_id: String,
    pub status: RunStatus,
    pub mode: ProcessMode,
    pub created_at_unix_ms: i64,
    pub updated_at_unix_ms: i64,
    pub resumed_at_unix_ms: Option<i64>,
    pub resume_count: u32,
    pub pause_after_segments: Option<usize>,
    pub command_line: Vec<String>,
    pub cursor_segment_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CaseSnapshot {
    pub case_id: String,
    pub case_sha256: String,
    pub original_manifest_sha256: String,
    pub title: String,
    pub description: String,
    pub devices: Vec<DeviceSpec>,
    pub segments: Vec<SegmentSpec>,
    pub alignment: AlignmentPolicy,
    pub resource_policy: ResourcePolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResumeCheck {
    pub checkpoint_segment_count: usize,
    pub before_resume_digest: String,
    pub after_reload_digest: String,
    pub state_roundtrip_equal: bool,
    pub anchor_segment_id: String,
    pub anchor_before_digest: String,
    pub anchor_after_digest: String,
    pub anchor_content_equal: bool,
    pub checked_at_unix_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RunSummary {
    pub segments_total: usize,
    pub segments_completed: usize,
    pub segments_pending: usize,
    pub matches: usize,
    pub missing: usize,
    pub divergent: usize,
    pub review: usize,
    pub degradation_count: usize,
    pub result_digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EvidenceIndex {
    pub directory: String,
    pub manifest_file: String,
    pub review_file: String,
    pub checksums_file: String,
    pub fingerprint_files: Vec<String>,
    pub key_evidence: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VerificationInfo {
    pub independent_replay_command: String,
    pub checksums_sha256: String,
    pub last_verdict: String,
    pub last_verified_at_unix_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RunRecord {
    pub schema: String,
    pub format_version: u32,
    pub case: CaseSnapshot,
    pub run: RunMeta,
    pub alignment: AlignmentResult,
    pub segments: Vec<SegmentRecord>,
    pub summary: RunSummary,
    pub resume_checks: Vec<ResumeCheck>,
    pub degradation_events: Vec<DegradationEvent>,
    pub evidence: EvidenceIndex,
    pub verification: VerificationInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FingerprintFrame {
    pub offset_us: i64,
    pub bands: Vec<u8>,
    pub rms: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FingerprintEvidence {
    pub schema: String,
    pub segment_id: String,
    pub device_id: String,
    pub algorithm: String,
    pub sample_rate_hz: u32,
    pub frame_us: i64,
    pub hop_us: i64,
    pub requested_range: MicroRange,
    pub decoded_range: MicroRange,
    pub pcm_sha256: String,
    pub fingerprint_sha256: String,
    pub frames: Vec<FingerprintFrame>,
}
