use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::{io_error, AppError, AppResult};
use crate::model::{
    DebugRecord, EvidenceState, Manifest, ManifestFile, RunStatus, TimelineEntry,
    VerificationCheck, MANIFEST_FILE, OUTPUT_WAV, RECORD_FILE, REVIEW_FILE, SCHEMA, TIMELINE_FILE,
    WAVEFORM_FILE,
};
use crate::wav::{analyze_timeline_waveforms, output_peak_and_clipping, probe_wav, round6};

pub const RECORD_FIELD_ORDER: &[&str] = &[
    "schema",
    "status",
    "job_id",
    "created_utc",
    "request",
    "inputs",
    "processing",
    "timeline",
    "anomalies",
    "output",
    "verification",
    "recording_policy",
];

pub const TIMELINE_HEADER: &str = "index,render_segment_index,track_id,channel,source_start_us,source_end_us,source_frame_start,source_frame_end,output_start_us,output_end_us,output_frame_start,output_frame_end,speed_rate_curve,speed_rate_applied,pitch_semitones_curve,pitch_semitones_applied,pitch_ratio_applied,pitch_evidence,waveform_frames,waveform_min,waveform_max,waveform_rms,waveform_peak_dbfs";

pub const WAVEFORM_HEADER: &str = "index,track_id,channel,output_start_us,output_end_us,output_frame_start,output_frame_end,min,max,rms,peak_dbfs,sample_frames";

#[derive(Debug, Clone)]
pub struct VerifyReport {
    pub checks: Vec<VerificationCheck>,
}

impl VerifyReport {
    pub fn passed(&self) -> bool {
        self.checks.iter().all(|check| check.passed)
    }
}

pub fn canonical_record_bytes(record: &DebugRecord) -> AppResult<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(record)?;
    bytes.push(b'\n');
    Ok(bytes)
}

pub fn load_record(path: &Path) -> AppResult<DebugRecord> {
    let bytes = fs::read(path).map_err(|err| io_error(path, err))?;
    let record: DebugRecord = serde_json::from_slice(&bytes)?;
    if record.schema != SCHEMA {
        return Err(AppError::InvalidInput(format!(
            "{}: unsupported schema {}",
            path.display(),
            record.schema
        )));
    }
    Ok(record)
}

pub fn write_artifact_set(record: &DebugRecord, output_dir: &Path) -> AppResult<Manifest> {
    fs::create_dir_all(output_dir).map_err(|err| io_error(output_dir, err))?;
    write_timeline_csv(&output_dir.join(TIMELINE_FILE), &record.timeline)?;
    write_waveform_csv(&output_dir.join(WAVEFORM_FILE), &record.timeline)?;
    write_review(&output_dir.join(REVIEW_FILE), record)?;

    let record_bytes = canonical_record_bytes(record)?;
    write_bytes_atomic(&output_dir.join(RECORD_FILE), &record_bytes)?;

    let mut files = Vec::new();
    for (role, path) in [
        ("output_audio", OUTPUT_WAV),
        ("timeline_csv", TIMELINE_FILE),
        ("waveform_csv", WAVEFORM_FILE),
        ("human_review", REVIEW_FILE),
    ] {
        let full_path = output_dir.join(path);
        let (size_bytes, sha256) = file_hash(&full_path)?;
        files.push(ManifestFile {
            role: role.to_string(),
            path: path.to_string(),
            size_bytes,
            sha256,
        });
    }
    let manifest = Manifest {
        schema: SCHEMA.to_string(),
        record_file: RECORD_FILE.to_string(),
        record_sha256: sha256_bytes(&record_bytes),
        files,
        reload_contract: crate::model::ReloadContract {
            field_order: RECORD_FIELD_ORDER
                .iter()
                .map(|value| value.to_string())
                .collect(),
            canonical_json_pretty: true,
            timeline_order: "ascending index; non-overlapping output_frame ranges".to_string(),
            verified: true,
        },
    };
    let manifest_bytes = pretty_json_bytes(&manifest)?;
    write_bytes_atomic(&output_dir.join(MANIFEST_FILE), &manifest_bytes)?;
    Ok(manifest)
}

