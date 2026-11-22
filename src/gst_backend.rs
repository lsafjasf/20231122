use crate::engine::AnalysisOutcome;
use crate::model::{LoudnessStats, db_from_power, round3};
use anyhow::{Context, Result, anyhow, bail, ensure};
use gst::prelude::*;
use gstreamer as gst;
use gstreamer_app::AppSink;
use std::path::Path;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct LevelAccumulator {
    sum_squares: f64,
    peak: f64,
    samples: u64,
}

impl LevelAccumulator {
    fn push_f32(&mut self, bytes: &[u8]) -> Result<()> {
        ensure!(
            bytes.len().is_multiple_of(4),
            "GStreamer F32 缓冲区未按样本对齐"
        );
        for chunk in bytes.as_chunks::<4>().0 {
            let value = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            let sample = value as f64;
            self.sum_squares += sample * sample;
            self.peak = self.peak.max(sample.abs());
            self.samples += 1;
        }
        Ok(())
    }
}

fn quote(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}

fn pipeline_from_description(description: &str) -> Result<gst::Pipeline> {
    gst::parse::launch(description)
        .context("构建 GStreamer 管线失败")?
        .downcast::<gst::Pipeline>()
        .map_err(|_| anyhow!("GStreamer 描述未产生 Pipeline"))
}

fn stream_error(message: &gst::Message) -> Option<anyhow::Error> {
    let gst::MessageView::Error(error) = message.view() else {
        return None;
    };
    let source = error
        .src()
        .map(|src| src.name().to_string())
        .unwrap_or_else(|| "pipeline".to_string());
    Some(anyhow!(
        "{source}: {} ({})",
        error.error(),
        error.debug().unwrap_or_default()
    ))
}

fn wait_for_completion(pipeline: &gst::Pipeline, timeout_seconds: u64) -> Result<()> {
    let bus = pipeline.bus().context("GStreamer 管线没有 bus")?;
    loop {
        let message = bus
            .timed_pop_filtered(
                gst::ClockTime::from_seconds(timeout_seconds),
                &[gst::MessageType::Eos, gst::MessageType::Error],
            )
            .context("GStreamer 管线处理超时")?;
        if let Some(error) = stream_error(&message) {
            bail!("{error:#}");
        }
        if matches!(message.view(), gst::MessageView::Eos(_)) {
            return Ok(());
        }
    }
}

fn seek_window(pipeline: &gst::Pipeline, start_ns: u64, stop_ns: u64) -> Result<()> {
    pipeline
        .set_state(gst::State::Paused)
        .context("GStreamer 无法进入 PAUSED")?;
    let (state_result, _, _) = pipeline.state(gst::ClockTime::from_seconds(10));
    state_result.context("GStreamer 预滚超时")?;
    pipeline
        .seek(
            1.0,
            gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
            gst::SeekType::Set,
            gst::ClockTime::from_nseconds(start_ns),
            gst::SeekType::Set,
            gst::ClockTime::from_nseconds(stop_ns),
        )
        .map_err(|error| anyhow!("锚点窗口 seek 失败: {error:?}"))?;
    pipeline
        .set_state(gst::State::Playing)
        .context("GStreamer 无法进入 PLAYING")?;
    Ok(())
}

