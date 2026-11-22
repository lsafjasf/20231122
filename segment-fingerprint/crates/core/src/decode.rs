//! Audio decoding and time-window extraction.
//!
//! Primary backend is GStreamer (`uridecodebin ! audioconvert ! audioresample
//! ! appsink`) producing mono S16LE PCM at the fingerprint sample rate.
//! If GStreamer cannot construct or run the pipeline and the input is a plain
//! RIFF WAVE file, `wav_fallback` decodes the file directly and records the
//! degradation instead of failing the run.

use crate::model::MicroRange;
use gstreamer::prelude::*;
use gstreamer::{Caps, ClockTime, Element, ElementFactory, Pipeline, SeekFlags, State};
use gstreamer_app::AppSink;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct DecodeRequest {
    pub source_path: PathBuf,
    pub requested_range: MicroRange,
    pub sample_rate_hz: u32,
}

#[derive(Debug, Clone)]
pub struct DecodedWindow {
    pub pcm_s16le: Vec<u8>,
    pub decoded_range: MicroRange,
    pub backend: String,
    pub pipeline_description: String,
    pub used_fallback: bool,
}

pub fn ensure_initialized() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        gstreamer::init().expect("GStreamer initialization failed");
    });
}

pub fn pipeline_description(path: &Path, sample_rate_hz: u32) -> String {
    format!(
        "uridecodebin uri={} ! audioconvert ! audioresample ! audio/x-raw,format=S16LE,channels=1,rate={} ! appsink",
        glib::filename_to_uri(path, None).expect("valid file path").as_str(),
        sample_rate_hz
    )
}

fn wav_or_media_duration_us(path: &Path) -> anyhow::Result<i64> {
    if !is_wav_file(path) {
        return Ok(i64::MAX);
    }
    let bytes = fs::read(path)?;
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        anyhow::bail!("not a RIFF/WAVE file");
    }
    let mut offset = 12usize;
    let mut sample_rate = 0u32;
    let mut byte_rate = 0u32;
    let mut data_len = 0usize;
    while offset + 8 <= bytes.len() {
        let id = &bytes[offset..offset + 4];
        let size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into()?) as usize;
        let body = offset + 8;
        match id {
            b"fmt " => {
                sample_rate = u32::from_le_bytes(bytes[body + 4..body + 8].try_into()?);
                byte_rate = u32::from_le_bytes(bytes[body + 8..body + 12].try_into()?);
            }
            b"data" => data_len = size,
            _ => {}
        }
        offset = body + size + (size & 1);
    }
    if sample_rate == 0 || byte_rate == 0 {
        anyhow::bail!("WAV missing fmt chunk");
    }
    Ok(data_len as i64 * 1_000_000 / byte_rate as i64)
}

pub fn decode_window(req: &DecodeRequest) -> anyhow::Result<DecodedWindow> {
    ensure_initialized();
    match decode_window_gstreamer(req) {
        Ok(window) => Ok(window),
        Err(gst_error) => {
            let path = req.source_path.clone();
            if is_wav_file(&path) {
                let window = wav_fallback(req, gst_error.to_string())?;
                Ok(window)
            } else {
                Err(gst_error)
            }
        }
    }
}

