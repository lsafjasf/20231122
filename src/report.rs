use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{SecondsFormat, Utc};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::model::{
    extension_record, format_timecode, BlackInterval, CompatibilityRecord, DetectorParams,
    ExtensionStatus, IntervalState, ManualConfirmation, ManualStatus, ReplayEvent, ReplayRecord,
    ReplayStatus, ReviewReport, SourceRecord, TrackKind, TrackRecord, Verdict, WaveSample,
    Waveform, LEGACY_SCHEMA_VERSION, SCHEMA_VERSION,
};

#[derive(Debug, Error)]
pub enum ReportError {
    #[error("cannot read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot write {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid report JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported report format: {0}")]
    Unsupported(String),
    #[error("invalid legacy report: {0}")]
    Legacy(String),
}

pub fn now_utc() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub fn new_id(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("{prefix}-{nanos:x}")
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

pub fn sha256_file(path: &Path) -> Result<(String, u64), std::io::Error> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 1024 * 1024];
    let mut total = 0u64;
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total += count as u64;
        hasher.update(&buffer[..count]);
    }
    Ok((hex::encode(hasher.finalize()), total))
}

pub fn source_record(path: &Path) -> Result<SourceRecord, ReportError> {
    let metadata = fs::metadata(path).map_err(|source| ReportError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let (sha256, size_bytes) = sha256_file(path).map_err(|source| ReportError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let modified_utc = metadata
        .modified()
        .ok()
        .map(chrono::DateTime::<Utc>::from)
        .map(|time| time.to_rfc3339_opts(SecondsFormat::Millis, true))
        .unwrap_or_else(now_utc);
    Ok(SourceRecord {
        path: path.display().to_string(),
        uri: format!("file://{}", path.display()),
        file_name: path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("unknown")
            .to_string(),
        extension: extension_record(path),
        size_bytes,
        modified_utc,
        sha256,
        probe_caps: "pending".into(),
    })
}

pub fn new_replay(recovered: bool, note: &str, replayed_from: Option<String>) -> ReplayRecord {
    let mut replay = ReplayRecord {
        session_id: new_id("replay"),
        started_at: now_utc(),
        recovered,
        recovery_note: note.to_string(),
        replay_number: 1,
        replayed_from_report: replayed_from,
        status: ReplayStatus::Running,
        ..ReplayRecord::default()
    };
    replay.events.push(ReplayEvent {
        sequence: 1,
        at_ms: 0,
        frame: None,
        track_id: None,
        kind: "recovery_replay_started".into(),
        detail: if recovered {
            format!("从恢复现场开始重放。{note}")
        } else {
            format!("开始常规重放。{note}")
        },
        params_fingerprint: String::new(),
        waveform_sample: None,
        evidence_refs: Vec::new(),
    });
    replay
}

pub fn report_to_json(report: &ReviewReport) -> Result<String, ReportError> {
    Ok(serde_json::to_string_pretty(report)?)
}

pub fn write_report(path: &Path, report: &ReviewReport) -> Result<(), ReportError> {
    let text = report_to_json(report)?;
    write_atomic(path, text.as_bytes())
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), ReportError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| ReportError::Write {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let temporary = path.with_extension(format!(
        "{}.tmp",
        path.extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("json")
    ));
    {
        let mut file = File::create(&temporary).map_err(|source| ReportError::Write {
            path: temporary.clone(),
            source,
        })?;
        file.write_all(bytes).map_err(|source| ReportError::Write {
            path: temporary.clone(),
            source,
        })?;
        file.sync_all().map_err(|source| ReportError::Write {
            path: temporary.clone(),
            source,
        })?;
    }
    fs::rename(&temporary, path).map_err(|source| ReportError::Write {
        path: path.to_path_buf(),
        source,
    })
}

pub fn read_report(path: &Path) -> Result<ReviewReport, ReportError> {
    let bytes = fs::read(path).map_err(|source| ReportError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    parse_report(&bytes)
}

pub fn parse_report(bytes: &[u8]) -> Result<ReviewReport, ReportError> {
    let value: Value = serde_json::from_slice(bytes)?;
    migrate_value(value)
}

fn migrate_value(value: Value) -> Result<ReviewReport, ReportError> {
    let looks_like_v2 = value.get("auto_conclusion").is_some()
        && value.get("manual_confirmation").is_some()
        && value.get("source").is_some();
    if looks_like_v2 {
        let input_schema = value
            .get("schema_version")
            .and_then(Value::as_str)
            .unwrap_or(SCHEMA_VERSION)
            .to_string();
        let mut report: ReviewReport = serde_json::from_value(value)?;
        report.schema_version = SCHEMA_VERSION.into();
        report.compatibility.input_schema = input_schema;
        report.compatibility.output_schema = SCHEMA_VERSION.into();
        return Ok(report);
    }
    migrate_legacy(value)
}

fn migrate_legacy(value: Value) -> Result<ReviewReport, ReportError> {
    let object = value
        .as_object()
        .ok_or_else(|| ReportError::Unsupported("top-level JSON must be an object".into()))?;
    let path_text = value_text(
        &value,
        &["file", "file_path", "path", "source_path", "media", "input"],
    )
    .ok_or_else(|| ReportError::Legacy("missing file/file_path/path field".into()))?;
    let path = PathBuf::from(&path_text);
    let mut source = if path.exists() {
        source_record(&path)?
    } else {
        let mut source = SourceRecord {
            path: path_text.clone(),
            uri: format!("file://{path_text}"),
            file_name: path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("unknown")
                .into(),
            extension: extension_record(&path),
            size_bytes: 0,
            modified_utc: "unknown".into(),
            sha256: value_text(&value, &["sha256", "file_sha256", "source_sha256"])
                .unwrap_or_default(),
            probe_caps: "not available in legacy report".into(),
        };
        source.extension.content_probe = "legacy report has no content probe".into();
        source
    };
    if let Some(status) = value_text(&value, &["extension_status"]) {
        source.extension.status = match status.as_str() {
            "known" => ExtensionStatus::Known,
            "no_extension" => ExtensionStatus::NoExtension,
            _ => ExtensionStatus::Unknown,
        };
        source.extension.recorded = true;
    }

    let threshold = value_number(&value, &["threshold", "luma_threshold", "pix_th"])
        .map(|value| if value > 1.0 { value / 100.0 } else { value })
        .unwrap_or(0.10) as f32;
    let min_duration_ms = value_number(
        &value,
        &[
            "min_duration_ms",
            "minimum_duration_ms",
            "black_min_duration_ms",
            "min_duration",
            "minimum_duration",
        ],
    )
    .map(|number| {
        let explicit_ms = [
            "min_duration_ms",
            "minimum_duration_ms",
            "black_min_duration_ms",
        ]
        .iter()
        .any(|key| value.get(*key).is_some());
        if explicit_ms || number > 100.0 {
            number.round() as u64
        } else {
            (number * 1000.0).round() as u64
        }
    })
    .unwrap_or(500);
    let sample_fps = value_number(&value, &["sample_fps", "fps"])
        .unwrap_or(5.0)
        .round()
        .clamp(1.0, 60.0) as u32;
    let params = DetectorParams {
        luma_threshold: threshold.clamp(0.0, 1.0),
        min_duration_ms: min_duration_ms.max(1),
        sample_fps,
        ..DetectorParams::default()
    };
    let recovered =
        value_bool(&value, &["recovered", "recovery", "was_recovered"]).unwrap_or(false);
    let generated_at = value_text(
        &value,
        &["generated_at", "created_at", "timestamp", "analyzed_at"],
    )
    .unwrap_or_else(now_utc);
    let replay = ReplayRecord {
        session_id: value_text(&value, &["session_id"]).unwrap_or_else(|| new_id("legacy")),
        started_at: generated_at.clone(),
        recovered,
        recovery_note: value_text(&value, &["recovery_note"]).unwrap_or_default(),
        replay_number: value_number(&value, &["replay_number"])
            .map(|value| value.max(1.0) as u32)
            .unwrap_or(1),
        replayed_from_report: value_text(&value, &["replayed_from_report", "parent_report_id"]),
        status: ReplayStatus::Completed,
        selected_track: Some("video-0".into()),
        duration_ms: value_number(&value, &["duration_ms", "duration"])
            .map(|duration| {
                if duration > 10_000.0 {
                    duration.round() as u64
                } else {
                    (duration * 1000.0).round() as u64
                }
            })
            .unwrap_or(0),
        events: vec![ReplayEvent {
            sequence: 1,
            at_ms: 0,
            frame: None,
            track_id: Some("video-0".into()),
            kind: "legacy_report_loaded".into(),
            detail: "旧格式报告已迁移到 v2；原始波形不可用，按黑场区间重建示意波形。".into(),
            params_fingerprint: params.fingerprint(),
            waveform_sample: None,
            evidence_refs: Vec::new(),
        }],
    };
    let mut report = ReviewReport::new(source, replay, params.clone());
    report.generated_at = generated_at;
    report.report_id = value_text(&value, &["report_id", "id"]).unwrap_or_else(|| new_id("legacy"));
    let input_schema = value_text(&value, &["schema_version", "version", "format"])
        .map(|version| {
            if version == "1" {
                LEGACY_SCHEMA_VERSION.to_string()
            } else {
                version
            }
        })
        .unwrap_or_else(|| LEGACY_SCHEMA_VERSION.into());
    report.compatibility = CompatibilityRecord {
        input_schema,
        output_schema: SCHEMA_VERSION.into(),
        migrated: true,
        warnings: vec![
            "旧格式报告已迁移至 v2；未伪造原始采样。".into(),
            "波形由历史黑场区间重建，仅用于定位，不作为新证据。".into(),
        ],
    };
    let segments = value
        .get("black_segments")
        .or_else(|| value.get("black_intervals"))
        .or_else(|| value.get("segments"))
        .or_else(|| value.get("detections"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    report.auto_conclusion.intervals = legacy_intervals(&segments, &params);
    report.auto_conclusion.verdict = if report.auto_conclusion.intervals.is_empty() {
        Verdict::Clean
    } else {
        Verdict::BlackFramesFound
    };
    report.auto_conclusion.headline = if report.auto_conclusion.intervals.is_empty() {
        "旧报告未包含黑场区间".into()
    } else {
        format!(
            "旧报告记录 {} 段黑场（迁移结果）",
            report.auto_conclusion.intervals.len()
        )
    };
    report.auto_conclusion.confidence = 0.50;
    report.auto_conclusion.reason_codes = vec!["LEGACY_REPORT_IMPORTED".into()];
    report.auto_conclusion.limitations = vec![
        "旧格式无逐帧亮度波形，无法复核自动阈值的实际命中情况".into(),
        "如源文件仍存在，请重新执行恢复后重放以生成 v2 证据".into(),
    ];
    report.manual_confirmation = legacy_manual(&value);
    let max_end = report
        .auto_conclusion
        .intervals
        .iter()
        .map(|interval| interval.end_ms)
        .max()
        .unwrap_or(0)
        .max(report.replay.duration_ms)
        .max(1000);
    report.waveform = reconstructed_waveform(&report.auto_conclusion.intervals, max_end, &params);
    report.auto_conclusion.analyzed_samples = report.waveform.samples.len() as u64;
    report.auto_conclusion.analyzed_ms = report.waveform.covered_ms;
    report.tracks = vec![TrackRecord {
        id: "video-0".into(),
        index: 0,
        kind: TrackKind::Video,
        stream_id: None,
        caps: "legacy report (no caps)".into(),
        source_width: None,
        source_height: None,
        source_fps: Some(params.sample_fps as f64),
        frames_sampled: report.waveform.samples.len() as u64,
    }];
    if object.contains_key("unknown_extension") {
        report.source.extension.status = ExtensionStatus::Unknown;
        report.source.extension.recorded = true;
    }
    Ok(report)
}

fn legacy_intervals(segments: &[Value], params: &DetectorParams) -> Vec<BlackInterval> {
    segments
        .iter()
        .enumerate()
        .filter_map(|(index, segment)| {
            let start_ms = interval_time(segment, true)?;
            let end_ms = interval_time(segment, false)?;
            if end_ms <= start_ms {
                return None;
            }
            let track = value_text(segment, &["track_id", "track"])
                .map(|value| {
                    if value.chars().all(|character| character.is_ascii_digit()) {
                        format!("video-{value}")
                    } else {
                        value
                    }
                })
                .unwrap_or_else(|| "video-0".into());
            let sample_start = start_ms * params.sample_fps as u64 / 1000;
            let sample_end = end_ms * params.sample_fps as u64 / 1000;
            Some(BlackInterval {
                id: value_text(segment, &["id"]).unwrap_or_else(|| format!("bf-{:04}", index + 1)),
                track_id: track,
                start_ms,
                end_ms,
                start_frame: start_ms * params.sample_fps as u64 / 1000,
                end_frame: end_ms * params.sample_fps as u64 / 1000,
                sample_start,
                sample_end,
                mean_luma: value_number(segment, &["mean_luma", "mean"])
                    .map(|value| value as f32)
                    .unwrap_or(0.0),
                lowest_luma: value_number(segment, &["lowest_luma", "min_luma"])
                    .map(|value| value as f32)
                    .unwrap_or(0.0),
                params_fingerprint: params.fingerprint(),
                evidence_refs: Vec::new(),
                state: IntervalState::AutoDetected,
            })
        })
        .collect()
}

fn interval_time(segment: &Value, start: bool) -> Option<u64> {
    let keys = if start {
        [
            "start_ms",
            "start",
            "from_ms",
            "from",
            "begin",
            "timestamp_ms",
        ]
    } else {
        ["end_ms", "end", "to_ms", "to", "finish", "stop_ms"]
    };
    for key in keys {
        if let Some(value) = segment.get(key).and_then(Value::as_f64) {
            let is_explicit_ms = key.contains("_ms") || key == "timestamp_ms";
            return Some(if is_explicit_ms || value > 10_000.0 {
                value.round().max(0.0) as u64
            } else {
                (value * 1000.0).round().max(0.0) as u64
            });
        }
    }
    None
}

fn legacy_manual(value: &Value) -> ManualConfirmation {
    let nested = value.get("manual_confirmation");
    let get = |keys: &[&str]| -> Option<String> {
        nested
            .and_then(|item| value_text(item, keys))
            .or_else(|| value_text(value, keys))
    };
    let raw_status = get(&["status", "review_status", "manual_status"])
        .or_else(|| {
            value_bool(value, &["reviewed"]).map(|reviewed| {
                if reviewed {
                    "confirmed".into()
                } else {
                    "pending".into()
                }
            })
        })
        .unwrap_or_else(|| "pending".into());
    let status = match raw_status.to_ascii_lowercase().as_str() {
        "confirmed" | "approved" | "accepted" => ManualStatus::Confirmed,
        "rejected" | "declined" => ManualStatus::Rejected,
        "needs_review" | "review" | "hold" => ManualStatus::NeedsReview,
        _ => ManualStatus::Pending,
    };
    let reviewed_at = get(&["reviewed_at", "confirmed_at"]).or_else(|| {
        if status == ManualStatus::Pending {
            None
        } else {
            Some(get(&["created_at", "timestamp", "reviewed_at"]).unwrap_or_else(now_utc))
        }
    });
    ManualConfirmation {
        status,
        reviewer: get(&["reviewer", "operator", "reviewed_by"]).unwrap_or_default(),
        reviewed_at,
        note: get(&["note", "review_note", "comment"]).unwrap_or_default(),
        checks: Vec::new(),
    }
}

fn reconstructed_waveform(
    intervals: &[BlackInterval],
    end_ms: u64,
    params: &DetectorParams,
) -> Waveform {
    let period = (1000 / params.sample_fps.max(1) as u64).max(50);
    let max_samples = 12_000u64;
    let step = (end_ms / max_samples).max(period);
    let mut samples = Vec::new();
    let mut time_ms = 0u64;
    let mut index = 0u64;
    while time_ms <= end_ms {
        let interval = intervals
            .iter()
            .find(|interval| time_ms >= interval.start_ms && time_ms < interval.end_ms);
        let is_black = interval.is_some();
        samples.push(WaveSample {
            sample: index,
            time_ms,
            pts_ns: time_ms * 1_000_000,
            frame: time_ms * params.sample_fps as u64 / 1000,
            track_id: interval
                .map(|item| item.track_id.clone())
                .unwrap_or_else(|| "video-0".into()),
            y_mean: if is_black { 0.0 } else { 0.12 },
            y_min: if is_black { 0 } else { 20 },
            y_max: if is_black { 2 } else { 48 },
            black_pixel_ratio: if is_black { 1.0 } else { 0.0 },
            is_black,
            params_fingerprint: params.fingerprint(),
        });
        index += 1;
        time_ms = time_ms.saturating_add(step);
    }
    let mut waveform = crate::luma::waveform_from_samples(samples, step);
    waveform.source = "legacy interval rasterization (位置示意，不是原始波形)".into();
    waveform
}

fn value_text(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key))
        .and_then(|value| match value {
            Value::String(text) if !text.trim().is_empty() => Some(text.clone()),
            Value::Number(number) => Some(number.to_string()),
            _ => None,
        })
}

fn value_number(value: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter()
        .find_map(|key| value.get(*key))
        .and_then(|value| match value {
            Value::Number(number) => number.as_f64(),
            Value::String(text) => text.parse::<f64>().ok(),
            _ => None,
        })
}

fn value_bool(value: &Value, keys: &[&str]) -> Option<bool> {
    keys.iter()
        .find_map(|key| value.get(*key))
        .and_then(|value| match value {
            Value::Bool(boolean) => Some(*boolean),
            Value::String(text) => match text.to_ascii_lowercase().as_str() {
                "true" | "yes" | "1" | "recovered" => Some(true),
                "false" | "no" | "0" => Some(false),
                _ => None,
            },
            _ => None,
        })
}

pub fn render_markdown(report: &ReviewReport) -> String {
    let mut out = String::new();
    out.push_str("# 镜头黑场检测复核报告\n\n");
    out.push_str(&format!("- 报告 ID：`{}`\n", report.report_id));
    out.push_str(&format!("- 生成时间：{}\n", report.generated_at));
    out.push_str(&format!("- 源文件：`{}`\n", report.source.path));
    out.push_str(&format!("- SHA-256：`{}`\n", report.source.sha256));
    out.push_str(&format!(
        "- 扩展名：{}\n",
        match &report.source.extension.raw_extension {
            Some(ext) => format!("`.{ext}` / {:?}", report.source.extension.status),
            None => format!("无 / {:?}", report.source.extension.status),
        }
    ));
    out.push_str(&format!(
        "- 重放：会话 `{}`，第 {} 次，恢复状态 {}\n",
        report.replay.session_id,
        report.replay.replay_number,
        if report.replay.recovered {
            "已恢复"
        } else {
            "常规"
        }
    ));
    out.push_str(&format!(
        "- 格式兼容：输入 {} → 输出 {}；迁移 {}\n\n",
        report.compatibility.input_schema,
        report.compatibility.output_schema,
        if report.compatibility.migrated {
            "是"
        } else {
            "否"
        }
    ));

    out.push_str("## 自动结论（只读）\n\n");
    out.push_str(&format!(
        "**{}** · 置信度 {:.0}% · 状态 {:?}\n\n",
        report.auto_conclusion.headline,
        report.auto_conclusion.confidence * 100.0,
        report.auto_conclusion.verdict
    ));
    out.push_str(&format!(
        "- 判定：`{:?}`\n- 采样：{} 点 / {} ms\n- 处理参数指纹：`{}`\n",
        report.auto_conclusion.verdict,
        report.auto_conclusion.analyzed_samples,
        report.auto_conclusion.analyzed_ms,
        report.detector.fingerprint()
    ));
    if !report.auto_conclusion.reason_codes.is_empty() {
        out.push_str(&format!(
            "- 原因码：{}\n",
            report.auto_conclusion.reason_codes.join(", ")
        ));
    }
    if !report.auto_conclusion.limitations.is_empty() {
        out.push_str("\n限制与歧义：\n");
        for limitation in &report.auto_conclusion.limitations {
            out.push_str(&format!("- {}\n", limitation));
        }
    }
    out.push('\n');

    out.push_str("## 人工确认（独立于自动结论）\n\n");
    out.push_str(&format!(
        "- 状态：`{:?}`\n- 复核人：{}\n- 复核时间：{}\n- 备注：{}\n\n",
        report.manual_confirmation.status,
        if report.manual_confirmation.reviewer.is_empty() {
            "未填写"
        } else {
            &report.manual_confirmation.reviewer
        },
        report
            .manual_confirmation
            .reviewed_at
            .as_deref()
            .unwrap_or("未确认"),
        if report.manual_confirmation.note.is_empty() {
            "无"
        } else {
            &report.manual_confirmation.note
        }
    ));
    if !report.manual_confirmation.checks.is_empty() {
        out.push_str("| 区间 | 人工判定 | 备注 |\n|---|---|---|\n");
        for check in &report.manual_confirmation.checks {
            out.push_str(&format!(
                "| {} | {:?} | {} |\n",
                check.interval_id,
                check.decision,
                markdown_cell(&check.note)
            ));
        }
        out.push('\n');
    }

    out.push_str("## 时间点、轨道、帧、参数与波形对应\n\n");
    out.push_str("| 区间 | 起始时间码 | 结束时间码 | 轨道 | 采样/帧 | 参数指纹 | 关键证据 |\n");
    out.push_str("|---|---|---|---|---|---|---|\n");
    for interval in &report.auto_conclusion.intervals {
        out.push_str(&format!(
            "| {} | {} | {} | {} | 样本 {}-{} / 帧 {}-{} | `{}` | {} |\n",
            interval.id,
            format_timecode(interval.start_ms),
            format_timecode(interval.end_ms),
            interval.track_id,
            interval.sample_start,
            interval.sample_end,
            interval.start_frame,
            interval.end_frame,
            interval.params_fingerprint,
            if interval.evidence_refs.is_empty() {
                "无".into()
            } else {
                interval.evidence_refs.join("<br>")
            }
        ));
    }
    if report.auto_conclusion.intervals.is_empty() {
        out.push_str("| - | - | - | - | - | - | 无自动检测区间 |\n");
    }
    out.push('\n');
    out.push_str(&format!(
        "波形来源：{}；单位：{}；周期 {} ms；覆盖 {} ms。\n\n",
        report.waveform.source,
        report.waveform.unit,
        report.waveform.sample_period_ms,
        report.waveform.covered_ms
    ));

    if let Some(failure) = &report.failure {
        out.push_str("## 失败与后续操作\n\n");
        out.push_str(&format!(
            "**{}** (`{}`)\n\n{}\n\n",
            failure.title, failure.code, failure.detail
        ));
        for (index, action) in failure.actions.iter().enumerate() {
            out.push_str(&format!(
                "{}. **{}** — {}\n",
                index + 1,
                action.label,
                action.detail
            ));
            if let Some(command) = &action.command {
                out.push_str(&format!("   ```sh\n   {command}\n   ```\n"));
            }
        }
        out.push('\n');
    }

    out.push_str("## 证据清单\n\n");
    if report.evidence.is_empty() {
        out.push_str("无已封存的证据文件。\n\n");
    } else {
        out.push_str("| ID | 类型 | 文件 | 字节 | SHA-256 |\n|---|---|---|---|---|\n");
        for evidence in &report.evidence {
            out.push_str(&format!(
                "| {} | {:?} | `{}` | {} | `{}` |\n",
                evidence.id, evidence.kind, evidence.relative_path, evidence.bytes, evidence.sha256
            ));
        }
        out.push('\n');
    }
    if !report.compatibility.warnings.is_empty() {
        out.push_str("## 兼容性提示\n\n");
        for warning in &report.compatibility.warnings {
            out.push_str(&format!("- {warning}\n"));
        }
    }
    out
}

fn markdown_cell(text: &str) -> String {
    text.replace('|', "\\|").replace('\n', " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_v1_is_migrated_without_inventing_raw_waveform() {
        let legacy = serde_json::json!({
            "version": 1,
            "file": "/tmp/recovery.recx",
            "recovered": true,
            "threshold": 10,
            "minimum_duration": 0.5,
            "black_segments": [
                {"start": 1.2, "end": 1.9, "track": 0, "mean_luma": 0.02}
            ],
            "reviewed": true,
            "reviewer": "alice",
            "review_note": "confirmed on restored copy"
        });
        let bytes = serde_json::to_vec(&legacy).unwrap();
        let report = parse_report(&bytes).unwrap();
        assert!(report.compatibility.migrated);
        assert_eq!(report.schema_version, SCHEMA_VERSION);
        assert_eq!(report.source.extension.status, ExtensionStatus::Unknown);
        assert!(report.source.extension.recorded);
        assert!((report.detector.luma_threshold - 0.10).abs() < 0.001);
        assert_eq!(report.detector.min_duration_ms, 500);
        assert_eq!(report.auto_conclusion.verdict, Verdict::BlackFramesFound);
        assert_eq!(report.auto_conclusion.intervals.len(), 1);
        assert_eq!(report.auto_conclusion.intervals[0].start_ms, 1200);
        assert_eq!(report.auto_conclusion.intervals[0].end_ms, 1900);
        assert_eq!(report.manual_confirmation.status, ManualStatus::Confirmed);
        assert_eq!(report.manual_confirmation.reviewer, "alice");
        assert!(report.waveform.source.contains("legacy"));
        assert!(report
            .compatibility
            .warnings
            .iter()
            .any(|warning| warning.contains("未伪造")));
    }

    #[test]
    fn v2_roundtrip_keeps_auto_and_manual_separate() {
        let mut report = crate::analysis::demo_output().report;
        report.manual_confirmation.status = ManualStatus::Confirmed;
        report.manual_confirmation.reviewer = "reviewer-7".into();
        report.manual_confirmation.reviewed_at = Some(now_utc());
        let json = report_to_json(&report).unwrap();
        let parsed = parse_report(json.as_bytes()).unwrap();
        assert_eq!(parsed.auto_conclusion.verdict, Verdict::BlackFramesFound);
        assert_eq!(parsed.manual_confirmation.status, ManualStatus::Confirmed);
        assert_eq!(parsed.manual_confirmation.reviewer, "reviewer-7");
        assert_eq!(
            parsed.auto_conclusion.intervals[0].state,
            IntervalState::AutoDetected
        );
    }

    #[test]
    fn markdown_keeps_read_only_auto_section_before_manual_section() {
        let mut report = crate::analysis::demo_output().report;
        report.manual_confirmation.status = ManualStatus::NeedsReview;
        report.manual_confirmation.reviewer = "operator".into();
        report.manual_confirmation.note = "check source restore".into();
        let markdown = render_markdown(&report);
        let auto = markdown.find("## 自动结论（只读）").unwrap();
        let manual = markdown.find("## 人工确认（独立于自动结论）").unwrap();
        assert!(auto < manual);
        assert!(markdown.contains("NeedsReview"));
        assert!(markdown.contains("check source restore"));
    }

    #[test]
    fn unknown_extension_is_recorded_as_unknown_but_analyzable() {
        let extension = extension_record(std::path::Path::new("camera-A.recx"));
        assert_eq!(extension.status, ExtensionStatus::Unknown);
        assert!(extension.recorded);
        assert_eq!(extension.raw_extension.as_deref(), Some("recx"));
        assert!(extension.policy.contains("content probe is authoritative"));
    }
}
