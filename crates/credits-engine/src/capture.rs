use std::io::{BufReader, BufWriter};
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use gstreamer as gst;
use gstreamer_app::prelude::*;
use gstreamer_video as gst_video;

use crate::gst_support::ensure_gst;

#[derive(Debug, Clone)]
pub struct CapturedFrame {
    pub width: u32,
    pub height: u32,
    pub pts_ns: Option<u64>,
    pub fps: Option<f64>,
    pub rgba: Vec<u8>,
}

pub fn capture_frame(path: &Path, seconds: f64) -> Result<CapturedFrame> {
    ensure_gst()?;
    let uri = gst::glib::filename_to_uri(path, None)
        .map_err(|error| anyhow!("cannot build URI for {}: {error}", path.display()))?;
    let pipeline = gst::Pipeline::new();
    let decode = gst::ElementFactory::make("uridecodebin")
        .property("uri", uri.as_str())
        .build()
        .context("uridecodebin is unavailable")?;
    let convert = gst::ElementFactory::make("videoconvert")
        .build()
        .context("videoconvert is unavailable")?;
    let caps = gst::Caps::builder("video/x-raw")
        .field("format", "RGBA")
        .build();
    let capsfilter = gst::ElementFactory::make("capsfilter")
        .property("caps", &caps)
        .build()
        .context("capsfilter is unavailable")?;
    let sink = gstreamer_app::AppSink::builder()
        .max_buffers(1)
        .drop(true)
        .sync(false)
        .build();
    let sink_element = sink.clone().upcast::<gst::Element>();
    pipeline.add_many([&decode, &convert, &capsfilter, &sink_element])?;
    gst::Element::link_many([&convert, &capsfilter, &sink_element])?;

    let convert_for_pad = convert.clone();
    decode.connect_pad_added(move |_, pad| {
        let caps = pad.current_caps().or_else(|| Some(pad.query_caps(None)));
        let is_video = caps
            .as_ref()
            .and_then(|caps| caps.structure(0))
            .map(|structure| structure.name().starts_with("video/"))
            .unwrap_or_else(|| pad.name().starts_with("video"));
        if is_video {
            if let Some(sink_pad) = convert_for_pad.static_pad("sink") {
                let _ = pad.link(&sink_pad);
            }
        }
    });

    pipeline.set_state(gst::State::Paused)?;
    let (state_result, current, _) = pipeline.state(gst::ClockTime::from_seconds(15));
    if state_result.is_err() || current != gst::State::Paused {
        let bus = pipeline.bus();
        if let Some(bus) = bus {
            crate::gst_support::error_from_bus(
                &bus,
                gst::ClockTime::from_mseconds(500),
                "frame preroll",
            )?;
        }
        let _ = pipeline.set_state(gst::State::Null);
        return Err(anyhow!(
            "pipeline did not preroll for {} (state {current:?})",
            path.display()
        ));
    }
    let seek_result = pipeline.seek_simple(
        gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
        gst::ClockTime::from_nseconds((seconds.max(0.0) * 1e9) as u64),
    );
    if let Err(error) = seek_result {
        let _ = pipeline.set_state(gst::State::Null);
        return Err(anyhow!(
            "cannot seek {} to {seconds:.3}s: {error}",
            path.display()
        ));
    }
    pipeline.set_state(gst::State::Playing)?;

    let sample = sink.try_pull_sample(gst::ClockTime::from_seconds(20));
    let _ = pipeline.set_state(gst::State::Null);
    let Some(sample) = sample else {
        let bus = pipeline.bus();
        if let Some(bus) = bus {
            crate::gst_support::error_from_bus(
                &bus,
                gst::ClockTime::from_mseconds(500),
                "frame capture",
            )?;
        }
        return Err(anyhow!(
            "no video frame at {seconds:.3}s in {} (the object may be audio-only or unseekable)",
            path.display()
        ));
    };

    let caps = sample
        .caps()
        .ok_or_else(|| anyhow!("frame sample has no caps"))?;
    let info = gst_video::VideoInfo::from_caps(caps)
        .map_err(|error| anyhow!("unsupported frame caps: {error}"))?;
    if info.format() != gst_video::VideoFormat::Rgba {
        return Err(anyhow!("expected RGBA frame, got {:?}", info.format()));
    }
    let width = info.width();
    let height = info.height();
    let stride = info.stride()[0] as usize;
    let buffer = sample
        .buffer()
        .ok_or_else(|| anyhow!("frame sample has no buffer"))?;
    let map = buffer
        .map_readable()
        .map_err(|error| anyhow!("cannot map frame buffer: {error}"))?;
    let bytes = map.as_slice();
    let row_bytes = width as usize * 4;
    let needed = stride
        .checked_mul(height as usize)
        .ok_or_else(|| anyhow!("frame dimensions overflow"))?;
    if bytes.len() < needed || stride < row_bytes {
        return Err(anyhow!(
            "frame buffer is too small: {} bytes, expected {needed}",
            bytes.len()
        ));
    }
    let mut rgba = Vec::with_capacity(row_bytes * height as usize);
    for row in 0..height as usize {
        let start = row * stride;
        rgba.extend_from_slice(&bytes[start..start + row_bytes]);
    }

    let fps = caps
        .structure(0)
        .and_then(|structure| structure.get::<gst::Fraction>("framerate").ok())
        .filter(|fraction| fraction.denom() != 0)
        .map(|fraction| fraction.numer() as f64 / fraction.denom() as f64);

    Ok(CapturedFrame {
        width,
        height,
        pts_ns: buffer.pts().map(|pts| pts.nseconds()),
        fps,
        rgba,
    })
}

pub fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<()> {
    let expected = width as usize * height as usize * 4;
    if rgba.len() != expected {
        return Err(anyhow!(
            "RGBA buffer has {} bytes, expected {expected}",
            rgba.len()
        ));
    }
    let file =
        std::fs::File::create(path).with_context(|| format!("cannot create {}", path.display()))?;
    let writer = BufWriter::new(file);
    let mut encoder = png::Encoder::new(writer, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(rgba)?;
    Ok(())
}

pub fn read_png_rgba(path: &Path) -> Result<(u32, u32, Vec<u8>)> {
    let decoder = png::Decoder::new(BufReader::new(std::fs::File::open(path)?));
    let mut reader = decoder.read_info()?;
    let mut buffer = vec![
        0;
        reader
            .output_buffer_size()
            .ok_or_else(|| anyhow!("PNG decoder did not report an output size"))?
    ];
    let info = reader.next_frame(&mut buffer)?;
    buffer.truncate(info.buffer_size());
    if info.color_type != png::ColorType::Rgba {
        return Err(anyhow!("{} is not RGBA", path.display()));
    }
    Ok((info.width, info.height, buffer))
}