fn decode_window_gstreamer(req: &DecodeRequest) -> anyhow::Result<DecodedWindow> {
    let uri = glib::filename_to_uri(&req.source_path, None)?;
    let pipeline = Pipeline::new();
    let decodebin = ElementFactory::make("uridecodebin")
        .property("uri", uri.as_str())
        .build()?;
    let convert = ElementFactory::make("audioconvert").build()?;
    let resample = ElementFactory::make("audioresample").build()?;
    let appsink = AppSink::builder()
        .name("sink")
        .caps(
            &Caps::builder("audio/x-raw")
                .field("format", "S16LE")
                .field("channels", 1i32)
                .field("rate", req.sample_rate_hz as i32)
                .build(),
        )
        .build();
    pipeline.add_many([decodebin.upcast_ref(), &convert, &resample, appsink.upcast_ref()])?;
    Element::link_many([&convert, &resample, appsink.upcast_ref()])?;

    let convert_weak = convert.downgrade();
    decodebin.connect_pad_added(move |_src, pad| {
        if let Some(convert) = convert_weak.upgrade() {
            if let Some(sink_pad) = convert.static_pad("sink") {
                let _ = pad.link(&sink_pad);
            }
        }
    });

    let start = ClockTime::from_useconds(req.requested_range.start_us.max(0) as u64);
    let stop = ClockTime::from_useconds(req.requested_range.end_us.max(0) as u64);
    let media_duration_us = wav_or_media_duration_us(&req.source_path).unwrap_or(0);
    let full_stream =
        req.requested_range.start_us <= 0 && req.requested_range.end_us >= media_duration_us;
    pipeline.set_state(State::Paused)?;
    let state_ok = pipeline.state(ClockTime::from_seconds(5)).0.is_ok();
    if !state_ok {
        anyhow::bail!("GStreamer pipeline did not reach PAUSED within 5s");
    }
    if !full_stream {
        pipeline.seek(
            1.0,
            SeekFlags::FLUSH | SeekFlags::ACCURATE,
            gstreamer::SeekType::Set,
            start,
            gstreamer::SeekType::Set,
            stop,
        )?;
    }
    pipeline.set_state(State::Playing)?;

    let mut pcm = Vec::new();
    let bus = pipeline.bus().expect("pipeline has a bus");
    // Full-stream decoding ends with a bus EOS. A bounded segment seek does
    // not emit EOS at the stop position, so repeated idle pulls after the
    // segment data drained mark the end of the window instead.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut idle_pulls = 0u32;
    let mut saw_eos = false;
    loop {
        while let Some(message) = bus.pop_filtered(&[
            gstreamer::MessageType::Error,
            gstreamer::MessageType::Eos,
        ]) {
            match message.view() {
                gstreamer::MessageView::Error(err) => {
                    let _ = pipeline.set_state(State::Null);
                    anyhow::bail!("GStreamer error: {}", err.error());
                }
                gstreamer::MessageView::Eos(_) => saw_eos = true,
                _ => {}
            }
        }
        if saw_eos {
            while let Ok(sample) = appsink.pull_sample() {
                if let Some(buffer) = sample.buffer() {
                    let map = buffer.map_readable()?;
                    pcm.extend_from_slice(map.as_slice());
                }
            }
            break;
        }
        match appsink.try_pull_sample(ClockTime::from_mseconds(200)) {
            Some(sample) => {
                idle_pulls = 0;
                if let Some(buffer) = sample.buffer() {
                    let map = buffer.map_readable()?;
                    pcm.extend_from_slice(map.as_slice());
                }
            }
            None => {
                idle_pulls += 1;
                if !full_stream && idle_pulls >= 3 {
                    break;
                }
            }
        }
        if std::time::Instant::now() > deadline {
            let _ = pipeline.set_state(State::Null);
            anyhow::bail!(
                "GStreamer decode deadline exceeded for {}",
                req.source_path.display()
            );
        }
    }
    let _ = pipeline.set_state(State::Null);

    if pcm.is_empty() {
        anyhow::bail!("GStreamer produced no samples for {:?}", req.requested_range);
    }

    Ok(DecodedWindow {
        pcm_s16le: pcm,
        decoded_range: req.requested_range.clone(),
        backend: "gstreamer-uridecodebin".to_string(),
        pipeline_description: pipeline_description(&req.source_path, req.sample_rate_hz),
        used_fallback: false,
    })
}

fn is_wav_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("wav"))
}

