//! Self-verifying export bundles.
//!
//! A colleague can review an export without this application:
//! * `project.snapshot.json` — full project document with ordered fields.
//! * `keyframes.csv` — flat keyframe index readable in any spreadsheet.
//! * `comparisons.csv` — every matched pair and delta.
//! * `evidence/<segment>/run-<run>/...` — retained thumbnails and run
//!   manifests plus the hash-chained `audit.jsonl`.
//! * `SHA256SUMS` — relative-path digest list verifiable with
//!   `sha256sum -c SHA256SUMS` on any machine.
//! * `MANIFEST.json` — bundle index with its own digest and tool metadata.
//! * `REVIEW.md` — step-by-step independent review instructions.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::hash::{digest_equal, sha256_bytes, sha256_file};
use crate::model::project::Project;
use crate::storage::ProjectStore;
use crate::time::format_unix_nanos;
use crate::{Error, Result};

/// One row of the flat keyframe index.
#[derive(Debug, Serialize)]
pub struct KeyframeRow {
    pub segment_id: String,
    pub segment_label: String,
    pub device_id: String,
    pub group_id: String,
    pub keyframe_index: u32,
    pub pts_ns: i64,
    pub device_time_utc: String,
    pub reference_time_utc: String,
    pub is_decoder_keyframe: bool,
    pub thumbnail_file: String,
    pub thumbnail_sha256: String,
    pub thumbnail_bytes: u64,
}

/// One row of the comparison export.
#[derive(Debug, Serialize)]
pub struct ComparisonRow {
    pub comparison_id: String,
    pub group_id: String,
    pub pair_index: u32,
    pub segment_a_id: String,
    pub segment_b_id: String,
    pub keyframe_a_index: u32,
    pub keyframe_b_index: u32,
    pub reference_time_a_utc: String,
    pub reference_time_b_utc: String,
    pub delta_ns: i64,
    pub thumbnail_identical: bool,
}

