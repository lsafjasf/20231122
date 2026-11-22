//! GStreamer 频谱分析：解码 -> 转换 -> spectrum 测量 -> 噪声判定。
//! 支持中途取消（保留部分证据），失败时给出可执行后续操作。

use crate::model::*;
use gstreamer as gst;
use gstreamer::prelude::*;
use sha2::Digest;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

pub const BANDS: u32 = 48;
pub const INTERVAL_NS: u64 = 100_000_000; // 100ms
pub const NOISE_HEADROOM_DB: f64 = 12.0; // 高出本底噪声多少 dB 判为噪声
pub const MARGIN_DB: f64 = 2.0; // 阈值附近的无法判断边界带

pub struct AnalysisOutcome {
    pub status: RunStatus,
    pub progress: f64,
    pub error: Option<String>,
    pub next_actions: Vec<String>,
    pub steps: Vec<ProvenanceStep>,
    pub conclusions: Vec<BandConclusion>,
}

pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let data = std::fs::read(path)?;
    Ok(format!("{:x}", sha2::Sha256::digest(&data)))
}

fn step(id: &str, kind: &str, desc: &str, params: &[(&str, String)], t0: u64) -> ProvenanceStep {
    ProvenanceStep {
        id: id.to_string(),
        kind: kind.to_string(),
        description: desc.to_string(),
        params: params.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(),
        started_ts: t0,
        ended_ts: now_ts(),
    }
}

