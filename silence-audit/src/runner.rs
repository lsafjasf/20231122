//! 批处理编排：逐对象分析、异常隔离、证据生成与临时状态清理。

use std::fs;
use std::path::{Path, PathBuf};

use uuid::Uuid;

use crate::model::{ObjectResult, ObjectStatus, SilenceParams};
use crate::pipeline;
use crate::report::{self, RunReport};
use crate::snapshot::{self, SnapshotRecord};
use crate::ui;

/// 一次运行的输出句柄。
pub struct RunOutput {
    pub run_dir: PathBuf,
    pub report_path: PathBuf,
    pub report_sha256: String,
    pub report: RunReport,
}

/// 运行整批快照分析。
///
/// - 每个对象独立处理，单个失败仅记录错误并继续其余对象；
/// - 所有临时状态集中在运行目录下的 `tmp/`，结束时统一清理；
/// - 输出报告、波形 PNG 与哈希清单，构成可复核证据链。
pub fn run_batch(
    snapshot_path: &Path,
    output_root: &Path,
    params: &SilenceParams,
    media_base: Option<&Path>,
) -> anyhow::Result<RunOutput> {
    let snapshot_bytes = fs::read(snapshot_path)?;
    let snapshot_sha = report::sha256_hex(&snapshot_bytes);

    let run_id = format!("run-{}", Uuid::new_v4());
    let run_dir = output_root.join(&run_id);
    let evidence_dir = run_dir.join("evidence");
    let tmp_dir = run_dir.join("tmp");
    fs::create_dir_all(&evidence_dir)?;
    fs::create_dir_all(&tmp_dir)?;

    // 快照解析失败属于致命错误：无法得到对象清单；仍清理临时目录后返回。
    let records = match snapshot::parse_snapshot(&String::from_utf8_lossy(&snapshot_bytes)) {
        Ok(records) => records,
        Err(err) => {
            cleanup_tmp(&tmp_dir);
            anyhow::bail!("快照解析失败: {err}");
        }
    };

    let mut report = RunReport::new(run_id, snapshot_path, snapshot_sha);
    let mut results: Vec<ObjectResult> = Vec::with_capacity(records.len());

    for record in records {
        let result = process_one(
            &record,
            params,
            media_base,
            &run_dir,
            &evidence_dir,
            &tmp_dir,
        );
        results.push(result);
    }

    for result in results {
        report
            .evidence
            .extend(result.evidence_files.iter().filter_map(|rel| {
                let abs = run_dir.join(rel);
                report::hash_file(&run_dir, &abs, &result.object_id, evidence_kind(rel)).ok()
            }));
        report.record(result);
    }

    let (report_path, report_sha256) = report.write_to(&run_dir)?;

    // 结束时清理临时状态（证据与报告保留）。
    cleanup_tmp(&tmp_dir);

    Ok(RunOutput {
        run_dir,
        report_path,
        report_sha256,
        report,
    })
}

/// 处理单个对象；任何错误都被收敛为 `Failed` 结果，不向上扩散。
fn process_one(
    record: &SnapshotRecord,
    params: &SilenceParams,
    media_base: Option<&Path>,
    run_dir: &Path,
    evidence_dir: &Path,
    tmp_dir: &Path,
) -> ObjectResult {
    match try_process(record, params, media_base, run_dir, evidence_dir, tmp_dir) {
        Ok(result) => result,
        Err(err) => {
            eprintln!("[{}] 处理失败，继续其余对象: {err}", record.object_id);
            ObjectResult::failed(&record.object_id, record.source_format, err)
        }
    }
}

fn try_process(
    record: &SnapshotRecord,
    params: &SilenceParams,
    media_base: Option<&Path>,
    run_dir: &Path,
    evidence_dir: &Path,
    tmp_dir: &Path,
) -> anyhow::Result<ObjectResult> {
    let media_path = resolve_media(&record.media_path, media_base);
    if !media_path.is_file() {
        anyhow::bail!("媒体文件不存在: {}", media_path.display());
    }

    // 先把输入媒体哈希并复制为证据，固定复核输入。
    let media_copy = tmp_dir.join(format!(
        "{}.input{}",
        record.object_id,
        extension_of(&media_path)
    ));
    fs::copy(&media_path, &media_copy)?;

    let analysis = pipeline::analyze(&media_copy, params, record.truncated)?;

    let mut result = ObjectResult {
        object_id: record.object_id.clone(),
        source_format: record.source_format.to_string(),
        media_path: media_path.display().to_string(),
        track_ref: record.track_ref.clone(),
        status: if record.truncated {
            ObjectStatus::Truncated
        } else {
            ObjectStatus::Ok
        },
        error: None,
        segments: analysis.segments,
        waveform: Some(analysis.waveform),
        params: Some(params.clone()),
        evidence_files: Vec::new(),
    };

    // 波形 PNG 证据。
    let png_rel = format!("evidence/{}.waveform.png", record.object_id);
    let png_abs = evidence_dir.join(format!("{}.waveform.png", record.object_id));
    ui::render_png(&result, &png_abs, 1000, 360)?;
    result.evidence_files.push(png_rel);

    // 输入媒体提升为长期证据（移出 tmp）。
    let kept_media = evidence_dir.join(format!(
        "{}.input{}",
        record.object_id,
        extension_of(&media_path)
    ));
    fs::rename(&media_copy, &kept_media)?;
    result.evidence_files.push(pathdiff(run_dir, &kept_media));

    Ok(result)
}

fn resolve_media(path: &str, base: Option<&Path>) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        return p.to_path_buf();
    }
    match base {
        Some(base) => base.join(p),
        None => p.to_path_buf(),
    }
}

fn extension_of(path: &Path) -> String {
    path.extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default()
}

fn pathdiff(run_dir: &Path, abs: &Path) -> String {
    abs.strip_prefix(run_dir)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| abs.display().to_string())
}

fn evidence_kind(rel: &str) -> &'static str {
    if rel.ends_with(".png") {
        "waveform-png"
    } else {
        "media-copy"
    }
}

/// 递归删除临时目录内容；删除失败不影响主流程结果。
fn cleanup_tmp(tmp_dir: &Path) {
    if let Err(err) = fs::remove_dir_all(tmp_dir) {
        eprintln!("警告: 临时目录清理失败 {}: {err}", tmp_dir.display());
    }
}
