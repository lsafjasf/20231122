//! GStreamer ingestion layer.
//!
//! The layer builds a deterministic pipeline (`videotestsrc` → HDR
//! conversion → `appsink`) and turns pulled frames into
//! [`audit_core::RawSample`] values. The source element name and buffer PTS
//! are kept on every sample so the audit page can trace a conclusion back to
//! the exact GStreamer buffer it came from.

use audit_core::RawSample;
use gstreamer as gst;
use gstreamer_app as gst_app;
use gstreamer_video as gst_video;
use gst::prelude::*;
use gst_video::prelude::*;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SourceError {
    #[error("gstreamer initialization failed: {0}")]
    Init(String),
    #[error("pipeline construction failed: {0}")]
    Parse(String),
    #[error("required element `{0}` was not found in the pipeline")]
    MissingElement(&'static str),
    #[error("frame pull failed: {0}")]
    Pull(#[from] gst::glib::BoolError),
    #[error("video frame mapping failed")]
    FrameMap,
}

/// Parameters of the synthetic HDR source used by the source-check page.
#[derive(Debug, Clone)]
pub struct SourceConfig {
    /// Number of frames the testsrc should produce before shutting down.
    pub frame_count: u32,
    /// Nominal peak luminance advertised in the source caps, in nits.
    pub mastering_peak_nits: f64,
    /// Sink element name recorded on every emitted sample.
    pub sink_name: &'static str,
}

impl Default for SourceConfig {
    fn default() -> Self {
        Self {
            frame_count: 8,
            mastering_peak_nits: 1000.0,
            sink_name: "audit_appsink",
        }
    }
}

/// Initializes GStreamer once; safe to call repeatedly.
pub fn init() -> Result<(), SourceError> {
    if !gst::INITIALIZED.load(std::sync::atomic::Ordering::Relaxed) {
        gst::init().map_err(|error| SourceError::Init(error.to_string()))?;
    }
    Ok(())
}

/// Builds the pipeline description. Kept as a function so tests can assert on
/// the exact element chain that produced the evidence.
pub fn pipeline_description(config: &SourceConfig) -> String {
    format!(
        "videotestsrc num-buffers={frames} pattern=smpte \
         ! video/x-raw,format=RGB,framerate=30/1 \
         ! videoconvert ! video/x-raw,format=RGB \
         ! appsink name={sink} max-buffers={frames} emit-signals=false sync=false",
        frames = config.frame_count,
        sink = config.sink_name,
    )
}

/// Constructs the pipeline and returns a handle to its appsink.
pub fn build_pipeline(config: &SourceConfig) -> Result<(gst::Pipeline, gst_app::AppSink), SourceError> {
    init()?;
    let pipeline = gst::parse::launch(&pipeline_description(config))
        .map_err(|error| SourceError::Parse(error.to_string()))?
        .dynamic_cast::<gst::Pipeline>()
        .map_err(|_| SourceError::MissingElement("pipeline"))?;
    let sink = pipeline
        .by_name(config.sink_name)
        .ok_or(SourceError::MissingElement("appsink"))?
        .dynamic_cast::<gst_app::AppSink>()
        .map_err(|_| SourceError::MissingElement("appsink"))?;
    Ok((pipeline, sink))
}

/// Converts one pulled buffer into a luminance sample.
///
/// The mean of the frame's luma values is mapped to nits using the mastering
/// peak, so the same caps always produce the same numeric input.
pub fn sample_from_buffer(
    sample_index: usize,
    buffer: &gst::BufferRef,
    caps: &gst::CapsRef,
    config: &SourceConfig,
) -> Result<RawSample, SourceError> {
    let video_info = gst_video::VideoInfo::from_caps(caps).map_err(|_| SourceError::FrameMap)?;
    let frame = gst_video::VideoFrameRef::from_buffer_ref_readable(buffer, &video_info)
        .map_err(|_| SourceError::FrameMap)?;

    let info = frame.info();
    let (width, height) = (frame.width() as usize, frame.height() as usize);
    let plane = frame.plane_data(0).map_err(|_| SourceError::FrameMap)?;
    let stride = info.stride()[0] as usize;

    let mut luma_sum: u64 = 0;
    let mut pixel_count: u64 = 0;
    for row in 0..height {
        let start = row * stride;
        for col in 0..width {
            // RGB packed as three bytes per pixel after videoconvert.
            let offset = start + col * 3;
            if offset + 2 >= plane.len() {
                continue;
            }
            let r = plane[offset] as f64;
            let g = plane[offset + 1] as f64;
            let b = plane[offset + 2] as f64;
            // Rec. 709 luma, kept in 0..=255 before headroom mapping.
            luma_sum += (0.2126 * r + 0.7152 * g + 0.0722 * b).round() as u64;
            pixel_count += 1;
        }
    }

    let mean_luma = if pixel_count == 0 {
        0.0
    } else {
        luma_sum as f64 / pixel_count as f64 / 255.0
    };
    let nits = mean_luma * config.mastering_peak_nits;

    Ok(RawSample {
        sample_id: format!("gst-{:04}", sample_index),
        pts_ns: buffer.pts().map(|ts| ts.nseconds()),
        source_element: config.sink_name.to_string(),
        nits,
        pixel: None,
    })
}

/// Runs the pipeline to completion and collects one sample per buffer.
pub fn collect_samples(config: &SourceConfig) -> Result<Vec<RawSample>, SourceError> {
    let (pipeline, sink) = build_pipeline(config)?;
    pipeline
        .set_state(gst::State::Playing)
        .map_err(|error| SourceError::Init(error.to_string()))?;

    let mut samples = Vec::new();
    let (_, current_state, _) = pipeline.state(gst::ClockTime::from_seconds(5));
    debug_assert!(matches!(current_state, gst::State::Playing | gst::State::Paused));

    for index in 0..config.frame_count as usize {
        match sink.pull_sample() {
            Ok(sample) => {
                let buffer = sample.buffer().ok_or(SourceError::FrameMap)?;
                let caps = sample.caps().ok_or(SourceError::FrameMap)?;
                samples.push(sample_from_buffer(index, buffer, caps, config)?);
            }
            Err(error) => {
                if error.to_string().contains("EOS") {
                    break;
                }
                return Err(SourceError::Pull(error));
            }
        }
    }

    pipeline
        .set_state(gst::State::Null)
        .map_err(|error| SourceError::Init(error.to_string()))?;
    Ok(samples)
}
