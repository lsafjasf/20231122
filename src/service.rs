use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::evidence::{append_event, now_rfc3339, read_journal, sha256_file};
use crate::model::{ExportRecord, ProjectSession, RunRecord, RunStatus, SegmentRange};
use crate::project::{run_dir, save_session, verify_original, RUNS_DIR};
use crate::render::{render_segment, RenderOutcome};

#[derive(Debug, Clone)]
pub struct RunJob {
    pub session: ProjectSession,
    pub segment_index: usize,
    pub run_id: String,
    pub run_number: u32,
    pub output_path: PathBuf,
}

pub fn begin_run(session: &mut ProjectSession, segment_index: usize) -> Result<RunJob> {
    session.validate().map_err(anyhow::Error::msg)?;
    let segment = session
        .segment(segment_index)
        .map_err(anyhow::Error::msg)?
        .clone();
    let range = segment
        .range(session.media.duration_ns)
        .map_err(anyhow::Error::msg)?;
    let run_number = segment.runs.iter().map(|run| run.number).max().unwrap_or(0) + 1;
    let run_id = Uuid::new_v4().to_string();
    let output_path = run_dir(session, segment_index, run_number).join("segment.webm");
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("无法创建运行目录：{}", parent.display()))?;
    }
    let record = RunRecord {
        id: run_id.clone(),
        number: run_number,
        started_at: now_rfc3339(),
        finished_at: None,
        status: RunStatus::Running,
        anchor_ns: range.anchor_ns,
        in_offset_ns: range.in_offset_ns,
        out_offset_ns: range.out_offset_ns,
        range_start_ns: range.start_ns,
        range_end_ns: range.end_ns,
        measured_start_ns: None,
        measured_end_ns: None,
        mask: segment.mask.clone().sanitized(),
        source_sha256: session.original.sha256.clone(),
        output_relative_path: None,
        output_sha256: None,
        evidence: Vec::new(),
        error: None,
    };
    session
        .segment_mut(segment_index)
        .map_err(anyhow::Error::msg)?
        .runs
        .push(record);
    save_session(session)?;
    append_event(
        &session.project_dir,
        "run_started",
        Some(segment_index),
        Some(run_id.clone()),
        format!("片段 {} 第 {run_number} 次重跑开始", segment_index + 1),
        json!({
            "anchor_ns": range.anchor_ns,
            "in_offset_ns": range.in_offset_ns,
            "out_offset_ns": range.out_offset_ns,
            "range_start_ns": range.start_ns,
            "range_end_ns": range.end_ns,
            "mask": range_mask_json(&segment.mask),
        }),
    )?;
    Ok(RunJob {
        session: session.clone(),
        segment_index,
        run_id,
        run_number,
        output_path,
    })
}

pub fn execute_run(job: &RunJob) -> Result<RenderOutcome> {
    render_segment(
        &job.session,
        job.segment_index,
        job.run_number,
        &job.output_path,
        true,
    )
}

pub fn finish_run(
    session: &mut ProjectSession,
    segment_index: usize,
    run_id: &str,
    result: Result<RenderOutcome, String>,
) -> Result<()> {
    let project_dir = session.project_dir.clone();
    let run = session
        .segment_mut(segment_index)
        .map_err(anyhow::Error::msg)?
        .runs
        .iter_mut()
        .find(|run| run.id == run_id)
        .ok_or_else(|| anyhow::anyhow!("未找到运行记录 {run_id}"))?;
    run.finished_at = Some(now_rfc3339());
    match result {
        Ok(outcome) => {
            run.status = RunStatus::Succeeded;
            run.output_relative_path = outcome
                .output_path
                .strip_prefix(&project_dir)
                .ok()
                .map(|path| path.to_string_lossy().to_string())
                .or_else(|| Some(outcome.output_path.to_string_lossy().to_string()));
            run.output_sha256 = Some(outcome.output_sha256);
            run.measured_start_ns = Some(outcome.measured_start_ns);
            run.measured_end_ns = Some(outcome.measured_end_ns);
            run.evidence = outcome.evidence;
            run.error = None;
            append_event(
                &project_dir,
                "run_succeeded",
                Some(segment_index),
                Some(run_id.to_string()),
                format!("片段 {} 第 {} 次重跑完成", segment_index + 1, run.number),
                json!({
                    "output": run.output_relative_path,
                    "output_sha256": run.output_sha256,
                    "evidence_count": run.evidence.len(),
                }),
            )?;
        }
        Err(error) => {
            run.status = RunStatus::Failed;
            run.error = Some(error.clone());
            append_event(
                &project_dir,
                "run_failed",
                Some(segment_index),
                Some(run_id.to_string()),
                format!("片段 {} 第 {} 次重跑失败", segment_index + 1, run.number),
                json!({ "error": error }),
            )?;
        }
    }
    save_session(session)?;
    Ok(())
}

