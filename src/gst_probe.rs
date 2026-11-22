use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use gstreamer as gst;
use gstreamer::glib::object::Cast;
use gstreamer::prelude::*;
use gstreamer_app::AppSink;

pub const PIPELINE_LABEL: &str =
    "filesrc → wavparse → audioconvert → audioresample → capsfilter(F32LE/8ch/48k) → appsink";

#[derive(Clone, Debug)]
pub struct PcmProbe {
    pub samples: Vec<f32>,
    pub caps: String,
    pub elapsed_ms: u128,
}

pub fn decode_f32le(path: &Path) -> Result<PcmProbe> {
    gst::init().context("GStreamer 初始化失败")?;
    let started = Instant::now();

    let pipeline = gst::Pipeline::new();
    let filesrc = gst::ElementFactory::make("filesrc")
        .property("location", path.to_string_lossy().as_ref())
        .build()
        .context("无法创建 filesrc")?;
    let wavparse = gst::ElementFactory::make("wavparse")
        .build()
        .context("缺少 wavparse 插件")?;
    let convert = gst::ElementFactory::make("audioconvert")
        .build()
        .context("缺少 audioconvert 插件")?;
    let resample = gst::ElementFactory::make("audioresample")
        .build()
        .context("缺少 audioresample 插件")?;
    let caps = gst::Caps::builder("audio/x-raw")
        .field("format", "F32LE")
        .field("channels", 8i32)
        .field("rate", 48_000i32)
        .field("layout", "interleaved")
        .build();
    let capsfilter = gst::ElementFactory::make("capsfilter")
        .property("caps", &caps)
        .build()
        .context("无法创建 capsfilter")?;
    let sink = gst::ElementFactory::make("appsink")
        .property("sync", false)
        .property("max-buffers", 64u32)
        .property("drop", false)
        .build()
        .context("缺少 appsink 插件")?;
    let sink = sink
        .downcast::<AppSink>()
        .map_err(|_| anyhow::anyhow!("appsink 类型不匹配"))?;

    pipeline.add_many([
        &filesrc,
        &wavparse,
        &convert,
        &resample,
        &capsfilter,
        sink.upcast_ref(),
    ])?;
    gst::Element::link_many([
        &filesrc,
        &wavparse,
        &convert,
        &resample,
        &capsfilter,
        sink.upcast_ref(),
    ])?;

    let bus = pipeline.bus().context("Pipeline 没有 bus")?;
    pipeline.set_state(gst::State::Playing)?;

    let mut samples = Vec::with_capacity(18 * 48_000 * 8);
    let mut eos = false;
    let mut runtime_error: Option<String> = None;
    let deadline = Instant::now() + Duration::from_secs(30);

    while !eos && Instant::now() < deadline {
        if let Some(sample) = sink.try_pull_sample(gst::ClockTime::from_mseconds(150))
            && let Some(buffer) = sample.buffer()
        {
            let map = buffer.map_readable().context("无法映射 GStreamer buffer")?;
            for chunk in map.as_chunks::<4>().0 {
                samples.push(f32::from_le_bytes(*chunk));
            }
        }
        while let Some(message) = bus.pop() {
            use gst::MessageView;
            match message.view() {
                MessageView::Eos(..) => eos = true,
                MessageView::Error(error) => {
                    runtime_error = Some(format!(
                        "{} [{}]",
                        error.error(),
                        error.debug().unwrap_or_else(|| "无调试信息".into())
                    ));
                    eos = true;
                }
                _ => {}
            }
        }
    }
    let _ = pipeline.set_state(gst::State::Null);

    if let Some(error) = runtime_error {
        bail!("GStreamer 解码失败: {error}");
    }
    if samples.is_empty() {
        bail!("GStreamer 未输出样本（超时或来源为空）");
    }
    let caps = sink
        .static_pad("sink")
        .and_then(|pad| pad.current_caps())
        .map(|caps| caps.to_string())
        .unwrap_or_else(|| "audio/x-raw,F32LE,channels=8,rate=48000".to_string());

    Ok(PcmProbe {
        samples,
        caps,
        elapsed_ms: started.elapsed().as_millis(),
    })
}
