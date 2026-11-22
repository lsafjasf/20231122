//! End-to-end orchestration: two revision snapshots per object, evidence
//! retention, temp-state cleanup and per-object fault isolation.

use crate::discover::{discover_objects, evidence_dir};
use crate::error::{DownmixError, Result};
use crate::hash::sha256_file;
use crate::manifest::{FileEvidence, SnapshotManifest};
use crate::model::{
    relative, AudioObject, CorrelationPoint, ProcessingParams, Revision, SnapshotStatus,
};
use crate::pipeline::{probe_channels, render_downmix};
use crate::recipe::build_params;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct RunSummary {
    pub started_at: String,
    pub finished_at: String,
    pub input_root: PathBuf,
    pub evidence_root: PathBuf,
    pub total_objects: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub skipped: usize,
    pub results: Vec<ObjectResult>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct ObjectResult {
    pub object_id: String,
    pub path: PathBuf,
    pub unknown_extension: bool,
    pub status: SnapshotStatus,
    pub detail: Option<String>,
    pub evidence_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct RunConfig {
    pub input_root: PathBuf,
    pub evidence_root: PathBuf,
}

fn now_utc() -> String {
    let secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

/// Run the full reviewable downmix flow over every object under `input_root`.
///
/// A failure on one object never aborts the batch; the error is recorded and
/// processing continues with the remaining objects.
pub fn run_batch(config: &RunConfig) -> Result<RunSummary> {
    let started_at = now_utc();
    std::fs::create_dir_all(&config.evidence_root)
        .map_err(|e| DownmixError::io(&config.evidence_root, e))?;
    let objects = discover_objects(&config.input_root)?;
    let mut results = Vec::with_capacity(objects.len());

    for object in &objects {
        let result = process_object(config, object);
        match result {
            Ok(r) => results.push(r),
            Err(err) => {
                log::error!("object {path:?} failed: {err}", path = object.path);
                record_failure(config, object, &err);
                results.push(ObjectResult {
                    object_id: object.id.clone(),
                    path: object.path.clone(),
                    unknown_extension: object.is_unknown_extension(),
                    status: SnapshotStatus::Failed,
                    detail: Some(err.to_string()),
                    evidence_dir: relative(&evidence_dir(&config.evidence_root, object), &config.evidence_root),
                });
            }
        }
    }

    let succeeded = results
        .iter()
        .filter(|r| matches!(r.status, SnapshotStatus::Completed))
        .count();
    let failed = results
        .iter()
        .filter(|r| matches!(r.status, SnapshotStatus::Failed))
        .count();
    let skipped = results
        .iter()
        .filter(|r| matches!(r.status, SnapshotStatus::Skipped))
        .count();
    let summary = RunSummary {
        started_at,
        finished_at: now_utc(),
        input_root: config.input_root.clone(),
        evidence_root: config.evidence_root.clone(),
        total_objects: objects.len(),
        succeeded,
        failed,
        skipped,
        results,
    };
    let summary_path = config.evidence_root.join("run-summary.json");
    let json = serde_json::to_string_pretty(&summary)
        .map_err(|e| DownmixError::Manifest(e.to_string()))?;
    std::fs::write(&summary_path, json).map_err(|e| DownmixError::io(summary_path, e))?;
    Ok(summary)
}

fn process_object(config: &RunConfig, object: &AudioObject) -> Result<ObjectResult> {
    let dir = evidence_dir(&config.evidence_root, object);
    std::fs::create_dir_all(&dir).map_err(|e| DownmixError::io(&dir, e))?;

    // Every temp artifact lives under this guard and is removed on all paths.
    let temp = tempfile::Builder::new()
        .prefix("downmix-tmp-")
        .tempdir_in(&config.evidence_root)
        .map_err(|e| DownmixError::io(&config.evidence_root, e))?;
    let outcome = render_both_revisions(object, &dir, temp.path());
    let status = match &outcome {
        Ok(()) => SnapshotStatus::Completed,
        Err(_) => SnapshotStatus::Failed,
    };
    // Explicit close; TempDir::drop would also remove, but we want errors logged.
    if let Err(err) = temp.close() {
        log::warn!("temp cleanup failed under {dir:?}: {err}");
    }
    outcome?;
    Ok(ObjectResult {
        object_id: object.id.clone(),
        path: object.path.clone(),
        unknown_extension: object.is_unknown_extension(),
        status,
        detail: None,
        evidence_dir: relative(&dir, &config.evidence_root),
    })
}

fn render_both_revisions(object: &AudioObject, dir: &Path, temp: &Path) -> Result<()> {
    for revision in [Revision::Legacy, Revision::Current] {
        render_one_revision(object, dir, temp, revision)?;
    }
    Ok(())
}

fn render_one_revision(
    object: &AudioObject,
    dir: &Path,
    temp: &Path,
    revision: Revision,
) -> Result<()> {
    let created_at = now_utc();
    let temp_wav = temp.join(format!("{}.wav", revision.label()));

    // Probe the real channel count before committing the coefficient matrix;
    // content sniffing alone cannot tell channel layout apart.
    let input_channels = probe_channels(&object.path)?;
    let params = build_params(revision, input_channels, 48_000, "F32LE")?;

    let manifest = match render_downmix(&object.path, &temp_wav, &params) {
        Ok((result, waveform)) => {
            let final_wav = dir.join(revision.output_name());
            std::fs::rename(&temp_wav, &final_wav)
                .map_err(|e| DownmixError::io(&final_wav, e))?;
            let size = metadata_len(&final_wav)?;
            let sha = sha256_file(&final_wav)?;
            let correlation: Vec<CorrelationPoint> = waveform.points.clone();
            SnapshotManifest {
                schema: crate::manifest::CURRENT_SCHEMA,
                revision,
                created_at,
                object: object.clone(),
                params: ProcessingParams::with_result(params, &result),
                status: SnapshotStatus::Completed,
                output: Some(FileEvidence {
                    path: relative(&final_wav, dir).to_string_lossy().into_owned(),
                    sha256: sha,
                    size_bytes: size,
                }),
                waveform: Some(waveform),
                correlation,
                error: None,
            }
        }
        Err(err) => SnapshotManifest {
            schema: crate::manifest::CURRENT_SCHEMA,
            revision,
            created_at,
            object: object.clone(),
            params,
            status: SnapshotStatus::Failed,
            output: None,
            waveform: None,
            correlation: Vec::new(),
            error: Some(err.to_string()),
        },
    };
    manifest.write(dir)?;
    if let Some(err) = manifest.error {
        return Err(DownmixError::Gst(err));
    }
    Ok(())
}

fn metadata_len(path: &Path) -> Result<u64> {
    std::fs::metadata(path)
        .map(|m| m.len())
        .map_err(|e| DownmixError::io(path, e))
}

fn record_failure(config: &RunConfig, object: &AudioObject, err: &DownmixError) {
    let dir = evidence_dir(&config.evidence_root, object);
    if let Err(io_err) = std::fs::create_dir_all(&dir) {
        log::error!("cannot create evidence dir {dir:?}: {io_err}");
        return;
    }
    for revision in [Revision::Legacy, Revision::Current] {
        let manifest = SnapshotManifest {
            schema: crate::manifest::CURRENT_SCHEMA,
            revision,
            created_at: now_utc(),
            object: object.clone(),
            params: crate::model::ProcessingParams::unavailable(revision),
            status: SnapshotStatus::Failed,
            output: None,
            waveform: None,
            correlation: Vec::new(),
            error: Some(err.to_string()),
        };
        if let Err(write_err) = manifest.write(&dir) {
            log::error!("cannot write failure manifest: {write_err}");
        }
    }
}
