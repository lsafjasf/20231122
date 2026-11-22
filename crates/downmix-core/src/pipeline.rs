//! GStreamer-backed downmix pipeline with raw sample capture for waveform
//! correlation evidence.

use crate::error::{DownmixError, Result};
use crate::model::{CorrelationPoint, ProcessingParams, WaveformSummary};
use gstreamer as gst;
use gstreamer_app as gst_app;
use gstreamer_audio as gst_audio;
use gstreamer_pbutils as gst_pbutils;
use gst::prelude::*;
use gst::{element_error, glib};
use std::path::Path;
use std::sync::{Arc, Mutex};

/// Interval between recorded correlation points, in output frames.
pub const CORRELATION_FRAME_INTERVAL: u64 = 1024;

#[derive(Debug, Clone)]
pub struct DownmixResult {
    pub sample_rate: u32,
    pub sample_format: String,
    pub input_channels: usize,
    pub output_channels: usize,
    pub total_frames: u64,
    pub duration_ms: u64,
}

/// Captured raw stereo frames plus running statistics.
#[derive(Default)]
struct Capture {
    interleaved: Vec<f32>,
    channels: usize,
    sample_rate: u32,
}

fn init_gst() -> Result<()> {
    gst::init().map_err(|e| DownmixError::Gst(e.to_string()))
}

/// Probe the first audio stream's channel count without decoding the whole
/// object. Works for sniffed/unknown-extension files because discovery is
/// content based.
pub fn probe_channels(input: &Path) -> Result<usize> {
    init_gst()?;
    let discoverer = gst_pbutils::Discoverer::new(gst::ClockTime::from_seconds(15))
        .map_err(|e| DownmixError::Gst(format!("discoverer init: {e:?}")))?;
    let uri = glib::filename_to_uri(input, None)
        .map_err(|e| DownmixError::Gst(format!("uri conversion: {e:?}")))?;
    let info = discoverer
        .discover_uri(&uri)
        .map_err(|e| {
            DownmixError::Gst(format!(
                "discovery failed for {input:?}: {e:?}"
            ))
        })?;
    for stream in info.audio_streams() {
        let channels = stream.channels();
        if channels > 0 {
            return Ok(channels as usize);
        }
    }
    Err(DownmixError::Stream(format!(
        "no audio stream found in {input:?}"
    )))
}

fn link_tee_branch(tee: &gst::Element, queue: &gst::Element) -> Result<()> {
    let src = tee.request_pad_simple("src_%u").ok_or_else(|| {
        DownmixError::Gst("tee could not allocate a request pad".to_string())
    })?;
    let sink = queue
        .static_pad("sink")
        .ok_or_else(|| DownmixError::Gst("queue sink pad missing".to_string()))?;
    src.link(&sink)
        .map_err(|e| DownmixError::Gst(format!("tee branch link failed: {e:?}")))?;
    Ok(())
}

/// `audiomixmatrix` expects a GstArray of doubles, row-major (output rows).
fn set_matrix_property(
    element: &gst::Element,
    matrix: &[f64],
    in_channels: usize,
) -> Result<()> {
    // GStreamer 1.28 expects an array of per-output-row arrays.
    let rows = matrix
        .chunks(in_channels)
        .map(|row| {
            let inner: gst::Array = row.iter().map(|v| v.to_send_value()).collect();
            inner.to_send_value()
        })
        .collect::<Vec<_>>();
    let array = gst::Array::from_values(rows);
    element.set_property("matrix", array);
    Ok(())
}