/// 运行完整分析；`cancel` 置位时中途取消并保留已测得的部分频谱。
/// `shared_progress` 若提供，按千分比实时写入进度。
pub fn run_analysis(
    input: &Path,
    cancel: Arc<AtomicBool>,
    shared_progress: Option<Arc<AtomicU32>>,
) -> AnalysisOutcome {
    let mut steps = Vec::new();
    let t0 = now_ts();

    // S1 输入校验
    let meta = match std::fs::metadata(input) {
        Ok(m) => m,
        Err(e) => {
            return fail(
                steps,
                format!("无法读取输入文件 {}: {e}", input.display()),
                vec![
                    "确认文件路径拼写与访问权限".into(),
                    "在来源检查页重新选择音频文件".into(),
                ],
            );
        }
    };
    let digest = match sha256_file(input) {
        Ok(d) => d,
        Err(e) => {
            return fail(steps, format!("计算 SHA-256 失败: {e}"), vec!["检查磁盘与文件权限".into()])
        }
    };
    steps.push(step(
        "S1",
        "输入校验",
        "读取原始输入并计算 SHA-256 指纹",
        &[
            ("path", input.display().to_string()),
            ("sha256", digest.clone()),
            ("size_bytes", meta.len().to_string()),
        ],
        t0,
    ));

    // S2-S4 构建并运行管线
    let pipeline = match build_pipeline(input) {
        Ok(p) => p,
        Err(outcome) => return outcome_with_steps(outcome, steps),
    };
    steps.push(step(
        "S2",
        "解码",
        "filesrc ! decodebin 解出原始音频流",
        &[("pipeline", "filesrc ! decodebin".into())],
        t0,
    ));
    steps.push(step(
        "S3",
        "格式转换",
        "audioconvert ! audioresample 统一为可测量格式",
        &[("elements", "audioconvert, audioresample".into())],
        t0,
    ));

    if pipeline.set_state(gst::State::Playing).is_err() {
        return outcome_with_steps(
            fail(
                Vec::new(),
                "管线无法进入播放状态".into(),
                vec!["检查 GStreamer 插件是否完整 (GST_PLUGIN_PATH)".into()],
            ),
            steps,
        );
    }

    let bus = pipeline.bus().expect("pipeline bus");
    let mut duration = pipeline.query_duration::<gst::ClockTime>();
    let mut sums = vec![0.0f64; BANDS as usize];
    let mut msg_count: u64 = 0;
    let mut last_stream_ns: u64 = 0;
    let mut cancelled = false;
    let mut runtime_error: Option<String> = None;

    loop {
        if cancel.load(Ordering::SeqCst) {
            cancelled = true;
            break;
        }
        let msg = bus.timed_pop_filtered(
            gst::ClockTime::from_mseconds(100),
            &[gst::MessageType::Element, gst::MessageType::Eos, gst::MessageType::Error],
        );
        let Some(msg) = msg else { continue };
        if duration.is_none() {
            duration = pipeline.query_duration::<gst::ClockTime>();
        }
        match msg.view() {
            gst::MessageView::Element(e) => {
                let Some(s) = e.structure() else { continue };
                if s.name() != "spectrum" {
                    continue;
                }
                if let Ok(mags) = s.get::<gst::List>("magnitude") {
                    for (i, v) in mags.iter().enumerate().take(BANDS as usize) {
                        if let Ok(db) = v.get::<f32>() {
                            sums[i] += db as f64;
                        }
                    }
                    msg_count += 1;
                }
                if let Ok(st) = s.get::<u64>("stream-time") {
                    last_stream_ns = st;
                    if let (Some(sp), Some(d)) = (&shared_progress, duration) {
                        let ns = d.nseconds();
                        if ns > 0 {
                            sp.store(
                                ((st as f64 / ns as f64) * 1000.0).min(1000.0) as u32,
                                Ordering::SeqCst,
                            );
                        }
                    }
                }
            }
            gst::MessageView::Eos(_) => break,
            gst::MessageView::Error(err) => {
                runtime_error = Some(format!(
                    "{} ({})",
                    err.error(),
                    err.debug().unwrap_or_default()
                ));
                break;
            }
            _ => {}
        }
    }
    let _ = pipeline.set_state(gst::State::Null);

    let progress = match (duration, last_stream_ns) {
        (Some(d), st) if d.nseconds() > 0 => {
            (st as f64 / d.nseconds() as f64).min(1.0)
        }
        _ => 0.0,
    };

    steps.push(step(
        "S4",
        "频谱测量",
        "spectrum 元素按频段累计幅度",
        &[
            ("bands", BANDS.to_string()),
            ("interval_ns", INTERVAL_NS.to_string()),
            ("messages", msg_count.to_string()),
            ("processed_ns", last_stream_ns.to_string()),
        ],
        t0,
    ));

    if let Some(e) = runtime_error {
        return outcome_with_steps(
            fail(
                Vec::new(),
                format!("解码/测量出错: {e}"),
                vec![
                    "安装 gst-plugins-ugly 或 gst-libav 以支持该编码".into(),
                    "用 ffmpeg 转码为 WAV 后重试: ffmpeg -i 输入 输出.wav".into(),
                    "确认文件未损坏且包含音频流".into(),
                ],
            ),
            steps,
        );
    }

    // S5 判定（即使取消，也对已测得的部分数据给出部分结论）
    let conclusions = if msg_count > 0 {
        classify(&sums, msg_count, steps.last().map(|s| s.id.clone()).unwrap_or_default(), &mut steps, t0)
    } else {
        Vec::new()
    };

    if cancelled {
        AnalysisOutcome {
            status: RunStatus::Cancelled,
            progress,
            error: None,
            next_actions: vec![
                "点击“开始分析”重新运行完整分析".into(),
                "已保留取消前的部分频谱证据，可在结论区查看".into(),
                "若文件过大，可先裁剪片段再分析".into(),
            ],
            steps,
            conclusions,
        }
    } else {
        AnalysisOutcome {
            status: RunStatus::Completed,
            progress: 1.0,
            error: None,
            next_actions: Vec::new(),
            steps,
            conclusions,
        }
    }
}

fn outcome_with_steps(mut o: AnalysisOutcome, mut steps: Vec<ProvenanceStep>) -> AnalysisOutcome {
    steps.append(&mut o.steps);
    o.steps = steps;
    o
}

fn fail(steps: Vec<ProvenanceStep>, error: String, next_actions: Vec<String>) -> AnalysisOutcome {
    AnalysisOutcome {
        status: RunStatus::Failed,
        progress: 0.0,
        error: Some(error),
        next_actions,
        steps,
        conclusions: Vec::new(),
    }
}

