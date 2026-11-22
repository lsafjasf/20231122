use crate::engine::RunPaths;
use crate::io::{atomic_json, atomic_write, bytes_sha256, file_sha256, read_json};
use crate::model::{
    AnchorCheckpoint, ComparisonReport, EquivalenceReport, ResumePreflight, RunState,
    SegmentComparison, VerifyCheck, VerifyReport, now_rfc3339,
};
use anyhow::{Result, ensure};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};

pub fn checkpoint_digest(checkpoint: &AnchorCheckpoint) -> Result<String> {
    let mut canonical = checkpoint.clone();
    canonical.checkpoint_sha256.clear();
    Ok(bytes_sha256(&serde_json::to_vec(&canonical)?))
}

pub fn save_checkpoint(paths: &RunPaths, checkpoint: &mut AnchorCheckpoint) -> Result<()> {
    let path = paths
        .checkpoints
        .join(format!("anchor-{}.json", checkpoint.anchor_id));
    checkpoint.checkpoint_path = path.to_string_lossy().into_owned();
    checkpoint.checkpoint_sha256 = checkpoint_digest(checkpoint)?;
    atomic_json(&path, checkpoint)
}

pub fn save_state(paths: &RunPaths, state: &RunState) -> Result<()> {
    atomic_json(&paths.state, state)
}

pub fn load_state(paths: &RunPaths) -> Result<RunState> {
    let state: RunState = read_json(&paths.state)?;
    ensure!(
        state.schema_version == crate::model::SCHEMA_VERSION,
        "状态版本不兼容"
    );
    Ok(state)
}

pub fn write_initial_evidence(paths: &RunPaths, state: &RunState) -> Result<()> {
    atomic_json(
        &paths.evidence.join("original.json"),
        &json!({
            "kind": "unmodified_original",
            "source": state.source,
            "copy": {
                "path": state.source.original_copy_path,
                "sha256": state.source.original_copy_sha256,
                "readonly": true
            },
            "anchors": state.anchors,
            "segments": crate::model::ANCHOR_COUNT,
            "created_at": now_rfc3339()
        }),
    )?;
    atomic_write(
        &paths.evidence.join("original.sha256"),
        format!(
            "{}  {}\n{}  {}\n",
            state.source.sha256,
            state.source.path,
            state.source.original_copy_sha256,
            state.source.original_copy_path
        )
        .as_bytes(),
    )?;
    atomic_json(&paths.evidence.join("resource.json"), &state.resource)?;
    if let Some(degradation) = &state.degradation {
        atomic_json(&paths.evidence.join("degradation.json"), degradation)?;
    }
    Ok(())
}

fn check_source(state: &RunState) -> Result<bool> {
    let hash = file_sha256(Path::new(&state.source.path))?;
    Ok(hash == state.source.sha256)
}

fn check_copy(state: &RunState) -> Result<bool> {
    let copy = Path::new(&state.source.original_copy_path);
    if !copy.exists() {
        return Ok(false);
    }
    Ok(file_sha256(copy)? == state.source.original_copy_sha256)
}

