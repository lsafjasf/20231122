use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow, bail};
use chrono::Utc;
use gstreamer as gst;
use gstreamer::prelude::*;

use crate::model::{
    BoundarySamples, CorrectionRun, EvidenceSampleChange, ExtremeParams, PeakSample,
    SegmentAnalysis,
};

const ANALYSIS_RATE: u32 = 48_000;
const LEVEL_INTERVAL_NS: u64 = 20_000_000;

pub fn init() -> Result<()> {
    gst::init().context("GStreamer 初始化失败")?;
    Ok(())
}

#[derive(Debug, Clone)]
pub struct MediaInfo {
    pub duration_ms: u64,
    pub sample_rate: u32,
    pub channels: u32,
}

pub fn discover(path: &Path) -> Result<MediaInfo> {
    let discoverer = gstreamer_pbutils::Discoverer::new(gst::ClockTime::from_seconds(5))
        .context("无法创建媒体探测器")?;
    let uri = gst::glib::filename_to_uri(path, None)
        .with_context(|| format!("媒体路径无效: {}", path.display()))?;
    let info = discoverer
        .discover_uri(&uri)
        .with_context(|| format!("无法读取媒体: {}", path.display()))?;
    let duration_ms = info
        .duration()
        .map(|value| value.mseconds())
        .filter(|value| *value > 0)
        .ok_or_else(|| anyhow!("媒体时长不可用"))?;
    let audio = info
        .audio_streams()
        .first()
        .cloned()
        .ok_or_else(|| anyhow!("媒体中没有音频轨道"))?;
    Ok(MediaInfo {
        duration_ms,
        sample_rate: audio.sample_rate(),
        channels: audio.channels(),
    })
}

fn quoted(value: &Path) -> String {
    value
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}

fn bus_error(message: &gst::Message) -> Option<anyhow::Error> {
    if let gst::MessageView::Error(error) = message.view() {
        let source = error
            .src()
            .map(|source| source.name().to_string())
            .unwrap_or_else(|| "pipeline".into());
        Some(anyhow!(
            "{}: {} ({})",
            source,
            error.error(),
            error.debug().unwrap_or_default()
        ))
    } else {
        None
    }
}

fn wait_for_pipeline(pipeline: &gst::Pipeline, timeout_seconds: u64) -> Result<()> {
    let bus = pipeline.bus().ok_or_else(|| anyhow!("管线没有消息总线"))?;
    let deadline = gst::ClockTime::from_seconds(timeout_seconds);
    loop {
        let Some(message) = bus.timed_pop_filtered(
            deadline,
            &[
                gst::MessageType::Error,
                gst::MessageType::Eos,
                gst::MessageType::AsyncDone,
            ],
        ) else {
            bail!("媒体处理超时（{timeout_seconds}s）");
        };
        if let Some(error) = bus_error(&message) {
            return Err(error);
        }
        match message.view() {
            gst::MessageView::Eos(_) => return Ok(()),
            gst::MessageView::AsyncDone(_) => return Ok(()),
            _ => {}
        }
    }
}

pub fn capture_levels(path: &Path) -> Result<Vec<PeakSample>> {
    let description = format!(
        "filesrc location=\"{}\" ! decodebin ! audioconvert ! audioresample ! \
         audio/x-raw,format=F32LE,channels=1,rate={ANALYSIS_RATE} ! \
         level interval={LEVEL_INTERVAL_NS} post-messages=true ! fakesink sync=false",
        quoted(path)
    );
    let pipeline = gst::parse::launch(&description)
        .context("构建峰值分析管线失败")?
        .downcast::<gst::Pipeline>()
        .map_err(|_| anyhow!("峰值分析管线类型不正确"))?;
    let bus = pipeline
        .bus()
        .ok_or_else(|| anyhow!("分析管线没有消息总线"))?;
    pipeline
        .set_state(gst::State::Playing)
        .context("启动峰值分析失败")?;

    let mut samples = Vec::new();
    let mut fallback_index = 0u64;
    loop {
        let Some(message) = bus.timed_pop_filtered(
            gst::ClockTime::from_seconds(30),
            &[
                gst::MessageType::Error,
                gst::MessageType::Eos,
                gst::MessageType::Element,
            ],
        ) else {
            let _ = pipeline.set_state(gst::State::Null);
            bail!("峰值分析超时");
        };
        if let Some(error) = bus_error(&message) {
            let _ = pipeline.set_state(gst::State::Null);
            return Err(error);
        }
        match message.view() {
            gst::MessageView::Eos(_) => break,
            gst::MessageView::Element(_) => {
                let Some(structure) = message.structure() else {
                    continue;
                };
                if structure.name() != "level" {
                    continue;
                }
                let at_ms = structure
                    .get::<u64>("endtime")
                    .ok()
                    .map(|value| (value / 1_000_000) as i64)
                    .unwrap_or_else(|| (fallback_index * LEVEL_INTERVAL_NS / 1_000_000) as i64);
                fallback_index += 1;
                let peak = first_level_value(structure, "peak");
                let rms = first_level_value(structure, "rms");
                samples.push(PeakSample {
                    at_ms,
                    peak_dbfs: peak.max(-120.0),
                    rms_dbfs: rms.max(-120.0),
                });
            }
            _ => {}
        }
    }
    let _ = pipeline.set_state(gst::State::Null);
    if samples.is_empty() {
        bail!("GStreamer 未返回任何 level 样本");
    }
    Ok(samples)
}