pub fn analyze_window(
    input: &Path,
    start_ns: u64,
    stop_ns: u64,
    sample_rate: u32,
    channels: u16,
) -> Result<AnalysisOutcome> {
    let description = format!(
        "filesrc location=\"{}\" ! wavparse ! audioconvert ! audioresample ! \
         audio/x-raw,format=F32LE,channels={channels},rate={sample_rate},layout=interleaved ! \
         appsink name=levelcapture sync=false emit-signals=false max-buffers=16",
        quote(input)
    );
    let pipeline = pipeline_from_description(&description)?;
    let appsink = pipeline
        .by_name("levelcapture")
        .context("找不到 appsink")?
        .downcast::<AppSink>()
        .map_err(|_| anyhow!("levelcapture 不是 AppSink"))?;
    let accumulator = Arc::new(Mutex::new(LevelAccumulator::default()));
    let capture = accumulator.clone();
    let capture_error = Arc::new(Mutex::new(None::<String>));
    let capture_error_cb = capture_error.clone();
    appsink.set_callbacks(
        gstreamer_app::AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                let buffer = sample.buffer().ok_or(gst::FlowError::Error)?;
                let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;
                let Ok(mut level) = capture.lock() else {
                    return Err(gst::FlowError::Error);
                };
                if let Err(error) = level.push_f32(map.as_slice()) {
                    if let Ok(mut slot) = capture_error_cb.lock() {
                        *slot = Some(error.to_string());
                    }
                    return Err(gst::FlowError::Error);
                }
                Ok(gst::FlowSuccess::Ok)
            })
            .build(),
    );

    seek_window(&pipeline, start_ns, stop_ns)?;
    let result = wait_for_completion(&pipeline, 30);
    let _ = pipeline.set_state(gst::State::Null);
    result?;
    if let Some(error) = capture_error.lock().ok().and_then(|slot| slot.clone()) {
        bail!("响度统计失败: {error}");
    }
    let level = accumulator.lock().map_err(|_| anyhow!("响度统计锁损坏"))?;
    ensure!(level.samples > 0, "GStreamer 未返回任何音频样本");
    let rms_dbfs = db_from_power(level.sum_squares / level.samples as f64);
    let peak_dbfs = if level.peak <= 1.0e-12 {
        -120.0
    } else {
        20.0 * level.peak.log10()
    };
    Ok(AnalysisOutcome {
        stats: LoudnessStats {
            rms_dbfs: round3(rms_dbfs),
            peak_dbfs: round3(peak_dbfs),
            sample_count: level.samples,
            sample_rate,
            channels,
            measurement: format!(
                "GStreamer appsink F32LE interleaved, {sample_rate} Hz, {channels} ch"
            ),
        },
        pipeline: description,
    })
}

pub fn render_window(
    input: &Path,
    output: &Path,
    start_ns: u64,
    stop_ns: u64,
    gain_linear: f64,
    sample_rate: u32,
    channels: u16,
) -> Result<String> {
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let description = format!(
        "filesrc location=\"{}\" ! wavparse ! audioconvert ! volume volume={gain_linear:.9} ! \
         audioresample ! audio/x-raw,format=S16LE,channels={channels},rate={sample_rate},layout=interleaved ! \
         wavenc ! filesink location=\"{}\"",
        quote(input),
        quote(output)
    );
    let pipeline = pipeline_from_description(&description)?;
    seek_window(&pipeline, start_ns, stop_ns)?;
    let result = wait_for_completion(&pipeline, 30);
    let _ = pipeline.set_state(gst::State::Null);
    result?;
    ensure!(
        output.exists(),
        "GStreamer 未生成段输出: {}",
        output.display()
    );
    Ok(description)
}

pub fn concat_segments(
    segments: &[std::path::PathBuf],
    output: &Path,
    sample_rate: u32,
    channels: u16,
) -> Result<String> {
    ensure!(!segments.is_empty(), "没有可拼接的分段输出");
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut description = format!(
        "concat name=calibrated ! audioconvert ! \
         audio/x-raw,format=S16LE,channels={channels},rate={sample_rate},layout=interleaved ! \
         wavenc ! filesink location=\"{}\" ",
        quote(output)
    );
    for segment in segments {
        description.push_str(&format!(
            "filesrc location=\"{}\" ! wavparse ! calibrated. ",
            quote(segment)
        ));
    }
    let pipeline = pipeline_from_description(&description)?;
    pipeline
        .set_state(gst::State::Playing)
        .context("无法启动拼接管线")?;
    let result = wait_for_completion(&pipeline, 30);
    let _ = pipeline.set_state(gst::State::Null);
    result?;
    ensure!(output.exists(), "未生成最终校准 WAV");
    Ok(description)
}

pub fn gstreamer_version() -> String {
    gst::version_string().to_string()
}
