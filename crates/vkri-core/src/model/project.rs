use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::model::ids::new_id;
use crate::time::ClockOffset;

/// Schema version embedded in every project file.
pub const SCHEMA_VERSION: u32 = 1;

/// Lifecycle state of a single media segment. Stored as a stable string enum
/// so project files remain readable across builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentStatus {
    /// Registered, never extracted.
    Pending,
    /// An extraction run finished.
    Extracted,
    /// The last extraction run was interrupted or failed; evidence from prior
    /// successful runs is retained.
    Interrupted,
    /// Re-extraction is requested; the previous keyframes stay available until
    /// a replacement run completes successfully.
    Stale,
}

impl SegmentStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            SegmentStatus::Pending => "pending",
            SegmentStatus::Extracted => "extracted",
            SegmentStatus::Interrupted => "interrupted",
            SegmentStatus::Stale => "stale",
        }
    }
}

/// Strategy used to decide which decoded frames become keyframes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyframeMode {
    /// Keep frames whose PTS delta from the last kept frame reaches
    /// `interval_ns` (uniform sampling, deterministic).
    Interval,
    /// Keep every decoder keyframe / IDR frame reported by GStreamer.
    KeyframeOnly,
}

/// Per-segment extraction configuration. Re-running with a changed config
/// produces a new run record while previous evidence remains on disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractionConfig {
    pub mode: KeyframeMode,
    pub interval_ns: i64,
    pub thumbnail_max_edge: u32,
    pub jpeg_quality: u32,
}

impl Default for ExtractionConfig {
    fn default() -> Self {
        Self {
            mode: KeyframeMode::Interval,
            interval_ns: 1_000_000_000,
            thumbnail_max_edge: 480,
            jpeg_quality: 85,
        }
    }
}

/// One recording device. Two devices observing the same repeated object are
/// expected to have unsynchronised clocks; each carries its own offset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    pub id: String,
    pub label: String,
    pub clock_offset: ClockOffset,
    pub notes: String,
}

impl Device {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            id: new_id("dev"),
            label: label.into(),
            clock_offset: ClockOffset::zero(),
            notes: String::new(),
        }
    }
}

/// A frame selected as a keyframe. The thumbnail hash is the content address
/// used by evidence directories and export manifests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Keyframe {
    pub index: u32,
    pub pts_ns: i64,
    pub device_time_unix_ns: i64,
    pub reference_time_unix_ns: i64,
    pub is_decoder_keyframe: bool,
    pub thumbnail_sha256: String,
    pub thumbnail_file: String,
    pub thumbnail_bytes: u64,
}

/// Append-only evidence of one extraction attempt on a segment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunRecord {
    pub run_id: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub completed: bool,
    pub cancelled: bool,
    pub config: ExtractionConfig,
    pub frames_seen: u64,
    pub keyframes_written: u64,
    pub last_pts_ns: Option<i64>,
    pub message: String,
}

/// A media fragment belonging to one device. Every segment is an independent
/// unit of work with its own status, config, keyframes and audit trail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaSegment {
    pub id: String,
    pub device_id: String,
    pub label: String,
    pub source_path: PathBuf,
    pub source_sha256: String,
    pub source_bytes: u64,
    pub duration_ns: Option<i64>,
    pub clock_origin_unix_ns: i64,
    pub group_id: String,
    pub status: SegmentStatus,
    pub config: ExtractionConfig,
    pub keyframes: Vec<Keyframe>,
    pub runs: Vec<RunRecord>,
    pub last_error: Option<String>,
}

impl MediaSegment {
    pub fn new(
        device_id: impl Into<String>,
        label: impl Into<String>,
        source_path: PathBuf,
        clock_origin_unix_ns: i64,
        group_id: impl Into<String>,
    ) -> Self {
        Self {
            id: new_id("seg"),
            device_id: device_id.into(),
            label: label.into(),
            source_path,
            source_sha256: String::new(),
            source_bytes: 0,
            duration_ns: None,
            clock_origin_unix_ns,
            group_id: group_id.into(),
            status: SegmentStatus::Pending,
            config: ExtractionConfig::default(),
            keyframes: Vec::new(),
            runs: Vec::new(),
            last_error: None,
        }
    }

    pub fn is_runnable(&self) -> bool {
        matches!(
            self.status,
            SegmentStatus::Pending | SegmentStatus::Interrupted | SegmentStatus::Stale
        ) || matches!(self.status, SegmentStatus::Extracted)
    }
}

/// One matched keyframe pair inside a comparison report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComparisonEntry {
    pub pair_index: u32,
    pub segment_a_id: String,
    pub segment_b_id: String,
    pub keyframe_a_index: u32,
    pub keyframe_b_index: u32,
    pub reference_time_a_unix_ns: i64,
    pub reference_time_b_unix_ns: i64,
    pub delta_ns: i64,
    pub thumbnail_identical: bool,
}

/// A pairwise comparison report between two segments (typically one per device
/// for the same repeated object / group).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comparison {
    pub id: String,
    pub created_at: String,
    pub group_id: String,
    pub segment_a_id: String,
    pub segment_b_id: String,
    pub match_window_ns: i64,
    pub entries: Vec<ComparisonEntry>,
    pub median_delta_ns: Option<i64>,
    pub suggested_offset_b_ns: Option<i64>,
    pub notes: String,
}

/// Build provenance recorded in every project file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppInfo {
    pub app: String,
    pub schema_version: u32,
    pub app_version: String,
    pub created_at: String,
    pub updated_at: String,
}

/// Root project document. Vectors are the source of truth for ordering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub app_info: AppInfo,
    pub name: String,
    pub description: String,
    pub evidence_dir: String,
    pub devices: Vec<Device>,
    pub segments: Vec<MediaSegment>,
    pub comparisons: Vec<Comparison>,
}

impl Project {
    pub fn segment_mut(&mut self, id: &str) -> Option<&mut MediaSegment> {
        self.segments.iter_mut().find(|segment| segment.id == id)
    }

    pub fn segment(&self, id: &str) -> Option<&MediaSegment> {
        self.segments.iter().find(|segment| segment.id == id)
    }

    pub fn device(&self, id: &str) -> Option<&Device> {
        self.devices.iter().find(|device| device.id == id)
    }

    /// Segments for the same repeated object, in registration order.
    pub fn segments_in_group(&self, group_id: &str) -> Vec<&MediaSegment> {
        self.segments
            .iter()
            .filter(|segment| segment.group_id == group_id)
            .collect()
    }

    pub fn touch(&mut self) {
        self.app_info.updated_at = crate::time::format_unix_nanos(
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default(),
        );
    }
}
