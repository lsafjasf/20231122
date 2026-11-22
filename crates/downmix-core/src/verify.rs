//! Independent re-verification of retained evidence.
//!
//! Verification never trusts the manifest at face value: it re-hashes the
//! referenced files from disk and re-checks the correlation rows against the
//! stored waveform summary. Both legacy and current manifests are accepted.

use crate::discover::evidence_dir;
use crate::error::{DownmixError, Result};
use crate::hash::sha256_file;
use crate::manifest::SnapshotManifest;
use crate::model::{Revision, SnapshotStatus};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct VerificationReport {
    pub evidence_root: PathBuf,
    pub object_reports: Vec<ObjectVerification>,
    pub clean: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct ObjectVerification {
    pub object_id: String,
    pub source_path: PathBuf,
    pub legacy: SnapshotCheck,
    pub current: SnapshotCheck,
    pub ok: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct SnapshotCheck {
    pub revision: Revision,
    pub manifest_present: bool,
    pub output_hash_matches: bool,
    pub correlation_consistent: bool,
    pub status: Option<SnapshotStatus>,
    pub problems: Vec<String>,
}

impl SnapshotCheck {
    fn missing(revision: Revision) -> Self {
        Self {
            revision,
            manifest_present: false,
            output_hash_matches: false,
            correlation_consistent: false,
            status: None,
            problems: vec!["manifest missing".to_string()],
        }
    }

    fn ok(&self) -> bool {
        self.manifest_present && self.correlation_consistent && self.problems.is_empty()
    }
}

pub fn verify_evidence(
    evidence_root: &Path,
    objects: &[crate::model::AudioObject],
) -> Result<VerificationReport> {
    let mut object_reports = Vec::new();
    for object in objects {
        let dir = evidence_dir(evidence_root, object);
        let legacy = check_snapshot(&dir, Revision::Legacy)?;
        let current = check_snapshot(&dir, Revision::Current)?;
        let ok = legacy.ok() && current.ok();
        object_reports.push(ObjectVerification {
            object_id: object.id.clone(),
            source_path: object.path.clone(),
            legacy,
            current,
            ok,
        });
    }
    let clean = object_reports.iter().all(|r| r.ok);
    Ok(VerificationReport {
        evidence_root: evidence_root.to_path_buf(),
        object_reports,
        clean,
    })
}

fn check_snapshot(dir: &Path, revision: Revision) -> Result<SnapshotCheck> {
    let manifest_path = dir.join(revision.manifest_name());
    if !manifest_path.exists() {
        return Ok(SnapshotCheck::missing(revision));
    }
    let manifest = SnapshotManifest::load_any(&manifest_path)?;
    let mut problems = Vec::new();
    let mut output_hash_matches = false;

    match (&manifest.status, &manifest.output) {
        (SnapshotStatus::Completed, Some(evidence)) => {
            let output_path = dir.join(&evidence.path);
            if !output_path.exists() {
                problems.push(format!("output missing: {}", evidence.path));
            } else {
                let actual = sha256_file(&output_path)?;
                let actual_len = std::fs::metadata(&output_path)
                    .map(|m| m.len())
                    .unwrap_or(0);
                if actual != evidence.sha256 {
                    problems.push("output sha256 mismatch".to_string());
                }
                if actual_len != evidence.size_bytes {
                    problems.push("output size mismatch".to_string());
                }
                output_hash_matches =
                    actual == evidence.sha256 && actual_len == evidence.size_bytes;
            }
        }
        (SnapshotStatus::Completed, None) => {
            problems.push("completed snapshot without output evidence".to_string());
        }
        (SnapshotStatus::Failed, None) => {
            if manifest.error.is_none() {
                problems.push("failed snapshot without recorded error".to_string());
            }
        }
        (SnapshotStatus::Skipped, _) => {
            problems.push("snapshot was skipped".to_string());
        }
        (SnapshotStatus::Failed, Some(_)) => {
            problems.push("failed snapshot must not reference an output".to_string());
        }
    }

    let correlation_consistent = check_correlation(&manifest, &mut problems);
    Ok(SnapshotCheck {
        revision,
        manifest_present: true,
        output_hash_matches,
        correlation_consistent,
        status: Some(manifest.status),
        problems,
    })
}

fn check_correlation(
    manifest: &SnapshotManifest,
    problems: &mut Vec<String>,
) -> bool {
    if matches!(manifest.status, SnapshotStatus::Failed | SnapshotStatus::Skipped) {
        if manifest.error.is_some() {
            return true;
        }
        problems.push("non-completed snapshot without error detail".to_string());
        return false;
    }
    if manifest.correlation.is_empty() {
        problems.push("correlation table empty".to_string());
        return false;
    }
    let mut previous_frame = None;
    for point in &manifest.correlation {
        if point.frame_sha256.len() != 64 {
            problems.push("correlation frame hash malformed".to_string());
            return false;
        }
        if let Some(prev) = previous_frame {
            if point.frame_index <= prev {
                problems.push("correlation frames not monotonic".to_string());
                return false;
            }
        }
        previous_frame = Some(point.frame_index);
    }
    if let Some(wave) = &manifest.waveform {
        let last = manifest.correlation.last().expect("non-empty checked above");
        if last.frame_index >= wave.total_frames {
            problems.push("correlation frame beyond waveform length".to_string());
            return false;
        }
        if wave.duration_ms == 0 {
            problems.push("waveform duration is zero".to_string());
            return false;
        }
    }
    true
}

/// Remove transient directories left behind by an interrupted run. Returns the
/// number of entries removed.
pub fn clean_temp_state(evidence_root: &Path) -> Result<usize> {
    if !evidence_root.exists() {
        return Ok(0);
    }
    let mut removed = 0;
    for entry in std::fs::read_dir(evidence_root)
        .map_err(|e| DownmixError::io(evidence_root, e))?
    {
        let entry = entry.map_err(|e| DownmixError::io(evidence_root, e))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("downmix-tmp-") {
            std::fs::remove_dir_all(entry.path())
                .map_err(|e| DownmixError::io(entry.path(), e))?;
            removed += 1;
        }
    }
    Ok(removed)
}