fn wav_fallback(req: &DecodeRequest, upstream_error: String) -> anyhow::Result<DecodedWindow> {
    let bytes = fs::read(&req.source_path)?;
    let wav = parse_wav(&bytes)?;
    if wav.channels != 1 || wav.bits_per_sample != 16 {
        anyhow::bail!(
            "WAV fallback supports mono S16LE only (channels={}, bits={}); upstream: {}",
            wav.channels,
            wav.bits_per_sample,
            upstream_error
        );
    }
    let start_sample = (req.requested_range.start_us.max(0) * wav.sample_rate as i64 / 1_000_000) as usize;
    let end_sample = (req.requested_range.end_us.max(0) * wav.sample_rate as i64 / 1_000_000) as usize;
    let clamped_end = end_sample.min(wav.samples);
    let clamped_start = start_sample.min(clamped_end);
    let byte_start = clamped_start * 2;
    let byte_end = clamped_end * 2;
    let mut pcm = wav.data[byte_start..byte_end].to_vec();

    let decoded_range = MicroRange {
        start_us: clamped_start as i64 * 1_000_000 / wav.sample_rate as i64,
        end_us: clamped_end as i64 * 1_000_000 / wav.sample_rate as i64,
    };

    if wav.sample_rate != req.sample_rate_hz {
        pcm = linear_resample(&pcm, wav.sample_rate, req.sample_rate_hz);
    }

    Ok(DecodedWindow {
        pcm_s16le: pcm,
        decoded_range,
        backend: "wav-file-fallback".to_string(),
        pipeline_description: format!(
            "wav-file-fallback path={} (gstreamer unavailable: {})",
            req.source_path.display(),
            upstream_error
        ),
        used_fallback: true,
    })
}

#[derive(Debug)]
struct ParsedWav<'a> {
    sample_rate: u32,
    channels: u16,
    bits_per_sample: u16,
    samples: usize,
    data: &'a [u8],
}

fn parse_wav(bytes: &[u8]) -> anyhow::Result<ParsedWav<'_>> {
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        anyhow::bail!("not a RIFF/WAVE file");
    }
    let mut offset = 12usize;
    let mut sample_rate = 0u32;
    let mut channels = 0u16;
    let mut bits = 0u16;
    let mut data: &[u8] = &[];
    while offset + 8 <= bytes.len() {
        let id = &bytes[offset..offset + 4];
        let size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into()?) as usize;
        let body_start = offset + 8;
        let body_end = body_start + size;
        if body_end > bytes.len() {
            anyhow::bail!("truncated WAV chunk {:?}", id);
        }
        match id {
            b"fmt " => {
                channels = u16::from_le_bytes(bytes[body_start + 2..body_start + 4].try_into()?);
                sample_rate = u32::from_le_bytes(bytes[body_start + 4..body_start + 8].try_into()?);
                bits = u16::from_le_bytes(bytes[body_start + 14..body_start + 16].try_into()?);
            }
            b"data" => data = &bytes[body_start..body_end],
            _ => {}
        }
        offset = body_end + (size & 1);
    }
    if data.is_empty() || sample_rate == 0 || channels == 0 || bits == 0 {
        anyhow::bail!("WAV missing fmt/data chunks");
    }
    let frame_bytes = channels as usize * bits as usize / 8;
    let samples = data.len() / frame_bytes;
    Ok(ParsedWav {
        sample_rate,
        channels,
        bits_per_sample: bits,
        samples,
        data,
    })
}

fn linear_resample(pcm: &[u8], from_rate: u32, to_rate: u32) -> Vec<u8> {
    let input: Vec<i16> = pcm.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
    if input.is_empty() || from_rate == 0 {
        return Vec::new();
    }
    let out_len = (input.len() as u64 * to_rate as u64 / from_rate as u64) as usize;
    let mut out = Vec::with_capacity(out_len * 2);
    for i in 0..out_len {
        let src_pos = i as f64 * from_rate as f64 / to_rate as f64;
        let left = src_pos.floor() as usize;
        let frac = src_pos - left as f64;
        let right = (left + 1).min(input.len() - 1);
        let value = (input[left] as f64 * (1.0 - frac) + input[right] as f64 * frac) as i16;
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_wav() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&36u32.to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&8000u32.to_le_bytes());
        bytes.extend_from_slice(&16000u32.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&4u32.to_le_bytes());
        bytes.extend_from_slice(&100i16.to_le_bytes());
        bytes.extend_from_slice(&200i16.to_le_bytes());
        let parsed = parse_wav(&bytes).unwrap();
        assert_eq!(parsed.sample_rate, 8000);
        assert_eq!(parsed.samples, 2);
    }
}
