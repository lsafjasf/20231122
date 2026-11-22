//! 基于 GStreamer 的静音检测管线。
//!
//! 管线结构：
//! `filesrc ! decodebin ! audioconvert ! audioresample ! capsfilter(F32LE/单采样率)
//!  ! tee ! queue ! level(message) ! fakesink`
//!
//! `level` 元素按固定间隔上报每声道 RMS/峰值（dB），据此做带迟滞的静音状态机判定；
//! 同时在 capsfilter 的源 pad 上挂探针收集原始 PCM，聚合为波形证据。

use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::gst::prelude::*;
use crate::gst::{glib, message, ClockTime, PadProbeReturn, PadProbeType};

use crate::gst_audio::{AudioBuffer, AudioFormat, AudioInfo};

use crate::model::{SilenceParams, SilenceSegment, Waveform};

/// level 元素每个时间窗上报的观测值。
#[derive(Debug, Clone, Copy)]
struct LevelSample {
    start_ns: u64,
    duration_ns: u64,
    /// 各声道 RMS（dB）；对多声道取最差（最大）值代表该窗整体能量。
    rms_db: f64,
    /// 各声道峰值（dB），用于静音段证据补充。
    peak_db: f64,
}

/// 单个音频文件的分析产物。
#[derive(Debug, Clone)]
pub struct Analysis {
    pub segments: Vec<SilenceSegment>,
    pub waveform: Waveform,
}

