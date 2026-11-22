use std::path::Path;

use anyhow::{Context, Result};
use gstreamer::prelude::*;
use gstreamer_app::{AppSink, AppSrc};
use serde::{Deserialize, Serialize};

use crate::model::{format_timecode, Config, HEIGHT, WIDTH};
use crate::source::{crc32, frame_rgb};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PipelineProbe {
    pub status: String,
    pub backend: String,
    pub elements: Vec<String>,
    pub caps: String,
    pub queue_max_buffers: u32,
    pub queue_max_bytes: u64,
    pub input_frames: usize,
    pub output_frames: usize,
    pub crc_match: bool,
    pub samples: Vec<PipelineSample>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PipelineSample {
    pub frame_idx: u64,
    pub pts_ns: u64,
    pub timecode: String,
    pub input_crc: u32,
    pub output_crc: u32,
    pub match_input: bool,
}

pub fn probe(config: &Config, evidence_dir: &Path) -> Result<PipelineProbe> {
    gstreamer::init().context("initialize GStreamer")?;
    let description = format!(
        "appsrc name=src is-live=false format=time \
         caps=video/x-raw,format=RGB,width={WIDTH},height={HEIGHT},framerate={}/1 \
         ! queue max-size-buffers={} max-size-bytes={} max-size-time=0 leaky=downstream \
         ! videoconvert \
         ! appsink name=sink sync=false async=false emit-signals=false",
        config.fps, config.extreme.queue_max_buffers, config.extreme.queue_max_bytes
    );
    let pipeline = gstreamer::parse::launch(&description)
        .context("parse GStreamer pipeline")?
        .downcast::<gstreamer::Pipeline>()
        .map_err(|_| anyhow::anyhow!("launch description did not produce a pipeline"))?;
    let source = pipeline
        .by_name("src")
        .context("appsrc missing")?
        .downcast::<AppSrc>()
        .map_err(|_| anyhow::anyhow!("src is not appsrc"))?;
    let sink = pipeline
        .by_name("sink")
        .context("appsink missing")?
        .downcast::<AppSink>()
        .map_err(|_| anyhow::anyhow!("sink is not appsink"))?;

    pipeline.set_state(gstreamer::State::Playing)?;
    let input_indices = [
        config.boundary_before_idx(),
        config.boundary_before_idx().saturating_add(1),
        config.boundary_after_idx(),
    ];
    let mut expected = Vec::new();
    for frame_idx in input_indices {
        let pts_ns =
            config
                .nominal_pts_ns(frame_idx)
                .saturating_add_signed(crate::source::jitter_ns(
                    frame_idx,
                    if config.in_extreme_window(frame_idx) {
                        config.extreme.jitter_ns
                    } else {
                        0
                    },
                ));
        let rgb = frame_rgb(frame_idx);
        let crc = crc32(&rgb);
        let mut buffer = gstreamer::Buffer::from_slice(rgb);
        {
            let mutable = buffer.get_mut().context("buffer unexpectedly shared")?;
            mutable.set_pts(gstreamer::ClockTime::from_nseconds(pts_ns));
            mutable.set_duration(gstreamer::ClockTime::from_nseconds(
                config.frame_period_ns(),
            ));
        }
        source
            .push_buffer(buffer)
            .with_context(|| format!("push frame {frame_idx}"))?;
        expected.push((frame_idx, pts_ns, crc));
    }
    source.end_of_stream()?;

    let mut samples = Vec::new();
    let timeout = gstreamer::ClockTime::from_seconds(5);
    for (frame_idx, pts_ns, input_crc) in expected {
        let sample = sink
            .try_pull_sample(timeout)
            .with_context(|| format!("timed out waiting for frame {frame_idx}"))?;
        let buffer = sample.buffer().context("sample has no buffer")?;
        let map = buffer.map_readable().context("map output buffer")?;
        let output_crc = crc32(map.as_slice());
        samples.push(PipelineSample {
            frame_idx,
            pts_ns,
            timecode: format_timecode(pts_ns, config.fps),
            input_crc,
            output_crc,
            match_input: input_crc == output_crc,
        });
    }
    let caps = sink
        .static_pad("sink")
        .and_then(|pad| pad.current_caps())
        .map(|caps| caps.to_string())
        .unwrap_or_else(|| "unknown".into());
    pipeline.set_state(gstreamer::State::Null)?;

    let probe = PipelineProbe {
        status: "ok".into(),
        backend: format!("Rust / {}", gstreamer::version_string()),
        elements: vec![
            "appsrc".into(),
            "queue(leaky=downstream)".into(),
            "videoconvert".into(),
            "appsink".into(),
        ],
        caps,
        queue_max_buffers: config.extreme.queue_max_buffers,
        queue_max_bytes: config.extreme.queue_max_bytes,
        input_frames: samples.len(),
        output_frames: samples.len(),
        crc_match: samples.iter().all(|sample| sample.match_input),
        samples,
    };
    crate::evidence::write_json(&evidence_dir.join("pipeline.json"), &probe)?;
    Ok(probe)
}
