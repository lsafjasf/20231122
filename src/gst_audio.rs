use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use gst::prelude::*;
use gst_app::{AppSink, AppSinkCallbacks};
use serde::Serialize;

use crate::failure::{Failure, Result};
use crate::sha256;
use crate::waveform::{ANALYSIS_RATE, DEFAULT_MAX_SECONDS, WaveAccumulator, WaveformSummary};

#[derive(Debug, Clone, Serialize)]
pub struct AudioAnalysis {
    pub waveform: WaveformSummary,
    pub output_wav: PathBuf,
    pub output_wav_sha256: String,
    pub gstreamer_caps: Option<String>,
    pub max_seconds: u64,
}

struct DecodeState {
    waveform: WaveAccumulator,
    writer: Option<hound::WavWriter<BufWriter<File>>>,
    error: Option<String>,
}

pub fn analyze_file(input: &Path, output_wav: &Path, max_seconds: u64) -> Result<AudioAnalysis> {
    gst::init().map_err(|error| {
        Failure::new(
            "gst_init",
            "GStreamer 初始化失败",
            "确认 `libgstreamer1.0-dev` 与运行时插件已安装，并运行 `gst-inspect-1.0 --version` 自检。",
        )
        .with_detail(error.to_string())
    })?;

    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: ANALYSIS_RATE,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let writer = hound::WavWriter::create(output_wav, spec).map_err(|error| {
        Failure::new(
            "waveform_wav",
            format!("无法创建输出波形：{}", output_wav.display()),
            "检查证据目录写权限与磁盘空间后重试。",
        )
        .with_detail(error.to_string())
    })?;
    let state = Arc::new(Mutex::new(DecodeState {
        waveform: WaveAccumulator::new(ANALYSIS_RATE, max_seconds),
        writer: Some(writer),
        error: None,
    }));
    let caps_seen = Arc::new(Mutex::new(None::<String>));

    let pipeline = gst::Pipeline::new();
    let filesrc = gst::ElementFactory::make("filesrc")
        .property("location", input)
        .build()
        .map_err(|error| {
            Failure::new(
                "gst_filesrc",
                "创建 GStreamer filesrc 失败",
                "安装 GStreamer core 插件，并确认输入路径可读。",
            )
            .with_detail(error.to_string())
        })?;
    let decodebin = gst::ElementFactory::make("decodebin")
        .build()
        .map_err(|error| Failure::gst("创建 decodebin", error))?;
    let audioconvert = gst::ElementFactory::make("audioconvert")
        .build()
        .map_err(|error| Failure::gst("创建 audioconvert", error))?;
    let audioresample = gst::ElementFactory::make("audioresample")
        .build()
        .map_err(|error| Failure::gst("创建 audioresample", error))?;
    let capsfilter = gst::ElementFactory::make("capsfilter")
        .property(
            "caps",
            gst::Caps::builder("audio/x-raw")
                .field("format", "F32LE")
                .field("channels", 1i32)
                .field("rate", ANALYSIS_RATE as i32)
                .field("layout", "interleaved")
                .build(),
        )
        .build()
        .map_err(|error| Failure::gst("创建 capsfilter", error))?;
    let appsink = gst::ElementFactory::make("appsink")
        .property("sync", false)
        .property("emit-signals", false)
        .property("max-buffers", 4u32)
        .build()
        .map_err(|error| Failure::gst("创建 appsink", error))?
        .downcast::<AppSink>()
        .map_err(|_| {
            Failure::new(
                "gst_appsink",
                "appsink 类型转换失败",
                "检查 GStreamer base 插件版本。",
            )
        })?;

    pipeline
        .add_many([
            &filesrc,
            &decodebin,
            &audioconvert,
            &audioresample,
            &capsfilter,
            appsink.upcast_ref(),
        ])
        .map_err(|error| Failure::gst("装配解码管线", error))?;
    filesrc
        .link(&decodebin)
        .map_err(|error| Failure::gst("连接 filesrc/decodebin", error))?;
    gst::Element::link_many([
        &audioconvert,
        &audioresample,
        &capsfilter,
        appsink.upcast_ref(),
    ])
    .map_err(|error| Failure::gst("连接音频转换链", error))?;

    {
        let audioconvert = audioconvert.clone();
        let caps_seen = caps_seen.clone();
        decodebin.connect_pad_added(move |_, pad| {
            let caps = pad.query_caps(None).to_string();
            if !caps.contains("audio/") && !caps.contains("audio/x-raw") {
                return;
            }
            if let Ok(mut seen) = caps_seen.lock() {
                *seen = Some(caps);
            }
            if let Some(sink_pad) = audioconvert.static_pad("sink")
                && !sink_pad.is_linked()
            {
                let _ = pad.link(&sink_pad);
            }
        });
    }

    {
        let state = state.clone();
        let callbacks = AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                let buffer = sample.buffer().ok_or(gst::FlowError::Error)?;
                let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;
                let mut guard = state.lock().map_err(|_| gst::FlowError::Error)?;
                if guard.error.is_some() {
                    return Err(gst::FlowError::Error);
                }
                let bytes = map.as_slice();
                if bytes.len() % 4 != 0 {
                    guard.error = Some("GStreamer 输出的 F32 缓冲区长度不是 4 的倍数".to_string());
                    return Err(gst::FlowError::Error);
                }
                let (chunks, _) = bytes.as_chunks::<4>();
                let samples: Vec<f32> = chunks
                    .iter()
                    .map(|chunk| f32::from_le_bytes(*chunk))
                    .collect();
                if let Some(writer) = guard.writer.as_mut() {
                    for sample in &samples {
                        if let Err(error) = writer.write_sample(*sample) {
                            guard.error = Some(format!("写入输出波形失败：{error}"));
                            return Err(gst::FlowError::Error);
                        }
                    }
                }
                guard.waveform.push(&samples);
                if guard.waveform.is_truncated() {
                    return Err(gst::FlowError::Eos);
                }
                Ok(gst::FlowSuccess::Ok)
            })
            .build();
        appsink.set_callbacks(callbacks);
    }

    pipeline
        .set_state(gst::State::Playing)
        .map_err(|error| Failure::gst("启动解码管线", error))?;
    let bus = pipeline.bus().ok_or_else(|| {
        Failure::new(
            "gst_bus",
            "GStreamer 管线没有 bus",
            "这是运行环境异常；保存当前证据后用 `GST_DEBUG=2` 重新运行。",
        )
    })?;
    let mut failure = None;
    while let Some(message) = bus.timed_pop_filtered(
        gst::ClockTime::NONE,
        &[gst::MessageType::Eos, gst::MessageType::Error],
    ) {
        match message.view() {
            gst::MessageView::Eos(_) => break,
            gst::MessageView::Error(error) => {
                failure = Some(Failure::gst(
                    "GStreamer 解码",
                    format!("{} ({:?})", error.error(), error.debug()),
                ));
                break;
            }
            _ => {}
        }
    }
    let _ = pipeline.set_state(gst::State::Null);

    let (waveform, state_error) = {
        let mut guard = state.lock().map_err(|_| {
            Failure::new(
                "decode_state",
                "解码状态锁损坏",
                "保留输入和日志后重新启动评估程序。",
            )
        })?;
        if let Some(writer) = guard.writer.take()
            && let Err(error) = writer.finalize()
        {
            guard.error = Some(format!("完成输出波形 WAV 失败：{error}"));
        }
        let waveform = std::mem::replace(
            &mut guard.waveform,
            WaveAccumulator::new(ANALYSIS_RATE, max_seconds),
        )
        .finish();
        (waveform, guard.error.clone())
    };
    if let Some(error) = state_error {
        failure = Some(Failure::new(
            "decode_write",
            error,
            "检查证据目录空间与权限；若音频仍在解码，先转为 WAV 再重试。",
        ));
    }
    if let Some(failure) = failure {
        return Err(failure);
    }
    if waveform.total_samples == 0 {
        return Err(Failure::new(
            "decode_empty",
            "GStreamer 没有解码出任何音频采样",
            "运行 `gst-discoverer-1.0 <文件>` 检查码流；必要时用 `decodebin` 转为 WAV 后重试。",
        ));
    }

    let caps = caps_seen
        .lock()
        .ok()
        .and_then(|caps| caps.clone())
        .or_else(|| {
            appsink
                .static_pad("sink")
                .and_then(|pad| pad.current_caps())
                .map(|caps| caps.to_string())
        });
    let output_wav_sha256 = sha256::file_sha256(output_wav)?;
    Ok(AudioAnalysis {
        waveform,
        output_wav: output_wav.to_path_buf(),
        output_wav_sha256,
        gstreamer_caps: caps,
        max_seconds,
    })
}

pub fn file_uri(path: &Path) -> Result<String> {
    let path = path.canonicalize().map_err(|e| {
        Failure::new(
            "file_uri_path",
            format!("无法解析重放文件的绝对路径：{}", path.display()),
            "确认文件存在且路径可读，然后重新运行评估。",
        )
        .with_detail(e.to_string())
    })?;
    gst::glib::filename_to_uri(&path, None)
        .map(|uri| uri.to_string())
        .map_err(|error| {
            Failure::new(
                "file_uri",
                format!("无法构造文件 URI：{}", path.display()),
                "确认路径是绝对路径且包含合法 UTF-8 文件名。",
            )
            .with_detail(error.to_string())
        })
}

pub fn max_seconds() -> u64 {
    std::env::var("ALC_MAX_SECONDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_MAX_SECONDS)
}