/// Build and run the downmix pipeline, collecting raw output samples.
pub fn render_downmix(
    input: &Path,
    output_wav: &Path,
    params: &ProcessingParams,
) -> Result<(DownmixResult, WaveformSummary)> {
    init_gst()?;

    let pipeline = gst::Pipeline::with_name("surround-downmix");
    let filesrc = gst::ElementFactory::make("filesrc")
        .property("location", input.to_string_lossy().as_ref())
        .build()?;
    let decodebin = gst::ElementFactory::make("decodebin").build()?;
    let queue = gst::ElementFactory::make("queue").build()?;
    let convert_in = gst::ElementFactory::make("audioconvert").build()?;
    let resample = gst::ElementFactory::make("audioresample").build()?;
    let capsfilter = gst::ElementFactory::make("capsfilter")
        .property(
            "caps",
            gst::Caps::builder("audio/x-raw")
                .field("format", gst_audio::AUDIO_FORMAT_F32.to_str())
                .field("channels", params.input_channels as i32)
                .field("layout", "interleaved")
                .build(),
        )
        .build()?;
    let mixmatrix = gst::ElementFactory::make("audiomixmatrix")
        .property("out-channels", 2u32)
        .property("in-channels", params.input_channels as u32)
        .build()?;
    set_matrix_property(
        &mixmatrix,
        &params.coefficient_matrix,
        params.input_channels,
    )?;
    let convert_out = gst::ElementFactory::make("audioconvert").build()?;
    let tee = gst::ElementFactory::make("tee")
        .name("waveform-tee")
        .build()?;
    let queue_file = gst::ElementFactory::make("queue").build()?;
    let queue_sink = gst::ElementFactory::make("queue").build()?;
    let wavenc = gst::ElementFactory::make("wavenc").build()?;
    let filesink = gst::ElementFactory::make("filesink")
        .property("location", output_wav.to_string_lossy().as_ref())
        .build()?;
    let appsink = gst_app::AppSink::builder()
        .caps(
            &gst::Caps::builder("audio/x-raw")
                .field("format", gst_audio::AUDIO_FORMAT_F32.to_str())
                .field("channels", 2i32)
                .field("layout", "interleaved")
                .build(),
        )
        .build();

    pipeline.add_many([
        &filesrc,
        &decodebin,
        &queue,
        &convert_in,
        &resample,
        &capsfilter,
        &mixmatrix,
        &convert_out,
        &tee,
        &queue_file,
        &queue_sink,
        &wavenc,
        &filesink,
        appsink.upcast_ref(),
    ])?;
    gst::Element::link_many([&filesrc, &decodebin])?;
    gst::Element::link_many([
        &queue,
        &convert_in,
        &resample,
        &capsfilter,
        &mixmatrix,
        &convert_out,
        &tee,
    ])?;
    gst::Element::link_many([&queue_file, &wavenc, &filesink])?;
    gst::Element::link(&queue_sink, appsink.upcast_ref::<gst::Element>())?;
    link_tee_branch(&tee, &queue_file)?;
    link_tee_branch(&tee, &queue_sink)?;

    let decodebin_weak = decodebin.downgrade();
    let queue_weak = queue.downgrade();
    decodebin.connect_pad_added(move |_bin, pad| {
        let is_audio = pad
            .current_caps()
            .and_then(|caps| caps.structure(0).map(|s| s.name().starts_with("audio/")))
            .unwrap_or(true);
        if is_audio {
            if let Some(queue) = queue_weak.upgrade() {
                if let Some(sink) = queue.static_pad("sink") {
                    if pad.can_link(&sink) {
                        if let Err(err) = pad.link(&sink) {
                            log::error!("decodebin pad link failed: {err:?}");
                        }
                    }
                }
            }
        }
        let _ = decodebin_weak;
    });

    let capture = Arc::new(Mutex::new(Capture {
        channels: 2,
        ..Default::default()
    }));
    let sink_capture = capture.clone();
    appsink.set_callbacks(
        gst_app::AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let sample = sink.pull_sample().map_err(|e| {
                    element_error!(
                        sink,
                        gst::ResourceError::Read,
                        ["appsink pull failed: {e:?}"]
                    );
                    gst::FlowError::Error
                })?;
                let buffer = sample.buffer().ok_or_else(|| {
                    element_error!(
                        sink,
                        gst::ResourceError::Read,
                        ["sample without buffer"]
                    );
                    gst::FlowError::Error
                })?;
                let map = buffer.map_readable().map_err(|_| {
                    element_error!(sink, gst::ResourceError::Read, ["buffer map failed"]);
                    gst::FlowError::Error
                })?;
                let info = sample
                    .caps()
                    .and_then(|caps| gst_audio::AudioInfo::from_caps(caps).ok());
                let mut cap = sink_capture.lock().expect("capture poisoned");
                if let Some(info) = info {
                    cap.channels = info.channels() as usize;
                    cap.sample_rate = info.rate();
                }
                let floats: &[f32] = bytemuck_cast(map.as_slice());
                cap.interleaved.extend_from_slice(floats);
                Ok(gst::FlowSuccess::Ok)
            })
            .build(),
    );

    let bus = pipeline.bus().ok_or_else(|| {
        DownmixError::Gst("pipeline has no bus".to_string())
    })?;

    pipeline.set_state(gst::State::Playing)?;
    let outcome = loop {
        let msg = match bus.timed_pop_filtered(
            gst::ClockTime::from_seconds(30),
            &[gst::MessageType::Error, gst::MessageType::Eos],
        ) {
            Some(msg) => msg,
            None => {
                break Err(DownmixError::Gst(
                    "timed out waiting for pipeline completion".to_string(),
                ));
            }
        };
        match msg.view() {
            gst::MessageView::Eos(..) => break Ok(()),
            gst::MessageView::Error(err) => {
                break Err(DownmixError::Gst(format!(
                    "{}: {}",
                    err.error(),
                    err.debug().unwrap_or_default()
                )));
            }
            _ => continue,
        }
    };
    pipeline.set_state(gst::State::Null)?;
    outcome?;

    let mut cap = capture
        .lock()
        .map_err(|e| DownmixError::Gst(e.to_string()))?;
    let taken = std::mem::take(&mut *cap);
    drop(cap);
    let summary = build_waveform_summary(&taken, params.sample_rate)?;
    let result = DownmixResult {
        sample_rate: taken.sample_rate,
        sample_format: "F32LE".to_string(),
        input_channels: params.input_channels,
        output_channels: taken.channels,
        total_frames: summary.total_frames,
        duration_ms: summary.duration_ms,
    };
    Ok((result, summary))
}

