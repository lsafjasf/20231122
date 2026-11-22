#![allow(clippy::chunks_exact_to_as_chunks)]

use std::path::Path;

use anyhow::{anyhow, Context, Result};
use credits_core::waveform::{Peak, WaveformEvidence};
use gstreamer as gst;
use gstreamer_app::prelude::*;

use crate::gst_support::ensure_gst;

const SAMPLE_RATE: u32 = 8_000;
const MAX_PEAKS: usize = 20_000;
const MIN_BUCKET_MS: f64 = 10.0;

pub fn extract_waveform(path: &Path, duration_hint: Option<f64>) -> Result<WaveformEvidence> {
    ensure_gst()?;
    let uri = gst::glib::filename_to_uri(path, None)
        .map_err(|error| anyhow!("cannot build URI for {}: {error}", path.display()))?;
    let pipeline = gst::Pipeline::new();
    let decode = gst::ElementFactory::make("uridecodebin")
        .property("uri", uri.as_str())
        .build()
        .context("uridecodebin is unavailable")?;
    let convert = gst::ElementFactory::make("audioconvert")
        .build()
        .context("audioconvert is unavailable")?;
    let resample = gst::ElementFactory::make("audioresample")
        .build()
        .context("audioresample is unavailable")?;
    let caps = gst::Caps::builder("audio/x-raw")
        .field("format", "F32LE")
        .field("rate", SAMPLE_RATE as i32)
        .field("channels", 1_i32)
        .field("layout", "interleaved")
        .build();
    let capsfilter = gst::ElementFactory::make("capsfilter")
        .property("caps", &caps)
        .build()
        .context("capsfilter is unavailable")?;
    let sink = gstreamer_app::AppSink::builder()
        .max_buffers(4)
        .drop(false)
        .sync(false)
        .build();
    let sink_element = sink.clone().upcast::<gst::Element>();
    pipeline.add_many([&decode, &convert, &resample, &capsfilter, &sink_element])?;
    gst::Element::link_many([&convert, &resample, &capsfilter, &sink_element])?;

    let convert_for_pad = convert.clone();
    decode.connect_pad_added(move |_, pad| {
        let caps = pad.current_caps().or_else(|| Some(pad.query_caps(None)));
        let is_audio = caps
            .as_ref()
            .and_then(|caps| caps.structure(0))
            .map(|structure| structure.name().starts_with("audio/"))
            .unwrap_or_else(|| pad.name().starts_with("audio"));
        if is_audio {
            if let Some(sink_pad) = convert_for_pad.static_pad("sink") {
                let _ = pad.link(&sink_pad);
            }
        }
    });

    pipeline.set_state(gst::State::Playing)?;
    let bucket_ms = duration_hint
        .filter(|duration| duration.is_finite() && *duration > 0.0)
        .map(|duration| (duration * 1000.0 / MAX_PEAKS as f64).max(MIN_BUCKET_MS))
        .unwrap_or(MIN_BUCKET_MS);
    let bucket_samples = ((SAMPLE_RATE as f64 * bucket_ms / 1000.0).round() as usize).max(1);

    let mut builder = PeakBuilder::new(bucket_samples);
    let mut total_samples = 0_u64;
    let mut end_ns = 0_u64;
    let mut saw_sample = false;
    loop {
        let Some(sample) = sink.try_pull_sample(gst::ClockTime::from_seconds(15)) else {
            break;
        };
        saw_sample = true;
        let buffer = sample
            .buffer()
            .ok_or_else(|| anyhow!("audio sample has no buffer"))?;
        if let Some(pts) = buffer.pts() {
            let duration = buffer.duration().unwrap_or(gst::ClockTime::ZERO);
            end_ns = end_ns.max(pts.nseconds().saturating_add(duration.nseconds()));
        }
        let map = buffer
            .map_readable()
            .map_err(|error| anyhow!("cannot map audio buffer: {error}"))?;
        let bytes = map.as_slice();
        for chunk in bytes.chunks_exact(4) {
            let sample = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            builder.push(sample);
            total_samples += 1;
        }
    }
    let _ = pipeline.set_state(gst::State::Null);

    if !saw_sample {
        let bus = pipeline.bus();
        if let Some(bus) = bus {
            crate::gst_support::error_from_bus(
                &bus,
                gst::ClockTime::from_mseconds(500),
                "waveform extraction",
            )?;
        }
    }

    let peaks = builder.finish();
    let duration = duration_hint
        .filter(|duration| duration.is_finite() && *duration > 0.0)
        .unwrap_or(total_samples as f64 / SAMPLE_RATE as f64)
        .max(end_ns as f64 / 1e9);
    if peaks.is_empty() {
        return Err(anyhow!("no audio samples decoded from {}", path.display()));
    }
    Ok(WaveformEvidence::new(
        SAMPLE_RATE,
        bucket_ms,
        duration,
        peaks,
    ))
}

struct PeakBuilder {
    bucket_samples: usize,
    min: f32,
    max: f32,
    sum_sq: f64,
    count: usize,
    peaks: Vec<Peak>,
}

impl PeakBuilder {
    fn new(bucket_samples: usize) -> Self {
        Self {
            bucket_samples,
            min: 0.0,
            max: 0.0,
            sum_sq: 0.0,
            count: 0,
            peaks: Vec::new(),
        }
    }

    fn push(&mut self, sample: f32) {
        let sample = if sample.is_finite() {
            sample.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        if self.count == 0 {
            self.min = sample;
            self.max = sample;
        } else {
            self.min = self.min.min(sample);
            self.max = self.max.max(sample);
        }
        self.sum_sq += (sample as f64) * (sample as f64);
        self.count += 1;
        if self.count >= self.bucket_samples {
            self.flush();
        }
    }

    fn flush(&mut self) {
        if self.count == 0 {
            return;
        }
        let rms = (self.sum_sq / self.count as f64).sqrt() as f32;
        self.peaks.push(Peak::new(self.min, self.max, rms));
        self.min = 0.0;
        self.max = 0.0;
        self.sum_sq = 0.0;
        self.count = 0;
    }

    fn finish(mut self) -> Vec<Peak> {
        self.flush();
        self.peaks
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peak_builder_keeps_envelope() {
        let mut builder = PeakBuilder::new(4);
        for sample in [-0.5, 0.25, 0.75, -0.25, 0.0, 0.0, 0.0, 0.0] {
            builder.push(sample);
        }
        let peaks = builder.finish();
        assert_eq!(peaks.len(), 2);
        assert_eq!(peaks[0].min, -0.5);
        assert_eq!(peaks[0].max, 0.75);
    }
}
