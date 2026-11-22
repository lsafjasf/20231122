//! Snapshot manifests in two on-disk formats.
//!
//! * Legacy format (`manifest.legacy.json`): a flat document without an
//!   envelope version marker, as produced by older tooling.
//! * Current format (`manifest.v2.json`): a versioned envelope with explicit
//!   schema field and structured sections.
//!
//! Both formats can be read back through [`SnapshotManifest::load_any`], which
//! is the compatibility entry point used by the verification flow.

use crate::error::{DownmixError, Result};
use crate::model::{
    AudioObject, CorrelationPoint, ProcessingParams, Revision, SnapshotStatus, WaveformSummary,
};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const CURRENT_SCHEMA: u32 = 2;
pub const LEGACY_SCHEMA: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileEvidence {
    pub path: String,
    pub sha256: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SnapshotManifest {
    pub schema: u32,
    pub revision: Revision,
    pub created_at: String,
    pub object: AudioObject,
    pub params: ProcessingParams,
    pub status: SnapshotStatus,
    pub output: Option<FileEvidence>,
    pub waveform: Option<WaveformSummary>,
    pub correlation: Vec<CorrelationPoint>,
    pub error: Option<String>,
}

/// Exact wire shape of the legacy document.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct LegacyWire {
    snapshot_revision: String,
    recorded_at: String,
    source_path: String,
    source_sha256: String,
    source_size: u64,
    unknown_extension: bool,
    recipe: String,
    matrix: Vec<f64>,
    input_channels: usize,
    output_channels: usize,
    sample_rate: u32,
    sample_format: String,
    pipeline: String,
    status: String,
    output_path: Option<String>,
    output_sha256: Option<String>,
    output_size: Option<u64>,
    correlation: Vec<CorrelationPoint>,
    error: Option<String>,
}

/// Exact wire shape of the current versioned envelope.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct CurrentWire {
    schema: u32,
    envelope: String,
    manifest: SnapshotManifest,
}

impl SnapshotManifest {
    pub fn write(&self, dir: &Path) -> Result<std::path::PathBuf> {
        let path = dir.join(self.revision.manifest_name());
        match self.revision {
            Revision::Legacy => {
                let wire = LegacyWire::from_manifest(self);
                let json = serde_json::to_string_pretty(&wire)
                    .map_err(|e| DownmixError::Manifest(e.to_string()))?;
                std::fs::write(&path, json).map_err(|e| DownmixError::io(&path, e))?;
            }
            Revision::Current => {
                let wire = CurrentWire {
                    schema: CURRENT_SCHEMA,
                    envelope: "surround-downmix/snapshot".to_string(),
                    manifest: self.clone(),
                };
                let json = serde_json::to_string_pretty(&wire)
                    .map_err(|e| DownmixError::Manifest(e.to_string()))?;
                std::fs::write(&path, json).map_err(|e| DownmixError::io(&path, e))?;
            }
        }
        Ok(path)
    }

    /// Parse either the legacy or the current format from a JSON byte slice.
    pub fn parse_slice(bytes: &[u8]) -> Result<Self> {
        if let Ok(wire) = serde_json::from_slice::<CurrentWire>(bytes) {
            if wire.schema == CURRENT_SCHEMA {
                return Ok(wire.manifest);
            }
            return Err(DownmixError::Manifest(format!(
                "unsupported envelope schema {}",
                wire.schema
            )));
        }
        let wire = serde_json::from_slice::<LegacyWire>(bytes)
            .map_err(|e| DownmixError::Manifest(format!("unrecognized manifest format: {e}")))?;
        Ok(wire.into_manifest())
    }

    pub fn load_any(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).map_err(|e| DownmixError::io(path, e))?;
        Self::parse_slice(&bytes)
    }
}

impl LegacyWire {
    fn from_manifest(m: &SnapshotManifest) -> Self {
        Self {
            snapshot_revision: m.revision.label().to_string(),
            recorded_at: m.created_at.clone(),
            source_path: m.object.path.to_string_lossy().into_owned(),
            source_sha256: m.object.sha256.clone(),
            source_size: m.object.size_bytes,
            unknown_extension: m.object.is_unknown_extension(),
            recipe: m.params.recipe.clone(),
            matrix: m.params.coefficient_matrix.clone(),
            input_channels: m.params.input_channels,
            output_channels: m.params.output_channels,
            sample_rate: m.params.sample_rate,
            sample_format: m.params.sample_format.clone(),
            pipeline: m.params.pipeline_description.clone(),
            status: format!("{:?}", m.status).to_lowercase(),
            output_path: m.output.as_ref().map(|f| f.path.clone()),
            output_sha256: m.output.as_ref().map(|f| f.sha256.clone()),
            output_size: m.output.as_ref().map(|f| f.size_bytes),
            correlation: m.correlation.clone(),
            error: m.error.clone(),
        }
    }

    fn into_manifest(self) -> SnapshotManifest {
        let status = match self.status.as_str() {
            "failed" => SnapshotStatus::Failed,
            "skipped" => SnapshotStatus::Skipped,
            _ => SnapshotStatus::Completed,
        };
        let output = match (self.output_path, self.output_sha256, self.output_size) {
            (Some(path), Some(sha256), Some(size_bytes)) => Some(FileEvidence {
                path,
                sha256,
                size_bytes,
            }),
            _ => None,
        };
        let revision = if self.snapshot_revision == "current" {
            Revision::Current
        } else {
            Revision::Legacy
        };
        let object = AudioObject {
            id: crate::discover::object_id_from_path(std::path::Path::new(&self.source_path)),
            path: self.source_path.clone().into(),
            extension: std::path::Path::new(&self.source_path)
                .extension()
                .map(|e| e.to_string_lossy().into_owned())
                .unwrap_or_default(),
            detection: if self.unknown_extension {
                crate::model::DetectionKind::SniffedContent
            } else {
                crate::model::DetectionKind::KnownExtension
            },
            size_bytes: self.source_size,
            sha256: self.source_sha256,
        };
        let params = ProcessingParams {
            revision,
            recipe: self.recipe,
            input_channels: self.input_channels,
            output_channels: self.output_channels,
            input_layout: Vec::new(),
            output_layout: Vec::new(),
            coefficient_matrix: self.matrix,
            sample_rate: self.sample_rate,
            sample_format: self.sample_format,
            pipeline_description: self.pipeline,
        };
        SnapshotManifest {
            schema: LEGACY_SCHEMA,
            revision,
            created_at: self.recorded_at,
            object,
            params,
            status,
            output,
            waveform: None,
            correlation: self.correlation,
            error: self.error,
        }
    }
}