pub fn verify_artifact(output_dir: &Path) -> AppResult<VerifyReport> {
    let mut checks = Vec::new();
    let manifest_path = output_dir.join(MANIFEST_FILE);
    let manifest = read_manifest(&manifest_path)?;
    checks.push(VerificationCheck {
        name: "manifest_supported".to_string(),
        passed: manifest.schema == SCHEMA,
        detail: manifest.schema.clone(),
    });

    let record_path = output_dir.join(&manifest.record_file);
    let record_bytes = fs::read(&record_path).map_err(|err| io_error(&record_path, err))?;
    let record_hash = sha256_bytes(&record_bytes);
    checks.push(VerificationCheck {
        name: "record_sha256_matches_manifest".to_string(),
        passed: record_hash == manifest.record_sha256,
        detail: if record_hash == manifest.record_sha256 {
            "record.json matches manifest.record_sha256".to_string()
        } else {
            format!("{} != {}", record_hash, manifest.record_sha256)
        },
    });

    let record = load_record(&record_path)?;
    let canonical = canonical_record_bytes(&record)?;
    checks.push(VerificationCheck {
        name: "reload_preserves_fields_and_order".to_string(),
        passed: canonical == record_bytes,
        detail: "record.json is canonical pretty JSON and round-trips byte-for-byte".to_string(),
    });
    checks.push(VerificationCheck {
        name: "reload_contract_field_order".to_string(),
        passed: manifest.reload_contract.field_order
            == RECORD_FIELD_ORDER
                .iter()
                .map(|value| value.to_string())
                .collect::<Vec<_>>(),
        detail: manifest.reload_contract.field_order.join(","),
    });

    let manifest_ok = manifest.files.iter().all(|entry| {
        let path = output_dir.join(&entry.path);
        file_hash(&path)
            .map(|(size, hash)| size == entry.size_bytes && hash == entry.sha256)
            .unwrap_or(false)
    });
    checks.push(VerificationCheck {
        name: "manifest_file_hashes".to_string(),
        passed: manifest_ok,
        detail: format!("{} sidecar files checked", manifest.files.len()),
    });

    let output_path = output_dir.join(OUTPUT_WAV);
    let output_ok = if record.output.frames == 0 {
        output_path.is_file()
    } else {
        file_hash(&output_path)
            .map(|(size, hash)| size == record.output.size_bytes && hash == record.output.sha256)
            .unwrap_or(false)
    };
    checks.push(VerificationCheck {
        name: "output_wav_hash_and_size".to_string(),
        passed: output_ok,
        detail: record.output.sha256.clone(),
    });

    let wav_ok = probe_wav(&output_path)
        .map(|info| {
            info.frames == record.output.frames
                && info.channels == record.output.channels
                && info.sample_rate_hz == record.output.sample_rate_hz
        })
        .unwrap_or(false);
    checks.push(VerificationCheck {
        name: "output_wav_header_matches_record".to_string(),
        passed: wav_ok,
        detail: format!(
            "{} frames, {} Hz, {} ch",
            record.output.frames, record.output.sample_rate_hz, record.output.channels
        ),
    });

    let mut recalculated = record.timeline.clone();
    let waveform_recalculated = analyze_timeline_waveforms(
        &output_path,
        &mut recalculated,
        record.processing.analysed_channel,
    );
    let waveform_ok = waveform_recalculated.is_ok()
        && recalculated
            .iter()
            .zip(&record.timeline)
            .all(|(left, right)| left.waveform == right.waveform);
    checks.push(VerificationCheck {
        name: "waveform_recomputed_from_output".to_string(),
        passed: waveform_ok,
        detail: format!("{} timeline rows compared", record.timeline.len()),
    });

    let timeline_header_ok = first_line(&output_dir.join(TIMELINE_FILE))
        .map(|line| line == TIMELINE_HEADER)
        .unwrap_or(false);
    let waveform_header_ok = first_line(&output_dir.join(WAVEFORM_FILE))
        .map(|line| line == WAVEFORM_HEADER)
        .unwrap_or(false);
    checks.push(VerificationCheck {
        name: "csv_headers_and_order".to_string(),
        passed: timeline_header_ok && waveform_header_ok,
        detail: "timeline.csv and waveform.csv headers are stable".to_string(),
    });

    let timeline_rows = count_csv_data_rows(&output_dir.join(TIMELINE_FILE)).unwrap_or(0);
    let waveform_rows = count_csv_data_rows(&output_dir.join(WAVEFORM_FILE)).unwrap_or(0);
    checks.push(VerificationCheck {
        name: "csv_row_counts_match_record".to_string(),
        passed: timeline_rows == record.timeline.len() as u64
            && waveform_rows == record.timeline.len() as u64,
        detail: format!(
            "timeline rows={timeline_rows}, waveform rows={waveform_rows}, record rows={}",
            record.timeline.len()
        ),
    });

    checks.push(VerificationCheck {
        name: "record_checks_passed".to_string(),
        passed: record.verification.checks.iter().all(|check| check.passed),
        detail: format!("{} producer checks", record.verification.checks.len()),
    });
    Ok(VerifyReport { checks })
}

