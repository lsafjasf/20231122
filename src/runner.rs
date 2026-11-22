use std::fs::{self, File};
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gstreamer as gst;
use gstreamer::prelude::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::model::AttemptStatus;

const READ_CHUNK: usize = 1024 * 1024;

#[derive(Clone, Debug)]
pub struct RunRequest {
    pub source_path: PathBuf,
    pub output_path: PathBuf,
    pub start_ms: i64,
    pub duration_ms: i64,
    pub dot_path: Option<PathBuf>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunResult {
    pub status: AttemptStatus,
    pub pipeline: String,
    pub wall_time_ms: u64,
    pub output_size_bytes: Option<u64>,
    pub output_sha256: Option<String>,
    pub source_size_bytes: Option<u64>,
    pub source_sha256: Option<String>,
    pub error: Option<String>,
}

pub fn write_run_evidence(
    path: &Path,
    attempt_id: &str,
    segment_id: &str,
    source_path: &Path,
    start_ms: i64,
    duration_ms: i64,
    result: &RunResult,
) -> Result<(), String> {
    let payload = serde_json::json!({
        "schema_version": 1,
        "attempt_id": attempt_id,
        "segment_id": segment_id,
        "source_path": source_path,
        "requested_start_ms": start_ms,
        "requested_duration_ms": duration_ms,
        "result": result,
    });
    let bytes = serde_json::to_vec_pretty(&payload)
        .map_err(|error| format!("序列化运行证据失败: {error}"))?;
    crate::store::atomic_write(path, &bytes).map_err(|error| format!("写入运行证据失败: {error:#}"))
}

pub fn apply_run_result(attempt: &mut crate::model::AttemptRecord, result: RunResult) {
    attempt.status = result.status;
    attempt.finished_at = crate::model::now_rfc3339();
    attempt.pipeline = result.pipeline;
    attempt.wall_time_ms = result.wall_time_ms;
    attempt.output_size_bytes = result.output_size_bytes;
    attempt.output_sha256 = result.output_sha256;
    attempt.source_size_bytes = result.source_size_bytes;
    attempt.source_sha256 = result.source_sha256;
    attempt.error = result.error;
}

#[derive(Clone, Copy, Debug)]
struct EncodePlan {
    encoder: &'static str,
    parser: Option<&'static str>,
    muxer: &'static str,
    extension: &'static str,
}

pub fn initialize() -> Result<(), String> {
    gst::init().map_err(|error| format!("GStreamer 初始化失败: {error}"))
}

pub fn available_output_extension() -> &'static str {
    encode_plan().map(|plan| plan.extension).unwrap_or("mp4")
}

fn encode_plan() -> Result<EncodePlan, String> {
    let candidates = [
        EncodePlan {
            encoder: "x264enc",
            parser: Some("h264parse"),
            muxer: "mp4mux",
            extension: "mp4",
        },
        EncodePlan {
            encoder: "openh264enc",
            parser: Some("h264parse"),
            muxer: "mp4mux",
            extension: "mp4",
        },
        EncodePlan {
            encoder: "avenc_mpeg4",
            parser: None,
            muxer: "mp4mux",
            extension: "mp4",
        },
        EncodePlan {
            encoder: "vp8enc",
            parser: None,
            muxer: "webmmux",
            extension: "webm",
        },
    ];
    for plan in candidates {
        let has_encoder = gst::ElementFactory::find(plan.encoder).is_some();
        let has_muxer = gst::ElementFactory::find(plan.muxer).is_some();
        let has_parser = plan
            .parser
            .map(|name| gst::ElementFactory::find(name).is_some())
            .unwrap_or(true);
        if has_encoder && has_muxer && has_parser {
            return Ok(plan);
        }
    }
    Err("未找到可用的 GStreamer 视频编码器/封装器；请安装 gstreamer1.0-libav 或 gstreamer1.0-plugins-good"
        .to_string())
}

