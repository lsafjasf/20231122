use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::integrity::{
    ensure_safe_relative, evidence_file, sha256_bytes, sha256_file, verify_checksum_sidecar,
    write_checksum_sidecar, EvidenceFile,
};
use crate::model::{ProjectFormat, TypesetParameters};
use crate::project::ProjectSource;
use crate::waveform::{Peak, WaveformEvidence};
use crate::{CoreError, Result};

pub const REPORT_SCHEMA: &str = "credits-review/v2";
pub const REPORT_FILE: &str = "review.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInfo {
    pub name: String,
    pub version: String,
    pub backend: String,
    pub gstreamer_version: Option<String>,
    pub gtk_version: Option<String>,
}

impl Default for ToolInfo {
    fn default() -> Self {
        Self {
            name: "credits-review".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            backend: "rust + gstreamer + gtk4".to_string(),
            gstreamer_version: None,
            gtk_version: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ObjectStatus {
    Ok,
    Partial,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaEvidence {
    pub path: PathBuf,
    pub extension: Option<String>,
    pub unknown_extension: bool,
    pub size_bytes: u64,
    pub sha256: String,
    pub magic_hex: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StreamEvidence {
    pub index: u32,
    pub kind: String,
    pub caps: String,
    pub codec: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<f64>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProbeEvidence {
    pub uri: String,
    pub container: Option<String>,
    pub duration_s: Option<f64>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<f64>,
    pub streams: Vec<StreamEvidence>,
    pub discovery_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotVersion {
    Before,
    After,
}

impl SnapshotVersion {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Before => "before",
            Self::After => "after",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotEvidence {
    pub version: SnapshotVersion,
    pub label: String,
    pub file: String,
    pub sha256: String,
    pub width: u32,
    pub height: u32,
    pub pts_ns: Option<u64>,
    pub frame: u64,
    pub timecode: String,
    pub track: String,
    pub parameters_applied: bool,
    pub parameters: TypesetParameters,
    pub pipeline: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimePointEvidence {
    pub seconds: f64,
    pub clock: String,
    pub timecode: String,
    pub frame: u64,
    pub pts_ns: Option<u64>,
    pub fps: f64,
    pub track: String,
    pub requested: Option<String>,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PixelDiff {
    pub mean_abs: f64,
    pub changed_ratio: f64,
    pub compared_pixels: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorrelationEvidence {
    pub index: usize,
    pub time: TimePointEvidence,
    pub versions: Vec<SnapshotEvidence>,
    pub parameters: TypesetParameters,
    pub waveform_peak_index: usize,
    pub waveform_window: Vec<Peak>,
    #[serde(default)]
    pub waveform_file: String,
    pub pixel_diff: Option<PixelDiff>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectReview {
    pub id: String,
    pub status: ObjectStatus,
    pub media: MediaEvidence,
    pub probe: ProbeEvidence,
    pub correlations: Vec<CorrelationEvidence>,
    pub waveform: WaveformEvidence,
    #[serde(default)]
    pub waveform_json: String,
    #[serde(default)]
    pub waveform_svg: String,
    pub evidence_files: Vec<EvidenceFile>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailureRecord {
    pub object_id: String,
    pub stage: String,
    pub message: String,
    pub evidence_dir: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleanupReport {
    pub strategy: String,
    pub temp_path: String,
    pub removed: bool,
    pub residue: Vec<String>,
    pub completed_at_unix_ms: u64,
}

impl CleanupReport {
    pub fn pending(temp_path: impl Into<String>) -> Self {
        Self {
            strategy: "raii tempfile guard; key evidence is outside the temp tree".to_string(),
            temp_path: temp_path.into(),
            removed: false,
            residue: Vec::new(),
            completed_at_unix_ms: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ReviewSummary {
    pub object_count: usize,
    pub ok_count: usize,
    pub partial_count: usize,
    pub failed_count: usize,
    pub correlation_count: usize,
    pub snapshot_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewReport {
    pub schema: String,
    pub generated_at_unix_ms: u64,
    pub tool: ToolInfo,
    pub source_project: ProjectSource,
    pub project_title: String,
    pub project_format: ProjectFormat,
    pub fps: f64,
    pub summary: ReviewSummary,
    pub objects: Vec<ObjectReview>,
    pub failures: Vec<FailureRecord>,
    pub warnings: Vec<String>,
    pub cleanup: CleanupReport,
    pub normalized_project: Option<EvidenceFile>,
    pub evidence_files: Vec<EvidenceFile>,
}

impl ReviewReport {
    pub fn new(
        source_project: ProjectSource,
        project_title: String,
        format: ProjectFormat,
        fps: f64,
    ) -> Self {
        Self {
            schema: REPORT_SCHEMA.to_string(),
            generated_at_unix_ms: now_unix_ms(),
            tool: ToolInfo::default(),
            source_project,
            project_title,
            project_format: format,
            fps,
            summary: ReviewSummary::default(),
            objects: Vec::new(),
            failures: Vec::new(),
            warnings: Vec::new(),
            cleanup: CleanupReport::pending("<pending>"),
            normalized_project: None,
            evidence_files: Vec::new(),
        }
    }

    pub fn recompute_summary(&mut self) {
        let mut summary = ReviewSummary {
            object_count: self.objects.len() + self.failures.len(),
            failed_count: self.failures.len(),
            ..ReviewSummary::default()
        };
        for object in &self.objects {
            match object.status {
                ObjectStatus::Ok => summary.ok_count += 1,
                ObjectStatus::Partial => summary.partial_count += 1,
                ObjectStatus::Failed => summary.failed_count += 1,
            }
            summary.correlation_count += object.correlations.len();
            summary.snapshot_count += object
                .correlations
                .iter()
                .map(|correlation| correlation.versions.len())
                .sum::<usize>();
        }
        self.summary = summary;
    }

    pub fn add_evidence(&mut self, root: &Path, path: &Path) -> Result<()> {
        let entry = evidence_file(root, path)?;
        if !self
            .evidence_files
            .iter()
            .any(|existing| existing.path == entry.path)
        {
            self.evidence_files.push(entry);
        }
        Ok(())
    }

    pub fn write(&mut self, out_dir: &Path) -> Result<PathBuf> {
        self.recompute_summary();
        std::fs::create_dir_all(out_dir).map_err(|source| CoreError::Write {
            path: out_dir.to_path_buf(),
            source,
        })?;
        let report_path = out_dir.join(REPORT_FILE);
        let bytes = serde_json::to_vec_pretty(self)?;
        std::fs::write(&report_path, &bytes).map_err(|source| CoreError::Write {
            path: report_path.clone(),
            source,
        })?;
        let checksum = sha256_bytes(&bytes);
        write_checksum_sidecar(&report_path, &checksum)?;
        Ok(report_path)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationSummary {
    pub report_path: PathBuf,
    pub schema: String,
    pub checked_files: usize,
    pub failures: Vec<String>,
}

impl VerificationSummary {
    pub fn ok(&self) -> bool {
        self.failures.is_empty()
    }
}

pub fn load_report(path: &Path) -> Result<ReviewReport> {
    let report_path = if path.is_dir() {
        path.join(REPORT_FILE)
    } else {
        path.to_path_buf()
    };
    let bytes = std::fs::read(&report_path).map_err(|source| CoreError::Read {
        path: report_path.clone(),
        source,
    })?;
    let report: ReviewReport = serde_json::from_slice(&bytes)?;
    if report.schema != REPORT_SCHEMA {
        return Err(CoreError::UnsupportedSchema(report.schema));
    }
    Ok(report)
}

pub fn verify_report(path: &Path) -> Result<VerificationSummary> {
    let report_path = if path.is_dir() {
        path.join(REPORT_FILE)
    } else {
        path.to_path_buf()
    };
    let report = load_report(&report_path)?;
    let root = report_path.parent().unwrap_or_else(|| Path::new("."));
    let sidecar_checksum = verify_checksum_sidecar(&report_path)?;
    let actual_report_hash = sha256_file(&report_path)?;
    if sidecar_checksum != actual_report_hash {
        return Err(CoreError::Integrity(format!(
            "{} sidecar does not match report",
            report_path.display()
        )));
    }
    let mut failures = Vec::new();
    let mut checked_files = 0_usize;
    for entry in &report.evidence_files {
        let relative = match ensure_safe_relative(&entry.path) {
            Ok(relative) => relative,
            Err(error) => {
                failures.push(error.to_string());
                continue;
            }
        };
        let target = root.join(relative);
        match sha256_file(&target) {
            Ok(actual) if actual == entry.sha256 => checked_files += 1,
            Ok(actual) => failures.push(format!(
                "{} hash mismatch: expected {}, got {}",
                entry.path, entry.sha256, actual
            )),
            Err(error) => failures.push(error.to_string()),
        }
    }
    if let Some(entry) = &report.normalized_project {
        if let Ok(relative) = ensure_safe_relative(&entry.path) {
            let target = root.join(relative);
            match sha256_file(&target) {
                Ok(actual) if actual == entry.sha256 => checked_files += 1,
                Ok(actual) => failures.push(format!(
                    "{} hash mismatch: expected {}, got {}",
                    entry.path, entry.sha256, actual
                )),
                Err(error) => failures.push(error.to_string()),
            }
        }
    }
    Ok(VerificationSummary {
        report_path,
        schema: report.schema,
        checked_files,
        failures,
    })
}

pub fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_counts_snapshots() {
        let mut report = ReviewReport::new(
            ProjectSource {
                path: PathBuf::from("x.json"),
                format: ProjectFormat::LegacyV1,
                extension: Some("json".to_string()),
                unknown_extension: false,
                size_bytes: 0,
                sha256: String::new(),
                magic_hex: String::new(),
            },
            "x".to_string(),
            ProjectFormat::LegacyV1,
            24.0,
        );
        report.failures.push(FailureRecord {
            object_id: "broken".to_string(),
            stage: "probe".to_string(),
            message: "x".to_string(),
            evidence_dir: None,
        });
        report.recompute_summary();
        assert_eq!(report.summary.object_count, 1);
        assert_eq!(report.summary.failed_count, 1);
    }
}
