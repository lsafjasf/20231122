use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use gst::prelude::*;
use gstreamer as gst;

use crate::model::GstReplayEvidence;

#[derive(Default)]
struct ProbeStats {
    buffer_count: u64,
    decoded_bytes: u64,
    first_pts_ns: Option<u64>,
    last_pts_ns: Option<u64>,
}

pub fn replay_wav(path: &Path, expected_frames: u64) -> Result<GstReplayEvidence> {
    gst::init().context("initialize GStreamer")?;
    let pipeline = gst::Pipeline::new();
    let source = gst::ElementFactory::make("filesrc")
        .property("location", path.to_string_lossy().to_string())
        .build()
        .context("create filesrc")?;
    let parser = gst::ElementFactory::make("wavparse")
        .build()
        .context("create wavparse (install gst-plugins-good)")?;
    let converter = gst::ElementFactory::make("audioconvert")
        .build()
        .context("create audioconvert")?;
    let caps = gst::Caps::builder("audio/x-raw")
        .field("format", "S16LE")
        .field("channels", 1_i32)
        .field("rate", 48_000_i32)
        .build();
    let filter = gst::ElementFactory::make("capsfilter")
        .property("caps", &caps)
        .build()
        .context("create capsfilter")?;
    let sink = gst::ElementFactory::make("fakesink")
        .property("sync", false)
        .property("silent", true)
        .build()
        .context("create fakesink")?;

    pipeline
        .add_many([&source, &parser, &converter, &filter, &sink])
        .context("add pipeline elements")?;
    gst::Element::link_many([&source, &parser, &converter, &filter, &sink])
        .context("link pipeline elements")?;

    let stats = Arc::new(Mutex::new(ProbeStats::default()));
    let probe_stats = Arc::clone(&stats);
    let sink_pad = sink
        .static_pad("sink")
        .context("fakesink has no sink pad")?;
    sink_pad.add_probe(gst::PadProbeType::BUFFER, move |_, info| {
        if let Some(buffer) = info.buffer() {
            let mut stats = probe_stats.lock().expect("probe stats lock");
            stats.buffer_count += 1;
            stats.decoded_bytes += buffer.size() as u64;
            if let Some(pts) = buffer.pts() {
                stats.first_pts_ns.get_or_insert(pts.nseconds());
                stats.last_pts_ns = Some(pts.nseconds());
            }
        }
        gst::PadProbeReturn::Ok
    });

    pipeline
        .set_state(gst::State::Playing)
        .context("start replay pipeline")?;
    let bus = pipeline.bus().context("pipeline has no bus")?;
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut eos = false;
    let mut bus_messages = Vec::new();
    let mut bus_failed = false;

    while Instant::now() < deadline {
        let Some(message) = bus.timed_pop(gst::ClockTime::from_mseconds(100)) else {
            continue;
        };
        match message.view() {
            gst::MessageView::Eos(..) => {
                eos = true;
                break;
            }
            gst::MessageView::Error(error) => {
                bus_failed = true;
                bus_messages.push(format!(
                    "ERROR {}: {} ({})",
                    error.src().map(|src| src.path_string()).unwrap_or_default(),
                    error.error(),
                    error.debug().unwrap_or_default()
                ));
                break;
            }
            gst::MessageView::Warning(warning) => {
                bus_messages.push(format!(
                    "WARNING {}: {}",
                    warning
                        .src()
                        .map(|src| src.path_string())
                        .unwrap_or_default(),
                    warning.error()
                ));
            }
            _ => {}
        }
    }

    let sink_caps = sink
        .static_pad("sink")
        .and_then(|pad| pad.current_caps())
        .map(|caps| caps.to_string());
    let _ = pipeline.set_state(gst::State::Null);
    let stats = stats.lock().expect("probe stats lock").clone();
    let frame_bytes = 2_u64;
    let decoded_frames = if stats.decoded_bytes % frame_bytes == 0 {
        stats.decoded_bytes / frame_bytes
    } else {
        stats.decoded_bytes / frame_bytes + 1
    };
    let duration_ok = decoded_frames == expected_frames;
    let passed = eos && !bus_failed && duration_ok;

    if !eos && !bus_failed {
        bail!("GStreamer replay timed out after 15 seconds");
    }

    Ok(GstReplayEvidence {
        asset: path.to_path_buf(),
        pipeline: "filesrc ! wavparse ! audioconvert ! capsfilter(S16LE,48k,mono) ! fakesink"
            .to_owned(),
        eos,
        bus_messages,
        buffer_count: stats.buffer_count,
        decoded_bytes: stats.decoded_bytes,
        decoded_frames,
        first_pts_ns: stats.first_pts_ns,
        last_pts_ns: stats.last_pts_ns,
        sink_caps,
        expected_frames,
        duration_ok,
        passed,
    })
}

impl Clone for ProbeStats {
    fn clone(&self) -> Self {
        Self {
            buffer_count: self.buffer_count,
            decoded_bytes: self.decoded_bytes,
            first_pts_ns: self.first_pts_ns,
            last_pts_ns: self.last_pts_ns,
        }
    }
}