pub fn run_segment(
    request: &RunRequest,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64),
) -> RunResult {
    let started = Instant::now();
    let mut result = RunResult {
        status: AttemptStatus::Failed,
        pipeline: String::new(),
        wall_time_ms: 0,
        output_size_bytes: None,
        output_sha256: None,
        source_size_bytes: None,
        source_sha256: None,
        error: None,
    };

    match fingerprint_file(&request.source_path, cancel, &mut progress) {
        Ok((size, hash)) => {
            result.source_size_bytes = Some(size);
            result.source_sha256 = Some(hash);
        }
        Err(error) => {
            result.error = Some(error);
            result.wall_time_ms = elapsed_ms(started);
            return result;
        }
    }
    if cancel.load(Ordering::Relaxed) {
        result.status = AttemptStatus::Interrupted;
        result.error = Some("用户取消或进程准备退出".to_string());
        result.wall_time_ms = elapsed_ms(started);
        return result;
    }

    let plan = match encode_plan() {
        Ok(plan) => plan,
        Err(error) => {
            result.error = Some(error);
            result.wall_time_ms = elapsed_ms(started);
            return result;
        }
    };
    result.pipeline = pipeline_description(request, plan);

    match execute_pipeline(request, plan, cancel, &mut progress) {
        Ok(()) => match validate_output_container(&request.output_path, plan.extension) {
            Ok(()) => {
                result.status = AttemptStatus::Succeeded;
            }
            Err(error) => {
                result.error = Some(error);
            }
        },
        Err(error) => {
            result.error = Some(error);
        }
    }
    result.wall_time_ms = elapsed_ms(started);

    if request.output_path.exists() {
        match fs::metadata(&request.output_path) {
            Ok(metadata) if metadata.len() > 0 => {
                result.output_size_bytes = Some(metadata.len());
                match sha256_file(&request.output_path) {
                    Ok(hash) => result.output_sha256 = Some(hash),
                    Err(error) => result.error = Some(error),
                }
            }
            Ok(_) if result.status == AttemptStatus::Succeeded => {
                result.status = AttemptStatus::Failed;
                result.error = Some("GStreamer 已结束，但输出文件为空".to_string());
            }
            Err(error) => {
                result.status = AttemptStatus::Failed;
                result.error = Some(format!("无法读取输出文件: {error}"));
            }
            _ => {}
        }
    } else if result.status == AttemptStatus::Succeeded {
        result.status = AttemptStatus::Failed;
        result.error = Some("GStreamer 未生成输出文件".to_string());
    }
    result
}