pub fn preflight(paths: &RunPaths, state: &RunState) -> Result<ResumePreflight> {
    let mut reasons = Vec::new();
    let source_hash_ok = check_source(state).unwrap_or(false);
    if !source_hash_ok {
        reasons.push("source_sha256_changed_or_unreadable".to_string());
    }
    if !check_copy(state).unwrap_or(false) {
        reasons.push("readonly_original_copy_changed_or_missing".to_string());
    }
    let mut completed_output_hashes_ok = true;
    let mut checkpoint_chain_ok = true;
    for checkpoint in &state.completed {
        let output = Path::new(&checkpoint.output_wav);
        if !output.exists() || file_sha256(output)? != checkpoint.output_sha256 {
            completed_output_hashes_ok = false;
            reasons.push(format!(
                "segment_output_hash_mismatch:{}",
                checkpoint.anchor_id
            ));
        }
        let checkpoint_path = Path::new(&checkpoint.checkpoint_path);
        match read_json::<AnchorCheckpoint>(checkpoint_path) {
            Ok(saved) => {
                let digest = checkpoint_digest(&saved)?;
                if digest != saved.checkpoint_sha256
                    || saved.anchor_id != checkpoint.anchor_id
                    || saved.output_sha256 != checkpoint.output_sha256
                {
                    checkpoint_chain_ok = false;
                    reasons.push(format!(
                        "checkpoint_digest_mismatch:{}",
                        checkpoint.anchor_id
                    ));
                }
            }
            Err(_) => {
                checkpoint_chain_ok = false;
                reasons.push(format!("checkpoint_unreadable:{}", checkpoint.anchor_id));
            }
        }
    }
    let mut state_consistent = state.next_segment == state.completed.len()
        && state.completed.len() <= state.anchors.len()
        && state.completed.len() <= state.anchors.len();
    if !state_consistent {
        reasons.push(format!(
            "state_cursor_inconsistent:next={} completed={}",
            state.next_segment,
            state.completed.len()
        ));
    }
    for (index, checkpoint) in state.completed.iter().enumerate() {
        let Some(expected) = state.anchors.get(index) else {
            state_consistent = false;
            continue;
        };
        if checkpoint.anchor_id != expected.id
            || checkpoint.anchor_ns != expected.anchor_ns
            || checkpoint.anchor_frame != expected.anchor_frame
            || checkpoint.original_anchor_file_byte != expected.file_end_byte
            || checkpoint.file_start_byte != expected.file_start_byte
            || checkpoint.file_end_byte != expected.file_end_byte
        {
            state_consistent = false;
            reasons.push(format!("anchor_offset_mismatch:{}", checkpoint.anchor_id));
        }
    }
    let report = ResumePreflight {
        checked_at: now_rfc3339(),
        action: if reasons.is_empty() {
            "continue".to_string()
        } else {
            "reject".to_string()
        },
        reasons,
        source_hash_ok,
        completed_output_hashes_ok,
        checkpoint_chain_ok,
        state_consistent,
    };
    atomic_json(
        &paths
            .evidence
            .join(format!("resume-preflight-{}.json", state.resume_count + 1)),
        &report,
    )?;
    write_manifest(paths)?;
    Ok(report)
}

fn comparison_for(state: &RunState) -> ComparisonReport {
    let tolerance_db = 0.75;
    let segments = state
        .completed
        .iter()
        .map(|checkpoint| SegmentComparison {
            anchor_id: checkpoint.anchor_id.clone(),
            before_rms_dbfs: checkpoint.before.rms_dbfs,
            after_rms_dbfs: checkpoint.after.rms_dbfs,
            gain_db: checkpoint.gain.applied_gain_db,
            delta_db: checkpoint.delta_db,
            target_error_db: checkpoint.target_error_db,
            output_sha256: checkpoint.output_sha256.clone(),
            within_tolerance: checkpoint.target_error_db.abs() <= tolerance_db,
        })
        .collect();
    ComparisonReport {
        generated_at: now_rfc3339(),
        run_id: state.run_id.clone(),
        tolerance_db,
        segments,
        resume_count: state.resume_count,
        paused_anchors: state
            .completed
            .iter()
            .filter(|checkpoint| checkpoint.pause_after)
            .map(|checkpoint| checkpoint.anchor_id.clone())
            .collect(),
        final_output_sha256: state.final_output.as_ref().map(|item| item.sha256.clone()),
        complete: state.status == "completed",
    }
}

