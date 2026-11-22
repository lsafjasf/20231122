use std::path::Path;
use std::sync::OnceLock;

use anyhow::{anyhow, bail, Context, Result};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app::AppSink;

use crate::model::{sha256_hex, SOURCE_HEIGHT, SOURCE_WIDTH};
use crate::plan::scaled_size;

#[derive(Clone, Debug)]
pub struct DecoderChoice {
    pub name: String,
    pub fallback_reason: Option<String>,
}

#[derive(Clone, Debug)]
pub struct FrameSample {
    pub landed_timestamp_ms: u64,
    pub width: u32,
    pub height: u32,
    pub frame_sha256: String,
    rgba: Vec<u8>,
}

impl FrameSample {
    pub fn rgb_at(&self, x: u32, y: u32) -> Result<[u8; 3]> {
        if x >= self.width || y >= self.height {
            bail!(
                "sample point ({x},{y}) outside scaled frame {}x{}",
                self.width,
                self.height
            );
        }
        let offset = (y as usize * self.width as usize + x as usize) * 4;
        let pixel = self
            .rgba
            .get(offset..offset + 4)
            .ok_or_else(|| anyhow!("mapped RGBA buffer is truncated at byte {offset}"))?;
        Ok([pixel[0], pixel[1], pixel[2]])
    }
}

pub fn init() -> Result<()> {
    static INIT: OnceLock<Result<(), String>> = OnceLock::new();
    match INIT.get_or_init(|| gst::init().map_err(|error| error.to_string())) {
        Ok(()) => Ok(()),
        Err(error) => bail!("GStreamer initialization failed: {error}"),
    }
}

pub fn choose_decoder() -> DecoderChoice {
    if gst::ElementFactory::find("avdec_png").is_some() {
        DecoderChoice {
            name: "avdec_png".to_string(),
            fallback_reason: None,
        }
    } else {
        DecoderChoice {
            name: "pngdec".to_string(),
            fallback_reason: Some(
                "preferred decoder 'avdec_png' unavailable, falling back to software 'pngdec'"
                    .to_string(),
            ),
        }
    }
}

pub fn sample_frame(
    source: &Path,
    timestamp_ms: u64,
    scale_percent: u8,
    seek_timeout_ms: u64,
    _decoder: &str,
) -> Result<FrameSample> {
    init()?;
    if !source.is_file() {
        bail!("source does not exist: {}", source.display());
    }
    let (scaled_w, scaled_h) = scaled_size(SOURCE_WIDTH, SOURCE_HEIGHT, scale_percent);
    let pipeline = gst::Pipeline::new();
    let filesrc = gst::ElementFactory::make("filesrc")
        .property("location", source)
        .build()
        .context("create filesrc")?;
    let decodebin = gst::ElementFactory::make("decodebin")
        .build()
        .context("create decodebin")?;
    let convert = gst::ElementFactory::make("videoconvert")
        .build()
        .context("create videoconvert")?;
    let rgba_filter = gst::ElementFactory::make("capsfilter")
        .property(
            "caps",
            gst::Caps::builder("video/x-raw")
                .field("format", "RGBA")
                .build(),
        )
        .build()
        .context("create RGBA capsfilter")?;
    let scale = gst::ElementFactory::make("videoscale")
        .build()
        .context("create videoscale")?;
    let size_filter = gst::ElementFactory::make("capsfilter")
        .property(
            "caps",
            gst::Caps::builder("video/x-raw")
                .field("format", "RGBA")
                .field("width", scaled_w as i32)
                .field("height", scaled_h as i32)
                .build(),
        )
        .build()
        .context("create size capsfilter")?;
    let appsink = gst::ElementFactory::make("appsink")
        .property("sync", false)
        .property("emit-signals", false)
        .property("max-buffers", 1u32)
        .property("drop", true)
        .build()
        .context("create appsink")?;
    let appsink = appsink
        .downcast::<AppSink>()
        .map_err(|_| anyhow!("appsink downcast failed"))?;
    let sink_element = appsink.clone().upcast::<gst::Element>();

    pipeline.add_many([
        &filesrc,
        &decodebin,
        &convert,
        &rgba_filter,
        &scale,
        &size_filter,
        &sink_element,
    ])?;
    gst::Element::link_many([&convert, &rgba_filter, &scale, &size_filter, &sink_element])?;
    filesrc.link(&decodebin)?;
    let convert_for_pad = convert.clone();
    decodebin.connect_pad_added(move |_, source_pad| {
        if let Some(sink_pad) = convert_for_pad.static_pad("sink") {
            if !sink_pad.is_linked() {
                let _ = source_pad.link(&sink_pad);
            }
        }
    });

    pipeline.set_state(gst::State::Paused)?;
    pipeline
        .state(gst::ClockTime::from_seconds(10))
        .0
        .map_err(|error| anyhow!("pipeline preroll failed: {error:?}"))?;
    pipeline.seek_simple(
        gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
        gst::ClockTime::from_mseconds(timestamp_ms),
    )?;
    pipeline.set_state(gst::State::Playing)?;

    let timeout = if seek_timeout_ms == u64::MAX {
        gst::ClockTime::NONE
    } else {
        gst::ClockTime::from_mseconds(seek_timeout_ms.max(1)).into()
    };
    let sample = appsink
        .try_pull_sample(timeout)
        .ok_or_else(|| anyhow!("no frame arrived for timestamp {timestamp_ms} ms"))?;
    let buffer = sample.buffer().context("sample has no buffer")?;
    let pts = buffer
        .pts()
        .context("decoded frame has no presentation timestamp")?;
    let caps = sample.caps().context("sample has no negotiated caps")?;
    let structure = caps.structure(0).context("decoded frame caps are empty")?;
    let width = structure
        .get::<i32>("width")
        .context("decoded frame caps have no width")? as u32;
    let height = structure
        .get::<i32>("height")
        .context("decoded frame caps have no height")? as u32;
    let mapped = buffer
        .map_readable()
        .context("map decoded frame for sampling")?;
    let expected = width as usize * height as usize * 4;
    if mapped.size() < expected {
        bail!(
            "decoded RGBA frame is {} bytes, expected at least {expected}",
            mapped.size()
        );
    }
    let frame_sha256 = sha256_hex(&mapped[..expected]);
    let rgba = mapped[..expected].to_vec();
    drop(mapped);
    let _ = pipeline.set_state(gst::State::Null);
    Ok(FrameSample {
        landed_timestamp_ms: pts.mseconds(),
        width,
        height,
        frame_sha256,
        rgba,
    })
}
