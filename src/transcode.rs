//! GStreamer-backed per-segment transcoding and evidence collection.

use crate::model::DeviceId;
use crate::util::sha256_file;
use anyhow::{Context, Result, anyhow, bail};
use gst::prelude::*;
use gstreamer as gst;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

const VIDEO_WIDTH: i32 = 640;
const VIDEO_HEIGHT: i32 = 360;
const TIMEOUT_SECONDS: u64 = 180;

#[derive(Debug, Clone)]
pub struct TranscodeOutput {
    pub output_path: PathBuf,
    pub preview_path: Option<PathBuf>,
    pub pipeline: String,
    pub gst_version: String,
    pub elapsed_ms: u64,
    pub input_sha256: Option<String>,
    pub output_sha256: String,
    pub output_bytes: u64,
}

pub fn ensure_initialized() -> Result<()> {
    gst::init().map_err(|error| anyhow!("GStreamer 初始化失败：{error}"))
}

pub fn gstreamer_version() -> String {
    gst::version_string().to_string()
}

pub fn pipeline_description(input: &Path, output: &Path) -> String {
    format!(
        "filesrc location={} ! decodebin ! queue ! videoconvert ! videoscale \
         ! video/x-raw,width={VIDEO_WIDTH},height={VIDEO_HEIGHT},pixel-aspect-ratio=1/1 \
         ! vp8enc deadline=1000000 target-bitrate=1200000 \
         ! webmmux ! filesink location={}",
        quoted(input),
        quoted(output)
    )
}

pub fn transcode_segment(
    input: &Path,
    output: &Path,
    preview: Option<&Path>,
    trim: Option<(i64, i64)>,
) -> Result<TranscodeOutput> {
    ensure_initialized()?;
    if !input.is_file() {
        bail!("输入片段不存在：{}", input.display());
    }
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("创建产物目录失败：{}", parent.display()))?;
    }
    if output.exists() {
        fs::remove_file(output).with_context(|| format!("清理旧产物失败：{}", output.display()))?;
    }
    let pipeline_text = pipeline_description(input, output);
    let started = Instant::now();
    run_encode_pipeline(input, output, preview, trim)
        .with_context(|| format!("片段转码失败：{}", input.display()))?;
    let output_sha256 =
        sha256_file(output).with_context(|| format!("读取产物校验和失败：{}", output.display()))?;
    let output_bytes = fs::metadata(output)
        .with_context(|| format!("读取产物大小失败：{}", output.display()))?
        .len();
    let input_sha256 = sha256_file(input).ok();
    let preview_path = preview.filter(|path| path.is_file()).map(Path::to_path_buf);
    Ok(TranscodeOutput {
        output_path: output.to_path_buf(),
        preview_path,
        pipeline: pipeline_text,
        gst_version: gstreamer_version(),
        elapsed_ms: started.elapsed().as_millis() as u64,
        input_sha256,
        output_sha256,
        output_bytes,
    })
}

fn run_encode_pipeline(
    input: &Path,
    output: &Path,
    preview: Option<&Path>,
    trim: Option<(i64, i64)>,
) -> Result<()> {
    let pipeline = gst::Pipeline::new();
    let source = element("filesrc")?;
    source.set_property("location", path_string(input));
    let decode = element("decodebin")?;
    let queue = element("queue")?;
    let convert = element("videoconvert")?;
    let scale = element("videoscale")?;
    scale.set_property("add-borders", true);
    let caps = gst::Caps::builder("video/x-raw")
        .field("width", VIDEO_WIDTH)
        .field("height", VIDEO_HEIGHT)
        .field("pixel-aspect-ratio", gst::Fraction::new(1, 1))
        .build();
    let filter = element("capsfilter")?;
    filter.set_property("caps", &caps);
    let encoder = element("vp8enc")?;
    encoder.set_property("deadline", 1_000_000_i64);
    encoder.set_property("target-bitrate", 1_200_000_i32);
    let mux = element("webmmux")?;
    let sink = element("filesink")?;
    sink.set_property("location", path_string(output));

    pipeline.add_many([
        &source, &decode, &queue, &convert, &scale, &filter, &encoder, &mux, &sink,
    ])?;
    source.link(&decode)?;
    gst::Element::link_many([&queue, &convert, &scale, &filter, &encoder, &mux, &sink])?;

    let queue_for_callback = queue.clone();
    decode.connect_pad_added(move |_decode, pad| {
        let caps = pad.current_caps().unwrap_or_else(|| pad.query_caps(None));
        let is_video = caps
            .structure(0)
            .is_some_and(|structure| structure.name().starts_with("video/"));
        if !is_video {
            return;
        }
        if let Some(sink_pad) = queue_for_callback.static_pad("sink")
            && !sink_pad.is_linked()
        {
            let _ = pad.link(&sink_pad);
        }
    });

    if let Some((start_ms, stop_ms)) = trim {
        let start_ms = start_ms.max(0);
        let stop_ms = stop_ms.max(start_ms.saturating_add(1));
        let eos_sent = Arc::new(AtomicBool::new(false));
        let source_pad = queue
            .static_pad("src")
            .context("裁剪探针缺少 queue src pad")?;
        source_pad.add_probe(gst::PadProbeType::BUFFER, move |pad, info| {
            let Some(buffer) = info.buffer() else {
                return gst::PadProbeReturn::Ok;
            };
            let Some(pts) = buffer.pts() else {
                return gst::PadProbeReturn::Ok;
            };
            let pts_ms = pts.mseconds() as i64;
            if pts_ms < start_ms {
                return gst::PadProbeReturn::Drop;
            }
            if pts_ms >= stop_ms {
                if !eos_sent.swap(true, Ordering::SeqCst) {
                    let _ = pad.push_event(gst::event::Eos::new());
                }
                return gst::PadProbeReturn::Drop;
            }
            gst::PadProbeReturn::Ok
        });
    }
    let bus = pipeline.bus().context("管线未创建 bus")?;
    pipeline.set_state(gst::State::Playing)?;
    let timeout = gst::ClockTime::from_seconds(TIMEOUT_SECONDS);
    let message =
        bus.timed_pop_filtered(timeout, &[gst::MessageType::Eos, gst::MessageType::Error]);
    let finish_result = match message.as_ref().map(|message| message.view()) {
        Some(gst::MessageView::Eos(_)) => Ok(()),
        Some(gst::MessageView::Error(error)) => Err(anyhow!(
            "{}{}",
            error.error(),
            error
                .debug()
                .filter(|debug| !debug.is_empty())
                .map(|debug| format!(" ({debug})"))
                .unwrap_or_default()
        )),
        _ => Err(anyhow!("转码超时（{TIMEOUT_SECONDS} 秒）")),
    };
    let _ = pipeline.set_state(gst::State::Null);
    finish_result?;

    if let Some(preview_path) = preview {
        if let Some(parent) = preview_path.parent() {
            fs::create_dir_all(parent)?;
        }
        // The preview is supporting evidence; a failed thumbnail must not
        // invalidate a successful media transcode.
        let _ = render_first_frame(output, preview_path);
    }
    Ok(())
}