/// Bundle-level manifest written alongside the digest list.
#[derive(Debug, Serialize, Deserialize)]
pub struct BundleManifest {
    pub bundle_id: String,
    pub created_at: String,
    pub tool: String,
    pub tool_version: String,
    pub schema_version: u32,
    pub project_name: String,
    pub segment_count: usize,
    pub keyframe_count: usize,
    pub comparison_count: usize,
    pub files: Vec<BundleFile>,
    pub manifest_sha256: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BundleFile {
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
}

/// Result of an export run.
#[derive(Debug)]
pub struct ExportReport {
    pub bundle_dir: PathBuf,
    pub file_count: usize,
    pub manifest_sha256: String,
}

/// Create a complete export bundle under `export_root/<bundle_name>`.
pub fn export_bundle(
    store: &ProjectStore,
    project: &Project,
    export_root: &Path,
    bundle_name: &str,
    actor: &str,
) -> Result<ExportReport> {
    let bundle_dir = export_root.join(bundle_name);
    if bundle_dir.exists() {
        return Err(Error::ProjectExists(bundle_dir));
    }
    let evidence_target = bundle_dir.join("evidence");
    fs::create_dir_all(&evidence_target).map_err(|e| Error::io(&bundle_dir, e))?;

    // 1. Project snapshot (field/order preserving via serde struct order).
    let snapshot = serde_json::to_vec_pretty(project)?;
    fs::write(bundle_dir.join("project.snapshot.json"), snapshot)
        .map_err(|e| Error::io(&bundle_dir, e))?;

    // 2. Flat CSV indexes.
    write_keyframes_csv(bundle_dir.join("keyframes.csv"), project)?;
    write_comparisons_csv(bundle_dir.join("comparisons.csv"), project)?;

    // 3. Retained evidence: every run directory and audit trail per segment.
    let source_evidence = store.evidence_root();
    if source_evidence.exists() {
        copy_dir_recursive(&source_evidence, &evidence_target)?;
    }

    // 4. Review instructions.
    fs::write(
        bundle_dir.join("REVIEW.md"),
        review_instructions(project, bundle_name),
    )
    .map_err(|e| Error::io(&bundle_dir, e))?;

    // 5. Digest list over everything except MANIFEST/SHA256SUMS themselves.
    let mut files = Vec::new();
    for relative_path in collect_files(&bundle_dir)? {
        let absolute = bundle_dir.join(&relative_path);
        let bytes = fs::metadata(&absolute)
            .map_err(|e| Error::io(&absolute, e))?
            .len();
        let digest = sha256_file(&absolute)?;
        files.push(BundleFile {
            path: relative_path.to_string_lossy().replace('\\', "/"),
            sha256: digest,
            bytes,
        });
    }
    let mut sums =
        fs::File::create(bundle_dir.join("SHA256SUMS")).map_err(|e| Error::io(&bundle_dir, e))?;
    for file in &files {
        writeln!(sums, "{}  {}", file.sha256, file.path)
            .map_err(|e| Error::io(bundle_dir.join("SHA256SUMS"), e))?;
    }
    sums.sync_all().ok();

    let manifest = BundleManifest {
        bundle_id: crate::model::ids::new_id("bundle"),
        created_at: format_unix_nanos(chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()),
        tool: "vkri".to_string(),
        tool_version: env!("CARGO_PKG_VERSION").to_string(),
        schema_version: project.app_info.schema_version,
        project_name: project.name.clone(),
        segment_count: project.segments.len(),
        keyframe_count: project.segments.iter().map(|s| s.keyframes.len()).sum(),
        comparison_count: project.comparisons.len(),
        files,
        manifest_sha256: String::new(),
    };
    let canonical = serde_json::to_vec_pretty(&manifest)?;
    let manifest_sha256 = sha256_bytes(&canonical);
    let mut finalized = manifest;
    finalized.manifest_sha256 = manifest_sha256.clone();
    fs::write(
        bundle_dir.join("MANIFEST.json"),
        serde_json::to_vec_pretty(&finalized)?,
    )
    .map_err(|e| Error::io(&bundle_dir, e))?;

    store.audit_global(
        actor,
        "export.created",
        &format!("bundle={bundle_name} files={}", finalized.files.len()),
    )?;
    Ok(ExportReport {
        bundle_dir,
        file_count: finalized.files.len(),
        manifest_sha256,
    })
}

fn write_keyframes_csv(path: PathBuf, project: &Project) -> Result<()> {
    let file = fs::File::create(&path).map_err(|e| Error::io(&path, e))?;
    let mut writer = csv::Writer::from_writer(file);
    for segment in &project.segments {
        let device_label = project
            .device(&segment.device_id)
            .map(|device| device.label.clone())
            .unwrap_or_else(|| segment.device_id.clone());
        for keyframe in &segment.keyframes {
            writer.serialize(KeyframeRow {
                segment_id: segment.id.clone(),
                segment_label: segment.label.clone(),
                device_id: device_label.clone(),
                group_id: segment.group_id.clone(),
                keyframe_index: keyframe.index,
                pts_ns: keyframe.pts_ns,
                device_time_utc: crate::time::format_unix_nanos(keyframe.device_time_unix_ns),
                reference_time_utc: crate::time::format_unix_nanos(keyframe.reference_time_unix_ns),
                is_decoder_keyframe: keyframe.is_decoder_keyframe,
                thumbnail_file: keyframe.thumbnail_file.clone(),
                thumbnail_sha256: keyframe.thumbnail_sha256.clone(),
                thumbnail_bytes: keyframe.thumbnail_bytes,
            })?;
        }
    }
    writer.flush().map_err(|e| Error::io(&path, e))?;
    Ok(())
}

fn write_comparisons_csv(path: PathBuf, project: &Project) -> Result<()> {
    let file = fs::File::create(&path).map_err(|e| Error::io(&path, e))?;
    let mut writer = csv::Writer::from_writer(file);
    for comparison in &project.comparisons {
        for entry in &comparison.entries {
            writer.serialize(ComparisonRow {
                comparison_id: comparison.id.clone(),
                group_id: comparison.group_id.clone(),
                pair_index: entry.pair_index,
                segment_a_id: entry.segment_a_id.clone(),
                segment_b_id: entry.segment_b_id.clone(),
                keyframe_a_index: entry.keyframe_a_index,
                keyframe_b_index: entry.keyframe_b_index,
                reference_time_a_utc: crate::time::format_unix_nanos(
                    entry.reference_time_a_unix_ns,
                ),
                reference_time_b_utc: crate::time::format_unix_nanos(
                    entry.reference_time_b_unix_ns,
                ),
                delta_ns: entry.delta_ns,
                thumbnail_identical: entry.thumbnail_identical,
            })?;
        }
    }
    writer.flush().map_err(|e| Error::io(&path, e))?;
    Ok(())
}

fn copy_dir_recursive(source: &Path, target: &Path) -> Result<()> {
    for entry in walkdir::WalkDir::new(source) {
        let entry = entry.map_err(|e| {
            Error::io(
                source,
                e.into_io_error()
                    .unwrap_or_else(|| std::io::Error::other("walkdir failure")),
            )
        })?;
        let relative = entry
            .path()
            .strip_prefix(source)
            .map_err(|_| Error::Validation("walkdir prefix mismatch".into()))?;
        let destination = target.join(relative);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&destination).map_err(|e| Error::io(&destination, e))?;
        } else {
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
            }
            fs::copy(entry.path(), &destination).map_err(|e| Error::io(&destination, e))?;
        }
    }
    Ok(())
}

