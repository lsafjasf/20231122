//! On-disk layout, atomic writes and evidence bundle generation.

use crate::digest::{canonical_json, sha256_hex_bytes};
use crate::fingerprint::ALGORITHM;
use crate::model::*;
use anyhow::{bail, Context};
use std::io::Write;
use std::path::{Path, PathBuf};

pub fn file_sha256(path: &Path) -> anyhow::Result<String> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(sha256_hex_bytes(&bytes))
}

pub fn load_case(case_path: &Path) -> anyhow::Result<(CaseFile, Vec<u8>, String)> {
    let bytes = std::fs::read(case_path)
        .with_context(|| format!("reading case {}", case_path.display()))?;
    let case: CaseFile = serde_json::from_slice(&bytes).context("parsing case json")?;
    let original_manifest_sha = sha256_hex_bytes(&canonical_json(&case).into_bytes());
    Ok((case, bytes, original_manifest_sha))
}

pub fn resolve_media_path(case_path: &Path, media_path: &str) -> PathBuf {
    let candidate = PathBuf::from(media_path);
    if candidate.is_absolute() {
        return candidate;
    }
    case_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(candidate)
}

pub fn preserve_original_case(
    output_dir: &Path,
    case_bytes: &[u8],
    original_manifest_sha: &str,
) -> anyhow::Result<()> {
    let input_dir = output_dir.join("input");
    std::fs::create_dir_all(&input_dir)?;
    std::fs::write(input_dir.join("case.json"), case_bytes)?;
    let canonical = {
        let case: serde_json::Value = serde_json::from_slice(case_bytes)?;
        canonical_json(&case)
    };
    std::fs::write(output_dir.join("case.original.json"), canonical)?;
    let stored = file_sha256(&output_dir.join("case.original.json"))?;
    if stored != original_manifest_sha {
        bail!("case.original.json digest mismatch after write");
    }
    Ok(())
}

pub fn write_checkpoint(output_dir: &Path, record: &RunRecord) -> anyhow::Result<()> {
    let target = output_dir.join("checkpoint.json");
    let tmp = output_dir.join("checkpoint.json.tmp");
    let mut file = std::fs::File::create(&tmp)?;
    file.write_all(canonical_json(record).as_bytes())?;
    file.sync_all()?;
    std::fs::rename(tmp, target)?;
    Ok(())
}

pub fn empty_evidence_index() -> EvidenceIndex {
    EvidenceIndex {
        directory: "evidence".to_string(),
        manifest_file: "evidence/manifest.json".to_string(),
        review_file: "review.md".to_string(),
        checksums_file: "checksums.sha256".to_string(),
        fingerprint_files: Vec::new(),
        key_evidence: Vec::new(),
    }
}

pub fn empty_verification() -> VerificationInfo {
    VerificationInfo {
        independent_replay_command: String::new(),
        checksums_sha256: String::new(),
        last_verdict: "pending".to_string(),
        last_verified_at_unix_ms: 0,
    }
}

pub fn build_evidence_index(
    _output_dir: &Path,
    segments: &[SegmentRecord],
) -> anyhow::Result<EvidenceIndex> {
    let mut fingerprint_files = Vec::new();
    for segment in segments {
        for device in &segment.devices {
            fingerprint_files.push(device.evidence_file.clone());
        }
    }
    fingerprint_files.sort();
    Ok(EvidenceIndex {
        directory: "evidence".to_string(),
        manifest_file: "evidence/manifest.json".to_string(),
        review_file: "review.md".to_string(),
        checksums_file: "checksums.sha256".to_string(),
        fingerprint_files,
        key_evidence: vec![
            "case.original.json".to_string(),
            "evidence/alignment.json".to_string(),
            "review.md".to_string(),
        ],
    })
}

