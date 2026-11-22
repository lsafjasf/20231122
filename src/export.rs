use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{SecondsFormat, Utc};
use uuid::Uuid;

use crate::model::{
    ExportedFile, ReviewManifest, SCHEMA_VERSION, TOOL_NAME, TOOL_VERSION, Workspace,
    format_time_ms,
};
use crate::runner::sha256_file;
use crate::store::{EVENTS_FILE, EVIDENCE_DIR, atomic_write};

pub const MANIFEST_FILE: &str = "manifest.json";
pub const CHECKSUMS_FILE: &str = "SHA256SUMS";
pub const README_FILE: &str = "README.md";
pub const SEGMENTS_CSV: &str = "segments.csv";
pub const ATTEMPTS_CSV: &str = "attempts.csv";
pub const COMPARISONS_CSV: &str = "comparisons.csv";
pub const OUTPUTS_DIR: &str = "outputs";

pub const SEGMENTS_HEADER: &str = "ordinal,segment_id,label,timeline_start_ms,duration_ms,reason,source_1_device_id,source_1_device_name,source_1_path,source_1_offset_ms,source_1_resolved_start_ms,source_1_resolved_end_ms,source_2_device_id,source_2_device_name,source_2_path,source_2_offset_ms,source_2_resolved_start_ms,source_2_resolved_end_ms,attempt_count,comparison_count,selected_attempt_id";
pub const ATTEMPTS_HEADER: &str = "attempt_no,attempt_id,segment_id,device_id,device_name,status,started_at,finished_at,source_path,source_sha256,source_size_bytes,resolved_start_ms,duration_ms,output_path,output_sha256,output_size_bytes,wall_time_ms,pipeline,error,evidence_file";
pub const COMPARISONS_HEADER: &str = "comparison_id,segment_id,created_at,left_attempt_id,right_attempt_id,left_status,right_status,left_size_bytes,right_size_bytes,size_delta_bytes,left_duration_ms,right_duration_ms,checksum_equal,same_source,verdict";

#[derive(Clone, Debug)]
pub struct ExportReport {
    pub bundle_dir: PathBuf,
    pub manifest_path: PathBuf,
    pub checksums_path: PathBuf,
    pub file_count: usize,
    pub copied_outputs: usize,
}

#[derive(Clone, Debug, Default)]
pub struct VerifyReport {
    pub checked_files: usize,
    pub attempts: usize,
    pub comparisons: usize,
    pub evidence_events: usize,
    pub errors: Vec<String>,
}

