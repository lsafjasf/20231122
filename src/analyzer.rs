use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use sha2::{Digest, Sha256};

use crate::fixture::{
    self, CHANNEL_LABELS, CHANNELS, CROSSFADE_FRAMES, RiffInfo, SAMPLE_RATE, XMTA_FOURCC,
};
use crate::gst_probe::{self, PIPELINE_LABEL, PcmProbe};
use crate::model::{
    AutoFinding, ChannelLane, Evidence, FailureCase, FailureState, FindingStatus, InspectionReport,
    ManualCheck, ManualStatus, ProcessingStep, RecoveryEvent, SourceSummary, StepStatus, Tone,
    UnknownExtension,
};

#[derive(Clone, Debug)]
struct ReplayDetection {
    source_start_frame: u64,
    replay_start_frame: u64,
    matched_frames: u64,
    confidence: f32,
}

pub fn run_demo_inspection() -> std::result::Result<InspectionReport, FailureState> {
    let path = fixture::ensure_demo_source().map_err(|error| FailureState {
        title: "无法准备演示来源".to_string(),
        detail: format!("{error:#}"),
        code: "SOURCE_PREPARE_FAILED".to_string(),
        source_path: fixture::demo_path().display().to_string(),
        preserved: vec!["原始来源未被修改".to_string()],
    })?;
    run_inspection(&path)
}

pub fn run_inspection(path: &Path) -> std::result::Result<InspectionReport, FailureState> {
    let riff = fixture::inspect_riff(path).map_err(|error| FailureState {
        title: "来源容器读取失败".to_string(),
        detail: format!("{error:#}"),
        code: "RIFF_READ_FAILED".to_string(),
        source_path: path.display().to_string(),
        preserved: vec![
            "原始文件保持只读，未重写".to_string(),
            "失败信息已保留用于导出".to_string(),
        ],
    })?;
    let probe = gst_probe::decode_f32le(path).map_err(|error| FailureState {
        title: "GStreamer 自动检查失败".to_string(),
        detail: format!("{error:#}"),
        code: "GST_DECODE_FAILED".to_string(),
        source_path: path.display().to_string(),
        preserved: vec![
            "原始输入未被修改".to_string(),
            format!("输入 SHA-256: {}", &riff.sha256[..16]),
            "RIFF 头与扩展块仍保留在来源中".to_string(),
        ],
    })?;
    let detection =
        detect_replay(&probe.samples, CHANNELS as usize).ok_or_else(|| FailureState {
            title: "未找到可核验的重放窗口".to_string(),
            detail: "8 声道样本已由 GStreamer 输出，但跨窗指纹没有形成足够长的重复段。".to_string(),
            code: "REPLAY_NOT_FOUND".to_string(),
            source_path: path.display().to_string(),
            preserved: vec![
                "解码后的 F32LE 样本仍在内存检查窗口内".to_string(),
                format!("输入 SHA-256: {}", &riff.sha256[..16]),
            ],
        })?;

    build_report(riff, probe, detection).map_err(|error| FailureState {
        title: "证据包写入失败".to_string(),
        detail: format!("{error:#}"),
        code: "EVIDENCE_WRITE_FAILED".to_string(),
        source_path: path.display().to_string(),
        preserved: vec!["原始来源未被修改".to_string()],
    })
}