pub fn write_manifest(options: &crate::pipeline::RunOptions, record: &RunRecord) -> anyhow::Result<()> {
    let manifest = serde_json::json!({
        "schema": BUNDLE_SCHEMA,
        "run_id": record.run.run_id,
        "case_id": record.case.case_id,
        "case_manifest_sha256": record.case.original_manifest_sha256,
        "generated_at_unix_ms": crate::pipeline::now_unix_ms(),
        "processing_stack": [
            "rust/1.92+",
            "gstreamer-1.28+ / uridecodebin + appsink",
            "gtk4-4.22+ control surface"
        ],
        "fingerprint_algorithm": ALGORITHM,
        "alignment_summary": format!(
            "{} 相对 {} 的观测偏移 {}us（置信度 {}‰，比较 {} 帧）",
            record.alignment.compared_device_id,
            record.alignment.anchor_device_id,
            record.alignment.observed_offset_us,
            record.alignment.confidence_milli,
            record.alignment.samples_compared
        ),
        "inputs": record.case.devices.iter().map(|device| {
            let path = Path::new(&device.media_path);
            serde_json::json!({
                "device_id": device.device_id,
                "media_path": device.media_path,
                "sha256": file_sha256(path).unwrap_or_default(),
                "size_bytes": std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
                "clock_epoch_unix_ms": device.clock_epoch_unix_ms,
                "clock_drift_ppm": device.clock_drift_ppm,
            })
        }).collect::<Vec<_>>(),
        "segment_order": record.segments.iter().map(|s| s.segment_id.clone()).collect::<Vec<_>>(),
        "evidence_files": record.evidence.fingerprint_files,
        "review_steps": [
            "先在 checksums.sha256 上运行 sha256sum -c checksums.sha256",
            "运行 segment-fingerprint verify --record run-record.json",
            "如有原始媒体，再加 --deep 独立重算 PCM 与内容指纹",
            "对照 review.md 中的缺口区间、时钟偏移和降级事件作最终结论"
        ],
    });
    std::fs::write(
        options.output_dir.join("evidence/manifest.json"),
        canonical_json(&manifest),
    )?;
    Ok(())
}

pub fn write_checksums(options: &crate::pipeline::RunOptions, record: &RunRecord) -> anyhow::Result<()> {
    let mut lines = Vec::new();
    let mut tracked = vec![
        "case.original.json".to_string(),
        "evidence/alignment.json".to_string(),
        "evidence/manifest.json".to_string(),
        "review.md".to_string(),
    ];
    tracked.extend(record.evidence.fingerprint_files.iter().cloned());
    tracked.sort();
    for relative in tracked {
        let path = options.output_dir.join(&relative);
        let digest = file_sha256(&path)?;
        lines.push(format!("{}  {}", digest, relative));
    }
    std::fs::write(
        options.output_dir.join("checksums.sha256"),
        format!("{}\n", lines.join("\n")),
    )?;
    Ok(())
}