pub fn file_hash(path: &Path) -> AppResult<(u64, String)> {
    let mut file = File::open(path).map_err(|err| io_error(path, err))?;
    let size = file.metadata().map_err(|err| io_error(path, err))?.len();
    let mut hasher = <sha2::Sha256 as sha2::Digest>::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|err| io_error(path, err))?;
        if read == 0 {
            break;
        }
        sha2::Digest::update(&mut hasher, &buffer[..read]);
    }
    Ok((size, hex::encode(sha2::Digest::finalize(hasher))))
}

pub fn sha256_bytes(bytes: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(bytes))
}

pub fn pretty_json_bytes<T: Serialize>(value: &T) -> AppResult<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn write_bytes_atomic(path: &Path, bytes: &[u8]) -> AppResult<()> {
    let temp_path = path.with_extension(format!(
        "{}.tmp",
        path.extension()
            .and_then(|value| value.to_str())
            .unwrap_or("file")
    ));
    {
        let mut file = File::create(&temp_path).map_err(|err| io_error(&temp_path, err))?;
        file.write_all(bytes)
            .map_err(|err| io_error(&temp_path, err))?;
        file.sync_all().map_err(|err| io_error(&temp_path, err))?;
    }
    fs::rename(&temp_path, path).map_err(|err| io_error(path, err))?;
    Ok(())
}

fn read_manifest(path: &Path) -> AppResult<Manifest> {
    let bytes = fs::read(path).map_err(|err| io_error(path, err))?;
    let manifest: Manifest = serde_json::from_slice(&bytes)?;
    if manifest.schema != SCHEMA {
        return Err(AppError::InvalidInput(format!(
            "{}: unsupported manifest schema {}",
            path.display(),
            manifest.schema
        )));
    }
    Ok(manifest)
}

fn first_line(path: &Path) -> AppResult<String> {
    let text = fs::read_to_string(path).map_err(|err| io_error(path, err))?;
    Ok(text
        .lines()
        .next()
        .unwrap_or("")
        .trim_end_matches('\r')
        .to_string())
}

fn count_csv_data_rows(path: &Path) -> AppResult<u64> {
    let text = fs::read_to_string(path).map_err(|err| io_error(path, err))?;
    Ok(text.lines().skip(1).filter(|line| !line.is_empty()).count() as u64)
}

fn write_timeline_csv(path: &Path, entries: &[TimelineEntry]) -> AppResult<()> {
    let mut writer = csv::WriterBuilder::new()
        .has_headers(false)
        .from_path(path)?;
    writer.write_record(TIMELINE_HEADER.split(','))?;
    for entry in entries {
        writer.write_record([
            entry.index.to_string(),
            entry.render_segment_index.to_string(),
            entry.track_id.clone(),
            entry.channel.to_string(),
            entry.source_start_us.to_string(),
            entry.source_end_us.to_string(),
            entry.source_frame_start.to_string(),
            entry.source_frame_end.to_string(),
            entry.output_start_us.to_string(),
            entry.output_end_us.to_string(),
            entry.output_frame_start.to_string(),
            entry.output_frame_end.to_string(),
            format_f64(entry.speed_rate_curve),
            format_f64(entry.speed_rate_applied),
            format_f64(entry.pitch_semitones_curve),
            format_f64(entry.pitch_semitones_applied),
            format_f64(entry.pitch_ratio_applied),
            evidence_state_name(entry.pitch_evidence).to_string(),
            entry.waveform.sample_frames.to_string(),
            format_f64(entry.waveform.min),
            format_f64(entry.waveform.max),
            format_f64(entry.waveform.rms),
            format_f64(entry.waveform.peak_dbfs),
        ])?;
    }
    writer.flush().map_err(|err| io_error(path, err))?;
    Ok(())
}

