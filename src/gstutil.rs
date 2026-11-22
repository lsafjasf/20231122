use anyhow::{anyhow, Context};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use std::path::Path;

use crate::audio::SAMPLE_RATE;

pub struct DecodeStats {
    pub total_samples: u64,
    pub duration_ns: u64,
    pub chunks: u64,
}

/// Decode `input` to mono f32le @ 48 kHz, invoking `on_chunk` per buffer with
/// (pcm_samples, buffer_pts_ns). Streaming: memory stays bounded regardless of
/// what the caller does with the chunks.
pub fn decode_f32<F: FnMut(&[f32], u64) -> anyhow::Result<()>>(
    input: &Path,
    mut on_chunk: F,
) -> anyhow::Result<DecodeStats> {
    let uri = format!("file://{}", input.canonicalize()?.display());
    let desc = format!(
        "uridecodebin uri=\"{uri}\" ! audioconvert ! audioresample ! audio/x-raw,format=F32LE,rate={SAMPLE_RATE},channels=1 ! appsink name=sink"
    );
    let pipe = gst::parse::launch(&desc)?
        .downcast::<gst::Pipeline>()
        .map_err(|_| anyhow!("not a pipeline"))?;
    let sink: gst_app::AppSink = pipe
        .by_name("sink")
        .ok_or_else(|| anyhow!("no appsink"))?
        .downcast()
        .map_err(|_| anyhow!("sink not appsink"))?;

    pipe.set_state(gst::State::Playing)?;
    let mut stats = DecodeStats { total_samples: 0, duration_ns: 0, chunks: 0 };
    loop {
        match sink.pull_sample() {
            Ok(sample) => {
                let buf = sample.buffer().ok_or_else(|| anyhow!("no buffer"))?;
                let pts = buf.pts().map(|c| c.nseconds()).unwrap_or(0);
                let map = buf.map_readable()?;
                let pcm = crate::audio::bytes_to_f32(&map);
                stats.total_samples += pcm.len() as u64;
                stats.duration_ns = pts + pcm.len() as u64 * 1_000_000_000 / SAMPLE_RATE as u64;
                stats.chunks += 1;
                on_chunk(&pcm, pts)?;
            }
            Err(_) if sink.is_eos() => break,
            Err(e) => {
                pipe.set_state(gst::State::Null).ok();
                return Err(anyhow!("pull_sample failed: {e}"));
            }
        }
    }
    pipe.set_state(gst::State::Null)?;
    Ok(stats)
}

/// Decode fully into memory (normal mode).
pub fn decode_to_vec(input: &Path) -> anyhow::Result<(Vec<f32>, DecodeStats)> {
    let mut out = Vec::new();
    let stats = decode_f32(input, |chunk, _pts| {
        out.extend_from_slice(chunk);
        Ok(())
    })?;
    Ok((out, stats))
}

/// Encode mono f32 @ 48 kHz to a 16-bit WAV file via GStreamer.
pub fn encode_wav(samples: &[f32], output: &Path) -> anyhow::Result<()> {
    let desc = format!(
        "appsrc name=src ! audio/x-raw,format=F32LE,rate={SAMPLE_RATE},channels=1 ! audioconvert ! audio/x-raw,format=S16LE,rate={SAMPLE_RATE},channels=1 ! wavenc ! filesink location=\"{}\"",
        output.display()
    );
    let pipe = gst::parse::launch(&desc)?
        .downcast::<gst::Pipeline>()
        .map_err(|_| anyhow!("not a pipeline"))?;
    let src: gst_app::AppSrc = pipe
        .by_name("src")
        .ok_or_else(|| anyhow!("no appsrc"))?
        .downcast()
        .map_err(|_| anyhow!("src not appsrc"))?;
    src.set_caps(Some(
        &gst::Caps::builder("audio/x-raw")
            .field("format", "F32LE")
            .field("rate", SAMPLE_RATE as i32)
            .field("channels", 1i32)
            .field("layout", "interleaved")
            .build(),
    ));

    pipe.set_state(gst::State::Playing)?;
    let bytes = crate::audio::f32_to_bytes(samples);
    let mut buf = gst::Buffer::with_size(bytes.len()).context("alloc buffer")?;
    {
        let bref = buf.get_mut().ok_or_else(|| anyhow!("buffer not writable"))?;
        bref.set_pts(gst::ClockTime::ZERO);
        bref.set_duration(gst::ClockTime::from_nseconds(
            samples.len() as u64 * 1_000_000_000 / SAMPLE_RATE as u64,
        ));
        let mut map = bref.map_writable()?;
        map.copy_from_slice(&bytes);
    }
    src.push_buffer(buf).map_err(|e| anyhow!("push_buffer: {e:?}"))?;
    src.end_of_stream().map_err(|e| anyhow!("eos: {e:?}"))?;

    let bus = pipe.bus().ok_or_else(|| anyhow!("no bus"))?;
    for msg in bus.iter_timed(gst::ClockTime::from_seconds(30)) {
        match msg.view() {
            gst::MessageView::Eos(_) => break,
            gst::MessageView::Error(e) => {
                pipe.set_state(gst::State::Null).ok();
                return Err(anyhow!("encode error: {}", e.error()));
            }
            _ => {}
        }
    }
    pipe.set_state(gst::State::Null)?;
    Ok(())
}