impl VerifyReport {
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

pub fn export_review_bundle(
    workspace: &Workspace,
    events: &[crate::model::EvidenceEvent],
    destination: impl AsRef<Path>,
    include_outputs: bool,
) -> Result<ExportReport> {
    workspace.validate().context("工作区无效，拒绝导出")?;
    let destination = destination.as_ref();
    if destination.exists() {
        bail!(
            "导出目录已存在；请选择新的目录以保留旧证据: {}",
            destination.display()
        );
    }
    let parent = destination.parent().context("导出目录缺少父目录")?;
    fs::create_dir_all(parent)?;
    let temp_dir = parent.join(format!(
        ".{}.tmp-{}",
        destination
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("review-bundle"),
        std::process::id()
    ));
    if temp_dir.exists() {
        fs::remove_dir_all(&temp_dir)?;
    }
    fs::create_dir_all(temp_dir.join(EVIDENCE_DIR))?;
    fs::create_dir_all(temp_dir.join(OUTPUTS_DIR))?;

    let result = (|| -> Result<ExportReport> {
        let mut exported_files = Vec::new();
        let readme = render_readme(workspace, events.len());
        write_export_file(
            &temp_dir,
            README_FILE,
            readme.as_bytes(),
            "人工复核说明与校验命令",
            &mut exported_files,
        )?;
        let segments_csv = render_segments_csv(workspace)?;
        write_export_file(
            &temp_dir,
            SEGMENTS_CSV,
            segments_csv.as_bytes(),
            "按工作区顺序展开的片段表",
            &mut exported_files,
        )?;
        let attempts_csv = render_attempts_csv(workspace)?;
        write_export_file(
            &temp_dir,
            ATTEMPTS_CSV,
            attempts_csv.as_bytes(),
            "按片段与尝试顺序展开的重跑记录",
            &mut exported_files,
        )?;
        let comparisons_csv = render_comparisons_csv(workspace)?;
        write_export_file(
            &temp_dir,
            COMPARISONS_CSV,
            comparisons_csv.as_bytes(),
            "按创建顺序展开的比较记录",
            &mut exported_files,
        )?;
        let events_text = render_events_jsonl(events)?;
        write_export_file(
            &temp_dir,
            &format!("{EVIDENCE_DIR}/{EVENTS_FILE}"),
            events_text.as_bytes(),
            "追加式证据日志（严格按 seq 排序）",
            &mut exported_files,
        )?;

        let mut copied_outputs = 0_usize;
        if include_outputs {
            copied_outputs = copy_outputs(workspace, &temp_dir, &mut exported_files)?;
        }

        let manifest = ReviewManifest {
            schema_version: SCHEMA_VERSION,
            tool_name: TOOL_NAME.to_string(),
            tool_version: TOOL_VERSION.to_string(),
            exported_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            bundle_id: format!("bundle-{}", Uuid::new_v4().simple()),
            workspace: workspace.clone(),
            evidence_events: events.to_vec(),
            exported_files,
        };
        manifest.validate()?;
        let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
        atomic_write(&temp_dir.join(MANIFEST_FILE), &manifest_bytes)?;

        let checksums = render_checksums(&temp_dir, &manifest)?;
        atomic_write(&temp_dir.join(CHECKSUMS_FILE), checksums.as_bytes())?;
        fs::rename(&temp_dir, destination).with_context(|| {
            format!(
                "将临时导出包原子移动到目标目录失败: {}",
                destination.display()
            )
        })?;

        Ok(ExportReport {
            bundle_dir: destination.to_path_buf(),
            manifest_path: destination.join(MANIFEST_FILE),
            checksums_path: destination.join(CHECKSUMS_FILE),
            file_count: manifest.exported_files.len() + 1,
            copied_outputs,
        })
    })();

    if result.is_err() && temp_dir.exists() {
        let _ = fs::remove_dir_all(&temp_dir);
    }
    result
}

pub fn load_review_bundle(bundle_dir: impl AsRef<Path>) -> Result<ReviewManifest> {
    let bundle_dir = bundle_dir.as_ref();
    let manifest_path = bundle_dir.join(MANIFEST_FILE);
    let bytes = fs::read(&manifest_path)
        .with_context(|| format!("读取复核清单失败: {}", manifest_path.display()))?;
    let manifest: ReviewManifest =
        serde_json::from_slice(&bytes).context("复核清单不是有效 JSON 或字段不兼容")?;
    manifest.validate()?;
    Ok(manifest)
}

pub fn verify_review_bundle(bundle_dir: impl AsRef<Path>) -> Result<VerifyReport> {
    let bundle_dir = bundle_dir.as_ref();
    let manifest = load_review_bundle(bundle_dir)?;
    let mut report = VerifyReport {
        evidence_events: manifest.evidence_events.len(),
        attempts: manifest
            .workspace
            .segments
            .iter()
            .map(|segment| segment.attempts.len())
            .sum(),
        comparisons: manifest
            .workspace
            .segments
            .iter()
            .map(|segment| segment.comparisons.len())
            .sum(),
        ..VerifyReport::default()
    };
    let mut seen_paths = HashSet::new();
    let mut verified_paths = HashSet::new();
    for expected in &manifest.exported_files {
        if !seen_paths.insert(expected.path.clone()) {
            report
                .errors
                .push(format!("清单重复列出文件: {}", expected.path));
            continue;
        }
        let path = bundle_dir.join(&expected.path);
        if !path.is_file() {
            report.errors.push(format!("缺少文件: {}", expected.path));
            continue;
        }
        let metadata = fs::metadata(&path)?;
        if metadata.len() != expected.size_bytes {
            report.errors.push(format!(
                "文件大小不符: {}（期望 {}，实际 {}）",
                expected.path,
                expected.size_bytes,
                metadata.len()
            ));
        }
        match sha256_file(&path) {
            Ok(actual) if actual == expected.sha256 => {
                verified_paths.insert(expected.path.clone());
            }
            Ok(actual) => report.errors.push(format!(
                "校验和不符: {}（期望 {}，实际 {}）",
                expected.path, expected.sha256, actual
            )),
            Err(error) => report.errors.push(error),
        }
    }
    let checksums_path = bundle_dir.join(CHECKSUMS_FILE);
    if !checksums_path.is_file() {
        report.errors.push(format!("缺少文件: {CHECKSUMS_FILE}"));
    } else {
        let text = fs::read_to_string(&checksums_path)?;
        let checksum_entries = parse_checksums(&text, &mut report)?;
        let checksum_paths: HashSet<String> = checksum_entries
            .iter()
            .map(|(path, _)| path.clone())
            .collect();
        if !checksum_paths.contains(MANIFEST_FILE) {
            report
                .errors
                .push("SHA256SUMS 未覆盖 manifest.json".to_string());
        }
        for path in &checksum_paths {
            if !seen_paths.contains(path) && path != MANIFEST_FILE {
                report
                    .errors
                    .push(format!("SHA256SUMS 含有清单外文件: {path}"));
            }
        }
        for (path, expected_hash) in checksum_entries {
            let actual_path = bundle_dir.join(&path);
            if !actual_path.is_file() {
                report.errors.push(format!("校验清单缺少文件: {path}"));
                continue;
            }
            match sha256_file(&actual_path) {
                Ok(actual) if actual == expected_hash => {
                    verified_paths.insert(path);
                }
                Ok(actual) => report.errors.push(format!(
                    "SHA256SUMS 不一致: {path}（清单 {}，实际 {}）",
                    expected_hash, actual
                )),
                Err(error) => report.errors.push(error),
            }
        }
    }
    report.checked_files = verified_paths.len();
    Ok(report)
}

impl ReviewManifest {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != SCHEMA_VERSION {
            bail!(
                "不支持的复核包 schema {}（期望 {}）",
                self.schema_version,
                SCHEMA_VERSION
            );
        }
        if self.tool_name != TOOL_NAME {
            bail!("复核包来自未知工具: {}", self.tool_name);
        }
        self.workspace.validate()?;
        let mut previous_seq = 0_u64;
        for event in &self.evidence_events {
            if event.seq <= previous_seq {
                bail!("证据事件顺序损坏：seq 必须严格递增");
            }
            previous_seq = event.seq;
        }
        let mut previous_order = 0_u32;
        for file in &self.exported_files {
            if file.order != previous_order.saturating_add(1) {
                bail!("导出文件 order 必须从 1 连续递增");
            }
            previous_order = file.order;
            if file.path.starts_with('/') || file.path.contains("..") {
                bail!("导出文件路径必须是包内相对路径: {}", file.path);
            }
        }
        Ok(())
    }
}

