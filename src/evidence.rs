use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::Utc;
use thiserror::Error;

use crate::analysis::KeyFrame;
use crate::model::{format_timecode, EvidenceKind, EvidenceRecord, ReplayEvent, ReviewReport};
use crate::report::{now_utc, report_to_json, sha256_hex, write_atomic};

#[derive(Debug, Error)]
pub enum EvidenceError {
    #[error("cannot create evidence directory {path}: {source}")]
    CreateDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot write evidence {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot encode key frame {id}: {detail}")]
    KeyFrame { id: String, detail: String },
    #[error("report serialization failed: {0}")]
    Report(#[from] crate::report::ReportError),
}

#[derive(Debug, Clone)]
pub struct BundleResult {
    pub root: PathBuf,
    pub report_path: PathBuf,
    pub markdown_path: PathBuf,
    pub manifest_path: PathBuf,
    pub report: ReviewReport,
}

pub fn write_bundle(
    base_dir: &Path,
    mut report: ReviewReport,
    keyframes: &[KeyFrame],
) -> Result<BundleResult, EvidenceError> {
    let root = base_dir.join(bundle_name(&report));
    fs::create_dir_all(root.join("frames")).map_err(|source| EvidenceError::CreateDir {
        path: root.clone(),
        source,
    })?;
    let mut hashed_files: Vec<EvidenceRecord> = Vec::new();
    for interval in &mut report.auto_conclusion.intervals {
        interval.evidence_refs.clear();
    }

    let waveform_path = root.join("waveform.csv");
    let waveform_csv = render_waveform_csv(&report);
    write_bytes(&waveform_path, waveform_csv.as_bytes())?;
    hashed_files.push(evidence_record(
        "ev-waveform",
        EvidenceKind::Waveform,
        "waveform.csv",
        waveform_csv.as_bytes(),
        "逐采样亮度波形；time_ms/frame/track/params 可直接定位",
    ));
    for interval in &mut report.auto_conclusion.intervals {
        interval.evidence_refs.push("evidence:ev-waveform".into());
    }

    let mut frame_refs: Vec<(String, String)> = Vec::new();
    for keyframe in keyframes {
        let png = keyframe
            .to_png()
            .map_err(|detail| EvidenceError::KeyFrame {
                id: keyframe.id.clone(),
                detail,
            })?;
        let safe = safe_component(&keyframe.id);
        let relative = format!("frames/{safe}.png");
        let path = root.join(&relative);
        write_bytes(&path, &png)?;
        let evidence_id = format!("ev-{safe}");
        let record = evidence_record(
            &evidence_id,
            EvidenceKind::KeyFrame,
            &relative,
            &png,
            &format!(
                "{} @ {} / frame {}",
                keyframe.reason,
                format_timecode(keyframe.time_ms),
                keyframe.frame
            ),
        );
        hashed_files.push(record);
        if let Some(interval_id) = &keyframe.interval_id {
            frame_refs.push((interval_id.clone(), evidence_id.clone()));
        }
        report.replay.events.push(ReplayEvent {
            sequence: report.replay.events.len() as u64 + 1,
            at_ms: keyframe.time_ms,
            frame: Some(keyframe.frame),
            track_id: Some(keyframe.track_id.clone()),
            kind: "key_frame_preserved".into(),
            detail: format!("{} 已写入 {}", keyframe.reason, relative),
            params_fingerprint: report.detector.fingerprint(),
            waveform_sample: None,
            evidence_refs: vec![format!("evidence:{evidence_id}")],
        });
    }
    for interval in &mut report.auto_conclusion.intervals {
        for (interval_id, evidence_id) in &frame_refs {
            if interval.id == *interval_id {
                interval
                    .evidence_refs
                    .push(format!("evidence:{evidence_id}"));
            }
        }
    }

    let fingerprint = format!(
        "{}  {}\npath={}\nsha256={}\nsize={}\nmodified_utc={}\nextension={:?}\nprobe_caps={}\n",
        report.source.sha256,
        report.source.file_name,
        report.source.path,
        report.source.sha256,
        report.source.size_bytes,
        report.source.modified_utc,
        report.source.extension.status,
        report.source.probe_caps
    );
    let fingerprint_path = root.join("source.sha256");
    write_bytes(&fingerprint_path, fingerprint.as_bytes())?;
    hashed_files.push(evidence_record(
        "ev-source-fingerprint",
        EvidenceKind::SourceFingerprint,
        "source.sha256",
        fingerprint.as_bytes(),
        "源文件指纹与内容探测结果",
    ));

    report.replay.events.push(ReplayEvent {
        sequence: report.replay.events.len() as u64 + 1,
        at_ms: report.auto_conclusion.analyzed_ms,
        frame: None,
        track_id: report.replay.selected_track.clone(),
        kind: "evidence_bundle_prepared".into(),
        detail: "自动结论、波形、关键帧和事件日志将写入 manifest.json".into(),
        params_fingerprint: report.detector.fingerprint(),
        waveform_sample: report
            .waveform
            .samples
            .len()
            .checked_sub(1)
            .map(|value| value as u64),
        evidence_refs: vec!["evidence:ev-waveform".into()],
    });
    let events_jsonl = report
        .replay
        .events
        .iter()
        .map(serde_json::to_string)
        .collect::<Result<Vec<_>, _>>()
        .map(|lines| lines.join("\n") + "\n")
        .unwrap_or_default();
    let events_path = root.join("events.jsonl");
    write_bytes(&events_path, events_jsonl.as_bytes())?;
    hashed_files.push(evidence_record(
        "ev-events",
        EvidenceKind::EventLog,
        "events.jsonl",
        events_jsonl.as_bytes(),
        "恢复后重放时间线；含轨道、帧、参数和证据引用",
    ));

    report.evidence = hashed_files.clone();
    let markdown = crate::report::render_markdown(&report);
    let markdown_path = root.join("review.md");
    write_bytes(&markdown_path, markdown.as_bytes())?;
    report.evidence.push(evidence_record(
        "ev-review",
        EvidenceKind::Report,
        "review.md",
        markdown.as_bytes(),
        "自动结论与人工确认分开呈现的可读报告",
    ));

    let report_path = root.join("report.json");
    let report_text = report_to_json(&report)?;
    write_atomic(&report_path, report_text.as_bytes())?;

    let mut all_files: Vec<serde_json::Value> = report
        .evidence
        .iter()
        .map(|record| {
            serde_json::json!({
                "id": record.id,
                "kind": format!("{:?}", record.kind),
                "path": record.relative_path,
                "bytes": record.bytes,
                "sha256": record.sha256,
            })
        })
        .collect();
    all_files.push(serde_json::json!({
        "id": "ev-report",
        "kind": "Report",
        "path": "report.json",
        "bytes": report_text.len(),
        "sha256": sha256_hex(report_text.as_bytes()),
    }));
    let readme_text = bundle_readme(&report);
    write_bytes(&root.join("README.txt"), readme_text.as_bytes())?;
    all_files.push(serde_json::json!({
        "id": "ev-readme",
        "kind": "Metadata",
        "path": "README.txt",
        "bytes": readme_text.len(),
        "sha256": sha256_hex(readme_text.as_bytes()),
    }));
    let manifest = serde_json::json!({
        "schema": "blackframe-evidence/1",
        "report_id": report.report_id,
        "sealed_at": now_utc(),
        "source_sha256": report.source.sha256,
        "algorithm": "sha256",
        "files": all_files,
        "note": "manifest 本身不纳入自身哈希；report.json、review.md、waveform、关键帧与事件日志均在此封存。"
    });
    let manifest_text = serde_json::to_string_pretty(&manifest).unwrap_or_default();
    let manifest_path = root.join("manifest.json");
    write_bytes(&manifest_path, manifest_text.as_bytes())?;
    seal_readonly(&root);

    Ok(BundleResult {
        root,
        report_path,
        markdown_path,
        manifest_path,
        report,
    })
}

fn bundle_name(report: &ReviewReport) -> String {
    let timestamp = Utc::now().format("%Y%m%d-%H%M%S-%3f");
    let suffix = report.report_id.rsplit('-').next().unwrap_or("evidence");
    format!("review-{timestamp}-{suffix}")
}

fn evidence_record(
    id: &str,
    kind: EvidenceKind,
    relative_path: &str,
    bytes: &[u8],
    note: &str,
) -> EvidenceRecord {
    EvidenceRecord {
        id: id.into(),
        kind,
        relative_path: relative_path.into(),
        bytes: bytes.len() as u64,
        sha256: sha256_hex(bytes),
        captured_at: now_utc(),
        note: note.into(),
    }
}

fn write_bytes(path: &Path, bytes: &[u8]) -> Result<(), EvidenceError> {
    let mut file = fs::File::create(path).map_err(|source| EvidenceError::Write {
        path: path.to_path_buf(),
        source,
    })?;
    file.write_all(bytes)
        .map_err(|source| EvidenceError::Write {
            path: path.to_path_buf(),
            source,
        })?;
    file.sync_all().map_err(|source| EvidenceError::Write {
        path: path.to_path_buf(),
        source,
    })
}

fn safe_component(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect()
}

fn render_waveform_csv(report: &ReviewReport) -> String {
    let mut csv = String::from(
        "sample,time_ms,frame,track_id,y_mean,y_min,y_max,black_pixel_ratio,is_black,params_fingerprint,pts_ns\n",
    );
    for sample in &report.waveform.samples {
        csv.push_str(&format!(
            "{},{},{},{},{:.6},{},{},{:.6},{},{},{}\n",
            sample.sample,
            sample.time_ms,
            sample.frame,
            sample.track_id,
            sample.y_mean,
            sample.y_min,
            sample.y_max,
            sample.black_pixel_ratio,
            if sample.is_black { 1 } else { 0 },
            sample.params_fingerprint,
            sample.pts_ns
        ));
    }
    csv
}

fn bundle_readme(report: &ReviewReport) -> String {
    format!(
        "镜头黑场检测证据包\n报告 ID: {}\n源文件: {}\n源 SHA-256: {}\n检查顺序:\n1. 打开 review.md 查看自动结论与人工确认。\n2. 用 waveform.csv 对照 report.json 中区间的 sample_start/sample_end。\n3. 打开 frames/ 下关键帧，核对时间码与帧号。\n4. 用 manifest.json 重新校验所有文件 SHA-256。\n\n自动结论不代表人工确认；两者在报告与界面中始终分栏。\n",
        report.report_id, report.source.path, report.source.sha256
    )
}

#[cfg(unix)]
fn seal_readonly(root: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                seal_readonly(&path);
            } else if let Ok(metadata) = fs::metadata(&path) {
                let permissions = fs::Permissions::from_mode(metadata.permissions().mode() & 0o555);
                let _ = fs::set_permissions(&path, permissions);
            }
        }
    }
}