fn build_pipeline(input: &Path) -> Result<gst::Pipeline, AnalysisOutcome> {
    gst::init().ok();
    let pipeline = gst::Pipeline::new();
    let make = |factory: &str, name: &str| -> Result<gst::Element, AnalysisOutcome> {
        gst::ElementFactory::make(factory).name(name).build().map_err(|_| {
            fail(
                Vec::new(),
                format!("缺少 GStreamer 元素: {factory}"),
                vec![
                    format!("安装包含 {factory} 的插件包（如 gst-plugins-good/base）"),
                    "检查 GST_PLUGIN_PATH 是否指向插件目录".into(),
                ],
            )
        })
    };
    let src = make("filesrc", "src")?;
    src.set_property("location", input.to_string_lossy().as_ref());
    let decode = make("decodebin", "decode")?;
    let convert = make("audioconvert", "convert")?;
    let resample = make("audioresample", "resample")?;
    let spectrum = make("spectrum", "spectrum")?;
    spectrum.set_property("bands", BANDS);
    spectrum.set_property("interval", INTERVAL_NS);
    spectrum.set_property("post-messages", true);
    spectrum.set_property("message-magnitude", true);
    let sink = make("fakesink", "sink")?;
    sink.set_property("sync", true); // 实时节奏，便于中途取消与进度观测

    pipeline
        .add_many([&src, &decode, &convert, &resample, &spectrum, &sink])
        .map_err(|e| fail(Vec::new(), format!("组装管线失败: {e}"), vec!["检查 GStreamer 安装".into()]))?;
    gst::Element::link_many([&src, &decode]).map_err(|e| fail(Vec::new(), e.to_string(), vec![]))?;
    gst::Element::link_many([&convert, &resample, &spectrum, &sink])
        .map_err(|e| fail(Vec::new(), e.to_string(), vec![]))?;

    let convert_sink = convert.static_pad("sink").expect("audioconvert sink pad");
    decode.connect_pad_added(move |_db, pad| {
        let caps = pad.current_caps().or_else(|| Some(pad.query_caps(None)));
        let Some(caps) = caps else { return };
        if caps.structure(0).map(|s| s.name().starts_with("audio/")).unwrap_or(false)
            && !convert_sink.is_linked()
        {
            let _ = pad.link(&convert_sink);
        }
    });
    Ok(pipeline)
}

/// 噪声判定：本底 = 各频段均值的中位数；高出本底 12dB 判噪声；
/// 阈值 ±2dB 边界带判“无法判断”，并保留关键证据。
fn classify(
    sums: &[f64],
    msg_count: u64,
    measure_step: String,
    steps: &mut Vec<ProvenanceStep>,
    t0: u64,
) -> Vec<BandConclusion> {
    let n = msg_count.max(1) as f64;
    let avgs: Vec<f64> = sums.iter().map(|s| s / n).collect();
    let mut sorted = avgs.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let floor = sorted[sorted.len() / 2];
    let threshold = floor + NOISE_HEADROOM_DB;

    steps.push(step(
        "S5",
        "噪声判定",
        "以中位数估计本底噪声，按阈值分类各频段",
        &[
            ("noise_floor_db", format!("{floor:.2}")),
            ("headroom_db", NOISE_HEADROOM_DB.to_string()),
            ("threshold_db", format!("{threshold:.2}")),
            ("margin_db", MARGIN_DB.to_string()),
        ],
        t0,
    ));
    let classify_step = steps.last().map(|s| s.id.clone()).unwrap_or_default();

    // spectrum 元素默认覆盖 0..=Nyquist；假设 44.1kHz 采样用于频率标注
    let nyquist = 22_050.0f64;
    avgs.iter()
        .enumerate()
        .map(|(i, avg)| {
            let freq_lo = i as f64 * nyquist / BANDS as f64;
            let freq_hi = (i + 1) as f64 * nyquist / BANDS as f64;
            let margin = threshold - avg;
            let verdict = if margin <= 0.0 {
                Verdict::Noise
            } else if margin <= MARGIN_DB {
                Verdict::Inconclusive
            } else {
                Verdict::Clean
            };
            let mut evidence = vec![
                ("平均幅度_dB".to_string(), format!("{avg:.2}")),
                ("本底噪声_dB".to_string(), format!("{floor:.2}")),
                ("判定阈值_dB".to_string(), format!("{threshold:.2}")),
                ("距阈值_dB".to_string(), format!("{margin:.2}")),
                ("测量消息数".to_string(), msg_count.to_string()),
            ];
            if verdict == Verdict::Inconclusive {
                evidence.push((
                    "边界说明".to_string(),
                    format!("幅度落在阈值 ±{MARGIN_DB}dB 边界带内，自动判定不可靠"),
                ));
            }
            BandConclusion {
                band_index: i,
                freq_lo_hz: freq_lo,
                freq_hi_hz: freq_hi,
                magnitude_db: *avg,
                noise_floor_db: floor,
                threshold_db: threshold,
                margin_db: margin,
                verdict,
                evidence,
                step_refs: vec![measure_step.clone(), classify_step.clone()],
            }
        })
        .collect()
}
