//! GStreamer-based subtitle extraction from the media object.
//!
//! The pipeline is `uridecodebin caps=text/x-raw ! appsink`, which lets
//! GStreamer auto-plug demuxers and subtitle parsers (embedded tracks in
//! MKV/MP4 as well as external .srt/.ass/.vtt files). Each appsink sample
//! becomes one `Cue` with stream-time timestamps.

use std::path::Path;

use anyhow::{Context, Result, bail};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use gstreamer_pbutils::prelude::*;

use crate::model::Cue;

pub fn init() -> Result<()> {
    gst::init().context("failed to initialize GStreamer")
}

/// Pull all subtitle cues from the media object at `path`.
pub fn extract_cues(path: &Path) -> Result<Vec<Cue>> {
    let abs = path
        .canonicalize()
        .with_context(|| format!("media object not found: {}", path.display()))?;
    let uri = gst::glib::filename_to_uri(&abs, None)
        .map_err(|e| anyhow::anyhow!("cannot build URI for {}: {e}", abs.display()))?;

    let decodebin = gst::ElementFactory::make("uridecodebin")
        .property("uri", &uri)
        .property("caps", gst::Caps::builder("text/x-raw").build())
        .build()
        .context("uridecodebin is unavailable (install gst-plugins-base/good)")?;
    let sink = gst::ElementFactory::make("appsink")
        .property("emit-signals", false)
        .build()
        .context("appsink is unavailable")?;
    let appsink = sink.clone().dynamic_cast::<gst_app::AppSink>().unwrap();

    let pipeline = gst::Pipeline::new();
    pipeline
        .add_many([&decodebin, &sink])
        .context("cannot assemble extraction pipeline")?;

    // Link each decoded subtitle pad to the appsink as it appears.
    let sink_pad = sink.static_pad("sink").unwrap();
    let pipeline_weak = pipeline.downgrade();
    decodebin.connect_pad_added(move |_db, src_pad| {
        let Some(pipeline) = pipeline_weak.upgrade() else {
            return;
        };
        if sink_pad.is_linked() {
            return;
        }
        if let Err(e) = src_pad.link(&sink_pad) {
            gst::warning!(gst::CAT_RUST, obj = pipeline, "pad link failed: {e}");
        }
    });

    pipeline
        .set_state(gst::State::Playing)
        .context("cannot start extraction pipeline")?;

    let mut cues = Vec::new();
    let mut index: u64 = 0;
    loop {
        match appsink.try_pull_sample(gst::ClockTime::from_seconds(5)) {
            Some(sample) => {
                let buffer = sample.buffer().context("sample without buffer")?;
                let start = buffer
                    .pts()
                    .map(|t| t.mseconds() as i64)
                    .unwrap_or_default();
                let end = buffer
                    .duration()
                    .map(|d| start + d.mseconds() as i64)
                    .unwrap_or(start);
                let map = buffer.map_readable().context("buffer not readable")?;
                let text = String::from_utf8_lossy(map.as_slice()).trim().to_string();
                drop(map);
                if !text.is_empty() {
                    cues.push(Cue {
                        index,
                        start_ms: start,
                        end_ms: end,
                        text,
                    });
                    index += 1;
                }
            }
            None => {
                if appsink.is_eos() {
                    break;
                }
                // Distinguish "still working" from a pipeline error.
                let bus = pipeline.bus().unwrap();
                if let Some(msg) = bus.pop_filtered(&[gst::MessageType::Error]) {
                    if let gst::MessageView::Error(err) = msg.view() {
                        pipeline.set_state(gst::State::Null).ok();
                        bail!("extraction failed: {}", err.error());
                    }
                }
            }
        }
    }

    pipeline.set_state(gst::State::Null).ok();
    if cues.is_empty() {
        bail!(
            "no subtitle cues found in {} (no text track or unsupported format)",
            abs.display()
        );
    }
    Ok(cues)
}

/// Discoverer-based stream summary, kept as capture-side evidence.
pub fn describe_object(path: &Path) -> Result<Vec<String>> {
    let abs = path.canonicalize()?;
    let uri = gst::glib::filename_to_uri(&abs, None)
        .map_err(|e| anyhow::anyhow!("cannot build URI: {e}"))?;
    let discoverer = gstreamer_pbutils::Discoverer::new(gst::ClockTime::from_seconds(10))
        .context("cannot create GStreamer Discoverer")?;
    let info = discoverer
        .discover_uri(&uri)
        .context("Discoverer failed on media object")?;
    let mut out = Vec::new();
    if let Some(top) = info.stream_info() {
        collect_streams(&top, &mut out);
    }
    Ok(out)
}

fn collect_streams(info: &gstreamer_pbutils::DiscovererStreamInfo, out: &mut Vec<String>) {
    out.push(format!(
        "{}: {}",
        info.stream_type_nick(),
        info.caps().map(|c| c.to_string()).unwrap_or_default()
    ));
    if let Some(next) = info.next() {
        collect_streams(&next, out);
    }
}