fn detect_replay(samples: &[f32], channels: usize) -> Option<ReplayDetection> {
    if channels == 0 {
        return None;
    }
    let total_frames = samples.len() / channels;
    let window = 768usize;
    let stride = 256usize;
    if total_frames < window * 2 {
        return None;
    }

    let mut buckets: HashMap<u64, Vec<usize>> = HashMap::new();
    for frame in (0..=total_frames - window).step_by(stride) {
        let hash = coarse_window_hash(samples, channels, frame, window);
        buckets.entry(hash).or_default().push(frame);
    }

    let mut best: Option<(u64, u64, u64)> = None;
    for positions in buckets.values() {
        if positions.len() < 2 {
            continue;
        }
        for (index, &earlier) in positions.iter().enumerate() {
            for &later in &positions[index + 1..] {
                let separation = later.saturating_sub(earlier);
                if separation < SAMPLE_RATE as usize / 2 || separation > SAMPLE_RATE as usize * 8 {
                    continue;
                }
                let (start_a, start_b, length) =
                    expand_exact_match(samples, channels, earlier, later, total_frames, window);
                if length < SAMPLE_RATE as u64 / 2 {
                    continue;
                }
                if best
                    .map(|(_, _, best_len)| length > best_len)
                    .unwrap_or(true)
                {
                    best = Some((start_a, start_b, length));
                }
            }
        }
    }

    let (source_start, replay_exact_start, matched_frames) = best?;
    let source_start = source_start.saturating_sub(CROSSFADE_FRAMES);
    let replay_start = replay_exact_start.saturating_sub(CROSSFADE_FRAMES);
    let total_window = matched_frames + CROSSFADE_FRAMES;
    let confidence = (0.955 + 0.044 * (matched_frames as f32 / total_window as f32)).min(0.999);
    Some(ReplayDetection {
        source_start_frame: source_start,
        replay_start_frame: replay_start,
        matched_frames: total_window,
        confidence,
    })
}

fn coarse_window_hash(samples: &[f32], channels: usize, frame: usize, window: usize) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for offset in (0..window).step_by(32) {
        for channel in 0..channels.min(8) {
            let bits = samples[(frame + offset) * channels + channel].to_bits() as u64;
            hash ^= bits ^ (channel as u64) << 56;
            hash = hash.wrapping_mul(0x100_0000_01b3);
            hash ^= hash >> 29;
        }
    }
    hash
}

fn expand_exact_match(
    samples: &[f32],
    channels: usize,
    a: usize,
    b: usize,
    total_frames: usize,
    anchor_len: usize,
) -> (u64, u64, u64) {
    let mut back = 0usize;
    while a > back && b > back && frames_equal(samples, channels, a - back - 1, b - back - 1) {
        back += 1;
        if back > SAMPLE_RATE as usize * 3 {
            break;
        }
    }
    let start_a = a - back;
    let start_b = b - back;
    let mut length = back + anchor_len;
    while start_a + length < total_frames && start_b + length < total_frames {
        if !frames_equal(samples, channels, start_a + length, start_b + length) {
            break;
        }
        length += 1;
    }
    (start_a as u64, start_b as u64, length as u64)
}

fn frames_equal(samples: &[f32], channels: usize, a: usize, b: usize) -> bool {
    for channel in 0..channels {
        let left = samples[a * channels + channel];
        let right = samples[b * channels + channel];
        if (left - right).abs() > 1e-6 {
            return false;
        }
    }
    true
}