fn write_export_file(
    root: &Path,
    relative: &str,
    bytes: &[u8],
    purpose: &str,
    exported_files: &mut Vec<ExportedFile>,
) -> Result<()> {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, bytes)?;
    exported_files.push(ExportedFile {
        order: exported_files.len() as u32 + 1,
        path: relative.to_string(),
        size_bytes: bytes.len() as u64,
        sha256: sha256_bytes(bytes),
        purpose: purpose.to_string(),
    });
    Ok(())
}

fn render_segments_csv(workspace: &Workspace) -> Result<String> {
    let mut writer = csv::WriterBuilder::new()
        .has_headers(false)
        .from_writer(Vec::new());
    writer.write_record(SEGMENTS_HEADER.split(','))?;
    for segment in &workspace.segments {
        let source_1 = segment.sources.first();
        let source_2 = segment.sources.get(1);
        writer.write_record([
            segment.ordinal.to_string(),
            segment.id.clone(),
            segment.label.clone(),
            segment.timeline_start_ms.to_string(),
            segment.duration_ms.to_string(),
            segment.reason.clone(),
            source_1.map(|s| s.device_id.clone()).unwrap_or_default(),
            source_1.map(|s| s.device_name.clone()).unwrap_or_default(),
            source_1.map(|s| s.media_path.clone()).unwrap_or_default(),
            source_1
                .map(|s| s.clock_offset_ms.to_string())
                .unwrap_or_default(),
            source_1
                .map(|s| s.resolved_start_ms.to_string())
                .unwrap_or_default(),
            source_1
                .map(|s| s.resolved_end_ms.to_string())
                .unwrap_or_default(),
            source_2.map(|s| s.device_id.clone()).unwrap_or_default(),
            source_2.map(|s| s.device_name.clone()).unwrap_or_default(),
            source_2.map(|s| s.media_path.clone()).unwrap_or_default(),
            source_2
                .map(|s| s.clock_offset_ms.to_string())
                .unwrap_or_default(),
            source_2
                .map(|s| s.resolved_start_ms.to_string())
                .unwrap_or_default(),
            source_2
                .map(|s| s.resolved_end_ms.to_string())
                .unwrap_or_default(),
            segment.attempts.len().to_string(),
            segment.comparisons.len().to_string(),
            segment.selected_attempt_id.clone().unwrap_or_default(),
        ])?;
    }
    Ok(String::from_utf8(writer.into_inner()?)?)
}

