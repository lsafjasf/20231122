//! Self-contained, independently reviewable export bundles.

use crate::model::{APP_VERSION, QueueItem, SCHEMA_VERSION, Session};
use crate::transcode;
use crate::util::{format_ms, now_rfc3339, safe_component, sha256_file};
use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

pub const EXPORT_SCHEMA: u32 = 1;

const SEGMENT_COLUMNS: &[&str] = &[
    "order",
    "segment_id",
    "label",
    "device",
    "source_path",
    "source_sha256",
    "device_clock_offset_ms",
    "raw_capture_start_ms",
    "corrected_start_ms",
    "in_ms",
    "out_ms",
    "duration_ms",
    "base_priority",
    "priority_override",
    "effective_priority",
    "queue_rank",
    "status",
    "latest_run_id",
    "latest_output_sha256",
    "correction_note",
];

const RUN_COLUMNS: &[&str] = &[
    "run_id",
    "segment_id",
    "attempt",
    "in_ms",
    "out_ms",
    "device_offset_ms",
    "effective_priority",
    "status",
    "started_at",
    "finished_at",
    "duration_ms",
    "input_path",
    "input_sha256",
    "output_path",
    "output_sha256",
    "output_bytes",
    "preview_path",
    "pipeline",
    "gst_version",
    "tool_version",
    "error",
];