fn execute_pipeline(
    request: &RunRequest,
    plan: EncodePlan,
    cancel: &AtomicBool,
    progress: &mut impl FnMut(u64),
) -> Result<(), String> {
    if request.duration_ms <= 0 {
        return Err("片段时长必须大于 0".to_string());
    }
    if let Some(parent) = request.output_path.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("创建输出目录失败: {error}"))?;
    }
    if request.output_path.exists() {
        fs::remove_file(&request.output_path)
            .map_err(|error| format!("清理旧输出失败: {error}"))?;
    }

    let uri = gst::glib::filename_to_uri(&request.source_path, None)
        .map_err(|error| format!("源路径无法转换为 URI: {error}"))?
        .to_string();
    let source = gst::ElementFactory::make("uridecodebin")
        .name("source")
        .property("uri", &uri)
        .property(
            "caps",
            gst::Caps::builder("video/x-raw")
                .field("format", "I420")
                .build(),
        )
        .build()
        .map_err(|error| format!("创建 uridecodebin 失败: {error}"))?;
    let convert = make_element("videoconvert")?;
    let rate = make_element("videorate")?;
    let caps = gst::Caps::builder("video/x-raw")
        .field("format", "I420")
        .field("framerate", gst::Fraction::new(30, 1))
        .build();
    let capsfilter = gst::ElementFactory::make("capsfilter")
        .name("segment_caps")
        .property("caps", &caps)
        .build()
        .map_err(|error| format!("创建 capsfilter 失败: {error}"))?;
    let encoder = gst::ElementFactory::make(plan.encoder)
        .name("encoder")
        .build()
        .map_err(|error| format!("创建编码器 {} 失败: {error}", plan.encoder))?;
    let parser = match plan.parser {
        Some(name) => Some(make_element(name)?),
        None => None,
    };
    let muxer = make_element(plan.muxer)?;
    let sink = gst::ElementFactory::make("filesink")
        .name("output")
        .property("location", request.output_path.to_string_lossy().as_ref())
        .property("sync", false)
        .build()
        .map_err(|error| format!("创建 filesink 失败: {error}"))?;

    let pipeline = gst::Pipeline::new();
    pipeline
        .add(&source)
        .and_then(|_| pipeline.add(&convert))
        .and_then(|_| pipeline.add(&rate))
        .and_then(|_| pipeline.add(&capsfilter))
        .and_then(|_| pipeline.add(&encoder))
        .map_err(|error| format!("装配管线失败: {error}"))?;
    if let Some(parser) = &parser {
        pipeline
            .add(parser)
            .map_err(|error| format!("装配解析器失败: {error}"))?;
    }
    pipeline
        .add(&muxer)
        .and_then(|_| pipeline.add(&sink))
        .map_err(|error| format!("装配封装器失败: {error}"))?;

    gst::Element::link_many([&convert, &rate, &capsfilter, &encoder])
        .map_err(|error| format!("连接视频处理链失败: {error}"))?;
    if let Some(parser) = &parser {
        gst::Element::link_many([&encoder, parser, &muxer, &sink])
            .map_err(|error| format!("连接编码封装链失败: {error}"))?;
    } else {
        gst::Element::link_many([&encoder, &muxer, &sink])
            .map_err(|error| format!("连接编码封装链失败: {error}"))?;
    }

    let convert_for_pad = convert.clone();
    source.connect_pad_added(move |_source, pad| {
        let Some(sink_pad) = convert_for_pad.static_pad("sink") else {
            return;
        };
        if sink_pad.is_linked() {
            return;
        }
        if let Some(caps) = pad.current_caps()
            && !caps.can_intersect(&gst::Caps::builder("video/x-raw").build())
        {
            return;
        }
        let _ = pad.link(&sink_pad);
    });

    let bus = pipeline
        .bus()
        .ok_or_else(|| "GStreamer 管线没有 bus".to_string())?;
    let start = request.start_ms.max(0) as u64;
    let stop = start.saturating_add(request.duration_ms.max(1) as u64);
    let eos_sent = Arc::new(AtomicBool::new(false));
    if let Some(pad) = capsfilter.static_pad("src") {
        let eos_sent = eos_sent.clone();
        pad.add_probe(gst::PadProbeType::BUFFER, move |pad, info| {
            let Some(buffer) = info.buffer() else {
                return gst::PadProbeReturn::Ok;
            };
            let Some(pts) = buffer.pts() else {
                return gst::PadProbeReturn::Ok;
            };
            let pts_ms = pts.mseconds();
            if pts_ms < start {
                return gst::PadProbeReturn::Drop;
            }
            if pts_ms >= stop {
                if !eos_sent.swap(true, Ordering::SeqCst) {
                    let _ = pad.push_event(gst::event::Eos::new());
                }
                return gst::PadProbeReturn::Drop;
            }
            gst::PadProbeReturn::Ok
        });
    }

    pipeline
        .set_state(gst::State::Paused)
        .map_err(|error| format!("管线进入 PAUSED 失败: {error}"))?;
    let (state_result, current, _pending) = pipeline.state(gst::ClockTime::from_seconds(12));
    if state_result.is_err() || current != gst::State::Paused {
        let detail =
            take_bus_error(&bus).unwrap_or_else(|| "等待媒体解码器就绪超时（12 s）".to_string());
        let _ = pipeline.set_state(gst::State::Null);
        return Err(detail);
    }

    pipeline
        .set_state(gst::State::Playing)
        .map_err(|error| format!("管线进入 PLAYING 失败: {error}"))?;

    let playback_started = Instant::now();
    let playback_timeout = Duration::from_millis(request.duration_ms.max(1) as u64 + 25_000);
    let mut reached_end = false;
    while playback_started.elapsed() < playback_timeout {
        if cancel.load(Ordering::Relaxed) {
            let _ = pipeline.set_state(gst::State::Null);
            return Err("用户取消或进程准备退出".to_string());
        }
        progress(
            start.saturating_add(
                playback_started
                    .elapsed()
                    .as_millis()
                    .min(request.duration_ms.max(0) as u128) as u64,
            ),
        );
        if let Some(message) = bus.iter_timed(gst::ClockTime::from_mseconds(100)).next() {
            match message.view() {
                gst::MessageView::Error(error) => {
                    let error_text = format_message_error(&message, error);
                    let _ = pipeline.set_state(gst::State::Null);
                    return Err(error_text);
                }
                gst::MessageView::Eos(_) => {
                    reached_end = true;
                    break;
                }
                _ => {}
            }
        }
    }
    if !reached_end {
        let _ = pipeline.set_state(gst::State::Null);
        return Err(format!(
            "等待片段 EOS 超时：请求 {} ms",
            request.duration_ms
        ));
    }
    if let Some(dot_path) = &request.dot_path {
        pipeline.debug_to_dot_file_with_ts(gst::DebugGraphDetails::all(), dot_path);
    }
    pipeline
        .set_state(gst::State::Null)
        .map_err(|error| format!("关闭管线失败: {error}"))?;
    let _ = pipeline.state(gst::ClockTime::from_seconds(5));
    Ok(())
}

fn take_bus_error(bus: &gst::Bus) -> Option<String> {
    let message = bus.pop_filtered(&[gst::MessageType::Error])?;
    match message.view() {
        gst::MessageView::Error(error) => Some(format_message_error(&message, error)),
        _ => None,
    }
}

fn validate_output_container(path: &Path, extension: &str) -> Result<(), String> {
    let metadata = fs::metadata(path).map_err(|error| format!("无法读取输出文件: {error}"))?;
    if metadata.len() == 0 {
        return Err("GStreamer 已结束，但输出文件为空".to_string());
    }
    let mut file = File::open(path).map_err(|error| format!("无法打开输出文件: {error}"))?;
    match extension {
        "mp4" => {
            validate_mp4_atoms(&mut file, metadata.len())?;
        }
        "webm" => {
            let mut head = [0_u8; 4];
            file.read_exact(&mut head)
                .map_err(|error| format!("读取输出文件头失败: {error}"))?;
            if !head.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]) {
                return Err("输出不是有效 WebM：缺少 EBML 头".to_string());
            }
        }
        _ => {}
    }
    Ok(())
}