fn write_waveform_csv(path: &Path, entries: &[TimelineEntry]) -> AppResult<()> {
    let mut writer = csv::WriterBuilder::new()
        .has_headers(false)
        .from_path(path)?;
    writer.write_record(WAVEFORM_HEADER.split(','))?;
    for entry in entries {
        writer.write_record([
            entry.index.to_string(),
            entry.track_id.clone(),
            entry.channel.to_string(),
            entry.output_start_us.to_string(),
            entry.output_end_us.to_string(),
            entry.output_frame_start.to_string(),
            entry.output_frame_end.to_string(),
            format_f64(entry.waveform.min),
            format_f64(entry.waveform.max),
            format_f64(entry.waveform.rms),
            format_f64(entry.waveform.peak_dbfs),
            entry.waveform.sample_frames.to_string(),
        ])?;
    }
    writer.flush().map_err(|err| io_error(path, err))?;
    Ok(())
}

fn write_review(path: &Path, record: &DebugRecord) -> AppResult<()> {
    let mut text = String::new();
    text.push_str("# 变速不变调试听复核记录\n\n");
    text.push_str(&format!("- 作业: `{}`\n", record.job_id));
    text.push_str(&format!("- 状态: `{:?}`\n", record.status));
    text.push_str(&format!("- 场景: `{}`\n", record.request.scenario_name));
    text.push_str(&format!("- 创建时间: `{}`\n", record.created_utc));
    text.push_str(&format!(
        "- 处理链: `{}`\n",
        record.processing.plugin_chain.join(" -> ")
    ));
    text.push_str(&format!(
        "- 源音频: {} 帧, {} Hz, {} 声道\n",
        record.inputs.audio.frames,
        record.inputs.audio.sample_rate_hz,
        record.inputs.audio.channels
    ));
    text.push_str(&format!(
        "- 输出波形: {} 帧, {} us, peak {} dBFS\n",
        record.output.frames, record.output.duration_us, record.output.peak_dbfs
    ));
    text.push_str(&format!(
        "- 窗口: {} us（请求 {} us），渲染段 {} 个\n",
        record.processing.window_us,
        record.request.window_us,
        record.processing.render_segment_count
    ));
    text.push_str(&format!(
        "- 速度范围: {} .. {}；音高范围: {} .. {} st\n\n",
        record.processing.speed_parameter.min,
        record.processing.speed_parameter.max,
        record.processing.pitch_parameter.min,
        record.processing.pitch_parameter.max
    ));

    text.push_str("## 异常与证据\n\n");
    if record.anomalies.is_empty() {
        text.push_str("未发现异常。\n\n");
    } else {
        text.push_str("| ID | 类型 | 轨道/通道 | 源时间 us | 输出时间 us | 源帧 | 参数 | 波形 |\n");
        text.push_str("|---|---|---|---|---|---|---|---|\n");
        for anomaly in &record.anomalies {
            text.push_str(&format!(
                "| {} | {:?} | {}/{} | {}..{} | {}..{} | {}..{} | speed {} (applied {}), pitch {} st (applied {} st, ratio {}) | {} |\n",
                anomaly.id,
                anomaly.kind,
                anomaly.track_id,
                anomaly.channel,
                anomaly.source_span.start_us,
                anomaly.source_span.end_us,
                anomaly.output_span.start_us,
                anomaly.output_span.end_us,
                anomaly.source_frames.start,
                anomaly.source_frames.end,
                anomaly.parameters.speed_rate_curve,
                anomaly.parameters.speed_rate_applied,
                anomaly.parameters.pitch_semitones_curve,
                anomaly.parameters.pitch_semitones_applied,
                anomaly.parameters.pitch_ratio_applied,
                waveform_summary(anomaly.waveform_at_gap)
            ));
        }
        text.push('\n');
    }

    text.push_str("## 逐窗映射示例\n\n");
    text.push_str("| # | 源时间 us | 输出时间 us | 源帧 | 输出帧 | speed 曲线/应用 | pitch 曲线/应用 st | 证据 | 波形 RMS/peak |\n");
    text.push_str("|---|---|---|---|---|---|---|---|---|\n");
    for (row, entry) in review_rows(record).into_iter().enumerate() {
        text.push_str(&format!(
            "| {} | {}..{} | {}..{} | {}..{} | {}..{} | {}/{} | {}/{} | {} | {}/{} |\n",
            row + 1,
            entry.source_start_us,
            entry.source_end_us,
            entry.output_start_us,
            entry.output_end_us,
            entry.source_frame_start,
            entry.source_frame_end,
            entry.output_frame_start,
            entry.output_frame_end,
            format_f64(entry.speed_rate_curve),
            format_f64(entry.speed_rate_applied),
            format_f64(entry.pitch_semitones_curve),
            format_f64(entry.pitch_semitones_applied),
            evidence_state_name(entry.pitch_evidence),
            format_f64(entry.waveform.rms),
            format_f64(entry.waveform.peak_dbfs),
        ));
    }
    if record.timeline.len() > REVIEW_ROW_LIMIT {
        text.push_str(&format!(
            "\n仅展示异常附近和前后的样本；完整 {} 行见 `timeline.csv`。\n",
            record.timeline.len()
        ));
    }

    text.push_str("\n## 独立复核\n\n");
    text.push_str("1. `sha256sum record.json timeline.csv waveform.csv output.wav`\n");
    text.push_str(&format!(
        "2. `{} verify --artifact {}`\n",
        "tempo-pitch-debug",
        shell_quote(&record.request.output_dir)
    ));
    text.push_str(
        "3. 对照 `waveform.csv` 的 `output_frame_*` 与 `output.wav` 的 16-bit PCM 帧；\n",
    );
    text.push_str("4. 对照 `timeline.csv` 的源时间/源帧、渲染参数、输出帧与异常表。\n\n");
    text.push_str("## 关键哈希\n\n");
    text.push_str(&format!("- 音频输入: `{}`\n", record.inputs.audio.sha256));
    text.push_str(&format!(
        "- 速度曲线: `{}`\n",
        record.inputs.speed_curve.sha256
    ));
    text.push_str(&format!(
        "- 音高轨迹: `{}`\n",
        record.inputs.pitch_track.sha256
    ));
    text.push_str(&format!("- 输出 WAV: `{}`\n", record.output.sha256));
    text.push_str(&format!(
        "- 记录重载: `{}`\n",
        if record.verification.reload_round_trip {
            "字段与顺序已验证"
        } else {
            "待复核"
        }
    ));
    write_bytes_atomic(path, text.as_bytes())
}

