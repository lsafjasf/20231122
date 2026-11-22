//! Persistent project model.
//!
//! The field order here is the canonical on-disk order: serde serializes
//! struct fields in declaration order and the project stores fragments in a
//! `Vec`, so reloading an artifact preserves both field names and ordering.

use serde::{Deserialize, Serialize};

pub const PROJECT_SCHEMA_VERSION: u32 = 1;

/// Lifecycle state of a single fragment. Stored verbatim in the project file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FragmentStatus {
    /// Registered, never processed.
    Pending,
    /// A processing attempt is/was running; `last_error` tells them apart.
    Running,
    /// Finished and evidence recorded.
    Done,
    /// Last attempt failed; the fragment can be rerun independently.
    Failed,
}

impl FragmentStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            FragmentStatus::Pending => "pending",
            FragmentStatus::Running => "running",
            FragmentStatus::Done => "done",
            FragmentStatus::Failed => "failed",
        }
    }
}

/// Immutable evidence captured from one concrete processing attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunEvidence {
    /// 1-based attempt counter, never reused.
    pub attempt: u32,
    /// RFC 3339 UTC timestamp of completion.
    pub finished_at: String,
    /// Full GStreamer pipeline description, so a reviewer can rerun it.
    pub pipeline_desc: String,
    pub gst_version: String,
    pub input_path: String,
    pub input_sha256: String,
    pub input_sample_rate: u32,
    pub input_channels: u32,
    /// Exact PCM frame count read from the WAV header (0 when unavailable).
    pub input_frames: u64,
    pub output_path: String,
    pub output_sha256: String,
    pub output_sample_rate: u32,
    pub output_channels: u32,
    pub output_frames: u64,
    /// Expected frame count given the source duration and target rate.
    pub expected_frames: u64,
    /// Silence frames synthesized for the declared missing segment.
    pub gap_frames_inserted: u64,
    pub duration_ns: u64,
    /// Byte size of the output artifact.
    pub output_bytes: u64,
}

/// A separately re-runnable media fragment with its missing-segment gap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fragment {
    /// Stable identifier, unique inside the project (`f0001`, ...).
    pub id: String,
    /// Human/device label, e.g. the recording device the clip came from.
    pub device_label: String,
    /// Source media file (any GStreamer-decodable audio container).
    pub source_path: String,
    /// Clock rate the source device nominally ran at.
    pub source_rate: u32,
    /// Common project rate every fragment is converted to.
    pub target_rate: u32,
    /// Start of the missing segment on the source timeline, in nanoseconds.
    pub gap_start_ns: u64,
    /// Length of the missing segment caused by clock desynchronization.
    pub gap_duration_ns: u64,
    /// Free-text justification for the gap boundaries.
    pub gap_reason: String,
    pub status: FragmentStatus,
    /// Attempts already consumed, including failed ones.
    pub attempts: u32,
    /// Relative path of the latest output under the project directory.
    pub output_rel_path: Option<String>,
    /// Last failure message, cleared on success.
    pub last_error: Option<String>,
    /// Full history; the last entry is the current accepted evidence.
    pub evidence: Vec<RunEvidence>,
    /// Updated on every mutation; used to detect external edits.
    pub updated_at: String,
}

impl Fragment {
    pub fn new(
        id: String,
        device_label: String,
        source_path: String,
        source_rate: u32,
        target_rate: u32,
        gap_start_ns: u64,
        gap_duration_ns: u64,
        gap_reason: String,
        now: String,
    ) -> Self {
        Self {
            id,
            device_label,
            source_path,
            source_rate,
            target_rate,
            gap_start_ns,
            gap_duration_ns,
            gap_reason,
            status: FragmentStatus::Pending,
            attempts: 0,
            output_rel_path: None,
            last_error: None,
            evidence: Vec::new(),
            updated_at: now,
        }
    }
}

/// Whole resumable workspace, serialized as `project.srcrepair.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub schema_version: u32,
    pub project_id: String,
    pub created_at: String,
    pub updated_at: String,
    /// Common output sample rate every fragment must share.
    pub target_rate: u32,
    /// Ordered fragments; order is preserved on save/reload.
    pub fragments: Vec<Fragment>,
}

impl Project {
    pub fn new(project_id: String, target_rate: u32, now: String) -> Self {
        Self {
            schema_version: PROJECT_SCHEMA_VERSION,
            project_id,
            created_at: now.clone(),
            updated_at: now,
            target_rate,
            fragments: Vec::new(),
        }
    }

    pub fn next_fragment_id(&self) -> String {
        format!("f{:04}", self.fragments.len() + 1)
    }

    pub fn find_index(&self, id: &str) -> Option<usize> {
        self.fragments.iter().position(|f| f.id == id)
    }
}
