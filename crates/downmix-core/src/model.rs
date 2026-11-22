use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// How an input file was recognized.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DetectionKind {
    /// File extension was recognized directly.
    KnownExtension,
    /// Extension was unknown/missing; format was recovered via content sniffing.
    SniffedContent,
}

/// A single discovered audio object on disk.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AudioObject {
    pub id: String,
    pub path: PathBuf,
    pub extension: String,
    pub detection: DetectionKind,
    pub size_bytes: u64,
    pub sha256: String,
}

impl AudioObject {
    pub fn is_unknown_extension(&self) -> bool {
        matches!(self.detection, DetectionKind::SniffedContent)
    }
}

/// Downmix revision recorded for one object. Two revisions are always kept so
/// reviewers can compare the legacy recipe against the current recipe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Revision {
    /// Legacy downmix recipe paired with the legacy manifest envelope.
    Legacy,
    /// Current downmix recipe paired with the versioned manifest envelope.
    Current,
}

impl Revision {
    pub fn label(self) -> &'static str {
        match self {
            Revision::Legacy => "legacy",
            Revision::Current => "current",
        }
    }
    pub fn manifest_name(self) -> &'static str {
        match self {
            Revision::Legacy => "manifest.legacy.json",
            Revision::Current => "manifest.v2.json",
        }
    }
    pub fn output_name(self) -> &'static str {
        match self {
            Revision::Legacy => "downmix.legacy.wav",
            Revision::Current => "downmix.current.wav",
        }
    }
}

/// Fully described processing parameters for one downmix run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProcessingParams {
    pub revision: Revision,
    pub recipe: String,
    pub input_channels: usize,
    pub output_channels: usize,
    pub input_layout: Vec<String>,
    pub output_layout: Vec<String>,
    /// Flat row-major matrix: row = output channel, column = input channel.
    pub coefficient_matrix: Vec<f64>,
    pub sample_rate: u32,
    pub sample_format: String,
    pub pipeline_description: String,
}

impl ProcessingParams {
    /// Override negotiated fields from the actual render result.
    pub fn with_result(mut self, result: &crate::pipeline::DownmixResult) -> Self {
        self.sample_rate = result.sample_rate;
        self.input_channels = result.input_channels;
        self.output_channels = result.output_channels;
        self
    }

    /// Placeholder used when a pipeline could not even be constructed.
    pub fn unavailable(revision: Revision) -> Self {
        Self {
            revision,
            recipe: "unavailable".to_string(),
            input_channels: 0,
            output_channels: 0,
            input_layout: Vec::new(),
            output_layout: Vec::new(),
            coefficient_matrix: Vec::new(),
            sample_rate: 0,
            sample_format: "unknown".to_string(),
            pipeline_description: String::new(),
        }
    }
}

/// One correlation row tying a time point to frame index and waveform evidence.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CorrelationPoint {
    pub pts_ns: u64,
    pub time_ms: u64,
    pub frame_index: u64,
    pub samples_per_frame: usize,
    pub peak: f64,
    pub rms: f64,
    pub frame_sha256: String,
}

/// Per-snapshot waveform summary of the rendered output file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WaveformSummary {
    pub total_frames: u64,
    pub duration_ms: u64,
    pub peak: f64,
    pub rms: f64,
    pub dc_offset: f64,
    pub points: Vec<CorrelationPoint>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotStatus {
    Completed,
    Failed,
    Skipped,
}

/// Evidence paths are always stored relative to the evidence root.
pub fn relative(path: &Path, root: &Path) -> PathBuf {
    path.strip_prefix(root)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| path.to_path_buf())
}