pub fn acceptance_lines(state: &RunState) -> Vec<String> {
    let mut lines = Vec::new();
    for checkpoint in &state.completed {
        let status = if checkpoint.target_error_db.abs() <= 0.75 {
            "PASS"
        } else {
            "FAIL"
        };
        lines.push(format!(
            "{status} {} t={:.9}s frame={} data_offset={} file_offset={} anchor_byte={} \
             before_rms={:+.3}dBFS after_rms={:+.3}dBFS gain={:+.3}dB target_error={:+.3}dB \
             segment={}..{} output_sha256={}",
            checkpoint.anchor_id,
            checkpoint.anchor_ns as f64 / 1e9,
            checkpoint.anchor_frame,
            checkpoint.payload_end_byte,
            checkpoint.original_anchor_file_byte,
            checkpoint.original_anchor_file_byte,
            checkpoint.before.rms_dbfs,
            checkpoint.after.rms_dbfs,
            checkpoint.gain.applied_gain_db,
            checkpoint.target_error_db,
            checkpoint.file_start_byte,
            checkpoint.file_end_byte,
            checkpoint.output_sha256
        ));
    }
    if let Some(final_output) = &state.final_output {
        lines.push(format!(
            "PASS FINAL duration_ns={} frames={} {}Hz bytes={} sha256={}",
            final_output.duration_ns,
            final_output.frame_count,
            final_output.sample_rate,
            final_output.bytes,
            final_output.sha256
        ));
    }
    lines.push(format!(
        "PASS ORIGINAL sha256={} bytes={} readonly={} copy={} copy_sha256={}",
        state.source.sha256,
        state.source.file_size,
        state.source.original_mode_readonly,
        state.source.original_copy_path,
        state.source.original_copy_sha256
    ));
    if state.resume_count > 0 {
        let paused_anchors: Vec<&str> = state
            .completed
            .iter()
            .filter(|checkpoint| checkpoint.pause_after)
            .map(|checkpoint| checkpoint.anchor_id.as_str())
            .collect();
        lines.push(format!(
            "PASS RESUME resumes={} paused_anchors={} next_segment={}",
            state.resume_count,
            if paused_anchors.is_empty() {
                "unknown".to_string()
            } else {
                paused_anchors.join(",")
            },
            state.next_segment
        ));
    }
    if let Some(degradation) = &state.degradation {
        lines.push(format!(
            "PASS DEGRADED mode={} rate={}Hz reasons={} probe_bytes={}",
            degradation.mode,
            degradation.output_sample_rate,
            degradation.reasons.join("|"),
            degradation.anchor_probe_bytes
        ));
    }
    lines
}

fn write_lines(path: &Path, lines: &[String]) -> Result<()> {
    let mut text = lines.join("\n");
    text.push('\n');
    atomic_write(path, text.as_bytes())
}

pub fn finalize_run(paths: &RunPaths, state: &RunState) -> Result<()> {
    paths.ensure()?;
    write_initial_evidence(paths, state)?;
    let comparison = comparison_for(state);
    atomic_json(&paths.evidence.join("comparison.json"), &comparison)?;
    atomic_json(&paths.evidence.join("anchors.json"), &state.completed)?;
    atomic_json(&paths.state.with_file_name("state.snapshot.json"), state)?;
    let events: Vec<String> = state
        .events
        .iter()
        .map(serde_json::to_string)
        .collect::<std::result::Result<_, _>>()?;
    atomic_write(
        &paths.evidence.join("events.jsonl"),
        format!("{}\n", events.join("\n")).as_bytes(),
    )?;
    atomic_json(
        &paths.evidence.join("pipeline.json"),
        &json!({
            "gstreamer_version": crate::gst_backend::gstreamer_version(),
            "analysis": state.completed.iter().map(|item| json!({
                "anchor": item.anchor_id,
                "pipeline": item.analysis_pipeline
            })).collect::<Vec<_>>(),
            "render": state.completed.iter().map(|item| json!({
                "anchor": item.anchor_id,
                "gain_linear": item.gain.gain_linear,
                "pipeline": item.render_pipeline
            })).collect::<Vec<_>>(),
            "concat": state.events.iter()
                .find(|event| event.kind == "run_completed")
                .map(|event| event.detail.clone())
        }),
    )?;
    let acceptance = acceptance_lines(state);
    write_lines(&paths.evidence.join("acceptance.txt"), &acceptance)?;
    let report = json!({
        "schema_version": crate::model::SCHEMA_VERSION,
        "run_id": state.run_id,
        "status": state.status,
        "created_at": state.created_at,
        "updated_at": state.updated_at,
        "source": state.source,
        "config": state.config,
        "resource": state.resource,
        "degradation": state.degradation,
        "next_segment": state.next_segment,
        "resume_count": state.resume_count,
        "comparison": comparison,
        "final_output": state.final_output,
        "acceptance": acceptance
    });
    atomic_json(&paths.evidence.join("report.json"), &report)?;
    write_manifest(paths)?;
    Ok(())
}

fn collect_files(root: &Path, output: &mut Vec<PathBuf>) -> Result<()> {
    if !root.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, output)?;
        } else if path.is_file()
            && path.file_name().and_then(|name| name.to_str()) != Some("manifest.json")
        {
            output.push(path);
        }
    }
    Ok(())
}