fn first_level_value(structure: &gst::StructureRef, field: &str) -> f32 {
    let Ok(values) = structure.get::<gtk4::glib::ValueArray>(field) else {
        return -120.0;
    };
    values
        .iter()
        .next()
        .and_then(|value| value.get::<f64>().ok())
        .map(|value| value as f32)
        .unwrap_or(-120.0)
}

pub fn analyze_segment(
    samples: &[PeakSample],
    start_ms: u64,
    end_ms: u64,
    time_offset_ms: u64,
    params: &ExtremeParams,
) -> SegmentAnalysis {
    let start = start_ms as i64;
    let end = end_ms as i64;
    let window = params.boundary_window_ms.max(20) as i64;
    let boundary = |boundary_ms: i64, before: bool| {
        let within_window = |sample: &&PeakSample| {
            (sample.at_ms - boundary_ms).abs() <= window
                && if before {
                    sample.at_ms < boundary_ms
                } else {
                    sample.at_ms > boundary_ms
                }
        };
        let candidates = samples.iter().filter(within_window);
        let mut selected = if before {
            candidates.max_by_key(|sample| sample.at_ms)
        } else {
            candidates.min_by_key(|sample| sample.at_ms)
        };
        if selected.is_none() {
            selected = if before {
                samples
                    .iter()
                    .filter(|sample| sample.at_ms < boundary_ms)
                    .max_by_key(|sample| sample.at_ms)
            } else {
                samples
                    .iter()
                    .filter(|sample| sample.at_ms > boundary_ms)
                    .min_by_key(|sample| sample.at_ms)
            };
        }
        selected.map(|sample| {
            let mut copy = sample.clone();
            copy.at_ms += time_offset_ms as i64;
            copy
        })
    };

    let in_range: Vec<&PeakSample> = samples
        .iter()
        .filter(|sample| sample.at_ms > start && sample.at_ms <= end)
        .collect();
    let peak_dbfs = in_range
        .iter()
        .map(|sample| sample.peak_dbfs)
        .fold(-120.0f32, f32::max);
    let rms_dbfs = if in_range.is_empty() {
        -120.0
    } else {
        let power = in_range
            .iter()
            .map(|sample| 10.0f32.powf(sample.rms_dbfs / 10.0))
            .sum::<f32>()
            / in_range.len() as f32;
        10.0 * power.max(1e-12).log10()
    };
    let warning_count = in_range
        .iter()
        .filter(|sample| sample.peak_dbfs > params.threshold_dbfs)
        .count();

    SegmentAnalysis {
        peak_dbfs,
        rms_dbfs,
        warning_count,
        sample_count: in_range.len(),
        boundaries: BoundarySamples {
            head_before: boundary(start, true),
            head_after: boundary(start, false),
            tail_before: boundary(end, true),
            tail_after: boundary(end, false),
        },
    }
}

struct LimiterState {
    gain: f32,
    attack: f32,
    release: f32,
}

impl LimiterState {
    fn new(params: &ExtremeParams) -> Self {
        let attack_ms = params.attack_ms.max(0.05);
        let release_ms = params.release_ms.max(1.0);
        let sample_rate = ANALYSIS_RATE as f32 * 0.001;
        Self {
            gain: 1.0,
            attack: 1.0 - (-1.0 / (attack_ms * sample_rate)).exp(),
            release: 1.0 - (-1.0 / (release_ms * sample_rate)).exp(),
        }
    }