const REVIEW_ROW_LIMIT: usize = 80;

fn review_rows(record: &DebugRecord) -> Vec<&TimelineEntry> {
    if record.timeline.len() <= REVIEW_ROW_LIMIT {
        return record.timeline.iter().collect();
    }
    let mut rows = Vec::new();
    rows.extend(record.timeline.iter().take(20));
    for anomaly in &record.anomalies {
        let start = anomaly.timeline_index_span.start as usize;
        let end = (anomaly.timeline_index_span.end as usize + 1).min(record.timeline.len());
        let start = start.saturating_sub(10);
        let end = end.saturating_add(10).min(record.timeline.len());
        rows.extend(record.timeline[start..end].iter());
    }
    rows.extend(record.timeline.iter().rev().take(20).rev());
    rows.sort_by_key(|entry| entry.index);
    rows.dedup_by_key(|entry| entry.index);
    rows
}

fn waveform_summary(stats: crate::model::WaveformStats) -> String {
    format!(
        "{} frames, {}/{}, rms {}, peak {} dBFS",
        stats.sample_frames,
        format_f64(stats.min),
        format_f64(stats.max),
        format_f64(stats.rms),
        format_f64(stats.peak_dbfs)
    )
}

fn evidence_state_name(state: EvidenceState) -> &'static str {
    match state {
        EvidenceState::Observed => "observed",
        EvidenceState::InterpolatedAcrossGap => "interpolated_across_gap",
        EvidenceState::HeldLastKnown => "held_last_known",
        EvidenceState::DefaultNominal => "default_nominal",
        EvidenceState::MissingAudio => "missing_audio",
    }
}

fn format_f64(value: f64) -> String {
    if value.is_finite() {
        let rounded = round6(value);
        let mut text = format!("{rounded}");
        if !text.contains('.') && !text.contains('e') && !text.contains('E') {
            text.push_str(".0");
        }
        text
    } else {
        "null".to_string()
    }
}

fn shell_quote(value: &str) -> String {
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || "/._-".contains(ch))
    {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

pub fn artifact_output_path(output_dir: &Path) -> PathBuf {
    output_dir.join(OUTPUT_WAV)
}

pub fn has_anomalies(record: &DebugRecord) -> bool {
    !record.anomalies.is_empty()
}

pub fn status_for(record: &DebugRecord) -> RunStatus {
    record.status
}

pub fn verified_peak(path: &Path) -> AppResult<(f64, u64)> {
    output_peak_and_clipping(path)
}
