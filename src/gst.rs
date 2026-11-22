use crate::model::{MediaFingerprint, ProxySpec};
use gst::prelude::*;
use gstreamer as gst;
use gstreamer_app as gst_app;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub fn init() -> Result<(), String> {
    gst::init().map_err(|e| format!("GStreamer 初始化失败: {e}"))
}

fn make(name: &str) -> Result<gst::Element, String> {
    gst::ElementFactory::make(name)
        .build()
        .map_err(|e| format!("缺少 GStreamer 元素 {name}: {e}"))
}

fn wait_for_eos(pipe: &gst::Pipeline, timeout: Duration) -> Result<(), String> {
    pipe.set_state(gst::State::Playing)
        .map_err(|e| format!("管线进入 PLAYING 失败: {e}"))?;
    let bus = pipe.bus().ok_or("管线没有 bus")?;
    let deadline = Instant::now() + timeout;
    loop {
        if Instant::now() > deadline {
            pipe.set_state(gst::State::Null).ok();
            return Err(format!("GStreamer 管线超时({}s)", timeout.as_secs()));
        }
        let Some(message) = bus.timed_pop(gst::ClockTime::from_mseconds(100)) else {
            continue;
        };
        use gst::MessageView;
        match message.view() {
            MessageView::Eos(_) => break,
            MessageView::Error(error) => {
                let text = format!(
                    "GStreamer 错误: {}; debug={}",
                    error.error(),
                    error.debug().unwrap_or_default()
                );
                pipe.set_state(gst::State::Null).ok();
                return Err(text);
            }
            _ => {}
        }
    }
    pipe.set_state(gst::State::Null)
        .map_err(|e| format!("管线回到 NULL 失败: {e}"))?;
    Ok(())
}

pub fn generate_png_source(
    output: &Path,
    pattern: &str,
    width: u32,
    height: u32,
) -> Result<(), String> {
    init()?;
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let src = make("videotestsrc")?;
    src.set_property("num-buffers", 1i32);
    src.set_property_from_str("pattern", pattern);
    let capsfilter = make("capsfilter")?;
    capsfilter.set_property(
        "caps",
        gst::Caps::builder("video/x-raw")
            .field("format", "RGB")
            .field("width", width as i32)
            .field("height", height as i32)
            .field("framerate", gst::Fraction::new(1, 1))
            .build(),
    );
    let encoder = make("pngenc")?;
    encoder.set_property("snapshot", true);
    let sink = make("filesink")?;
    sink.set_property("location", output.to_string_lossy().as_ref());

    let pipeline = gst::Pipeline::new();
    pipeline
        .add_many([&src, &capsfilter, &encoder, &sink])
        .map_err(|e| e.to_string())?;
    gst::Element::link_many([&src, &capsfilter, &encoder, &sink])
        .map_err(|e| format!("生成源管线连接失败: {e}"))?;
    wait_for_eos(&pipeline, Duration::from_secs(30))
}

pub fn build_proxy(source: &Path, output: &Path, spec: &ProxySpec) -> Result<(), String> {
    init()?;
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let src = make("filesrc")?;
    src.set_property("location", source.to_string_lossy().as_ref());
    let decoder = make("pngdec")?;
    let scale = make("videoscale")?;
    let capsfilter = make("capsfilter")?;
    capsfilter.set_property(
        "caps",
        gst::Caps::builder("video/x-raw")
            .field("format", spec.pixel_format.as_str())
            .field("width", spec.width as i32)
            .field("height", spec.height as i32)
            .build(),
    );
    let encoder = make(&spec.encoder)?;
    encoder.set_property("snapshot", true);
    let sink = make("filesink")?;
    sink.set_property("location", output.to_string_lossy().as_ref());

    let pipeline = gst::Pipeline::new();
    pipeline
        .add_many([&src, &decoder, &scale, &capsfilter, &encoder, &sink])
        .map_err(|e| e.to_string())?;
    gst::Element::link_many([&src, &decoder, &scale, &capsfilter, &encoder, &sink])
        .map_err(|e| format!("代理管线连接失败: {e}"))?;
    wait_for_eos(&pipeline, Duration::from_secs(30))
}