    fn process(&mut self, sample: f32, ceiling: f32) -> (f32, bool) {
        let magnitude = sample.abs();
        let target = if magnitude > ceiling && magnitude > 0.0 {
            (ceiling / magnitude).min(1.0)
        } else {
            1.0
        };
        let coefficient = if target < self.gain {
            self.attack
        } else {
            self.release
        };
        self.gain += (target - self.gain) * coefficient;
        let corrected = sample * self.gain;
        if corrected.abs() > ceiling {
            (corrected.signum() * ceiling, true)
        } else {
            (corrected, false)
        }
    }
}

fn wait_for_eos(pipeline: &gst::Pipeline, timeout_seconds: u64) -> Result<()> {
    let bus = pipeline
        .bus()
        .ok_or_else(|| anyhow!("渲染管线没有消息总线"))?;
    loop {
        let Some(message) = bus.timed_pop_filtered(
            gst::ClockTime::from_seconds(timeout_seconds),
            &[gst::MessageType::Error, gst::MessageType::Eos],
        ) else {
            bail!("局部渲染超时（{timeout_seconds}s）");
        };
        if let Some(error) = bus_error(&message) {
            return Err(error);
        }
        if matches!(message.view(), gst::MessageView::Eos(_)) {
            return Ok(());
        }
    }
}

fn render_segment(
    source: &Path,
    output: &Path,
    start_ms: u64,
    end_ms: u64,
    params: &ExtremeParams,
) -> Result<u64> {
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _ = std::fs::remove_file(output);
    let linear_gain = 10.0f32.powf(params.makeup_db / 20.0);
    let ceiling = 10.0f32.powf(params.ceiling_dbfs / 20.0);
    let description = format!(
        "filesrc location=\"{}\" ! decodebin ! audioconvert ! audioresample ! \
         audio/x-raw,format=F32LE,channels=1,rate={ANALYSIS_RATE} ! \
         volume volume={linear_gain} ! identity name=peakguard_limit silent=true ! \
         audioconvert ! wavenc ! filesink location=\"{}\"",
        quoted(source),
        quoted(output)
    );
    let pipeline = gst::parse::launch(&description)
        .context("构建局部修正管线失败")?
        .downcast::<gst::Pipeline>()
        .map_err(|_| anyhow!("修正管线类型不正确"))?;
    let identity = pipeline
        .by_name("peakguard_limit")
        .ok_or_else(|| anyhow!("找不到限幅节点"))?;
    let clipped = Arc::new(AtomicU64::new(0));
    let clipped_for_probe = clipped.clone();
    let limiter = Arc::new(Mutex::new(LimiterState::new(params)));
    let limiter_for_probe = limiter.clone();
    let src_pad = identity
        .static_pad("src")
        .ok_or_else(|| anyhow!("限幅节点没有输出 pad"))?;
    src_pad.add_probe(gst::PadProbeType::BUFFER, move |_, info| {
        let Some(buffer) = info.buffer_mut() else {
            return gst::PadProbeReturn::Ok;
        };
        let Ok(mut map) = buffer.make_mut().map_writable() else {
            return gst::PadProbeReturn::Ok;
        };
        let mut limiter = limiter_for_probe.lock().unwrap();
        let mut clipped_here = 0u64;
        for chunk in map.as_mut_slice().as_chunks_mut::<4>().0 {
            let sample = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            let (corrected, was_clipped) = limiter.process(sample, ceiling);
            if was_clipped {
                clipped_here += 1;
            }
            chunk.copy_from_slice(&corrected.to_le_bytes());
        }
        clipped_for_probe.fetch_add(clipped_here, Ordering::Relaxed);
        gst::PadProbeReturn::Ok
    });

    pipeline
        .set_state(gst::State::Paused)
        .context("预滚局部修正管线失败")?;
    wait_for_pipeline(&pipeline, 30)?;
    let start = gst::ClockTime::from_mseconds(start_ms);
    let stop = gst::ClockTime::from_mseconds(end_ms);
    if let Err(error) = pipeline.seek(
        1.0,
        gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
        gst::SeekType::Set,
        start,
        gst::SeekType::Set,
        stop,
    ) {
        let _ = pipeline.set_state(gst::State::Null);
        bail!("无法定位到片段起点 {} ms: {error}", start_ms);
    }
    pipeline
        .set_state(gst::State::Playing)
        .context("启动局部渲染失败")?;
    let result = wait_for_eos(&pipeline, 60);
    let _ = pipeline.set_state(gst::State::Null);
    result?;
    if !output.exists() {
        bail!("修正管线结束但未生成输出文件");
    }
    Ok(clipped.load(Ordering::Relaxed))
}

