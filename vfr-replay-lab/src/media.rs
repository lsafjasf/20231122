//! GStreamer 1.28 media layer.
//!
//! Three capabilities back the experiment:
//! - [`build_vfr_fixture`]: synthesize a real WebM whose frames carry
//!   irregular PTS values (a genuine variable-frame-rate clip).
//! - [`probe_pts`]: decode a clip and report every frame PTS as evidence.
//! - [`run_preview`]: run the preview pipeline (videorate normalizes the VFR
//!   stream to the engine-selected rate) and count delivered frames.

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app::AppSrc;
use gstreamer_video::VideoFormat;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub struct MediaConfig {
    pub uri: String,
    pub fps_num: i32,
    pub fps_den: i32,
}

/// Result of a headless preview run — the machine-checkable media evidence.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PreviewResult {
    pub frames_in: u64,
    pub frames_out: u64,
    pub observed_pts_ns: Vec<u64>,
    pub output_pts_ns: Vec<u64>,
    pub success: bool,
    pub error: Option<String>,
}

/// Irregular per-frame durations (nanoseconds) used to synthesize VFR clips.
/// Pattern: two short frames then one long frame, repeating.
pub const VFR_DELTAS_NS: [u64; 6] = [
    33_000_000, 33_000_000, 67_000_000, 33_000_000, 33_000_000, 67_000_000,
];

pub fn init() -> Result<(), String> {
    gst::init().map_err(|e| format!("gst::init failed: {e}"))
}

/// Build a VFR WebM fixture. Frames are pushed from `appsrc` with irregular
/// PTS; `matroskamux` preserves the timestamps, producing a true VFR file.
pub fn build_vfr_fixture(path: &str, width: i32, height: i32) -> Result<Vec<u64>, String> {
    init()?;
    let pts = pts_from_deltas(&VFR_DELTAS_NS);
    let frames = pts.len();

    let pipeline = gst::Pipeline::new();
    let appsrc = gst::ElementFactory::make("appsrc")
        .property("is-live", false)
        .property("format", gst::Format::Time)
        .build()
        .map_err(|e| format!("appsrc: {e}"))?;
    let enc = gst::ElementFactory::make("vp8enc")
        .property("deadline", 1i64)
        .build()
        .map_err(|e| format!("vp8enc: {e}"))?;
    let convert = gst::ElementFactory::make("videoconvert")
        .build()
        .map_err(|e| format!("videoconvert: {e}"))?;
    let mux = gst::ElementFactory::make("webmmux")
        .property("streamable", true)
        .build()
        .map_err(|e| format!("webmmux: {e}"))?;
    let sink = gst::ElementFactory::make("filesink")
        .property("location", path)
        .build()
        .map_err(|e| format!("filesink: {e}"))?;
    pipeline
        .add_many([&appsrc, &convert, &enc, &mux, &sink])
        .map_err(|e| format!("add: {e}"))?;
    gst::Element::link_many([&appsrc, &convert, &enc, &mux, &sink])
        .map_err(|e| format!("link: {e}"))?;

    // Deliberately no framerate field: VFR streams have no single rate, and
    // an unfixed (ranged) framerate would make the caps non-negotiable.
    let caps = gst::Caps::builder("video/x-raw")
        .field("format", VideoFormat::Rgbx.to_str())
        .field("width", width)
        .field("height", height)
        .build();
    let appsrc = appsrc.dynamic_cast::<AppSrc>().unwrap();
    appsrc.set_caps(Some(&caps));
    push_frames(&appsrc, &pts, width, height)?;
    run_to_completion(&pipeline)?;
    debug_assert!(frames >= 2);
    Ok(pts)
}

/// Decode a clip and collect every frame PTS — evidence of VFR timing.
pub fn probe_pts(uri: &str) -> Result<Vec<u64>, String> {
    init()?;
    let pipeline = gst::Pipeline::new();
    let src = make_file_src(uri)?;
    let decode = gst::ElementFactory::make("decodebin")
        .build()
        .map_err(|e| format!("decodebin: {e}"))?;
    let sink = gst::ElementFactory::make("fakesink")
        .property("sync", false)
        .build()
        .map_err(|e| format!("fakesink: {e}"))?;
    pipeline
        .add_many([&src, &decode, &sink])
        .map_err(|e| format!("add: {e}"))?;
    gst::Element::link(&src, &decode).map_err(|e| format!("link src: {e}"))?;

    let sink_weak = sink.downgrade();
    decode.connect_pad_added(move |_dbin, pad| {
        if let Some(sink) = sink_weak.upgrade() {
            let sink_pad = sink.static_pad("sink").unwrap();
            let _ = pad.link(&sink_pad);
        }
    });

    let pts = Arc::new(Mutex::new(Vec::new()));
    let pts_cb = Arc::clone(&pts);
    sink.static_pad("sink")
        .unwrap()
        .add_probe(gst::PadProbeType::BUFFER, move |_pad, info| {
            if let Some(buffer) = info.buffer() {
                pts_cb
                    .lock()
                    .unwrap()
                    .push(buffer.pts().unwrap().nseconds());
            }
            gst::PadProbeReturn::Ok
        });

    run_to_completion(&pipeline)?;
    let collected = pts.lock().unwrap().clone();
    Ok(collected)
}