pub fn export_segment(
    session: &ProjectSession,
    segment_index: usize,
    output_path: &Path,
) -> Result<RenderOutcome> {
    let integrity = verify_original(session);
    if !integrity.verified {
        bail!("原始素材完整性检查失败：{}", integrity.message);
    }
    let run_number = session
        .segment(segment_index)
        .map_err(anyhow::Error::msg)?
        .runs
        .iter()
        .map(|run| run.number)
        .max()
        .unwrap_or(0);
    render_segment(session, segment_index, run_number, output_path, false)
}

pub fn record_export(
    session: &mut ProjectSession,
    segment_index: usize,
    outcome: &RenderOutcome,
) -> Result<()> {
    let range = session
        .range_for(segment_index)
        .map_err(anyhow::Error::msg)?;
    let latest_run = session
        .last_successful_run(segment_index)
        .map(|run| run.id.clone());
    session.exports.push(ExportRecord {
        id: Uuid::new_v4().to_string(),
        at: now_rfc3339(),
        segment_index,
        run_id: latest_run,
        output_path: outcome.output_path.display().to_string(),
        output_sha256: outcome.output_sha256.clone(),
        start_ns: range.start_ns,
        end_ns: range.end_ns,
    });
    append_event(
        &session.project_dir,
        "export_recorded",
        Some(segment_index),
        None,
        format!("片段 {} 已导出", segment_index + 1),
        json!({
            "output": outcome.output_path,
            "sha256": outcome.output_sha256,
            "start_ns": range.start_ns,
            "end_ns": range.end_ns,
        }),
    )?;
    save_session(session)?;
    Ok(())
}

pub fn segment_duration(session: &ProjectSession, segment_index: usize) -> Result<SegmentRange> {
    session.range_for(segment_index).map_err(anyhow::Error::msg)
}

pub fn compare_latest_runs(session: &ProjectSession, segment_index: usize) -> Result<String> {
    let runs: Vec<&RunRecord> = session
        .segment(segment_index)
        .map_err(anyhow::Error::msg)?
        .runs
        .iter()
        .filter(|run| run.status == RunStatus::Succeeded)
        .collect();
    if runs.len() < 2 {
        bail!("至少需要两次成功重跑才能比较");
    }
    let before = runs[runs.len() - 2];
    let after = runs[runs.len() - 1];
    let mut lines = vec![
        format!(
            "对比片段 {}：第 {} 次 → 第 {} 次",
            segment_index + 1,
            before.number,
            after.number
        ),
        format!(
            "锚点   {} → {}",
            crate::timecode::format_timecode(before.anchor_ns),
            crate::timecode::format_timecode(after.anchor_ns)
        ),
        format!(
            "入点   {} / {} → {} / {}",
            before.range_start_ns,
            crate::timecode::format_timecode(before.range_start_ns),
            after.range_start_ns,
            crate::timecode::format_timecode(after.range_start_ns)
        ),
        format!(
            "出点   {} / {} → {} / {}",
            before.range_end_ns,
            crate::timecode::format_timecode(before.range_end_ns),
            after.range_end_ns,
            crate::timecode::format_timecode(after.range_end_ns)
        ),
        format!(
            "遮罩   {} → {}",
            before.mask.summary(),
            after.mask.summary()
        ),
        format!(
            "输出   {} → {}",
            before.output_sha256.as_deref().unwrap_or("-"),
            after.output_sha256.as_deref().unwrap_or("-")
        ),
        format!(
            "证据   {} 帧 → {} 帧",
            before.evidence.len(),
            after.evidence.len()
        ),
    ];
    let before_evidence: Vec<_> = before
        .evidence
        .iter()
        .map(|frame| (&frame.kind, &frame.sha256))
        .collect();
    let after_evidence: Vec<_> = after
        .evidence
        .iter()
        .map(|frame| (&frame.kind, &frame.sha256))
        .collect();
    if before_evidence != after_evidence {
        lines.push("证据帧哈希已变化".to_string());
    } else {
        lines.push("证据帧哈希一致".to_string());
    }
    Ok(lines.join("\n"))
}