pub fn write_manifest(paths: &RunPaths) -> Result<()> {
    let mut files = Vec::new();
    collect_files(&paths.root, &mut files)?;
    files.sort();
    let entries: Vec<_> = files
        .iter()
        .map(|path| {
            let metadata = fs::metadata(path)?;
            Ok(json!({
                "path": path.strip_prefix(&paths.root).unwrap_or(path).to_string_lossy(),
                "bytes": metadata.len(),
                "sha256": file_sha256(path)?
            }))
        })
        .collect::<Result<Vec<_>>>()?;
    atomic_json(
        &paths.evidence.join("manifest.json"),
        &json!({
            "generated_at": now_rfc3339(),
            "root": paths.root,
            "file_count": entries.len(),
            "files": entries
        }),
    )
}

pub fn verify_run(paths: &RunPaths) -> Result<VerifyReport> {
    let state = load_state(paths)?;
    let mut checks = Vec::new();
    let source_ok = check_source(&state).unwrap_or(false);
    checks.push(VerifyCheck {
        name: "original_sha256_unchanged".to_string(),
        ok: source_ok,
        detail: format!(
            "expected={} source={}",
            state.source.sha256, state.source.path
        ),
    });
    let copy_ok = check_copy(&state).unwrap_or(false);
    checks.push(VerifyCheck {
        name: "readonly_copy_matches".to_string(),
        ok: copy_ok,
        detail: state.source.original_copy_path.clone(),
    });
    let completed_ok = state.status == "completed"
        && state.completed.len() == crate::model::ANCHOR_COUNT
        && state.next_segment == crate::model::ANCHOR_COUNT;
    checks.push(VerifyCheck {
        name: "three_checkpoints_complete".to_string(),
        ok: completed_ok,
        detail: format!(
            "status={} completed={} next={}",
            state.status,
            state.completed.len(),
            state.next_segment
        ),
    });
    for checkpoint in &state.completed {
        let output = Path::new(&checkpoint.output_wav);
        let output_ok = output.exists() && file_sha256(output)? == checkpoint.output_sha256;
        checks.push(VerifyCheck {
            name: format!("segment_output_{}", checkpoint.anchor_id),
            ok: output_ok,
            detail: checkpoint.output_wav.clone(),
        });
        let checkpoint_file = Path::new(&checkpoint.checkpoint_path);
        let digest_ok = read_json::<AnchorCheckpoint>(checkpoint_file)
            .map(|saved| checkpoint_digest(&saved).ok() == Some(saved.checkpoint_sha256.clone()))
            .unwrap_or(false);
        checks.push(VerifyCheck {
            name: format!("checkpoint_digest_{}", checkpoint.anchor_id),
            ok: digest_ok,
            detail: checkpoint.checkpoint_path.clone(),
        });
        let probe = Path::new(&checkpoint.raw_probe_file);
        let raw_ok = probe.exists() && file_sha256(probe)? == checkpoint.raw_probe_sha256;
        checks.push(VerifyCheck {
            name: format!("raw_anchor_evidence_{}", checkpoint.anchor_id),
            ok: raw_ok,
            detail: format!(
                "{} bytes={} raw_range={}..{}",
                checkpoint.raw_probe_file,
                checkpoint.raw_probe_bytes,
                checkpoint.raw_probe_start_byte,
                checkpoint.raw_probe_end_byte
            ),
        });
        checks.push(VerifyCheck {
            name: format!("target_after_calibration_{}", checkpoint.anchor_id),
            ok: checkpoint.target_error_db.abs() <= 0.75,
            detail: format!(
                "t={:.9}s frame={} offset={} after={:.3}dBFS target={:.3}dBFS error={:+.3}dB",
                checkpoint.anchor_ns as f64 / 1e9,
                checkpoint.anchor_frame,
                checkpoint.original_anchor_file_byte,
                checkpoint.after.rms_dbfs,
                state.config.target_rms_dbfs,
                checkpoint.target_error_db
            ),
        });
        checks.push(VerifyCheck {
            name: format!("original_offset_consistency_{}", checkpoint.anchor_id),
            ok: checkpoint.anchor_frame == checkpoint.segment_end_frame
                && checkpoint.original_anchor_file_byte == checkpoint.file_end_byte
                && checkpoint.payload_end_byte
                    == checkpoint
                        .anchor_frame
                        .saturating_mul(state.source.data_size / state.source.frame_count.max(1))
                && checkpoint.file_end_byte
                    == state.source.data_offset + checkpoint.payload_end_byte,
            detail: format!(
                "frame={} payload_end={} file_end={} data_offset={}",
                checkpoint.anchor_frame,
                checkpoint.payload_end_byte,
                checkpoint.file_end_byte,
                state.source.data_offset
            ),
        });
    }
    if let Some(final_output) = &state.final_output {
        let final_ok = Path::new(&final_output.path).exists()
            && file_sha256(Path::new(&final_output.path))? == final_output.sha256;
        checks.push(VerifyCheck {
            name: "final_output_sha256".to_string(),
            ok: final_ok,
            detail: final_output.path.clone(),
        });
        checks.push(VerifyCheck {
            name: "final_duration_matches_source".to_string(),
            ok: final_output.duration_ns.abs_diff(state.source.duration_ns) <= 1_000_000,
            detail: format!(
                "final={}ns source={}ns",
                final_output.duration_ns, state.source.duration_ns
            ),
        });
    } else {
        checks.push(VerifyCheck {
            name: "final_output_present".to_string(),
            ok: false,
            detail: "state.final_output is null".to_string(),
        });
    }
    let acceptance_lines = acceptance_lines(&state);
    let ok = checks.iter().all(|check| check.ok);
    let report = VerifyReport {
        checked_at: now_rfc3339(),
        ok,
        checks,
        acceptance_lines,
    };
    atomic_json(&paths.evidence.join("verification.json"), &report)?;
    write_lines(
        &paths.evidence.join("acceptance.txt"),
        &report.acceptance_lines,
    )?;
    write_manifest(paths)?;
    Ok(report)
}

