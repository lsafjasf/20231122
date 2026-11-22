use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{anyhow, Context, Result};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app::AppSrc;

use crate::layout::CreditsLayout;
use crate::model::{CreditsSample, LayoutConfig};
use crate::render;

static INIT: OnceLock<Result<(), String>> = OnceLock::new();

pub fn init() -> Result<()> {
    let status = INIT.get_or_init(|| gst::init().map_err(|error| error.to_string()));
    status.clone().map_err(|error| anyhow!(error))
}

pub fn version() -> String {
    init().map(|_| gst::version_string().to_string()).unwrap_or_else(|_| "unavailable".into())
}

pub struct Transport {
    clock: gst::Clock,
    anchor_clock: Option<gst::ClockTime>,
    anchor_position: f64,
    playing: bool,
}

impl Transport {
    pub fn new() -> Result<Self> {
        init()?;
        Ok(Self {
            clock: gst::SystemClock::obtain(),
            anchor_clock: None,
            anchor_position: 0.0,
            playing: false,
        })
    }

    pub fn position(&self) -> f64 {
        if !self.playing {
            return self.anchor_position;
        }
        let Some(anchor) = self.anchor_clock else {
            return self.anchor_position;
        };
        let elapsed = self.clock.time().saturating_sub(anchor);
        self.anchor_position + elapsed.nseconds() as f64 / 1_000_000_000.0
    }

    pub fn play(&mut self) {
        if self.playing {
            return;
        }
        self.anchor_clock = Some(self.clock.time());
        self.playing = true;
    }

    pub fn pause(&mut self) {
        if !self.playing {
            return;
        }
        self.anchor_position = self.position();
        self.anchor_clock = None;
        self.playing = false;
    }

    pub fn seek(&mut self, seconds: f64) {
        self.anchor_position = seconds.max(0.0);
        if self.playing {
            self.anchor_clock = Some(self.clock.time());
        }
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }
}

pub fn export_video(
    path: impl AsRef<Path>,
    layout: &CreditsLayout,
    width: u32,
    height: u32,
    fps: u32,
) -> Result<PathBuf> {
    init()?;
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _ = std::fs::remove_file(path);

    let pipeline = gst::Pipeline::new();
    let caps = gst::Caps::builder("video/x-raw")
        .field("format", "BGRA")
        .field("width", width as i32)
        .field("height", height as i32)
        .field("framerate", gst::Fraction::new(fps as i32, 1))
        .build();
    let source = AppSrc::builder()
        .caps(&caps)
        .format(gst::Format::Time)
        .is_live(false)
        .build();
    source.set_property("block", true);
    source.set_property("max-bytes", 16_u64 * 1024 * 1024);
    let convert = gst::ElementFactory::make("videoconvert")
        .build()
        .context("missing GStreamer videoconvert")?;
    let encoder = gst::ElementFactory::make("openh264enc")
        .build()
        .context("missing GStreamer openh264enc")?;
    encoder.set_property("bitrate", 6_000_000_u32);
    encoder.set_property_from_str("complexity", "high");
    encoder.set_property("gop-size", 60_u32);
    let parser = gst::ElementFactory::make("h264parse")
        .build()
        .context("missing GStreamer h264parse")?;
    let mux = gst::ElementFactory::make("mp4mux")
        .build()
        .context("missing GStreamer mp4mux")?;
    let sink = gst::ElementFactory::make("filesink")
        .build()
        .context("missing GStreamer filesink")?;
    sink.set_property("location", path.to_string_lossy().to_string());
    pipeline
        .add_many([
            source.upcast_ref(),
            &convert,
            &encoder,
            &parser,
            &mux,
            &sink,
        ])
        .map_err(|error| anyhow!("cannot assemble GStreamer export: {error}"))?;
    gst::Element::link_many([
        source.upcast_ref(),
        &convert,
        &encoder,
        &parser,
        &mux,
        &sink,
    ])
    .map_err(|error| anyhow!("cannot link GStreamer export: {error}"))?;
    pipeline
        .set_state(gst::State::Playing)
        .context("cannot start GStreamer export")?;

    let scale_x = width as f64 / layout.canvas.width;
    let scale_y = height as f64 / layout.canvas.height;
    let scale = scale_x.min(scale_y);
    let frame_count = (layout.duration * fps as f64).ceil().max(1.0) as u32;
    for index in 0..frame_count {
        let time = index as f64 / fps as f64;
        let frame = render::render_proof(layout, time, scale, false)?;
        let buffer = frame_to_buffer(&frame, index, fps)?;
        source
            .push_buffer(buffer)
            .map_err(|error| anyhow!("GStreamer rejected video frame {index}: {error}"))?;
    }
    source
        .end_of_stream()
        .map_err(|error| anyhow!("cannot finish GStreamer appsrc: {error}"))?;

    let bus = pipeline
        .bus()
        .ok_or_else(|| anyhow!("GStreamer export pipeline has no bus"))?;
    let message = bus
        .timed_pop_filtered(
            gst::ClockTime::from_seconds(180),
            &[gst::MessageType::Error, gst::MessageType::Eos],
        )
        .ok_or_else(|| anyhow!("timed out waiting for GStreamer export"))?;
    let result = match message.view() {
        gst::MessageView::Error(error) => Err(anyhow!(
            "GStreamer export error: {} ({})",
            error.error(),
            error.debug().map(|value| value.to_string()).unwrap_or_default()
        )),
        gst::MessageView::Eos(_) => Ok(()),
        _ => Err(anyhow!("unexpected GStreamer export message")),
    };
    let _ = pipeline.set_state(gst::State::Null);
    result?;
    Ok(path.to_path_buf())
}

fn frame_to_buffer(frame: &render::Frame, index: u32, fps: u32) -> Result<gst::Buffer> {
    let packed = pack_rows(&frame.data, frame.stride, frame.width as usize, frame.height as usize);
    let mut buffer = gst::Buffer::with_size(packed.len())
        .map_err(|error| anyhow!("cannot allocate GStreamer frame: {error}"))?;
    {
        let buffer_ref = buffer
            .get_mut()
            .ok_or_else(|| anyhow!("cannot access GStreamer frame"))?;
        {
            let mut map = buffer_ref
                .map_writable()
                .map_err(|error| anyhow!("cannot map GStreamer frame: {error}"))?;
            map.copy_from_slice(&packed);
        }
        buffer_ref.set_pts(gst::ClockTime::from_nseconds(
            index as u64 * 1_000_000_000 / fps as u64,
        ));
        buffer_ref.set_duration(gst::ClockTime::from_nseconds(
            1_000_000_000 / fps as u64,
        ));
    }
    Ok(buffer)
}

fn pack_rows(data: &[u8], stride: usize, width: usize, height: usize) -> Vec<u8> {
    let row_bytes = width * 4;
    if stride == row_bytes {
        return data[..row_bytes * height].to_vec();
    }
    let mut packed = Vec::with_capacity(row_bytes * height);
    for row in 0..height {
        let start = row * stride;
        packed.extend_from_slice(&data[start..start + row_bytes]);
    }
    packed
}

pub fn export_demo_video(dir: impl AsRef<Path>, sample: &CreditsSample) -> Result<PathBuf> {
    let config = LayoutConfig::default();
    let layout = crate::layout::build(sample, &config)?;
    export_video(dir.as_ref().join("credits-long.mp4"), &layout, 1280, 720, 30)
}