fn render_attempts_csv(workspace: &Workspace) -> Result<String> {
    let mut writer = csv::WriterBuilder::new()
        .has_headers(false)
        .from_writer(Vec::new());
    writer.write_record(ATTEMPTS_HEADER.split(','))?;
    for segment in &workspace.segments {
        for attempt in &segment.attempts {
            writer.write_record([
                attempt.attempt_no.to_string(),
                attempt.id.clone(),
                attempt.segment_id.clone(),
                attempt.device_id.clone(),
                attempt.device_name.clone(),
                attempt.status.label().to_string(),
                attempt.started_at.clone(),
                attempt.finished_at.clone(),
                attempt.source_path.clone(),
                attempt.source_sha256.clone().unwrap_or_default(),
                attempt
                    .source_size_bytes
                    .map(|value| value.to_string())
                    .unwrap_or_default(),
                attempt.resolved_start_ms.to_string(),
                attempt.duration_ms.to_string(),
                attempt.output_path.clone(),
                attempt.output_sha256.clone().unwrap_or_default(),
                attempt
                    .output_size_bytes
                    .map(|value| value.to_string())
                    .unwrap_or_default(),
                attempt.wall_time_ms.to_string(),
                attempt.pipeline.clone(),
                attempt.error.clone().unwrap_or_default(),
                attempt.evidence_file.clone(),
            ])?;
        }
    }
    Ok(String::from_utf8(writer.into_inner()?)?)
}

fn render_comparisons_csv(workspace: &Workspace) -> Result<String> {
    let mut writer = csv::WriterBuilder::new()
        .has_headers(false)
        .from_writer(Vec::new());
    writer.write_record(COMPARISONS_HEADER.split(','))?;
    for segment in &workspace.segments {
        for comparison in &segment.comparisons {
            writer.write_record([
                comparison.id.clone(),
                segment.id.clone(),
                comparison.created_at.clone(),
                comparison.left_attempt_id.clone(),
                comparison.right_attempt_id.clone(),
                comparison.left_status.label().to_string(),
                comparison.right_status.label().to_string(),
                comparison
                    .left_size_bytes
                    .map(|value| value.to_string())
                    .unwrap_or_default(),
                comparison
                    .right_size_bytes
                    .map(|value| value.to_string())
                    .unwrap_or_default(),
                comparison
                    .size_delta_bytes
                    .map(|value| value.to_string())
                    .unwrap_or_default(),
                comparison.left_duration_ms.to_string(),
                comparison.right_duration_ms.to_string(),
                comparison.checksum_equal.to_string(),
                comparison.same_source.to_string(),
                comparison.verdict.clone(),
            ])?;
        }
    }
    Ok(String::from_utf8(writer.into_inner()?)?)
}

fn render_events_jsonl(events: &[crate::model::EvidenceEvent]) -> Result<String> {
    let mut output = String::new();
    for event in events {
        output.push_str(&serde_json::to_string(event)?);
        output.push('\n');
    }
    Ok(output)
}