fn collect_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in walkdir::WalkDir::new(root).sort_by(|a, b| a.path().cmp(b.path())) {
        let entry = entry.map_err(|e| {
            Error::io(
                root,
                e.into_io_error()
                    .unwrap_or_else(|| std::io::Error::other("walkdir")),
            )
        })?;
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(root)
            .map_err(|_| Error::Validation("walkdir prefix mismatch".into()))?
            .to_path_buf();
        let name = relative.to_string_lossy();
        if name == "MANIFEST.json" || name == "SHA256SUMS" {
            continue;
        }
        files.push(relative);
    }
    Ok(files)
}

fn review_instructions(project: &Project, bundle_name: &str) -> String {
    let mut text = String::new();
    text.push_str("# VKRI 导出包独立复核指南\n\n");
    text.push_str(&format!(
        "项目: {}  \n导出包: {bundle_name}\n\n",
        project.name
    ));
    text.push_str("## 1. 文件完整性\n\n");
    text.push_str("在本目录执行（任意装有 coreutils 的机器）：\n\n");
    text.push_str("```sh\nsha256sum -c SHA256SUMS\n```\n\n");
    text.push_str("所有条目必须输出 `OK`。再核对 `MANIFEST.json` 中的 `manifest_sha256`：\n\n");
    text.push_str("```sh\n# 去掉 manifest_sha256 行后重新计算，应与该字段一致\n");
    text.push_str("python3 - <<'PY'\nimport hashlib, json\np = 'MANIFEST.json'\nd = json.load(open(p))\nstored = d.pop('manifest_sha256')\ncanonical = json.dumps(d, indent=2, ensure_ascii=False).encode()\nprint('stored  :', stored)\nprint('rehashed:', hashlib.sha256(canonical).hexdigest())\nPY\n```\n\n");
    text.push_str("## 2. 关键帧索引\n\n");
    text.push_str("- `keyframes.csv` 每行一个关键帧，包含设备时间与修正后参考时间（UTC 纳秒）。\n");
    text.push_str(
        "- `thumbnail_sha256` 与 `evidence/` 下 JPEG 内容一一对应，可自行 `sha256sum` 抽查。\n",
    );
    text.push_str("- `comparisons.csv` 列出两台设备的逐对匹配和 `delta_ns`；中位时差及建议偏移见 `project.snapshot.json` 的 `comparisons`。\n\n");
    text.push_str("## 3. 审计链\n\n");
    text.push_str("每个片段的 `evidence/<segment_id>/audit.jsonl` 为追加式哈希链：后一条的 `prev_sha256` 等于前一条的 `record_sha256`。可用 `vkri verify-export` 自动校验，或逐行人工核对。\n\n");
    text.push_str("## 4. 时钟修正复核\n\n");
    text.push_str("参考时间 = 设备时钟起点(clock_origin_unix_ns) + 帧 PTS + 设备偏移(offset_ns)。对任一关键帧用该公式重算，应与 CSV 中的参考时间一致。\n");
    text
}

