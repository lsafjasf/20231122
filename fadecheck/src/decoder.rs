//! GStreamer 解码层。
//!
//! 管线：`filesrc ! decodebin ! audioconvert ! audioresample ! appsink`
//! decodebin 自动按容器/编码选择 demuxer 与 decoder；appsink 拉取
//! 固定规格（f32、单声道、可配置采样率）的 PCM 帧，供淡化分析使用。

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app::AppSink;
use gstreamer_audio::AudioFormat;

use crate::model::PipelineStep;

/// 一次解码的产物：样本与流信息。
#[derive(Debug, Clone)]
pub struct DecodedStream {
    /// 交错前的单声道 f32 样本，范围约 [-1, 1]。
    pub mono_samples: Vec<f32>,
    pub sample_rate: u32,
    pub channels: u32,
    pub steps: Vec<PipelineStep>,
}

/// 解码错误，区分“管线构建/媒体格式问题”与“运行期总线错误”。
#[derive(Debug)]
pub struct DecodeError {
    pub message: String,
    pub steps: Vec<PipelineStep>,
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for DecodeError {}

/// 初始化 GStreamer；进程内重复调用安全。
pub fn init() -> Result<(), String> {
    gst::init().map_err(|e| format!("GStreamer 初始化失败: {e}"))
}

fn step(order: u32, stage: &str, detail: &str, observation: Option<String>) -> PipelineStep {
    PipelineStep {
        order,
        stage: stage.to_string(),
        detail: detail.to_string(),
        observation,
    }
}

/// 解码文件到单声道 f32 PCM。
///
/// `target_rate` 为 Some 时插入重采样，用于生成第二个版本快照；
/// 为 None 时保留原始采样率，对应第一个版本快照。
pub fn decode_to_mono(
    path: &std::path::Path,
    target_rate: Option<u32>,
    label: &str,
) -> Result<DecodedStream, DecodeError> {
    let mut steps = Vec::new();
    steps.push(step(
        1,
        "filesrc",
        &format!("读取原始输入 {}", path.display()),
        None,
    ));
    steps.push(step(
        2,
        "decodebin",
        "自动探测容器与编码并解码为原始媒体流",
        None,
    ));
    steps.push(step(
        3,
        "audioconvert",
        "转换为 F32 样本格式并下混为单声道",
        None,
    ));

    let pipeline = gst::Pipeline::with_name(&format!("fadecheck-{label}"));
    let filesrc = gst::ElementFactory::make("filesrc")
        .property("location", path.to_str().unwrap_or_default())
        .build()
        .map_err(|e| DecodeError {
            message: format!("filesrc 创建失败: {e}"),
            steps: steps.clone(),
        })?;
    let decodebin = gst::ElementFactory::make("decodebin")
        .build()
        .map_err(|e| DecodeError {
            message: format!("decodebin 创建失败: {e}"),
            steps: steps.clone(),
        })?;
    let convert = gst::ElementFactory::make("audioconvert")
        .build()
        .map_err(|e| DecodeError {
            message: format!("audioconvert 创建失败: {e}"),
            steps: steps.clone(),
        })?;
    let resample = gst::ElementFactory::make("audioresample")
        .build()
        .map_err(|e| DecodeError {
            message: format!("audioresample 创建失败: {e}"),
            steps: steps.clone(),
        })?;
    let mut caps_builder = gst::Caps::builder("audio/x-raw")
        .field("format", AudioFormat::F32le.to_str())
        .field("layout", "interleaved")
        .field("channels", 1i32);
    if let Some(rate) = target_rate {
        caps_builder = caps_builder.field("rate", rate as i32);
    }
    let caps = caps_builder.build();
    let appsink = gst::ElementFactory::make("appsink")
        .property("caps", &caps)
        .property("sync", false)
        .property("emit-signals", false)
        .build()
        .map_err(|e| DecodeError {
            message: format!("appsink 创建失败: {e}"),
            steps: steps.clone(),
        })?;

    pipeline
        .add_many([&filesrc, &decodebin, &convert, &resample, &appsink])
        .and_then(|_| gst::Element::link_many([&filesrc, &decodebin]))
        .and_then(|_| gst::Element::link_many([&convert, &resample, &appsink]))
        .map_err(|e| DecodeError {
            message: format!("管线装配失败: {e}"),
            steps: steps.clone(),
        })?;

    // decodebin 的 src pad 是在媒体类型探测完成后动态出现的。
    let convert_weak = convert.downgrade();
    decodebin.connect_pad_added(move |_dbin, src_pad| {
        if let Some(convert) = convert_weak.upgrade() {
            // audioconvert 的 sink pad 接受任意音频原始/压缩输入，
            // 直接请求其静态 sink pad 进行链接；兼容性由后续元素协商。
            if let Some(sink_pad) = convert.static_pad("sink") {
                if let Err(other) = src_pad.link(&sink_pad) {
                    eprintln!(
                        "decodebin pad 链接返回: {other:?} (pad caps: {:?})",
                        src_pad.current_caps()
                    );
                }
            }
        }
    });

    pipeline
        .set_state(gst::State::Playing)
        .map_err(|e| DecodeError {
            message: format!("管线启动失败: {e}"),
            steps: steps.clone(),
        })?;

    let sink: AppSink = appsink.dynamic_cast().map_err(|_| DecodeError {
        message: "appsink 类型转换失败".to_string(),
        steps: steps.clone(),
    })?;

    let mut mono = Vec::new();
    let mut info_rate: Option<u32> = None;
    let mut info_channels: Option<u32> = None;
    let mut failure: Option<String> = None;
    let mut eos = false;

    loop {
        match sink.try_pull_sample(gst::ClockTime::from_seconds(1)) {
            Some(sample) => {
                let buffer = match sample.buffer() {
                    Some(b) => b,
                    None => continue,
                };
                if let Some(caps) = sample.caps() {
                    if let Some(s) = caps.structure(0) {
                        info_rate = s.get::<i32>("rate").ok().map(|r| r as u32);
                        info_channels = s.get::<i32>("channels").ok().map(|c| c as u32);
                    }
                }
                if let Ok(map) = buffer.map_readable() {
                    let bytes = map.as_slice();
                    let (words, _remainder) = bytes.as_chunks::<4>();
                    let mut tail = Vec::with_capacity(words.len());
                    for word in words {
                        tail.push(f32::from_le_bytes(*word));
                    }
                    mono.append(&mut tail);
                }
            }
            None => {
                // 超时或流结束：交由总线消息区分 EOS 与真实错误。
                if sink.is_eos() {
                    eos = true;
                }
            }
        }

        // 每个样本循环都检查总线，错误/警告需要作为证据保留。
        let bus = pipeline.bus();
        if let Some(bus) = bus {
            while let Some(msg) =
                bus.pop_filtered(&[gst::MessageType::Error, gst::MessageType::Warning])
            {
                match msg.view() {
                    gst::MessageView::Error(e) => {
                        failure = Some(format!("GStreamer 错误: {} ({:?})", e.error(), e.debug()));
                    }
                    gst::MessageView::Warning(w) => {
                        steps.push(step(
                            steps.len() as u32 + 1,
                            "bus-warning",
                            &format!("{:?}", w.debug()),
                            Some(w.error().to_string()),
                        ));
                    }
                    _ => {}
                }
            }
        }
        if failure.is_some() {
            break;
        }
        if eos {
            break;
        }
    }

    let _ = pipeline.set_state(gst::State::Null);

    if let Some(message) = failure {
        return Err(DecodeError { message, steps });
    }
    if mono.is_empty() {
        return Err(DecodeError {
            message: "解码完成但未获得任何音频样本".to_string(),
            steps,
        });
    }

    let rate = info_rate.unwrap_or(0);
    let channels = info_channels.unwrap_or(1);
    let resample_detail = match target_rate {
        Some(r) => format!("重采样到 {r} Hz（第二版本快照）"),
        None => String::from("保留原始采样率（第一版本快照）"),
    };
    steps.push(step(
        4,
        "audioresample",
        &resample_detail,
        Some(format!("输出 {rate} Hz, 单声道, {} 帧", mono.len())),
    ));
    steps.push(step(
        5,
        "appsink",
        "拉取 audio/x-raw F32 LE 交错样本并回收为单声道缓冲",
        Some(format!("{} 个 f32 帧", mono.len())),
    ));

    Ok(DecodedStream {
        mono_samples: mono,
        sample_rate: rate,
        channels,
        steps,
    })
}
