//! Serde data model for snapshot fixtures and diagnosis results.

use serde::{Deserialize, Serialize};

/// A single persisted resume record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumeRecord {
    pub record_id: String,
    pub manifest_url: String,
    pub resume_position_ms: u64,
    pub segment_seq: u64,
    #[serde(default)]
    pub integrity: Option<String>,
    #[serde(default)]
    pub submitted_at: Option<String>,
}

/// A versioned snapshot file on disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub schema_version: u32,
    pub snapshot_id: String,
    #[serde(default)]
    pub captured_at: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub records: Vec<ResumeRecord>,
}

/// Why a snapshot file could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotParseError {
    /// File could not be read.
    Io(String),
    /// JSON document ended prematurely (the truncated control case).
    Truncated { path: String, source: String },
    /// JSON was syntactically invalid for another reason.
    InvalidJson { path: String, source: String },
}

impl std::fmt::Display for SnapshotParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SnapshotParseError::Io(e) => write!(f, "io error: {e}"),
            SnapshotParseError::Truncated { path, source } => {
                write!(f, "truncated snapshot {path}: {source}")
            }
            SnapshotParseError::InvalidJson { path, source } => {
                write!(f, "invalid json in {path}: {source}")
            }
        }
    }
}

impl std::error::Error for SnapshotParseError {}

impl Snapshot {
    /// Load and parse a snapshot, classifying truncation explicitly.
    pub fn load(path: &std::path::Path) -> Result<Self, SnapshotParseError> {
        let bytes = std::fs::read(path).map_err(|e| SnapshotParseError::Io(e.to_string()))?;
        let display = path.display().to_string();
        match serde_json::from_slice::<Snapshot>(&bytes) {
            Ok(snapshot) => Ok(snapshot),
            Err(err) if err.is_eof() => Err(SnapshotParseError::Truncated {
                path: display,
                source: err.to_string(),
            }),
            Err(err) => Err(SnapshotParseError::InvalidJson {
                path: display,
                source: err.to_string(),
            }),
        }
    }
}

/// Verdict computed for one resume record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosisResult {
    pub record_id: String,
    pub schema_version: u32,
    pub manifest_url: String,
    pub resume_position_ms: u64,
    pub accepted: bool,
    pub status: String,
    pub detail: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_is_classified() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.json");
        std::fs::write(&path, r#"{"schema_version":2,"records":[{"record_id":"x""#).unwrap();
        let err = Snapshot::load(&path).unwrap_err();
        assert!(matches!(err, SnapshotParseError::Truncated { .. }));
    }
}