pub fn acceptance_value(session: &ProjectSession) -> Value {
    let integrity = verify_original(session);
    let segments: Vec<Value> = session
        .segments
        .iter()
        .map(|segment| {
            let range = segment.range(session.media.duration_ns).ok();
            json!({
                "index": segment.index + 1,
                "label": segment.label,
                "anchor_ns": segment.anchor_ns,
                "anchor_timecode": crate::timecode::format_timecode(segment.anchor_ns),
                "in_offset_ns": segment.in_offset_ns,
                "in_offset": crate::timecode::format_signed_offset(segment.in_offset_ns),
                "out_offset_ns": segment.out_offset_ns,
                "out_offset": crate::timecode::format_signed_offset(segment.out_offset_ns),
                "range": range.as_ref().map(|range| json!({
                    "start_ns": range.start_ns,
                    "start_timecode": range.start_timecode(),
                    "end_ns": range.end_ns,
                    "end_timecode": range.end_timecode(),
                    "duration_ns": range.duration_ns(),
                    "start_clamped": range.start_clamped,
                    "end_clamped": range.end_clamped,
                })),
                "mask": segment.mask,
                "runs": segment.runs,
                "latest_successful_run": segment.runs.iter().rev()
                    .find(|run| run.status == RunStatus::Succeeded)
                    .map(|run| run.id.clone()),
                "evidence_checks": run_evidence_checks(session, segment.index),
            })
        })
        .collect();
    json!({
        "schema_version": session.schema_version,
        "project_id": session.project_id,
        "title": session.title,
        "project_dir": session.project_dir,
        "original": {
            "source_path": session.original.source_path,
            "stored_relative_path": session.original.stored_relative_path,
            "sha256": session.original.sha256,
            "size_bytes": session.original.size_bytes,
            "integrity": integrity,
        },
        "media": {
            "duration_ns": session.media.duration_ns,
            "duration_timecode": crate::timecode::format_timecode(session.media.duration_ns),
            "width": session.media.width,
            "height": session.media.height,
            "fps": format!("{}/{}", session.media.fps_num, session.media.fps_den),
            "container": session.media.container,
            "video_codec": session.media.video_codec,
            "probe_engine": session.media.probe_engine,
        },
        "segments": segments,
        "exports": session.exports,
        "journal_events": read_journal(&session.project_dir).len(),
        "journal_path": session.project_dir.join(crate::evidence::JOURNAL_FILE),
    })
}

fn run_evidence_checks(session: &ProjectSession, segment_index: usize) -> Vec<Value> {
    let Some(segment) = session.segments.get(segment_index) else {
        return Vec::new();
    };
    let mut checks = Vec::new();
    for run in &segment.runs {
        if let Some(relative) = &run.output_relative_path {
            let path = session.project_dir.join(relative);
            let actual = path.is_file().then(|| sha256_file(&path).ok()).flatten();
            checks.push(json!({
                "kind": "output",
                "run_id": run.id,
                "path": path,
                "exists": path.is_file(),
                "expected_sha256": run.output_sha256,
                "actual_sha256": actual,
                "valid": actual.as_deref() == run.output_sha256.as_deref(),
            }));
        }
        let run_path = run_dir(session, segment_index, run.number);
        let manifest_path = run_path.join("evidence-manifest.json");
        if manifest_path.is_file() {
            checks.push(json!({
                "kind": "evidence_manifest",
                "run_id": run.id,
                "path": manifest_path,
                "exists": true,
                "sha256": sha256_file(&manifest_path).ok(),
                "valid": sha256_file(&manifest_path).is_ok(),
            }));
        }
        let preserved = read_preserved_evidence(&manifest_path);
        let frames = if run.evidence.is_empty() {
            preserved.as_slice()
        } else {
            run.evidence.as_slice()
        };
        for frame in frames {
            let path = session.project_dir.join(&frame.relative_path);
            let actual = path.is_file().then(|| sha256_file(&path).ok()).flatten();
            checks.push(json!({
                "kind": frame.kind,
                "run_id": run.id,
                "timestamp_ns": frame.timestamp_ns,
                "timestamp_timecode": frame.timestamp_timecode,
                "source_offset_ns": frame.source_offset_ns,
                "source_offset": frame.source_offset,
                "path": path,
                "exists": path.is_file(),
                "expected_sha256": frame.sha256,
                "actual_sha256": actual,
                "valid": actual.as_deref() == Some(frame.sha256.as_str()),
            }));
        }
    }
    checks
}

fn read_preserved_evidence(path: &Path) -> Vec<crate::model::EvidenceFrame> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return Vec::new();
    };
    value
        .get("evidence")
        .cloned()
        .and_then(|evidence| serde_json::from_value(evidence).ok())
        .unwrap_or_default()
}

fn range_mask_json(mask: &crate::mask::MaskConfig) -> Value {
    serde_json::to_value(mask).unwrap_or(Value::Null)
}

pub fn runs_root(session: &ProjectSession) -> PathBuf {
    session.project_dir.join(RUNS_DIR)
}
