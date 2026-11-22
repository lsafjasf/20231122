//! Reload containers through GStreamer's qtdemux and count the tracks that
//! actually initialize, independently of our own box walker.

use std::path::Path;
use std::sync::{Arc, Mutex};

use gstreamer as gst;
use gstreamer::prelude::*;

#[derive(Clone, Debug)]
pub struct ReloadResult {
    pub track_count: usize,
    pub track_kinds: Vec<String>,
    pub success: bool,
    pub error_message: Option<String>,
}

struct Collector {
    kinds: Vec<String>,
    error: Option<String>,
    eos: bool,
}

/// Initialize GStreamer once per process.
pub fn init() -> anyhow::Result<()> {
    static INIT: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
    INIT.get_or_init(|| gst::init().map_err(|e| e.to_string()))
        .clone()
        .map_err(anyhow::Error::msg)
}

/// Demux `path` with `filesrc ! qtdemux ! fakesink` and count the pads that
/// qtdemux exposes. Each pad corresponds to one track.
pub fn reload_and_count(path: &Path) -> anyhow::Result<ReloadResult> {
    init()?;

    let collector = Arc::new(Mutex::new(Collector {
        kinds: Vec::new(),
        error: None,
        eos: false,
    }));

    let pipeline = gst::Pipeline::new();
    let src = gst::ElementFactory::make("filesrc")
        .property("location", path.to_string_lossy().as_ref())
        .build()?;
    let demux = gst::ElementFactory::make("qtdemux").build()?;
    pipeline.add_many([&src, &demux])?;
    gst::Element::link(&src, &demux)?;

    let pad_collector = Arc::clone(&collector);
    let pipeline_for_pads = pipeline.clone();
    demux.connect_pad_added(move |_demux, pad| {
        let kind = guess_pad_kind(pad);
        let sink = match gst::ElementFactory::make("fakesink")
            .name(format!("sink-{}", pad.name()))
            .build()
        {
            Ok(e) => e,
            Err(e) => {
                pad_collector.lock().unwrap().error =
                    Some(format!("fakesink creation failed: {e}"));
                return;
            }
        };
        let _ = sink.set_property("sync", false);
        let bin = pipeline_for_pads.clone();
        if bin.add(&sink).is_err() {
            return;
        }
        let sink_pad = sink.static_pad("sink").expect("fakesink has sink pad");
        if let Err(e) = pad.link(&sink_pad) {
            pad_collector
                .lock()
                .unwrap()
                .error
                .get_or_insert_with(|| format!("pad link failed: {e}"));
            return;
        }
        sink.sync_state_with_parent().ok();
        pad_collector.lock().unwrap().kinds.push(kind);
    });

    let bus_collector = Arc::clone(&collector);
    let bus = pipeline.bus().expect("pipeline has a bus");
    let _ = bus.add_watch(move |_, msg| {
        use gst::MessageView;
        match msg.view() {
            MessageView::Error(e) => {
                let mut c = bus_collector.lock().unwrap();
                if c.error.is_none() {
                    c.error = Some(format!("{}: {}", e.error(), e.debug().unwrap_or_default()));
                }
                glib::ControlFlow::Break
            }
            MessageView::Eos(_) => {
                bus_collector.lock().unwrap().eos = true;
                glib::ControlFlow::Break
            }
            _ => glib::ControlFlow::Continue,
        }
    })?;

    pipeline.set_state(gst::State::Playing)?;
    let deadline = gst::ClockTime::from_seconds(15);
    while let Some(msg) = bus.timed_pop(deadline) {
        match msg.view() {
            gst::MessageView::Error(_) | gst::MessageView::Eos(_) => break,
            _ => continue,
        }
    }
    let state = pipeline.set_state(gst::State::Null)?;
    debug_assert!(matches!(
        state,
        gst::StateChangeSuccess::Success
            | gst::StateChangeSuccess::Async
            | gst::StateChangeSuccess::NoPreroll
    ));

    let c = collector.lock().unwrap();
    Ok(ReloadResult {
        track_count: c.kinds.len(),
        track_kinds: c.kinds.clone(),
        success: c.error.is_none(),
        error_message: c.error.clone(),
    })
}

fn guess_pad_kind(pad: &gst::Pad) -> String {
    let caps = match pad.current_caps() {
        Some(c) => c,
        None => pad.query_caps(None),
    };
    let structure = caps.structure(0);
    let name = structure.map(|s| s.name().as_str()).unwrap_or("");
    if name.starts_with("video/") {
        "video".to_string()
    } else if name.starts_with("audio/") {
        "audio".to_string()
    } else if name.starts_with("text/") || name.contains("sub") {
        "subtitle".to_string()
    } else {
        name.split('/').next().unwrap_or("unknown").to_string()
    }
}