#[derive(Debug, Clone, Serialize)]
pub struct ProofFile {
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct MediaCopy {
    pub segment_id: String,
    pub run_id: String,
    pub kind: String,
    pub original_path: String,
    pub bundled_path: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExportManifest {
    pub export_schema: u32,
    pub session_schema: u32,
    pub generated_at: String,
    pub tool_version: String,
    pub gstreamer_version: String,
    pub session_id: String,
    pub object_id: String,
    pub object_label: String,
    pub device_a_offset_ms: i64,
    pub device_b_offset_ms: i64,
    pub clock_sign_convention: String,
    pub queue_rule: String,
    pub segment_order: Vec<String>,
    pub queue: Vec<QueueItem>,
    pub segment_count: usize,
    pub run_count: usize,
    pub evidence_count: usize,
    pub files: Vec<ProofFile>,
    pub media_copies: Vec<MediaCopy>,
}

#[derive(Debug, Clone)]
pub struct ExportBundle {
    pub directory: PathBuf,
    pub manifest_path: PathBuf,
    pub checksums_path: PathBuf,
    pub file_count: usize,
    pub media_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct VerificationReport {
    pub directory: PathBuf,
    pub checked: usize,
    pub errors: Vec<String>,
}

impl VerificationReport {
    pub fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }
}

pub fn export_bundle(session: &Session, directory: &Path) -> Result<ExportBundle> {
    session.validate().map_err(anyhow::Error::msg)?;
    if directory.exists() {
        let non_empty = fs::read_dir(directory)
            .with_context(|| format!("读取导出目录失败：{}", directory.display()))?
            .next()
            .is_some();
        if non_empty {
            bail!("导出目录已存在且非空：{}", directory.display());
        }
    }
    fs::create_dir_all(directory)
        .with_context(|| format!("创建导出目录失败：{}", directory.display()))?;

    let mut files = Vec::new();
    write_proof(
        directory,
        "session.json",
        serde_json::to_vec_pretty(session).context("序列化会话快照失败")?,
        &mut files,
    )?;
    write_proof(
        directory,
        "segments.csv",
        segments_csv(session).into_bytes(),
        &mut files,
    )?;
    write_proof(
        directory,
        "runs.csv",
        runs_csv(session).into_bytes(),
        &mut files,
    )?;
    write_proof(
        directory,
        "evidence.jsonl",
        evidence_jsonl(session).into_bytes(),
        &mut files,
    )?;

    let media_copies = copy_media(session, directory)?;
    write_proof(
        directory,
        "media_index.csv",
        media_index_csv(&media_copies).into_bytes(),
        &mut files,
    )?;
    write_proof(
        directory,
        "REVIEW.md",
        review_markdown(session, &media_copies).into_bytes(),
        &mut files,
    )?;

    let manifest = ExportManifest {
        export_schema: EXPORT_SCHEMA,
        session_schema: SCHEMA_VERSION,
        generated_at: now_rfc3339(),
        tool_version: APP_VERSION.to_owned(),
        gstreamer_version: transcode::gstreamer_version(),
        session_id: session.session_id.clone(),
        object_id: session.duplicate_object.object_id.clone(),
        object_label: session.duplicate_object.label.clone(),
        device_a_offset_ms: session.devices[0].clock_offset_ms,
        device_b_offset_ms: session.devices[1].clock_offset_ms,
        clock_sign_convention: "offset_ms = device clock reading - shared reference clock"
            .to_owned(),
        queue_rule: "effective_priority DESC, corrected_start_ms ASC, ordinal ASC".to_owned(),
        segment_order: session
            .segments
            .iter()
            .map(|segment| segment.id.clone())
            .collect(),
        queue: session.queue(),
        segment_count: session.segments.len(),
        run_count: session.runs.len(),
        evidence_count: session.evidence.len(),
        files,
        media_copies: media_copies.clone(),
    };
    let manifest_bytes = serde_json::to_vec_pretty(&manifest).context("序列化清单失败")?;
    let manifest_path = directory.join("manifest.json");
    fs::write(&manifest_path, &manifest_bytes)
        .with_context(|| format!("写入清单失败：{}", manifest_path.display()))?;

    let mut proof_paths = manifest
        .files
        .iter()
        .map(|file| file.path.clone())
        .collect::<Vec<_>>();
    proof_paths.extend(media_copies.iter().map(|copy| copy.bundled_path.clone()));
    proof_paths.push("manifest.json".to_owned());
    let checksums_path = directory.join("checksums.sha256");
    let checksum_text = checksum_file(directory, &proof_paths)?;
    fs::write(&checksums_path, checksum_text)
        .with_context(|| format!("写入校验清单失败：{}", checksums_path.display()))?;

    Ok(ExportBundle {
        directory: directory.to_path_buf(),
        manifest_path,
        checksums_path,
        file_count: manifest.files.len() + 2,
        media_count: media_copies.len(),
    })
}

pub fn verify_bundle(directory: &Path) -> Result<VerificationReport> {
    let checksums_path = directory.join("checksums.sha256");
    let text = fs::read_to_string(&checksums_path)
        .with_context(|| format!("读取校验清单失败：{}", checksums_path.display()))?;
    let mut checked = 0_usize;
    let mut errors = Vec::new();
    let mut listed = HashSet::new();
    for (line_number, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let Some((expected, relative)) = line.split_once("  ") else {
            errors.push(format!("第 {} 行格式错误", line_number + 1));
            continue;
        };
        if relative.starts_with('/') || relative.split('/').any(|part| part == "..") {
            errors.push(format!("拒绝不安全的相对路径：{relative}"));
            continue;
        }
        listed.insert(relative.to_owned());
        let path = directory.join(relative);
        match sha256_file(&path) {
            Ok(actual) if actual == expected => checked += 1,
            Ok(actual) => errors.push(format!(
                "{relative} 哈希不一致：期望 {expected}，实际 {actual}"
            )),
            Err(error) => errors.push(format!("{relative} 无法读取：{error}")),
        }
    }
    if checked == 0 && errors.is_empty() {
        errors.push("校验清单为空".to_owned());
    }
    for required in [
        "manifest.json",
        "session.json",
        "segments.csv",
        "runs.csv",
        "evidence.jsonl",
        "media_index.csv",
        "REVIEW.md",
    ] {
        if !listed.contains(required) {
            errors.push(format!("校验清单缺少必需文件：{required}"));
        }
    }
    Ok(VerificationReport {
        directory: directory.to_path_buf(),
        checked,
        errors,
    })
}

fn write_proof(
    root: &Path,
    relative: &str,
    data: Vec<u8>,
    files: &mut Vec<ProofFile>,
) -> Result<()> {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file =
        fs::File::create(&path).with_context(|| format!("创建证据文件失败：{}", path.display()))?;
    file.write_all(&data)
        .with_context(|| format!("写入证据文件失败：{}", path.display()))?;
    files.push(ProofFile {
        path: relative.to_owned(),
        sha256: sha256_file(&path)?,
        bytes: data.len() as u64,
    });
    Ok(())
}

fn segments_csv(session: &Session) -> String {
    let ranks = session.queue_ranks();
    let mut output = String::new();
    output.push_str(&SEGMENT_COLUMNS.join(","));
    output.push('\n');
    for segment in &session.segments {
        let latest = session.latest_run(&segment.id);
        let offset = session.offset_for(segment.device);
        let row = [
            segment.ordinal.to_string(),
            segment.id.clone(),
            segment.label.clone(),
            device_key(segment.device).to_owned(),
            segment.source_path.clone(),
            segment.source_sha256.clone().unwrap_or_default(),
            offset.to_string(),
            segment.capture_start_ms.to_string(),
            session.normalized_start_ms(segment).to_string(),
            segment.in_ms.to_string(),
            segment.out_ms.to_string(),
            segment.duration_ms().to_string(),
            segment.base_priority.to_string(),
            segment
                .priority_override
                .map(|value| value.to_string())
                .unwrap_or_default(),
            segment.effective_priority().to_string(),
            ranks
                .get(&segment.id)
                .copied()
                .unwrap_or_default()
                .to_string(),
            segment_status_key(segment.status).to_owned(),
            segment.latest_run_id.clone().unwrap_or_default(),
            latest
                .and_then(|run| run.output_sha256.clone())
                .unwrap_or_default(),
            segment.correction_note.clone(),
        ];
        output.push_str(&csv_row(&row));
        output.push('\n');
    }
    output
}

fn runs_csv(session: &Session) -> String {
    let mut output = String::new();
    output.push_str(&RUN_COLUMNS.join(","));
    output.push('\n');
    for run in &session.runs {
        let row = [
            run.run_id.clone(),
            run.segment_id.clone(),
            run.attempt.to_string(),
            run.in_ms.to_string(),
            run.out_ms.to_string(),
            run.device_offset_ms.to_string(),
            run.effective_priority.to_string(),
            run_status_key(run.status).to_owned(),
            run.started_at.clone(),
            run.finished_at.clone(),
            run.duration_ms.to_string(),
            run.input_path.clone(),
            run.input_sha256.clone().unwrap_or_default(),
            run.output_path.clone().unwrap_or_default(),
            run.output_sha256.clone().unwrap_or_default(),
            run.output_bytes
                .map(|value| value.to_string())
                .unwrap_or_default(),
            run.preview_path.clone().unwrap_or_default(),
            run.pipeline.clone(),
            run.gst_version.clone(),
            run.tool_version.clone(),
            run.error.clone().unwrap_or_default(),
        ];
        output.push_str(&csv_row(&row));
        output.push('\n');
    }
    output
}

fn evidence_jsonl(session: &Session) -> String {
    let mut output = String::new();
    for event in &session.evidence {
        if let Ok(json) = serde_json::to_string(event) {
            output.push_str(&json);
            output.push('\n');
        }
    }
    output
}

fn media_index_csv(copies: &[MediaCopy]) -> String {
    let mut output = "segment_id,run_id,kind,original_path,bundled_path,sha256,bytes\n".to_owned();
    for copy in copies {
        output.push_str(&csv_row(&[
            copy.segment_id.clone(),
            copy.run_id.clone(),
            copy.kind.clone(),
            copy.original_path.clone(),
            copy.bundled_path.clone(),
            copy.sha256.clone(),
            copy.bytes.to_string(),
        ]));
        output.push('\n');
    }
    output
}

fn copy_media(session: &Session, root: &Path) -> Result<Vec<MediaCopy>> {
    let mut copies = Vec::new();
    for run in &session.runs {
        if let (Some(output), Some(sha256)) = (&run.output_path, &run.output_sha256) {
            let source = Path::new(output);
            if !source.is_file() {
                bail!(
                    "运行 {} 记录了成功产物，但文件已不存在，无法生成自包含复核包：{}",
                    run.run_id,
                    source.display()
                );
            }
            let extension = source
                .extension()
                .and_then(|value| value.to_str())
                .filter(|value| {
                    !value.is_empty() && value.chars().all(|ch| ch.is_ascii_alphanumeric())
                })
                .unwrap_or("webm");
            let relative = format!(
                "media/{}/{}.{}",
                safe_component(&run.segment_id),
                safe_component(&run.run_id),
                extension
            );
            let destination = root.join(&relative);
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(source, &destination)
                .with_context(|| format!("复制产物到复核包失败：{}", source.display()))?;
            let copied_sha = sha256_file(&destination)?;
            if &copied_sha != sha256 {
                bail!("产物复制后校验和不一致：{}", source.display());
            }
            copies.push(MediaCopy {
                segment_id: run.segment_id.clone(),
                run_id: run.run_id.clone(),
                kind: "output".to_owned(),
                original_path: output.clone(),
                bundled_path: relative,
                sha256: copied_sha,
                bytes: fs::metadata(&destination)?.len(),
            });
        }
        if let Some(preview) = &run.preview_path {
            let source = Path::new(preview);
            if source.is_file() {
                let relative = format!(
                    "previews/{}/{}.png",
                    safe_component(&run.segment_id),
                    safe_component(&run.run_id)
                );
                let destination = root.join(&relative);
                if let Some(parent) = destination.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::copy(source, &destination)
                    .with_context(|| format!("复制预览到复核包失败：{}", source.display()))?;
                copies.push(MediaCopy {
                    segment_id: run.segment_id.clone(),
                    run_id: run.run_id.clone(),
                    kind: "preview".to_owned(),
                    original_path: preview.clone(),
                    bundled_path: relative,
                    sha256: sha256_file(&destination)?,
                    bytes: fs::metadata(&destination)?.len(),
                });
            }
        }
    }
    Ok(copies)
}

fn review_markdown(session: &Session, copies: &[MediaCopy]) -> String {
    let mut text = String::new();
    text.push_str("# 转码队列局部修正 · 独立复核包\n\n");
    text.push_str(&format!(
        "- 对象：`{}`（{}）\n- 会话：`{}`\n- 工具版本：`{}`\n- GStreamer：`{}`\n",
        session.duplicate_object.object_id,
        session.duplicate_object.label,
        session.session_id,
        APP_VERSION,
        transcode::gstreamer_version()
    ));
    text.push_str(&format!(
        "- 片段数：{}；运行记录：{}；证据事件：{}；媒体副本：{}\n\n",
        session.segments.len(),
        session.runs.len(),
        session.evidence.len(),
        copies.len()
    ));
    text.push_str("## 时钟修正约定\n\n");
    text.push_str(
        "`offset_ms = 设备时钟读数 - 共享基准时钟`。正值表示设备时钟偏快；修正后的时间按 \
         `corrected_start_ms = raw_capture_start_ms - offset_ms + in_ms` 计算。\n\n",
    );
    text.push_str(&format!(
        "- 设备 A 偏移：`{} ms`（{}）\n- 设备 B 偏移：`{} ms`（{}）\n",
        session.devices[0].clock_offset_ms,
        session.devices[0].name,
        session.devices[1].clock_offset_ms,
        session.devices[1].name
    ));
    text.push_str(
        "- 队列规则：`effective_priority DESC, corrected_start_ms ASC, ordinal ASC`。\n\
         - 局部修正只改变 `priority_override`，原始 `base_priority` 与片段顺序永远保留。\n\n",
    );
    text.push_str("## 固定字段与顺序\n\n");
    text.push_str(
        "`session.json` 保留完整持久化模型；`segments.csv` 的列顺序如下，行顺序严格等于 \
         `session.json` 中 `segments` 数组的顺序（正常状态即 `ordinal` 升序）：\n\n",
    );
    text.push_str("```text\n");
    text.push_str(&SEGMENT_COLUMNS.join(","));
    text.push_str("\n```\n\n");
    text.push_str("`runs.csv` 的列顺序如下，行顺序严格等于运行发生顺序：\n\n");
    text.push_str("```text\n");
    text.push_str(&RUN_COLUMNS.join(","));
    text.push_str("\n```\n\n");
    text.push_str(
        "`evidence.jsonl` 每行一个事件，事件中的 `fields` 是数组而非对象，数组顺序即字段顺序。\
         `manifest.json` 另含 `segment_order`、队列排名和每个证据文件的哈希。\n\n",
    );
    text.push_str("## 片段与产物\n\n");
    text.push_str(
        "| 顺序 | 片段 | 设备 | 修正后起点 | 原优先级 | 生效优先级 | 队列排名 | 最新产物 SHA-256 |\n\
         |---:|---|---|---:|---:|---:|---:|---|\n",
    );
    let ranks = session.queue_ranks();
    for segment in &session.segments {
        let latest = session.latest_run(&segment.id);
        text.push_str(&format!(
            "| {} | `{}` {} | {} | {} | {} | {} | {} | `{}` |\n",
            segment.ordinal,
            segment.id,
            segment.label,
            device_key(segment.device),
            format_ms(session.normalized_start_ms(segment)),
            segment.base_priority,
            segment.effective_priority(),
            ranks.get(&segment.id).copied().unwrap_or_default(),
            latest
                .and_then(|run| run.output_sha256.as_deref())
                .unwrap_or("—")
        ));
    }
    text.push_str("\n## 同事复核步骤\n\n");
    text.push_str(
        "1. 在复核包根目录执行 `sha256sum -c checksums.sha256`，所有证据文件必须显示 `OK`。\n\
         2. 对照 `segments.csv` 的 `order` 与 `session.json` 的 `segments` 数组，确认字段与顺序未被重排。\n\
         3. 对照 `runs.csv` 的 `input_sha256` 和 `output_sha256`，再用 `sha256sum` 核验 \
         `media/` 中的媒体副本。\n\
         4. 对每个需要重跑的片段，使用本工具的 `--replay-run` 按原始 `in_ms`/`out_ms` 精确重放；\
         `pipeline` 字段同时给出完整的 GStreamer 元素链，可直接作为 `gst-launch-1.0` 的参数检查转码参数。\n\
         5. 检查 `gst_version`、`tool_version`、`pipeline`、起止时间和错误字段，确认运行环境与失败原因可追溯。\n\n",
    );
    text.push_str("### 精确单段重放\n\n");
    if let Some(run) = session.runs.iter().find(|run| run.output_path.is_some()) {
        text.push_str("```sh\n");
        text.push_str(&format!(
            "queue-priority-corrector --session session.json --replay-run {} --output replay.webm\n",
            run.run_id
        ));
        text.push_str("```\n\n");
        text.push_str(
            "`in_ms`/`out_ms` 由 GStreamer buffer probe 在解码后裁剪，单独运行 `pipeline` \
             字段会处理整段输入，因此精确裁剪请使用上面的重放命令。容器元数据可能导致重放产物哈希与原件不同；\
             复核时应同时核对输入哈希、裁剪区间、GStreamer 版本和导出包内媒体副本哈希。\n",
        );
    } else {
        text.push_str("当前尚无成功产物。先在本工具中对单个片段执行“重跑”，再重新导出。\n");
    }
    text
}

fn checksum_file(root: &Path, relative_paths: &[String]) -> Result<String> {
    let mut output = String::new();
    for relative in relative_paths {
        let path = root.join(relative);
        let hash = sha256_file(&path)
            .with_context(|| format!("计算导出文件哈希失败：{}", path.display()))?;
        output.push_str(&format!("{hash}  {relative}\n"));
    }
    Ok(output)
}

fn csv_row(values: &[String]) -> String {
    values
        .iter()
        .map(|value| {
            if value.contains([',', '"', '\n', '\r']) {
                format!("\"{}\"", value.replace('"', "\"\""))
            } else {
                value.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn device_key(device: crate::model::DeviceId) -> &'static str {
    match device {
        crate::model::DeviceId::A => "A",
        crate::model::DeviceId::B => "B",
    }
}

fn segment_status_key(status: crate::model::SegmentStatus) -> &'static str {
    match status {
        crate::model::SegmentStatus::Pending => "pending",
        crate::model::SegmentStatus::Running => "running",
        crate::model::SegmentStatus::Ready => "ready",
        crate::model::SegmentStatus::Failed => "failed",
    }
}

fn run_status_key(status: crate::model::RunStatus) -> &'static str {
    match status {
        crate::model::RunStatus::Succeeded => "succeeded",
        crate::model::RunStatus::Failed => "failed",
        crate::model::RunStatus::Interrupted => "interrupted",
    }
}