/// Run the preview pipeline. `videorate` converts VFR input into a constant
/// output rate chosen by the engine; both PTS sequences are captured.
pub fn run_preview(uri: &str, fps_num: i32, fps_den: i32) -> Result<PreviewResult, String> {
    init()?;
    let pipeline = gst::Pipeline::new();
    let src = make_file_src(uri)?;
    let decode = gst::ElementFactory::make("decodebin")
        .build()
        .map_err(|e| format!("decodebin: {e}"))?;
    let rate = gst::ElementFactory::make("videorate")
        .build()
        .map_err(|e| format!("videorate: {e}"))?;
    let capsfilter = gst::ElementFactory::make("capsfilter")
        .property(
            "caps",
            gst::Caps::builder("video/x-raw")
                .field("framerate", gst::Fraction::new(fps_num, fps_den))
                .build(),
        )
        .build()
        .map_err(|e| format!("capsfilter: {e}"))?;
    let sink = gst::ElementFactory::make("fakesink")
        .property("sync", false)
        .build()
        .map_err(|e| format!("fakesink: {e}"))?;
    pipeline
        .add_many([&src, &decode, &rate, &capsfilter, &sink])
        .map_err(|e| format!("add: {e}"))?;
    gst::Element::link(&src, &decode).map_err(|e| format!("link src: {e}"))?;
    gst::Element::link_many([&rate, &capsfilter, &sink]).map_err(|e| format!("link rate: {e}"))?;

    let rate_sink = rate.static_pad("sink").unwrap();
    let rate_src = rate.static_pad("src").unwrap();
    let in_pts = Arc::new(Mutex::new(Vec::new()));
    let out_pts = Arc::new(Mutex::new(Vec::new()));
    let in_cb = Arc::clone(&in_pts);
    let out_cb = Arc::clone(&out_pts);
    rate_sink.add_probe(gst::PadProbeType::BUFFER, move |_p, info| {
        if let Some(b) = info.buffer() {
            in_cb.lock().unwrap().push(b.pts().unwrap().nseconds());
        }
        gst::PadProbeReturn::Ok
    });
    rate_src.add_probe(gst::PadProbeType::BUFFER, move |_p, info| {
        if let Some(b) = info.buffer() {
            out_cb.lock().unwrap().push(b.pts().unwrap().nseconds());
        }
        gst::PadProbeReturn::Ok
    });

    let rate_weak = rate.downgrade();
    decode.connect_pad_added(move |_dbin, pad| {
        if let Some(rate) = rate_weak.upgrade() {
            let sink_pad = rate.static_pad("sink").unwrap();
            let _ = pad.link(&sink_pad);
        }
    });

    match run_to_completion(&pipeline) {
        Ok(()) => {
            let observed = in_pts.lock().unwrap().clone();
            let output = out_pts.lock().unwrap().clone();
            Ok(PreviewResult {
                frames_in: observed.len() as u64,
                frames_out: output.len() as u64,
                observed_pts_ns: observed,
                output_pts_ns: output,
                success: true,
                error: None,
            })
        }
        Err(e) => Ok(PreviewResult {
            frames_in: 0,
            frames_out: 0,
            observed_pts_ns: Vec::new(),
            output_pts_ns: Vec::new(),
            success: false,
            error: Some(e),
        }),
    }
}

fn pts_from_deltas(deltas: &[u64]) -> Vec<u64> {
    let mut pts = Vec::with_capacity(deltas.len() + 1);
    let mut t = 0u64;
    pts.push(t);
    for d in deltas {
        t += d;
        pts.push(t);
    }
    pts
}

fn make_file_src(uri_or_path: &str) -> Result<gst::Element, String> {
    let location = uri_or_path.strip_prefix("file://").unwrap_or(uri_or_path);
    gst::ElementFactory::make("filesrc")
        .property("location", location)
        .build()
        .map_err(|e| format!("filesrc: {e}"))
}

fn push_frames(appsrc: &AppSrc, pts: &[u64], width: i32, height: i32) -> Result<(), String> {
    let frame_size = (width * height * 4) as usize;
    let deltas = pts.windows(2).map(|w| w[1] - w[0]).collect::<Vec<_>>();
    for (idx, timestamp) in pts.iter().enumerate() {
        let mut buffer =
            gst::Buffer::with_size(frame_size).map_err(|e| format!("buffer alloc: {e}"))?;
        {
            let buf = buffer.get_mut().unwrap();
            buf.set_pts(gst::ClockTime::from_nseconds(*timestamp));
            let duration = deltas.get(idx).copied().unwrap_or(VFR_DELTAS_NS[0]);
            buf.set_duration(gst::ClockTime::from_nseconds(duration));
            let mut map = buf.map_writable().unwrap();
            let shade = ((idx as u8).wrapping_mul(37)).wrapping_add(12);
            map.as_mut_slice().fill(shade);
        }
        appsrc
            .push_buffer(buffer)
            .map_err(|e| format!("push_buffer: {e}"))?;
    }
    appsrc
        .end_of_stream()
        .map(|_| ())
        .map_err(|e| format!("eos: {e}"))
}

fn run_to_completion(pipeline: &gst::Pipeline) -> Result<(), String> {
    let bus = pipeline.bus().ok_or("pipeline has no bus")?;
    pipeline
        .set_state(gst::State::Playing)
        .map_err(|e| format!("set_state Playing: {e}"))?;
    for msg in bus.iter_timed(gst::ClockTime::NONE) {
        use gst::MessageView::*;
        match msg.view() {
            Eos(_) => break,
            Error(err) => {
                let _ = pipeline.set_state(gst::State::Null);
                return Err(format!("{:?}", err.error()));
            }
            _ => {}
        }
    }
    pipeline
        .set_state(gst::State::Null)
        .map_err(|e| format!("set_state Null: {e}"))?;
    Ok(())
}