fn bytemuck_cast(bytes: &[u8]) -> &[f32] {
    let count = bytes.len() / std::mem::size_of::<f32>();
    unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const f32, count) }
}

fn build_waveform_summary(cap: &Capture, sample_rate: u32) -> Result<WaveformSummary> {
    if cap.channels == 0 {
        return Err(DownmixError::Stream("no audio frames captured".into()));
    }
    let channels = cap.channels;
    let total_frames = (cap.interleaved.len() / channels) as u64;
    let rate = if cap.sample_rate != 0 {
        cap.sample_rate
    } else {
        sample_rate
    };
    let duration_ms = if rate > 0 {
        total_frames * 1000 / rate as u64
    } else {
        0
    };

    let mut global_peak = 0.0f64;
    let mut square_sum = 0.0f64;
    let mut abs_sum = 0.0f64;
    for sample in &cap.interleaved {
        let v = *sample as f64;
        global_peak = global_peak.max(v.abs());
        square_sum += v * v;
        abs_sum += v.abs();
    }
    let sample_count = cap.interleaved.len().max(1) as f64;
    let rms = (square_sum / sample_count).sqrt();
    let dc_offset = abs_sum / sample_count;

    let mut points = Vec::new();
    let mut frame_index = 0u64;
    while frame_index < total_frames {
        let start = frame_index as usize * channels;
        let end = ((frame_index as usize) + CORRELATION_FRAME_INTERVAL as usize)
            .min(total_frames as usize)
            * channels;
        let slice = &cap.interleaved[start..end];
        let mut peak = 0.0f64;
        let mut squares = 0.0f64;
        for sample in slice {
            let v = *sample as f64;
            peak = peak.max(v.abs());
            squares += v * v;
        }
        let frame_rms = (squares / slice.len().max(1) as f64).sqrt();
        let frame_bytes: &[u8] = unsafe {
            std::slice::from_raw_parts(
                slice.as_ptr() as *const u8,
                slice.len() * std::mem::size_of::<f32>(),
            )
        };
        let pts_ns = if rate > 0 {
            frame_index * 1_000_000_000 / rate as u64
        } else {
            0
        };
        points.push(CorrelationPoint {
            pts_ns,
            time_ms: if rate > 0 {
                frame_index * 1000 / rate as u64
            } else {
                0
            },
            frame_index,
            samples_per_frame: slice.len() / channels,
            peak,
            rms: frame_rms,
            frame_sha256: crate::hash::sha256_bytes(frame_bytes),
        });
        frame_index += CORRELATION_FRAME_INTERVAL;
    }

    Ok(WaveformSummary {
        total_frames,
        duration_ms,
        peak: global_peak,
        rms,
        dc_offset,
        points,
    })
}