pub fn render_first_frame(input: &Path, output: &Path) -> Result<()> {
    ensure_initialized()?;
    let pipeline = gst::Pipeline::new();
    let source = element("filesrc")?;
    source.set_property("location", path_string(input));
    let decode = element("decodebin")?;
    let convert = element("videoconvert")?;
    let encoder = element("pngenc")?;
    encoder.set_property("snapshot", true);
    let sink = element("filesink")?;
    sink.set_property("location", path_string(output));
    pipeline.add_many([&source, &decode, &convert, &encoder, &sink])?;
    source.link(&decode)?;
    gst::Element::link_many([&convert, &encoder, &sink])?;
    let convert_for_callback = convert.clone();
    decode.connect_pad_added(move |_decode, pad| {
        let caps = pad.current_caps().unwrap_or_else(|| pad.query_caps(None));
        let is_video = caps
            .structure(0)
            .is_some_and(|structure| structure.name().starts_with("video/"));
        if is_video
            && let Some(sink_pad) = convert_for_callback.static_pad("sink")
            && !sink_pad.is_linked()
        {
            let _ = pad.link(&sink_pad);
        }
    });
    pipeline.set_state(gst::State::Playing)?;
    let bus = pipeline.bus().context("预览管线未创建 bus")?;
    let message = bus.timed_pop_filtered(
        gst::ClockTime::from_seconds(30),
        &[gst::MessageType::Eos, gst::MessageType::Error],
    );
    let result = match message.as_ref().map(|message| message.view()) {
        Some(gst::MessageView::Eos(_)) => Ok(()),
        Some(gst::MessageView::Error(error)) => Err(anyhow!(error.error().to_string())),
        _ => Err(anyhow!("首帧提取超时")),
    };
    let _ = pipeline.set_state(gst::State::Null);
    result
}

pub fn generate_demo_clip(path: &Path, device: DeviceId) -> Result<()> {
    ensure_initialized()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let pipeline = gst::Pipeline::new();
    let source = element("videotestsrc")?;
    source.set_property("num-buffers", 420_i32);
    source.set_property(
        "foreground-color",
        match device {
            DeviceId::A => 0xff8c00ff_u32,
            DeviceId::B => 0x00a8c6ff_u32,
        },
    );
    let caps = gst::Caps::builder("video/x-raw")
        .field("width", 640)
        .field("height", 360)
        .field("framerate", gst::Fraction::new(30, 1))
        .build();
    let filter = element("capsfilter")?;
    filter.set_property("caps", &caps);
    let convert = element("videoconvert")?;
    let encoder = element("vp8enc")?;
    encoder.set_property("deadline", 1_000_000_i64);
    let mux = element("webmmux")?;
    let sink = element("filesink")?;
    sink.set_property("location", path_string(path));
    pipeline.add_many([&source, &filter, &convert, &encoder, &mux, &sink])?;
    gst::Element::link_many([&source, &filter, &convert, &encoder, &mux, &sink])?;
    pipeline.set_state(gst::State::Playing)?;
    let bus = pipeline.bus().context("示例管线未创建 bus")?;
    let message = bus.timed_pop_filtered(
        gst::ClockTime::from_seconds(60),
        &[gst::MessageType::Eos, gst::MessageType::Error],
    );
    let result = match message.as_ref().map(|message| message.view()) {
        Some(gst::MessageView::Eos(_)) => Ok(()),
        Some(gst::MessageView::Error(error)) => Err(anyhow!(error.error().to_string())),
        _ => Err(anyhow!("示例片段生成超时")),
    };
    let _ = pipeline.set_state(gst::State::Null);
    result
}

fn element(factory: &str) -> Result<gst::Element> {
    gst::ElementFactory::make(factory)
        .build()
        .map_err(|error| anyhow!("缺少 GStreamer 元素 {factory}：{error}"))
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn quoted(path: &Path) -> String {
    let value = path_string(path);
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}