pub fn write_review(options: &crate::pipeline::RunOptions, record: &RunRecord) -> anyhow::Result<()> {
    let mut md = String::new();
    md.push_str("# 片段内容指纹复核记录\n\n");
    md.push_str(&format!("- 案例：`{}` / {}\n", record.case.case_id, record.case.title));
    md.push_str(&format!("- 运行：`{}`\n", record.run.run_id));
    md.push_str(&format!(
        "- 状态：`{:?}`，模式：`{:?}`\n",
        record.run.status, record.run.mode
    ));
    md.push_str(&format!("- 产物 schema：`{}` v{}\n", record.schema, record.format_version));
    md.push_str(&format!("- 指纹算法：`{}`\n", ALGORITHM));
    md.push_str("- Rust：`1.92+`；GStreamer：`uridecodebin ! appsink`；GTK4：控制界面\n");
    md.push_str(&format!("- 原始清单 SHA-256：`{}`\n", record.case.original_manifest_sha256));
    md.push_str(&format!("- 结果摘要：`{}`\n\n", record.summary.result_digest));
    md.push_str("## 时钟与缺口\n\n");
    md.push_str(&format!(
        "{} 为锚点；{} 的观测本地时钟偏移为 `{} µs`，置信度 `{}‰`。\n\n",
        record.alignment.anchor_device_id,
        record.alignment.compared_device_id,
        record.alignment.observed_offset_us,
        record.alignment.confidence_milli
    ));
    md.push_str("| 片段 | 决策 | 相似度‰ | 覆盖率‰ | 缺口设备 | 缺口源时钟 µs | 统一时钟 µs |\n");
    md.push_str("|---|---|---:|---:|---|---|---|\n");
    for segment in &record.segments {
        let (gap_device, source_range, common_range) = match &segment.missing {
            Some(evidence) => (
                evidence.device_id.clone(),
                format!("[{}..{}]", evidence.source_clock_range.start_us, evidence.source_clock_range.end_us),
                format!("[{}..{}]", evidence.common_clock_range.start_us, evidence.common_clock_range.end_us),
            ),
            None => ("-".to_string(), "-".to_string(), "-".to_string()),
        };
        md.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} |\n",
            segment.segment_id,
            segment.decision.as_str(),
            segment.similarity_milli,
            segment.coverage_milli,
            gap_device,
            source_range,
            common_range
        ));
    }
    md.push_str("\n## 断点与恢复\n\n");
    for check in &record.resume_checks {
        md.push_str(&format!(
            "- 已加载 {} 个完成片段；状态摘要一致：`{}`；锚点 `{}` 内容一致：`{}`\n",
            check.checkpoint_segment_count,
            check.state_roundtrip_equal,
            check.anchor_segment_id,
            check.anchor_content_equal
        ));
    }
    if record.resume_checks.is_empty() {
        md.push_str("- 本次运行未发生暂停/恢复\n");
    }
    md.push_str("\n## 资源降级\n\n");
    if record.degradation_events.is_empty() {
        md.push_str("- 无降级事件\n");
    }
    for event in &record.degradation_events {
        md.push_str(&format!(
            "- #{} `{}` / `{}`：{} → {}；RSS {}MB；磁盘 {}MB；证据保留={}\n",
            event.sequence,
            event.stage,
            event.reason_code,
            event.from_mode,
            event.to_mode,
            event.observed_rss_mb,
            event.observed_free_disk_mb,
            event.evidence_preserved
        ));
    }
    md.push_str("\n## 独立复核步骤\n\n");
    md.push_str("1. 先在 checksums.sha256 上运行 sha256sum -c checksums.sha256\n");
    md.push_str("2. 运行 segment-fingerprint verify --record run-record.json\n");
    md.push_str("3. 如有原始媒体，再加 --deep 独立重算 PCM 与内容指纹\n");
    md.push_str("4. 对照 review.md 中的缺口区间、时钟偏移和降级事件作最终结论\n\n");
    md.push_str("校验命令：`segment-fingerprint verify --record run-record.json --deep`\n");
    std::fs::write(options.output_dir.join("review.md"), md)?;
    Ok(())
}

pub fn media_duration_us(path: &Path) -> anyhow::Result<i64> {
    if path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("wav"))
    {
        let bytes = std::fs::read(path)?;
        return wav_duration_us(&bytes);
    }
    bail!("only WAV inputs are supported in this build: {}", path.display());
}

fn wav_duration_us(bytes: &[u8]) -> anyhow::Result<i64> {
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        bail!("not a RIFF/WAVE file");
    }
    let mut offset = 12usize;
    let mut sample_rate = 0u32;
    let mut data_len = 0usize;
    let mut byte_rate = 0u32;
    while offset + 8 <= bytes.len() {
        let id = &bytes[offset..offset + 4];
        let size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into()?) as usize;
        let body = offset + 8;
        match id {
            b"fmt " => {
                sample_rate = u32::from_le_bytes(bytes[body + 4..body + 8].try_into()?);
                byte_rate = u32::from_le_bytes(bytes[body + 8..body + 12].try_into()?);
            }
            b"data" => data_len = size,
            _ => {}
        }
        offset = body + size + (size & 1);
    }
    if sample_rate == 0 || byte_rate == 0 {
        bail!("WAV missing fmt chunk");
    }
    Ok(data_len as i64 * 1_000_000 / byte_rate as i64)
}