fn copy_outputs(
    workspace: &Workspace,
    root: &Path,
    exported_files: &mut Vec<ExportedFile>,
) -> Result<usize> {
    let mut copied = 0;
    for segment in &workspace.segments {
        for attempt in &segment.attempts {
            if !attempt.status.is_terminal() {
                continue;
            }
            let source = Path::new(&attempt.output_path);
            if !source.is_file() {
                continue;
            }
            let extension = source
                .extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or("bin");
            let relative = format!(
                "{OUTPUTS_DIR}/seg{:03}-attempt{:03}-{}.{}",
                segment.ordinal,
                attempt.attempt_no,
                sanitize_component(&attempt.device_id),
                sanitize_component(extension)
            );
            let destination = root.join(&relative);
            fs::copy(source, &destination).with_context(|| {
                format!(
                    "复制尝试产物失败: {} -> {}",
                    source.display(),
                    destination.display()
                )
            })?;
            let bytes = fs::read(&destination)?;
            exported_files.push(ExportedFile {
                order: exported_files.len() as u32 + 1,
                path: relative,
                size_bytes: bytes.len() as u64,
                sha256: sha256_bytes(&bytes),
                purpose: format!(
                    "片段 {} 第 {} 次尝试（{}）产物副本",
                    segment.label, attempt.attempt_no, attempt.device_name
                ),
            });
            copied += 1;
        }
    }
    Ok(copied)
}

fn render_readme(workspace: &Workspace, event_count: usize) -> String {
    let missing = workspace
        .missing_segment
        .as_ref()
        .map(|missing| {
            format!(
                "{} · {} 起，持续 {}",
                missing.label,
                format_time_ms(missing.timeline_start_ms),
                format_time_ms(missing.duration_ms)
            )
        })
        .unwrap_or_else(|| "未记录".to_string());
    format!(
        "# ClockBridge 复核包\n\n\
         标题：{title}\n\
         工作区 ID：{workspace_id}\n\
         缺失片段：{missing}\n\
         证据事件：{event_count} 条\n\n\
         ## 独立复核步骤\n\n\
         1. 在本目录执行 `sha256sum -c SHA256SUMS`，确认所有记录和产物未变化。\n\
         2. 打开 `manifest.json`，核对 schema、双机时钟偏移、缺失片段起止与尝试数组顺序。\n\
         3. 对照 `segments.csv`、`attempts.csv`、`comparisons.csv` 检查每次重跑的输入指纹、时间范围、管线和结论。\n\
         4. 抽查 `outputs/` 中产物，并按 `attempts.csv` 的 `pipeline` 字段独立重跑；不要只依赖本机上的原始输出路径。\n\
         5. 按 `evidence/events.jsonl` 的 `seq` 顺序复核操作链；中断与失败事件同样属于证据，不应删除。\n\n\
         ## 重新载入约定\n\n\
         `manifest.json` 由固定字段顺序的 Rust 结构体生成。片段、尝试、比较、证据事件和导出文件均为数组，重新载入时不会按 map 键重排。\n\n\
         ## 时钟方向\n\n\
         设备时间 = 主时间线 + `clock_offset_ms`。设备 A 为 0；设备 B 的正偏移表示 B 的录制时码比主时间线快。\n",
        title = workspace.title,
        workspace_id = workspace.workspace_id,
        missing = missing,
    )
}

fn render_checksums(root: &Path, manifest: &ReviewManifest) -> Result<String> {
    let mut entries = Vec::new();
    let manifest_path = root.join(MANIFEST_FILE);
    entries.push((
        MANIFEST_FILE.to_string(),
        sha256_file(&manifest_path).map_err(anyhow::Error::msg)?,
    ));
    for file in &manifest.exported_files {
        entries.push((file.path.clone(), file.sha256.clone()));
    }
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    let mut output = String::new();
    for (path, hash) in entries {
        output.push_str(&format!("{hash}  {path}\n"));
    }
    Ok(output)
}

fn parse_checksums(text: &str, report: &mut VerifyReport) -> Result<Vec<(String, String)>> {
    let mut paths = HashSet::new();
    let mut entries = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Some((hash, path)) = line.split_once("  ") else {
            report.errors.push(format!("SHA256SUMS 行格式错误: {line}"));
            continue;
        };
        if hash.len() != 64 {
            report
                .errors
                .push(format!("SHA256SUMS 哈希长度错误: {path}"));
        }
        if !paths.insert(path.to_string()) {
            report.errors.push(format!("SHA256SUMS 重复文件: {path}"));
        }
        entries.push((path.to_string(), hash.to_string()));
    }
    Ok(entries)
}

fn sanitize_component(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn sha256_bytes(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}