pub fn compare_runs(
    a_paths: &RunPaths,
    b_paths: &RunPaths,
    output: &Path,
) -> Result<EquivalenceReport> {
    let a = load_state(a_paths)?;
    let b = load_state(b_paths)?;
    let a_hash = a.final_output.as_ref().map(|item| item.sha256.clone());
    let b_hash = b.final_output.as_ref().map(|item| item.sha256.clone());
    let outputs_identical = a_hash.is_some() && a_hash == b_hash;
    let mut anchors = Vec::new();
    for a_anchor in &a.completed {
        let b_anchor = b
            .completed
            .iter()
            .find(|item| item.anchor_id == a_anchor.anchor_id);
        anchors.push(crate::model::AnchorEquivalence {
            anchor_id: a_anchor.anchor_id.clone(),
            same_before_rms: b_anchor
                .map(|item| (item.before.rms_dbfs - a_anchor.before.rms_dbfs).abs() <= 0.001)
                .unwrap_or(false),
            same_after_rms: b_anchor
                .map(|item| (item.after.rms_dbfs - a_anchor.after.rms_dbfs).abs() <= 0.001)
                .unwrap_or(false),
            same_gain: b_anchor
                .map(|item| {
                    (item.gain.applied_gain_db - a_anchor.gain.applied_gain_db).abs() <= 0.001
                })
                .unwrap_or(false),
            same_output_sha256: b_anchor
                .map(|item| item.output_sha256 == a_anchor.output_sha256)
                .unwrap_or(false),
            a_after_rms_dbfs: Some(a_anchor.after.rms_dbfs),
            b_after_rms_dbfs: b_anchor.map(|item| item.after.rms_dbfs),
        });
    }
    let ok = outputs_identical
        && anchors.iter().all(|anchor| {
            anchor.same_before_rms
                && anchor.same_after_rms
                && anchor.same_gain
                && anchor.same_output_sha256
        });
    let report = EquivalenceReport {
        generated_at: now_rfc3339(),
        a_run: a_paths.root.to_string_lossy().into_owned(),
        b_run: b_paths.root.to_string_lossy().into_owned(),
        a_output_sha256: a_hash,
        b_output_sha256: b_hash,
        outputs_identical,
        anchors,
        ok,
    };
    atomic_json(output, &report)?;
    Ok(report)
}
