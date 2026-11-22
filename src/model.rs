//! Ordered data model for the subtitle font coverage report.
//!
//! Field order matters: serde serializes struct fields in declaration order,
//! and all maps use insertion order, so a save -> reload -> save round trip
//! is byte-stable.

use serde::{Deserialize, Serialize};
use serde_json::Map as OrderedMap;
use serde_json::Value;

pub const FORMAT_VERSION: u32 = 1;

/// A single subtitle cue extracted from the media object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cue {
    pub index: u64,
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
}

/// Coverage state of one Unicode scalar within the cue set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CoverageStatus {
    /// Covered by the primary font after full Pango shaping analysis.
    Covered,
    /// Covered only through a fallback font found by Pango itemization.
    CoveredByFallback,
    /// Not covered by any font known to the font map.
    Missing,
    /// Checked on the degraded path (charset probe only, no shaping).
    DegradedCheck,
}

/// Per-glyph coverage entry. `char` is stored as a one-char string so the
/// JSON document stays human-reviewable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoverageEntry {
    pub char: String,
    pub codepoint: String,
    pub status: CoverageStatus,
    pub font: Option<String>,
    pub occurrences: u64,
}

/// How a cue was processed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProcessingMode {
    Full,
    Degraded,
}

/// Per-cue record kept in the report (ordered by cue index).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CueRecord {
    pub cue_index: u64,
    pub start_ms: i64,
    pub end_ms: i64,
    pub mode: ProcessingMode,
    pub distinct_chars: u64,
    pub missing_chars: u64,
    /// Wall-clock timestamps from both devices (clocks are not synced).
    pub capture_ts_ms: i64,
    pub analysis_ts_ms: i64,
    /// analysis_ts normalized into the capture clock domain.
    pub normalized_analysis_ts_ms: i64,
    /// SHA-256 of "index|start|end|text" so reviewers can recompute.
    pub content_hash: String,
    /// SHA-256 over the per-glyph outcomes of this cue; recomputed in the
    /// post-resume verification window and compared with the pre-pause run.
    pub result_fingerprint: String,
}

/// A degradation event: resource pressure forced the cheap code path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DegradationEvent {
    pub seq: u64,
    pub at_cue_index: u64,
    pub reason: String,
    pub capture_ts_ms: i64,
    pub analysis_ts_ms: i64,
}

/// Result of the resume-consistency check: the verification window of cues
/// re-processed after resume must hash identically to the pre-pause pass.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResumeComparison {
    pub window_size: u64,
    pub pre_pause_hash: String,
    pub post_resume_hash: String,
    pub identical: bool,
}

/// Clock calibration between the capture device and the analysis device.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClockCalibration {
    pub capture_device: String,
    pub analysis_device: String,
    pub paired_samples: u64,
    /// Median of (analysis_ts - capture_ts) across beacon samples.
    pub estimated_offset_ms: i64,
    pub max_abs_deviation_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub session_id: String,
    pub started_capture_ts_ms: i64,
    pub started_analysis_ts_ms: i64,
    pub cues_from: u64,
    pub cues_to: u64,
    pub ended_by: String,
}

/// The exported report. Field declaration order is the JSON key order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub format_version: u32,
    pub report_kind: String,
    pub object_id: String,
    pub object_path: String,
    pub object_sha256: String,
    pub object_size_bytes: u64,
    pub generated_by: String,
    pub clock_calibration: ClockCalibration,
    pub total_cues: u64,
    pub processed_cues: u64,
    pub degraded_cues: u64,
    pub sessions: Vec<SessionInfo>,
    pub degradation_events: Vec<DegradationEvent>,
    pub resume_comparison: Option<ResumeComparison>,
    /// Glyph coverage, ordered by first occurrence in the cue stream.
    pub coverage: Vec<CoverageEntry>,
    pub cue_records: Vec<CueRecord>,
    pub evidence_sha256: String,
    pub report_sha256: String,
}

/// Pausable intermediate state, saved atomically as `checkpoint.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub format_version: u32,
    pub object_id: String,
    pub object_sha256: String,
    pub next_cue_index: u64,
    pub total_cues: u64,
    pub sessions: Vec<SessionInfo>,
    pub cue_records: Vec<CueRecord>,
    pub coverage: Vec<CoverageEntry>,
    pub degradation_events: Vec<DegradationEvent>,
    pub clock_samples: Vec<ClockSample>,
    /// Hash of the verification window processed just before the pause;
    /// recomputed after resume and compared.
    pub verification_window_hash: Option<String>,
    pub checkpoint_sha256: String,
}

/// One paired clock reading: (capture_ts_ms, analysis_ts_ms).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ClockSample {
    pub capture_ts_ms: i64,
    pub analysis_ts_ms: i64,
}

/// Ordered JSON map helper used by evidence events (insertion-ordered).
pub fn ordered_map(pairs: Vec<(&str, Value)>) -> OrderedMap<String, Value> {
    let mut m = OrderedMap::new();
    for (k, v) in pairs {
        m.insert(k.to_string(), v);
    }
    m
}

pub fn sha256_hex(data: &[u8]) -> String {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    h.update(data);
    hex_encode(&h.finalize())
}

pub fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

impl Cue {
    pub fn content_hash(&self) -> String {
        sha256_hex(
            format!("{}|{}|{}|{}", self.index, self.start_ms, self.end_ms, self.text).as_bytes(),
        )
    }
}

impl Checkpoint {
    /// Hash over everything except the trailing `checkpoint_sha256` field.
    pub fn compute_hash(&self) -> String {
        let mut clone = self.clone();
        clone.checkpoint_sha256 = String::new();
        sha256_hex(serde_json::to_string(&clone).unwrap().as_bytes())
    }
}

impl Report {
    pub fn compute_hash(&self) -> String {
        let mut clone = self.clone();
        clone.report_sha256 = String::new();
        sha256_hex(serde_json::to_string(&clone).unwrap().as_bytes())
    }
}