pub fn run_segment(
    segment_id: &str,
    segment_index: usize,
    source: &Path,
    output: &Path,
    start_ms: u64,
    end_ms: u64,
    params: &ExtremeParams,
) -> Result<CorrectionRun> {
    let original_levels = capture_levels(source)
        .with_context(|| format!("分析片段 {segment_index:02} 的原始音频失败"))?;
    let original = analyze_segment(&original_levels, start_ms, end_ms, 0, params);
    let handle_ms = params.boundary_window_ms.max(20) as u64;
    let source_end_ms = original_levels
        .last()
        .map(|sample| sample.at_ms.max(end_ms as i64) as u64)
        .unwrap_or(end_ms);
    let render_start_ms = start_ms.saturating_sub(handle_ms);
    let render_end_ms = end_ms.saturating_add(handle_ms).min(source_end_ms);
    let clipped_samples = render_segment(source, output, render_start_ms, render_end_ms, params)
        .with_context(|| format!("修正片段 {segment_index:02} 失败"))?;
    let corrected_levels = capture_levels(output)
        .with_context(|| format!("分析片段 {segment_index:02} 的修正音频失败"))?;
    let mut corrected_eval = params.clone();
    corrected_eval.threshold_dbfs = params.ceiling_dbfs + 0.05;
    let local_start = start_ms.saturating_sub(render_start_ms);
    let local_end = end_ms.saturating_sub(render_start_ms);
    let corrected = analyze_segment(
        &corrected_levels,
        local_start,
        local_end,
        render_start_ms,
        &corrected_eval,
    );

    let original_boundaries = original.boundaries.ordered();
    let corrected_boundaries = corrected.boundaries.ordered();
    let boundary_changes = original_boundaries
        .iter()
        .zip(corrected_boundaries.iter())
        .map(|((label, before), (_, after))| {
            let delta_db = match (before, after) {
                (Some(before), Some(after)) => Some(after.peak_dbfs - before.peak_dbfs),
                _ => None,
            };
            EvidenceSampleChange {
                boundary: (*label).to_string(),
                before: before.cloned(),
                after: after.cloned(),
                delta_db,
            }
        })
        .collect();
    let now = Utc::now();
    let headroom_recovered_db = if corrected.peak_dbfs < -90.0 {
        0.0
    } else {
        params.ceiling_dbfs - corrected.peak_dbfs
    };
    Ok(CorrectionRun {
        id: format!("{segment_id}-{}", now.timestamp_millis()),
        created_at: now,
        params: params.clone(),
        output_path: output.to_string_lossy().into_owned(),
        duration_ms: end_ms - start_ms,
        original,
        corrected,
        clipped_samples,
        headroom_recovered_db,
        boundary_changes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::ensure_demo_media;

    #[test]
    fn gstreamer_captures_peaks_and_renders_limited_clip() {
        init().unwrap();
        let root = std::env::temp_dir().join(format!("peakguard-test-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("demo.wav");
        let output = root.join("segment.wav");
        ensure_demo_media(&source).unwrap();

        let levels = capture_levels(&source).unwrap();
        assert!(
            levels.len() > 400,
            "expected level samples, got {}",
            levels.len()
        );
        let params = ExtremeParams::default();
        let analysis = analyze_segment(&levels, 4_000, 8_000, 0, &params);
        assert!(analysis.peak_dbfs > -3.0);
        assert!(analysis.warning_count > 0);
        assert!(analysis.boundaries.head_before.is_some());
        assert!(analysis.boundaries.tail_after.is_some());

        let run = run_segment("seg-02", 2, &source, &output, 4_000, 8_000, &params).unwrap();
        assert!(output.exists());
        assert!(run.corrected.peak_dbfs <= params.ceiling_dbfs + 0.2);
        assert!(run.corrected.boundaries.head_before.is_some());
        assert!(run.corrected.boundaries.head_after.is_some());
        assert!(run.corrected.boundaries.tail_before.is_some());
        assert!(run.corrected.boundaries.tail_after.is_some());
        println!(
            "peak {:.2} -> {:.2}, clipped {}, warnings {} -> {}",
            run.original.peak_dbfs,
            run.corrected.peak_dbfs,
            run.clipped_samples,
            run.original.warning_count,
            run.corrected.warning_count
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
