use gstreamer as gst;
use gstreamer::prelude::*;
use serde::Serialize;
use std::sync::{Arc, Mutex};

/// Latency statistics collected by a pad probe on the sink of a
/// low-latency pipeline: for every buffer, latency = running time at
/// arrival minus the buffer PTS.
#[derive(Debug, Default, Clone, Serialize)]
pub struct LatencyStats {
    pub buffers: u64,
    pub min_ms: f64,
    pub max_ms: f64,
    pub mean_ms: f64,
}

#[derive(Default)]
struct Accum {
    count: u64,
    sum_ms: f64,
    min_ms: f64,
    max_ms: f64,
}

pub struct RunOutput {
    pub stats: LatencyStats,
    pub samples_csv: String,
}

/// Build a pipeline from a snapshot description, run it for
/// `duration_ms`, and monitor per-buffer latency. Headless: no display
/// or windowing system is required.
pub fn run_monitored(pipeline_desc: &str, duration_ms: u64) -> Result<RunOutput, String> {
    gst::init().map_err(|e| format!("gst init: {e}"))?;
    let pipeline = gst::parse::launch(pipeline_desc)
        .map_err(|e| format!("parse pipeline '{pipeline_desc}': {e}"))?
        .downcast::<gst::Pipeline>()
        .map_err(|_| "top-level element is not a pipeline".to_string())?;

    let accum = Arc::new(Mutex::new(Accum::default()));
    let samples: Arc<Mutex<Vec<(u64, f64)>>> = Arc::new(Mutex::new(Vec::new()));

    let sink = pipeline
        .by_name("monitored-sink")
        .ok_or_else(|| "pipeline must name its sink 'monitored-sink'".to_string())?;
    let pad = sink
        .static_pad("sink")
        .ok_or_else(|| "monitored-sink has no sink pad".to_string())?;

    let clock = pipeline.clock();
    let accum_clone = Arc::clone(&accum);
    let samples_clone = Arc::clone(&samples);
    pad.add_probe(gst::PadProbeType::BUFFER, move |pad, info| {
        if let Some(gst::PadProbeData::Buffer(ref buffer)) = info.data {
            let pts_ns = buffer.pts().map(|p| p.nseconds()).unwrap_or(0);
            let element = pad.parent_element();
            let now_ns = element
                .as_ref()
                .and_then(|e| e.clock())
                .map(|c| c.time().nseconds())
                .unwrap_or(0);
            let base_ns = element
                .as_ref()
                .and_then(|e| e.base_time())
                .map(|t| t.nseconds())
                .unwrap_or(0);
            let running_ns = now_ns.saturating_sub(base_ns);
            let latency_ms = (running_ns.saturating_sub(pts_ns)) as f64 / 1e6;
            let mut acc = accum_clone.lock().unwrap();
            acc.count += 1;
            acc.sum_ms += latency_ms;
            if acc.count == 1 || latency_ms < acc.min_ms {
                acc.min_ms = latency_ms;
            }
            if latency_ms > acc.max_ms {
                acc.max_ms = latency_ms;
            }
            samples_clone
                .lock()
                .unwrap()
                .push((running_ns / 1_000_000, latency_ms));
        }
        gst::PadProbeReturn::Ok
    });

    pipeline
        .set_state(gst::State::Playing)
        .map_err(|e| format!("set playing: {e}"))?;
    std::thread::sleep(std::time::Duration::from_millis(duration_ms));
    pipeline
        .set_state(gst::State::Null)
        .map_err(|e| format!("set null: {e}"))?;
    let _ = clock;

    let acc = accum.lock().unwrap();
    let stats = LatencyStats {
        buffers: acc.count,
        min_ms: acc.min_ms,
        max_ms: acc.max_ms,
        mean_ms: if acc.count > 0 {
            acc.sum_ms / acc.count as f64
        } else {
            0.0
        },
    };
    let mut csv = String::from("running_time_ms,latency_ms\n");
    for (t, l) in samples.lock().unwrap().iter() {
        csv.push_str(&format!("{t},{l:.3}\n"));
    }
    Ok(RunOutput {
        stats,
        samples_csv: csv,
    })
}