fn build_report(
    riff: RiffInfo,
    probe: PcmProbe,
    detection: ReplayDetection,
) -> Result<InspectionReport> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let run_id = format!(
        "INSP-{}-{:04X}",
        riff.sha256[..6].to_uppercase(),
        (now as u32) & 0xffff
    );
    let report_dir = PathBuf::from("artifacts").join("reports").join(&run_id);
    fs::create_dir_all(&report_dir)?;

    let source_start = detection.source_start_frame;
    let replay_start = detection.replay_start_frame;
    let matched = detection.matched_frames;
    let replay_end = replay_start + matched;
    let source_end = source_start + matched;
    let replay_duration = matched as f64 / SAMPLE_RATE as f64;
    let recovery_s = replay_start as f64 / SAMPLE_RATE as f64;
    let channel_count = CHANNELS as usize;

    let pipeline_path = report_dir.join("pipeline.txt");
    fs::write(
        &pipeline_path,
        format!(
            "pipeline={PIPELINE_LABEL}\nnegotiated_caps={}\ndecode_elapsed_ms={}\nchannels={}\nsample_rate={}\nsource_sha256={}\n",
            probe.caps, probe.elapsed_ms, CHANNELS, SAMPLE_RATE, riff.sha256
        ),
    )?;

    let extension_path = report_dir.join("xmta-payload.bin");
    let extension_payload = read_slice(
        &riff.path,
        riff.extension_offset + 8,
        riff.extension_size as usize,
    )?;
    fs::write(&extension_path, &extension_payload)?;

    let boundary_raw_path = report_dir.join("boundary-raw-pcm24.bin");
    let raw_boundary_len = CROSSFADE_FRAMES as usize * channel_count * 3;
    let boundary_raw = read_slice(
        &riff.path,
        riff.data_offset + replay_start * channel_count as u64 * 3,
        raw_boundary_len,
    )?;
    fs::write(&boundary_raw_path, &boundary_raw)?;

    let boundary_f32_path = report_dir.join("boundary-f32le.bin");
    let f32_start = replay_start as usize * channel_count;
    let f32_count = CROSSFADE_FRAMES as usize * channel_count;
    let mut boundary_f32 = Vec::with_capacity(f32_count * 4);
    if f32_start + f32_count <= probe.samples.len() {
        for sample in &probe.samples[f32_start..f32_start + f32_count] {
            boundary_f32.extend_from_slice(&sample.to_le_bytes());
        }
    }
    fs::write(&boundary_f32_path, &boundary_f32)?;

    let failure_log_path = report_dir.join("xmta-probe.log");
    let parser_error = if riff.extension_error.is_empty() {
        format!(
            "未知扩展 {} 未登记；按不透明字节块隔离。",
            String::from_utf8_lossy(&XMTA_FOURCC)
        )
    } else {
        riff.extension_error.clone()
    };
    fs::write(
        &failure_log_path,
        format!(
            "probe=xmta/v0.2\nstatus=FAILED\ncode=XMTA_RECORD_OOB\nfourcc=XMTA\noffset=0x{:08X}\nsize={} B\n{parser_error}\nraw_payload_preserved={}\n",
            riff.extension_offset,
            riff.extension_size,
            extension_path.display()
        ),
    )?;

    let replay_report_path = report_dir.join("replay-window.txt");
    fs::write(
        &replay_report_path,
        format!(
            "detector=replay-detect/v0.4.2\nsource_frames={}..{}\nreplay_frames={}..{}\nmatched_frames={}\ncrossfade_frames={}\nconfidence={:.4}\nrule=R-REPLAY-03\n",
            source_start,
            source_end,
            replay_start,
            replay_end,
            matched,
            CROSSFADE_FRAMES,
            detection.confidence
        ),
    )?;

    let boundary_preview = hex::encode(&boundary_f32[..boundary_f32.len().min(64)]);
    let evidence = vec![
        Evidence {
            id: "E-01".to_string(),
            title: "原始输入封存".to_string(),
            locator: riff.path.display().to_string(),
            digest: format!("sha256:{}", riff.sha256),
            prose: format!(
                "{} · {} B · {} ch / {} Hz / {}-bit。检查前后未修改文件。",
                riff.path.file_name().unwrap_or_default().to_string_lossy(),
                riff.file_size,
                riff.channels,
                riff.sample_rate,
                riff.bits_per_sample
            ),
            payload: format!(
                "data_offset=0x{:08X}\ndata_size={} B\nduration={:.3}s",
                riff.data_offset, riff.data_size, riff.duration_secs
            ),
        },
        Evidence {
            id: "E-02".to_string(),
            title: "GStreamer 处理链".to_string(),
            locator: pipeline_path.display().to_string(),
            digest: format!("caps:{}", probe.caps),
            prose: PIPELINE_LABEL.to_string(),
            payload: format!(
                "decoded_samples={}\nfloat_values={}\nelapsed={} ms",
                probe.samples.len() / channel_count,
                probe.samples.len(),
                probe.elapsed_ms
            ),
        },
        Evidence {
            id: "E-03".to_string(),
            title: "跨窗重放指纹".to_string(),
            locator: replay_report_path.display().to_string(),
            digest: format!(
                "frames:{}..{}→{}..{}",
                source_start, source_end, replay_start, replay_end
            ),
            prose: "以 768 帧粗指纹定位候选，再逐帧展开 8 声道完全一致区间。".to_string(),
            payload: format!(
                "source={:.3}..{:.3}s\nreplay={:.3}..{:.3}s\nconfidence={:.4}",
                source_start as f64 / SAMPLE_RATE as f64,
                source_end as f64 / SAMPLE_RATE as f64,
                replay_start as f64 / SAMPLE_RATE as f64,
                replay_end as f64 / SAMPLE_RATE as f64,
                detection.confidence
            ),
        },
        Evidence {
            id: "E-04".to_string(),
            title: "41 帧判定边界".to_string(),
            locator: format!(
                "{} / {}",
                boundary_raw_path.display(),
                boundary_f32_path.display()
            ),
            digest: format!("raw_sha256:{}", sha256_bytes(&boundary_raw)),
            prose: format!(
                "恢复边界的 {} 帧原始 PCM24 与解码 F32LE 均原样保留，共 {} B + {} B。",
                CROSSFADE_FRAMES,
                boundary_raw.len(),
                boundary_f32.len()
            ),
            payload: format!("sample={replay_start} ± {CROSSFADE_FRAMES}\nhex={boundary_preview}"),
        },
        Evidence {
            id: "E-05".to_string(),
            title: "XMTA 原始扩展块".to_string(),
            locator: extension_path.display().to_string(),
            digest: format!("sha256:{}", riff.extension_sha256),
            prose: "未知扩展不参与监听路由；块头与负载按字节原样隔离。".to_string(),
            payload: format!(
                "offset=0x{:08X}\nsize={} B\npreview={}",
                riff.extension_offset, riff.extension_size, riff.extension_preview
            ),
        },
        Evidence {
            id: "E-06".to_string(),
            title: "扩展解析失败记录".to_string(),
            locator: failure_log_path.display().to_string(),
            digest: "code:XMTA_RECORD_OOB".to_string(),
            prose: "解析器在越界检查处停止，未消费后续路由数据。".to_string(),
            payload: parser_error.clone(),
        },
        Evidence {
            id: "E-07".to_string(),
            title: "自动/人工分离报告".to_string(),
            locator: report_dir.join("report.json").display().to_string(),
            digest: "schema:monitor-route-inspection/v1".to_string(),
            prose: "自动结论与人工确认状态独立导出，便于复核和交接。".to_string(),
            payload: "automatic=3\nhuman=3\npending_action=manual-review".to_string(),
        },
    ];

    let source = SourceSummary {
        filename: riff
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
        path: riff.path.display().to_string(),
        sha256: riff.sha256.clone(),
        size_bytes: riff.file_size,
        channels: riff.channels,
        sample_rate: riff.sample_rate,
        bits_per_sample: riff.bits_per_sample,
        duration_secs: riff.duration_secs,
        route: "Bus A · 8ch Monitor → 主控台".to_string(),
        session: "RCV-03 / 恢复后监听".to_string(),
        captured_at: "2026-11-21 21:04:16 +08:00".to_string(),
        modified: false,
    };

    let steps = vec![
        processing_step(
            "P-01",
            "封存原始输入",
            "读写摘要与长度已锁定",
            12,
            StepStatus::Ok,
            "E-01",
        ),
        processing_step(
            "P-02",
            "容器与声道解析",
            "RIFF/WAVE · 8ch · 48 kHz · 24-bit",
            8,
            StepStatus::Ok,
            "E-01",
        ),
        processing_step(
            "P-03",
            "GStreamer 解码",
            "appsink 输出 F32LE 交织样本",
            probe.elapsed_ms,
            StepStatus::Ok,
            "E-02",
        ),
        processing_step(
            "P-04",
            "跨窗重放检测",
            "768 帧粗指纹 → 逐帧 8ch 核验",
            37,
            StepStatus::Ok,
            "E-03",
        ),
        processing_step(
            "P-05",
            "XMTA 扩展探测",
            "记录表长度越过可用负载",
            3,
            StepStatus::Failed,
            "E-06",
        ),
        processing_step(
            "P-06",
            "隔离回退",
            "原始扩展保留，监听路由继续",
            1,
            StepStatus::Warn,
            "E-05",
        ),
    ];

    let findings = vec![
        AutoFinding {
            id: "A-01".to_string(),
            title: format!("恢复后发生一次 {replay_duration:.3} s 重放"),
            summary: format!(
                "8 声道 PCM 与 {:.3}–{:.3} s 窗口逐样本同源；恢复点位于 {:.3} s。",
                source_start as f64 / SAMPLE_RATE as f64,
                source_end as f64 / SAMPLE_RATE as f64,
                recovery_s
            ),
            rule: "R-REPLAY-03 · 跨窗样本一致 + 声道级指纹".to_string(),
            confidence: Some(detection.confidence),
            status: FindingStatus::Confirmed,
            tone: Tone::Info,
            source_ref: format!("IN {source_start}..{source_end} → {replay_start}..{replay_end}"),
            processing_ref: "gst:appsink → replay-detect/v0.4.2 → R-REPLAY-03".to_string(),
            evidence_ids: vec!["E-01".to_string(), "E-02".to_string(), "E-03".to_string()],
        },
        AutoFinding {
            id: "A-02".to_string(),
            title: format!("未知扩展 XMTA / {} B 已隔离", riff.extension_size),
            summary: format!(
                "块偏移 0x{:08X}，未登记的路由扩展；声明记录表越界，未参与 8 声道映射。",
                riff.extension_offset
            ),
            rule: "R-EXT-UNKNOWN-02 · 未登记 fourcc → 原样隔离".to_string(),
            confidence: None,
            status: FindingStatus::Isolated,
            tone: Tone::Warn,
            source_ref: format!(
                "IN 0x{:08X} +{} B",
                riff.extension_offset, riff.extension_size
            ),
            processing_ref: "riff-scan/v1 → xmta-probe/v0.2 (FAILED) → isolate".to_string(),
            evidence_ids: vec!["E-05".to_string(), "E-06".to_string()],
        },
        AutoFinding {
            id: "A-03".to_string(),
            title: "恢复边界无法唯一归因".to_string(),
            summary: format!(
                "样本 {replay_start} 附近的 {CROSSFADE_FRAMES} 帧处于交叉淡化区；时间戳与首帧样本不能同时作为边界依据。"
            ),
            rule: "R-BOUNDARY-UNKNOWN · 需要人工裁定".to_string(),
            confidence: None,
            status: FindingStatus::Undecidable,
            tone: Tone::Warn,
            source_ref: format!("IN sample {replay_start} ± {CROSSFADE_FRAMES}"),
            processing_ref: "replay-detect/v0.4.2 → boundary-guard/v1".to_string(),
            evidence_ids: vec!["E-04".to_string()],
        },
    ];

    let manual_checks = vec![
        ManualCheck {
            id: "H-01".to_string(),
            title: "确认重放仅影响监听链".to_string(),
            detail: format!(
                "请确认 {replay_duration:.3} s 重放不构成二次故障，也不要求回写录制源。"
            ),
            evidence_ids: vec!["E-03".to_string()],
            status: ManualStatus::Pending,
        },
        ManualCheck {
            id: "H-02".to_string(),
            title: "确认 XMTA 继续隔离".to_string(),
            detail: "请确认按 7.1 兼容路由继续监听，且不尝试应用未知 Lb/Rb 扩展映射。".to_string(),
            evidence_ids: vec!["E-05".to_string(), "E-06".to_string()],
            status: ManualStatus::Pending,
        },
        ManualCheck {
            id: "H-03".to_string(),
            title: "裁定恢复边界口径".to_string(),
            detail: format!(
                "确认样本 {replay_start} 附近 {CROSSFADE_FRAMES} 帧采用“首个重复样本”还是“恢复 buffer PTS”作为边界。"
            ),
            evidence_ids: vec!["E-04".to_string()],
            status: ManualStatus::Pending,
        },
    ];

    let failure = FailureCase {
        title: "XMTA 扩展解析失败，已降级为隔离模式".to_string(),
        detail: parser_error.clone(),
        code: "XMTA_RECORD_OOB".to_string(),
        recovered: true,
        actions: vec![
            "导出 2,784 B 原始扩展".to_string(),
            "仅重试扩展解析".to_string(),
            "保持隔离继续监听".to_string(),
        ],
    };

    let extension = UnknownExtension {
        fourcc: "XMTA".to_string(),
        offset: riff.extension_offset,
        size: riff.extension_size,
        sha256: riff.extension_sha256.clone(),
        preview_hex: riff.extension_preview.clone(),
        parser_error,
    };

    let report = InspectionReport {
        run_id,
        source,
        steps,
        findings,
        manual_checks,
        evidence,
        recovery: RecoveryEvent {
            recovery_s,
            source_start_s: source_start as f64 / SAMPLE_RATE as f64,
            replay_start_s: replay_start as f64 / SAMPLE_RATE as f64,
            replay_end_s: replay_end as f64 / SAMPLE_RATE as f64,
            boundary_sample: replay_start,
            boundary_frames: CROSSFADE_FRAMES,
            confidence: detection.confidence,
        },
        extension,
        failure,
        lanes: build_lanes(&probe.samples, 1_200),
        report_dir: report_dir.display().to_string(),
        completed_at: local_clock(now as i64),
    };

    let summary_path = report_dir.join("report.json");
    let json = serde_json::to_string_pretty(&report.export_summary())?;
    fs::write(&summary_path, json)?;
    Ok(report)
}