/// 运行一次 GStreamer 分析。
///
/// `truncated` 表示快照记录被截断：此时末尾仍在持续的静音段不闭合，
/// 其结束时间/结束帧标记为 `None`。
pub fn analyze(path: &Path, params: &SilenceParams, truncated: bool) -> anyhow::Result<Analysis> {
    crate::gst::init()?;

    let pipeline = crate::gst::Pipeline::new();
    let filesrc = crate::gst::ElementFactory::make("filesrc")
        .property("location", path.to_str().unwrap_or_default())
        .build()?;
    let decodebin = crate::gst::ElementFactory::make("decodebin").build()?;
    let convert = crate::gst::ElementFactory::make("audioconvert").build()?;
    let resample = crate::gst::ElementFactory::make("audioresample").build()?;
    let capsfilter = crate::gst::ElementFactory::make("capsfilter")
        .property(
            "caps",
            crate::gst::Caps::builder("audio/x-raw")
                .field("format", AudioFormat::F32le.to_str())
                .field("rate", params.resample_rate as i32)
                .field("channels", 1i32)
                .field("layout", "interleaved")
                .build(),
        )
        .build()?;
    let tee = crate::gst::ElementFactory::make("tee").build()?;
    let queue = crate::gst::ElementFactory::make("queue").build()?;
    let level = crate::gst::ElementFactory::make("level")
        .property("interval", ClockTime::from_mseconds(params.interval_millis))
        .property("message", true)
        .build()?;
    let fakesink = crate::gst::ElementFactory::make("fakesink").build()?;

    pipeline.add_many([
        &filesrc,
        &decodebin,
        &convert,
        &resample,
        &capsfilter,
        &tee,
        &queue,
        &level,
        &fakesink,
    ])?;
    crate::gst::Element::link_many([&filesrc, &decodebin])?;
    crate::gst::Element::link_many([
        &convert,
        &resample,
        &capsfilter,
        &tee,
        &queue,
        &level,
        &fakesink,
    ])?;

    // decodebin 采用动态 pad，就绪后接到 audioconvert。
    // 回调可能在流线程触发，用 WeakRef 在线程内 upgrade，避免跨线程解引用 SendWeakRef。
    let convert_weak: Arc<Mutex<glib::WeakRef<crate::gst::Element>>> =
        Arc::new(Mutex::new(convert.downgrade()));
    decodebin.connect_pad_added(move |_src, pad| {
        let weak = convert_weak.lock().unwrap().clone();
        if let Some(convert) = weak.upgrade() {
            if let Some(sink) = convert.static_pad("sink") {
                if pad.can_link(&sink) {
                    let _ = pad.link(&sink);
                }
            }
        }
    });

    // 原始 PCM 收集：在 capsfilter 源 pad 上挂 buffer 探针。
    let pcm: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
    let pcm_probe = Arc::clone(&pcm);
    let probe_pad = capsfilter.static_pad("src").expect("capsfilter src pad");
    probe_pad.add_probe(PadProbeType::BUFFER, move |pad, info| {
        if let Some(buffer) = info.buffer() {
            if let Some(caps) = pad.current_caps() {
                if let Ok(audio_info) = AudioInfo::from_caps(&caps) {
                    if let Ok(audio) =
                        AudioBuffer::from_buffer_readable(buffer.clone(), &audio_info)
                    {
                        if let Ok(plane) = audio.plane_data(0) {
                            let samples: &[f32] = bytemuck_cast(plane);
                            pcm_probe.lock().unwrap().extend_from_slice(samples);
                        }
                    }
                }
            }
        }
        PadProbeReturn::Ok
    });

    // level 消息收集。
    let levels: Arc<Mutex<Vec<LevelSample>>> = Arc::new(Mutex::new(Vec::new()));
    let levels_bus = Arc::clone(&levels);
    let bus = pipeline.bus().expect("pipeline bus");
    let mut eos = false;
    pipeline.set_state(crate::gst::State::Playing)?;

    while !eos {
        let msg = bus.timed_pop_filtered(
            ClockTime::from_seconds(10),
            &[
                crate::gst::MessageType::Error,
                crate::gst::MessageType::Eos,
                crate::gst::MessageType::Element,
            ],
        );
        let Some(msg) = msg else { continue };
        match msg.view() {
            message::MessageView::Error(e) => {
                let err = e.error();
                pipeline.set_state(crate::gst::State::Null)?;
                anyhow::bail!("GStreamer 错误: {err}");
            }
            message::MessageView::Eos(_) => eos = true,
            message::MessageView::Element(el) => {
                if let Some(structure) = el.structure() {
                    if structure.name().as_str() == "level" {
                        if let Some(sample) = parse_level(structure) {
                            levels_bus.lock().unwrap().push(sample);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    pipeline.set_state(crate::gst::State::Null)?;

    let levels = levels.lock().unwrap().clone();
    let pcm = pcm.lock().unwrap();
    anyhow::ensure!(
        !levels.is_empty(),
        "未采集到 level 观测值，文件可能不含音频流"
    );

    let duration_ns = levels
        .last()
        .map(|l| l.start_ns + l.duration_ns)
        .unwrap_or(0);
    let duration_secs = duration_ns as f64 / 1_000_000_000.0;
    let segments = detect_segments(&levels, params, params.resample_rate, truncated);
    let waveform = build_waveform(&pcm, params, duration_secs)?;

    Ok(Analysis { segments, waveform })
}

/// 把字节片按 f32 重新解释（F32LE 在小端主机上即原生 f32）。
fn bytemuck_cast(bytes: &[u8]) -> &[f32] {
    let count = bytes.len() / 4;
    unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const f32, count) }
}

/// 从 level 消息 structure 提取时间窗与多声道能量。
fn parse_level(structure: &crate::gst::StructureRef) -> Option<LevelSample> {
    let start: u64 = structure.get::<ClockTime>("stream-time").ok()?.nseconds();
    let duration: u64 = structure.get::<ClockTime>("duration").ok()?.nseconds();
    let rms: glib::ValueArray = structure.get("rms").ok()?;
    let peak: glib::ValueArray = structure.get("peak").ok()?;

    let rms_db = rms
        .as_slice()
        .iter()
        .filter_map(|v| v.get::<f64>().ok())
        .filter(|v| v.is_finite())
        .fold(SILENCE_FLOOR_DB, f64::max);
    let peak_db = peak
        .as_slice()
        .iter()
        .filter_map(|v| v.get::<f64>().ok())
        .filter(|v| v.is_finite())
        .fold(SILENCE_FLOOR_DB, f64::max);

    Some(LevelSample {
        start_ns: start,
        duration_ns: duration,
        rms_db,
        peak_db,
    })
}

/// 纯数字静音时 level 上报 -inf，统一归一到该有限地板值，保证报告可序列化、可复核。
const SILENCE_FLOOR_DB: f64 = -120.0;

/// 带迟滞的静音状态机：
/// 连续 `on_millis` 低于阈值进入静音；连续 `off_millis` 高于阈值退出静音。
fn detect_segments(
    levels: &[LevelSample],
    params: &SilenceParams,
    sample_rate: u32,
    truncated: bool,
) -> Vec<SilenceSegment> {
    let mut segments = Vec::new();
    let mut index = 0usize;
    let mut in_silence = false;
    let mut seg_start_ns: u64 = 0;
    let mut seg_min_rms = f64::INFINITY;
    let mut seg_max_peak = f64::NEG_INFINITY;
    let mut quiet_run_ns: u64 = 0;
    let mut loud_run_ns: u64 = 0;

    for sample in levels {
        let is_quiet = sample.rms_db <= params.threshold_db;
        if is_quiet {
            quiet_run_ns += sample.duration_ns;
            loud_run_ns = 0;
        } else {
            loud_run_ns += sample.duration_ns;
            quiet_run_ns = 0;
        }

        if !in_silence && quiet_run_ns >= ms(params.on_millis) {
            in_silence = true;
            seg_start_ns = sample.start_ns + sample.duration_ns.saturating_sub(quiet_run_ns);
            seg_min_rms = sample.rms_db;
            seg_max_peak = sample.peak_db;
        } else if in_silence {
            seg_min_rms = seg_min_rms.min(sample.rms_db);
            seg_max_peak = seg_max_peak.max(sample.peak_db);
            if loud_run_ns >= ms(params.off_millis) {
                let end_ns = sample.start_ns + sample.duration_ns.saturating_sub(loud_run_ns);
                segments.push(make_segment(
                    index,
                    seg_start_ns,
                    Some(end_ns),
                    seg_min_rms,
                    seg_max_peak,
                    sample_rate,
                    false,
                ));
                index += 1;
                in_silence = false;
                seg_min_rms = f64::INFINITY;
                seg_max_peak = f64::NEG_INFINITY;
            }
        }
    }

    // 流结束时仍处于静音：完整记录按 EOS 闭合，截断记录保持开放。
    if in_silence {
        let last = levels.last().unwrap();
        let end_ns = last.start_ns + last.duration_ns;
        segments.push(make_segment(
            index,
            seg_start_ns,
            if truncated { None } else { Some(end_ns) },
            seg_min_rms,
            seg_max_peak,
            sample_rate,
            truncated,
        ));
    }
    segments
}

fn make_segment(
    index: usize,
    start_ns: u64,
    end_ns: Option<u64>,
    min_rms_db: f64,
    max_peak_db: f64,
    sample_rate: u32,
    truncated: bool,
) -> SilenceSegment {
    let start_secs = start_ns as f64 / 1e9;
    let start_frame = (start_ns as f64 / 1e9 * sample_rate as f64).round() as u64;
    let (end_secs, end_frame, duration_secs) = match end_ns {
        Some(end) => {
            let secs = end as f64 / 1e9;
            let frame = (end as f64 / 1e9 * sample_rate as f64).round() as u64;
            (Some(secs), Some(frame), secs - start_secs)
        }
        None => (None, None, 0.0),
    };
    SilenceSegment {
        index,
        track: "track-0".to_string(),
        start_secs,
        end_secs,
        start_frame,
        end_frame,
        duration_secs,
        min_rms_db,
        max_peak_db,
        truncated,
    }
}

fn ms(value: u64) -> u64 {
    value * 1_000_000
}

/// 把原始 PCM 聚合为固定桶数的峰值/RMS 波形。
fn build_waveform(
    pcm: &[f32],
    params: &SilenceParams,
    duration_secs: f64,
) -> anyhow::Result<Waveform> {
    anyhow::ensure!(!pcm.is_empty(), "未采集到 PCM 样本");
    let buckets = params.waveform_buckets.clamp(1, pcm.len());
    let per = pcm.len().div_ceil(buckets);
    let mut peaks = Vec::with_capacity(buckets);
    let mut rms = Vec::with_capacity(buckets);
    let mut bucket_start_secs = Vec::with_capacity(buckets);

    for (idx, chunk) in pcm.chunks(per).enumerate() {
        let mut peak = 0.0f32;
        let mut sum_sq = 0.0f64;
        for &sample in chunk {
            let abs = sample.abs();
            peak = peak.max(abs);
            sum_sq += (sample as f64) * (sample as f64);
        }
        peaks.push(peak);
        rms.push((sum_sq / chunk.len() as f64).sqrt() as f32);
        bucket_start_secs.push(idx as f64 * per as f64 / params.resample_rate as f64);
    }

    Ok(Waveform {
        bucket_start_secs,
        peaks,
        rms,
        duration_secs,
        sample_rate: params.resample_rate,
        channels: 1,
    })
}