#[cfg(not(unix))]
fn seal_readonly(_root: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::sha256_hex;

    #[test]
    fn bundle_manifest_covers_every_preserved_file() {
        let directory = tempfile::tempdir().unwrap();
        let output = crate::analysis::demo_output();
        let bundle = write_bundle(directory.path(), output.report, &output.keyframes).unwrap();
        assert!(bundle.report_path.is_file());
        assert!(bundle.markdown_path.is_file());
        assert!(bundle.manifest_path.is_file());
        assert!(bundle.root.join("waveform.csv").is_file());
        assert!(bundle.root.join("events.jsonl").is_file());
        assert!(bundle.root.join("source.sha256").is_file());
        assert!(bundle.root.join("README.txt").is_file());
        assert!(bundle
            .report
            .auto_conclusion
            .intervals
            .iter()
            .all(|interval| !interval.evidence_refs.is_empty()));
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&bundle.manifest_path).unwrap()).unwrap();
        let files = manifest["files"].as_array().unwrap();
        assert!(files.len() >= 8);
        for entry in files {
            let relative = entry["path"].as_str().unwrap();
            let bytes = std::fs::read(bundle.root.join(relative)).unwrap();
            assert_eq!(entry["sha256"].as_str().unwrap(), sha256_hex(&bytes));
            assert_eq!(entry["bytes"].as_u64().unwrap(), bytes.len() as u64);
        }
        assert!(bundle.report.evidence.iter().any(|item| {
            matches!(item.kind, EvidenceKind::KeyFrame) && item.relative_path.starts_with("frames/")
        }));
    }
}
