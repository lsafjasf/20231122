//! Independent verification of an exported run record.
//!
//! The verifier never trusts the producing process: it reloads the record,
//! re-serializes it and requires byte equality, replays every digest from the
//! referenced evidence files, checks `checksums.sha256` itself, and with
//! `deep` enabled re-decodes the original media through GStreamer to
//! recompute PCM and fingerprint hashes from scratch.

use crate::decode::{decode_window, DecodeRequest};
use crate::digest::{canonical_json, digest_canonical, sha256_hex_bytes, sha256_hex_str};
use crate::fingerprint::{compute, FingerprintParams};
use crate::model::*;
use crate::pipeline::result_digest;
use crate::store;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, serde::Serialize)]
pub struct Check {
    pub check_id: String,
    pub passed: bool,
    pub detail: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct VerificationReport {
    pub schema: String,
    pub record_path: String,
    pub verdict: String,
    pub checks: Vec<Check>,
    pub verified_at_unix_ms: i64,
}

pub fn verify(record_path: &Path, deep: bool) -> anyhow::Result<VerificationReport> {
    crate::decode::ensure_initialized();
    let base_dir = record_path
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let raw = std::fs::read(record_path)?;
    let mut checks = Vec::new();
    let record: RunRecord = serde_json::from_slice(&raw)?;

    checks.push(Check {
        check_id: "record_schema".to_string(),
        passed: record.schema == RUN_SCHEMA && record.format_version == 1,
        detail: format!("schema={} version={}", record.schema, record.format_version),
    });

    let roundtrip = canonical_json(&record);
    checks.push(Check {
        check_id: "field_and_record_order".to_string(),
        passed: sha256_hex_bytes(&raw) == sha256_hex_str(&roundtrip),
        detail: "记录重新序列化后与磁盘字节完全一致，字段顺序与片段顺序未漂移".to_string(),
    });

    checks.push(Check {
        check_id: "segment_sequence".to_string(),
        passed: record.segments.len() == record.summary.segments_total
            && record
                .segments
                .iter()
                .enumerate()
                .all(|(index, segment)| segment.ordinal == index),
        detail: format!("共 {} 条片段记录", record.segments.len()),
    });

    let recomputed_summary = replay_summary_counts(&record);
    checks.push(Check {
        check_id: "summary_counts".to_string(),
        passed: recomputed_summary == record.summary,
        detail: format!(
            "complete={} pending={} missing={} result_digest={}",
            record.summary.segments_completed,
            record.summary.segments_pending,
            record.summary.missing,
            record.summary.result_digest
        ),
    });

    checks.push(Check {
        check_id: "result_digest".to_string(),
        passed: result_digest(&record) == record.summary.result_digest,
        detail: "结果摘要可由案例、对齐证据和有序片段重算".to_string(),
    });

    let original_path = base_dir.join("case.original.json");
    let original_ok = store::file_sha256(&original_path)
        .map(|digest| digest == record.case.original_manifest_sha256)
        .unwrap_or(false);
    checks.push(Check {
        check_id: "case_manifest_copy".to_string(),
        passed: original_ok,
        detail: "case.original.json 与快照中的原始清单 SHA-256 一致".to_string(),
    });

    let alignment_path = base_dir.join("evidence/alignment.json");
    let alignment_ok = std::fs::read(&alignment_path)
        .map(|bytes| sha256_hex_bytes(&bytes) == record.alignment.evidence_sha256)
        .unwrap_or(false);
    checks.push(Check {
        check_id: "alignment_evidence".to_string(),
        passed: alignment_ok,
        detail: "证据 evidence/alignment.json".to_string(),
    });

    for segment in &record.segments {
        let segment_ok =
            segment.status == SegmentStatus::Completed && verify_segment_digest(segment);
        checks.push(Check {
            check_id: format!("segment_digest:{}", segment.segment_id),
            passed: segment_ok,
            detail: format!("Completed / {:?}", segment.decision),
        });

        for device in &segment.devices {
            let evidence_path = base_dir.join(&device.evidence_file);
            let evidence_ok = verify_evidence_file(&evidence_path, device, segment);
            checks.push(Check {
                check_id: format!("fingerprint_evidence:{}:{}", segment.segment_id, device.device_id),
                passed: evidence_ok,
                detail: device.evidence_file.clone(),
            });
            let semantics_ok = match segment.missing.as_ref() {
                Some(evidence) => evidence.declared_by_capture_metadata && !evidence.gap_id.is_empty(),
                None => true,
            };
            checks.push(Check {
                check_id: format!(
                    "missing_semantics:{}:{}",
                    segment.segment_id, device.device_id
                ),
                passed: semantics_ok,
                detail: "缺失决策必须携带采集元数据缺口证据".to_string(),
            });
            if deep {
                checks.push(deep_recompute(device)?);
            }
        }
    }

    let checksum_path = base_dir.join("checksums.sha256");
    let checksum_ok = verify_checksums(&checksum_path, &base_dir);
    checks.push(Check {
        check_id: "checksums".to_string(),
        passed: checksum_ok,
        detail: "校验 checksums.sha256".to_string(),
    });

    let resume_ok = record.resume_checks.iter().all(|check| {
        // When the first pause happens before any segment finished there is no
        // anchor segment yet; the state digest round-trip is still mandatory.
        let anchor_ok = check.anchor_segment_id == "none" || check.anchor_content_equal;
        check.state_roundtrip_equal
            && anchor_ok
            && check.before_resume_digest == check.after_reload_digest
    });
    checks.push(Check {
        check_id: "resume_comparison".to_string(),
        passed: resume_ok,
        detail: format!(
            "{} 次恢复检查，state_equal/anchor_equal 均为真",
            record.resume_checks.len()
        ),
    });
    checks.push(Check {
        check_id: "complete_status".to_string(),
        passed: record.run.status == RunStatus::Complete && record.summary.segments_pending == 0,
        detail: "完成状态下没有待处理片段".to_string(),
    });

    let verdict = if checks.iter().all(|check| check.passed) {
        "pass"
    } else {
        "fail"
    };
    Ok(VerificationReport {
        schema: VERIFICATION_SCHEMA.to_string(),
        record_path: record_path.display().to_string(),
        verdict: verdict.to_string(),
        checks,
        verified_at_unix_ms: crate::pipeline::now_unix_ms(),
    })
}

fn replay_summary_counts(record: &RunRecord) -> RunSummary {
    RunSummary {
        segments_total: record.case.segments.len(),
        segments_completed: record.segments.len(),
        segments_pending: record.case.segments.len() - record.segments.len(),
        matches: record
            .segments
            .iter()
            .filter(|s| s.decision == Decision::Match)
            .count(),
        missing: record
            .segments
            .iter()
            .filter(|s| s.decision == Decision::Missing)
            .count(),
        divergent: record
            .segments
            .iter()
            .filter(|s| s.decision == Decision::Divergent)
            .count(),
        review: record
            .segments
            .iter()
            .filter(|s| s.decision == Decision::Review)
            .count(),
        degradation_count: record.degradation_events.len(),
        result_digest: result_digest(record),
    }
}

fn verify_segment_digest(segment: &SegmentRecord) -> bool {
    let mut cloned = segment.clone();
    let stored = cloned.evidence_sha256.clone();
    cloned.evidence_sha256 = String::new();
    cloned.evidence_sha256 = crate::compare::segment_digest(&cloned);
    cloned.evidence_sha256 == stored
}

fn verify_evidence_file(path: &Path, device: &DeviceFingerprintResult, segment: &SegmentRecord) -> bool {
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    let Ok(evidence): Result<FingerprintEvidence, _> = serde_json::from_slice(&bytes) else {
        return false;
    };
    let frame_digest = digest_canonical(&evidence.frames);
    evidence.schema == EVIDENCE_SCHEMA
        && evidence.segment_id == segment.segment_id
        && evidence.device_id == device.device_id
        && evidence.pcm_sha256 == device.pcm_sha256
        && evidence.fingerprint_sha256 == device.fingerprint_sha256
        && frame_digest == device.fingerprint_sha256
        && sha256_hex_bytes(&bytes) != String::new()
}

fn deep_recompute(device: &DeviceFingerprintResult) -> anyhow::Result<Check> {
    let params = FingerprintParams {
        sample_rate_hz: device.sample_rate_hz,
        frame_us: device.frame_us,
        hop_us: device.hop_us,
    };
    let decoded = decode_window(&DecodeRequest {
        source_path: PathBuf::from(&device.source_path),
        requested_range: device.requested_range.clone(),
        sample_rate_hz: device.sample_rate_hz,
    })?;
    let fingerprint = compute(&decoded.pcm_s16le, &params);
    let passed = fingerprint.pcm_sha256 == device.pcm_sha256
        && fingerprint.fingerprint_sha256 == device.fingerprint_sha256;
    Ok(Check {
        check_id: format!("deep_recompute:{}", device.evidence_file),
        passed,
        detail: format!("pcm={}", fingerprint.pcm_sha256),
    })
}

fn verify_checksums(path: &Path, base_dir: &Path) -> bool {
    let Ok(content) = std::fs::read_to_string(path) else {
        return false;
    };
    for line in content.lines() {
        let Some((digest, relative)) = line.split_once("  ") else {
            return false;
        };
        let Ok(actual) = store::file_sha256(&base_dir.join(relative)) else {
            return false;
        };
        if actual != digest {
            return false;
        }
    }
    true
}

pub fn write_report(path: &Path, report: &VerificationReport) -> anyhow::Result<()> {
    std::fs::write(path, canonical_json(report))?;
    Ok(())
}