/// Decode to RGBA and hash every raw frame. This is independent of encoder
/// container metadata and proves that a proxy is actually playable.
pub fn fingerprint_media(path: &Path, expected: &ProxySpec) -> Result<MediaFingerprint, String> {
    init()?;
    let src = make("filesrc")?;
    src.set_property("location", path.to_string_lossy().as_ref());
    let decoder = make("pngdec")?;
    let convert = make("videoconvert")?;
    let capsfilter = make("capsfilter")?;
    capsfilter.set_property(
        "caps",
        gst::Caps::builder("video/x-raw")
            .field("format", "RGBA")
            .build(),
    );
    let sink = make("appsink")?;
    sink.set_property("sync", false);
    sink.set_property("drop", false);
    sink.set_property("emit-signals", false);
    sink.set_property("max-buffers", 1u32);

    let pipeline = gst::Pipeline::new();
    pipeline
        .add_many([&src, &decoder, &convert, &capsfilter, &sink])
        .map_err(|e| e.to_string())?;
    gst::Element::link_many([&src, &decoder, &convert, &capsfilter, &sink])
        .map_err(|e| format!("解码校验管线连接失败: {e}"))?;
    let app_sink = sink
        .downcast::<gst_app::AppSink>()
        .map_err(|_| "appsink 类型转换失败".to_string())?;

    pipeline
        .set_state(gst::State::Playing)
        .map_err(|e| format!("解码校验进入 PLAYING 失败: {e}"))?;
    let bus = pipeline.bus().ok_or("解码校验管线没有 bus")?;
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut hasher = Sha256::new();
    let mut frames = 0u64;
    let mut width = 0u32;
    let mut height = 0u32;
    let mut format = String::new();
    loop {
        if Instant::now() > deadline {
            pipeline.set_state(gst::State::Null).ok();
            return Err("解码校验超时".into());
        }
        if let Some(sample) = app_sink.try_pull_sample(gst::ClockTime::from_mseconds(100)) {
            if let Some(caps) = sample.caps() {
                if let Some(s) = caps.structure(0) {
                    width = s.get::<i32>("width").unwrap_or(width as i32).max(0) as u32;
                    height = s.get::<i32>("height").unwrap_or(height as i32).max(0) as u32;
                    format = s.get::<String>("format").unwrap_or_else(|_| format.clone());
                }
            }
            let buffer = sample.buffer().ok_or("解码 sample 没有 buffer")?;
            let map = buffer.map_readable().map_err(|e| e.to_string())?;
            hasher.update(&map);
            frames += 1;
            continue;
        }
        let Some(message) = bus.timed_pop(gst::ClockTime::ZERO) else {
            continue;
        };
        use gst::MessageView;
        match message.view() {
            MessageView::Eos(_) => break,
            MessageView::Error(error) => {
                let text = format!(
                    "解码失败: {}; debug={}",
                    error.error(),
                    error.debug().unwrap_or_default()
                );
                pipeline.set_state(gst::State::Null).ok();
                return Err(text);
            }
            _ => {}
        }
    }
    pipeline.set_state(gst::State::Null).ok();
    if frames == 0 {
        return Err("解码成功但没有得到视频帧".into());
    }
    if width != expected.width || height != expected.height {
        return Err(format!(
            "代理尺寸不匹配: 实际 {width}x{height}, 期望 {}x{}",
            expected.width, expected.height
        ));
    }
    Ok(MediaFingerprint {
        width,
        height,
        format,
        frames,
        raw_sha256: format!("{:x}", hasher.finalize()),
    })
}

pub fn record_gst_env(output: &Path) -> Result<PathBuf, String> {
    init()?;
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut text = String::new();
    text.push_str(&format!("gstreamer_version={}\n", gst::version_string()));
    text.push_str(&format!(
        "gst_plugin_path={}\n",
        std::env::var("GST_PLUGIN_PATH").unwrap_or_default()
    ));
    text.push_str(&format!(
        "gst_plugin_scanner={}\n",
        std::env::var("GST_PLUGIN_SCANNER").unwrap_or_default()
    ));
    for name in [
        "videotestsrc",
        "pngenc",
        "pngdec",
        "videoscale",
        "filesrc",
        "filesink",
    ] {
        match gst::ElementFactory::find(name) {
            Some(factory) => text.push_str(&format!(
                "element.{name}=present; klass={}; description={}\n",
                factory.metadata(gst::ELEMENT_METADATA_KLASS).unwrap_or("?"),
                factory
                    .metadata(gst::ELEMENT_METADATA_DESCRIPTION)
                    .unwrap_or("?")
            )),
            None => text.push_str(&format!("element.{name}=MISSING\n")),
        }
    }
    crate::hash::write_text_atomic(output, &text)?;
    Ok(output.to_path_buf())
}