fn validate_mp4_atoms(file: &mut File, length: u64) -> Result<(), String> {
    let mut position = 0_u64;
    let mut has_ftyp = false;
    let mut has_moov = false;
    let mut box_count = 0_u32;
    while position.saturating_add(8) <= length && box_count < 100_000 {
        file.seek(SeekFrom::Start(position))
            .map_err(|error| format!("定位 MP4 box 失败: {error}"))?;
        let mut header = [0_u8; 8];
        file.read_exact(&mut header)
            .map_err(|error| format!("读取 MP4 box 头失败: {error}"))?;
        let compact_size = u32::from_be_bytes(header[0..4].try_into().unwrap()) as u64;
        let box_type = &header[4..8];
        let (box_size, header_size) = if compact_size == 1 {
            let mut large = [0_u8; 8];
            file.read_exact(&mut large)
                .map_err(|error| format!("读取 MP4 大 box 长度失败: {error}"))?;
            (u64::from_be_bytes(large), 16_u64)
        } else if compact_size == 0 {
            (length - position, 8_u64)
        } else {
            (compact_size, 8_u64)
        };
        if box_size < header_size || position.saturating_add(box_size) > length {
            return Err("输出 MP4 box 长度损坏，封装未完成".to_string());
        }
        if box_type == b"ftyp" {
            has_ftyp = true;
        }
        if box_type == b"moov" {
            has_moov = true;
        }
        position = position.saturating_add(box_size);
        box_count += 1;
    }
    if !has_ftyp {
        return Err("输出不是有效 MP4：缺少顶层 ftyp box".to_string());
    }
    if !has_moov {
        return Err("输出 MP4 未完成封装：缺少顶层 moov box".to_string());
    }
    Ok(())
}

fn make_element(name: &str) -> Result<gst::Element, String> {
    gst::ElementFactory::make(name)
        .name(name)
        .build()
        .map_err(|error| format!("创建 GStreamer 元素 {name} 失败: {error}"))
}

fn format_message_error(message: &gst::Message, error: &gst::message::Error) -> String {
    let source = message
        .src()
        .map(|src| src.name().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let debug = error
        .debug()
        .map(|value| value.to_string())
        .unwrap_or_default();
    if debug.is_empty() {
        format!("{source}: {}", error.error())
    } else {
        format!("{source}: {} ({debug})", error.error())
    }
}

fn pipeline_description(request: &RunRequest, plan: EncodePlan) -> String {
    let parser = plan
        .parser
        .map(|name| format!(" ! {name}"))
        .unwrap_or_default();
    format!(
        "filesrc/uridecodebin={} ! videoconvert ! videorate ! video/x-raw,format=I420,framerate=30/1 ! {}{} ! {} ! filesink={} [pad-probe=drop<{}ms;eos>={}ms]",
        request.source_path.display(),
        plan.encoder,
        parser,
        plan.muxer,
        request.output_path.display(),
        request.start_ms.max(0),
        request
            .start_ms
            .max(0)
            .saturating_add(request.duration_ms.max(1))
    )
}

pub fn sha256_file(path: &Path) -> Result<String, String> {
    let file =
        File::open(path).map_err(|error| format!("打开 {} 失败: {error}", path.display()))?;
    let mut reader = BufReader::with_capacity(READ_CHUNK, file);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; READ_CHUNK];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| format!("读取 {} 失败: {error}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn fingerprint_file(
    path: &Path,
    cancel: &AtomicBool,
    progress: &mut impl FnMut(u64),
) -> Result<(u64, String), String> {
    let metadata = fs::metadata(path).map_err(|error| format!("读取源媒体信息失败: {error}"))?;
    if !metadata.is_file() {
        return Err(format!("源路径不是文件: {}", path.display()));
    }
    let file = File::open(path).map_err(|error| format!("打开源媒体失败: {error}"))?;
    let mut reader = BufReader::with_capacity(READ_CHUNK, file);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; READ_CHUNK];
    let mut total = 0_u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("用户取消或进程准备退出".to_string());
        }
        let read = reader
            .read(&mut buffer)
            .map_err(|error| format!("读取源媒体失败: {error}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        total += read as u64;
        if total % (8 * READ_CHUNK as u64) < read as u64 {
            progress(total);
        }
    }
    Ok((total, format!("{:x}", hasher.finalize())))
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u64::MAX as u128) as u64
}