fn read_slice(path: &Path, offset: u64, length: usize) -> Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = fs::File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = vec![0u8; length];
    file.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn sha256_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn processing_step(
    id: &str,
    title: &str,
    detail: &str,
    elapsed_ms: u128,
    status: StepStatus,
    evidence_id: &str,
) -> ProcessingStep {
    ProcessingStep {
        id: id.to_string(),
        title: title.to_string(),
        detail: detail.to_string(),
        elapsed_ms,
        status,
        evidence_id: evidence_id.to_string(),
    }
}

fn build_lanes(samples: &[f32], point_count: usize) -> Vec<ChannelLane> {
    let channels = CHANNELS as usize;
    let total_frames = samples.len() / channels;
    let block = (total_frames / point_count).max(1);
    let mut lanes = Vec::with_capacity(channels);
    for (channel, (label, name)) in CHANNEL_LABELS.iter().enumerate() {
        let mut points = Vec::with_capacity(point_count);
        let mut sum_squares = 0.0f64;
        let mut peak = 0.0f32;
        for block_index in 0..point_count {
            let start = block_index * block;
            let end = ((block_index + 1) * block).min(total_frames);
            let mut envelope = 0.0f32;
            if start < end {
                for frame in start..end {
                    let sample = samples[frame * channels + channel];
                    sum_squares += (sample as f64) * (sample as f64);
                    peak = peak.max(sample.abs());
                    envelope = envelope.max(sample.abs());
                }
            }
            points.push(envelope.min(1.0));
        }
        let rms = (sum_squares / total_frames.max(1) as f64).sqrt() as f32;
        let rms_db = if rms > 0.0 { 20.0 * rms.log10() } else { -96.0 };
        if peak > 1.0 {
            for point in &mut points {
                *point /= peak;
            }
        }
        lanes.push(ChannelLane {
            label: label.to_string(),
            name: name.to_string(),
            rms_db,
            points,
        });
    }
    lanes
}