/// Independently verify an export bundle: digest list, manifest self-hash and
/// every audit hash chain contained in the bundle.
pub fn verify_bundle(bundle_dir: &Path) -> Result<Vec<String>> {
    let mut findings = Vec::new();
    let sums_path = bundle_dir.join("SHA256SUMS");
    let sums = fs::read_to_string(&sums_path).map_err(|e| Error::io(&sums_path, e))?;
    let mut checked = 0usize;
    for line in sums.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (digest, relative) = line
            .split_once("  ")
            .ok_or_else(|| Error::Verify(format!("malformed SHA256SUMS line: {line}")))?;
        let absolute = bundle_dir.join(relative);
        let actual = sha256_file(&absolute)?;
        if !digest_equal(&actual, digest) {
            return Err(Error::Verify(format!(
                "digest mismatch for {relative}: expected {digest}, got {actual}"
            )));
        }
        checked += 1;
    }
    findings.push(format!("SHA256SUMS: {checked} files verified OK"));

    let manifest_path = bundle_dir.join("MANIFEST.json");
    let manifest: BundleManifest = serde_json::from_slice(
        &fs::read(&manifest_path).map_err(|e| Error::io(&manifest_path, e))?,
    )?;
    let stored = manifest.manifest_sha256.clone();
    let mut without_hash = manifest;
    without_hash.manifest_sha256 = String::new();
    let rehashed = sha256_bytes(&serde_json::to_vec_pretty(&without_hash)?);
    if !digest_equal(&stored, &rehashed) {
        return Err(Error::Verify(format!(
            "MANIFEST self-hash mismatch: stored {stored}, rehashed {rehashed}"
        )));
    }
    findings.push("MANIFEST.json self-hash verified OK".to_string());

    let evidence = bundle_dir.join("evidence");
    if evidence.exists() {
        for entry in fs::read_dir(&evidence).map_err(|e| Error::io(&evidence, e))? {
            let entry = entry.map_err(|e| Error::io(&evidence, e))?;
            if !entry
                .file_type()
                .map_err(|e| Error::io(entry.path(), e))?
                .is_dir()
            {
                continue;
            }
            let segment_id = entry.file_name().to_string_lossy().to_string();
            if segment_id.starts_with('_') {
                let records = crate::audit::AuditLog::verify(&evidence, &segment_id)?;
                findings.push(format!(
                    "audit chain {segment_id}: {} records OK",
                    records.len()
                ));
                continue;
            }
            let records = crate::audit::AuditLog::verify(&evidence, &segment_id)?;
            findings.push(format!(
                "audit chain {segment_id}: {} records OK",
                records.len()
            ));
        }
    }
    findings.push("all export verification checks passed".to_string());
    Ok(findings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_self_hash_is_stable() {
        let manifest = BundleManifest {
            bundle_id: "bundle_x".into(),
            created_at: "t".into(),
            tool: "vkri".into(),
            tool_version: "0".into(),
            schema_version: 1,
            project_name: "p".into(),
            segment_count: 0,
            keyframe_count: 0,
            comparison_count: 0,
            files: Vec::new(),
            manifest_sha256: String::new(),
        };
        let first = sha256_bytes(&serde_json::to_vec_pretty(&manifest).unwrap());
        let second = sha256_bytes(&serde_json::to_vec_pretty(&manifest).unwrap());
        assert_eq!(first, second);
    }
}