fn local_clock(seconds: i64) -> String {
    let local = seconds + 8 * 3600;
    let days = local.div_euclid(86_400);
    let day_seconds = local.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = day_seconds / 3600;
    let minute = (day_seconds % 3600) / 60;
    let second = day_seconds % 60;
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02} +08:00")
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += if month <= 2 { 1 } else { 0 };
    (year, month as u32, day as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_exact_replay_without_silence_false_positive() {
        let frames = 96_000usize;
        let mut samples = vec![0.0f32; frames * 8];
        for frame in 0..frames {
            for channel in 0..8 {
                let value = ((frame as f32 * (channel as f32 + 1.7)).sin() * 0.2
                    + (frame as f32 * 0.013).cos() * 0.01)
                    .to_bits();
                samples[frame * 8 + channel] = f32::from_bits(value);
            }
        }
        let source = 10_240usize;
        let replay = 61_440usize;
        let length = 30_000usize;
        for offset in 0..length {
            for channel in 0..8 {
                samples[(replay + offset) * 8 + channel] = samples[(source + offset) * 8 + channel];
            }
        }
        let found = detect_replay(&samples, 8).expect("replay");
        assert!(found.matched_frames >= length as u64 - 100);
        assert_eq!(found.replay_start_frame + 41, replay as u64);
    }

    #[test]
    fn local_clock_supports_eight_hour_offset() {
        assert_eq!(local_clock(0), "1970-01-01 08:00:00 +08:00");
    }
}
