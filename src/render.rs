use crate::history::sha256_bytes;
use crate::model::ClipState;
use anyhow::{Context, Result, bail};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app::AppSrc;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

const SOURCE_AMPLITUDE: f32 = 0.4;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RenderInfo {
    pub path: String,
    pub state_sha256: String,
    pub media_sha256: String,
    pub bytes: u64,
    pub duration_ms: u64,
    pub sample_rate: u32,
    pub channels: u16,
}

pub fn init() -> Result<()> {
    gst::init().context("failed to initialize GStreamer")?;
    Ok(())
}

pub fn render_clip(state: &ClipState, output: &Path) -> Result<RenderInfo> {
    init()?;
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create media directory {}", parent.display()))?;
    }
    if output.exists() {
        fs::remove_file(output)
            .with_context(|| format!("failed to replace {}", output.display()))?;
    }

    let samples = synth_samples(state)?;
    let payload = samples_to_le_bytes(&samples);
    let trim_duration_ms = state.trim_end_ms - state.trim_start_ms;

    let description = "appsrc name=source ! audioconvert ! volume name=gain ! audioconvert ! \
         capsfilter name=output_caps ! wavenc ! filesink name=output";
    let pipeline = gst::parse::launch(description)
        .context("failed to construct GStreamer render pipeline")?
        .downcast::<gst::Pipeline>()
        .map_err(|_| anyhow::anyhow!("GStreamer returned a non-pipeline element"))?;

    let source = pipeline
        .by_name("source")
        .context("render pipeline is missing appsrc")?
        .downcast::<AppSrc>()
        .map_err(|_| anyhow::anyhow!("source is not appsrc"))?;
    let gain = pipeline
        .by_name("gain")
        .context("render pipeline is missing volume")?;
    let output_caps = pipeline
        .by_name("output_caps")
        .context("render pipeline is missing capsfilter")?;
    let sink = pipeline
        .by_name("output")
        .context("render pipeline is missing filesink")?;

    let caps = raw_caps(state.source.sample_rate, state.source.channels, "F32LE");
    source.set_caps(Some(&caps));
    source.set_format(gst::Format::Time);
    source.set_property("is-live", false);
    source.set_property("block", true);

    let output_caps_value = raw_caps(state.source.sample_rate, state.source.channels, "S16LE");
    output_caps.set_property("caps", &output_caps_value);
    gain.set_property("volume", state.gain_milli as f64 / 1000.0);
    sink.set_property("location", output.to_string_lossy().as_ref());

    pipeline.set_state(gst::State::Playing)?;
    let (state_change, _, _) = pipeline.state(gst::ClockTime::from_seconds(5));
    state_change.context("GStreamer did not reach PLAYING for render pipeline")?;

    let mut buffer = gst::Buffer::from_slice(payload);
    {
        let buffer = buffer
            .get_mut()
            .context("failed to acquire render buffer")?;
        buffer.set_pts(gst::ClockTime::ZERO);
        buffer.set_duration(gst::ClockTime::from_mseconds(trim_duration_ms as u64));
    }
    source
        .push_buffer(buffer)
        .context("appsrc rejected the trimmed audio buffer")?;
    source
        .end_of_stream()
        .context("failed to send EOS to appsrc")?;

    let bus = pipeline.bus().context("render pipeline has no bus")?;
    let message = bus
        .timed_pop_filtered(
            gst::ClockTime::from_seconds(15),
            &[gst::MessageType::Eos, gst::MessageType::Error],
        )
        .context("timed out waiting for GStreamer render result")?;

    let result = match message.view() {
        gst::MessageView::Eos(_) => Ok(()),
        gst::MessageView::Error(error) => Err(anyhow::anyhow!(
            "GStreamer render error: {} ({:?})",
            error.error(),
            error.debug()
        )),
        _ => Err(anyhow::anyhow!("unexpected GStreamer message")),
    };

    let _ = pipeline.set_state(gst::State::Null);
    result?;

    let bytes = fs::read(output).with_context(|| format!("failed to read {}", output.display()))?;
    let duration_ms = wav_duration_ms(&bytes, state.source.sample_rate, state.source.channels)
        .context("rendered output is not a readable PCM WAV")?;
    Ok(RenderInfo {
        path: output.display().to_string(),
        state_sha256: crate::history::state_sha256(state),
        media_sha256: sha256_bytes(&bytes),
        bytes: bytes.len() as u64,
        duration_ms,
        sample_rate: state.source.sample_rate,
        channels: state.source.channels,
    })
}

pub fn render_into_dir(state: &ClipState, directory: &Path, stem: &str) -> Result<RenderInfo> {
    render_clip(state, &directory.join(format!("{stem}.wav")))
}

fn synth_samples(state: &ClipState) -> Result<Vec<f32>> {
    if state.source.sample_rate == 0 || state.source.channels == 0 {
        bail!("sample_rate and channels must be positive");
    }
    if state.trim_start_ms >= state.trim_end_ms {
        bail!("trim range is empty");
    }
    if state.trim_end_ms > state.source.duration_ms {
        bail!("trim range exceeds source duration");
    }
    let total_samples =
        (state.source.duration_ms as u64 * state.source.sample_rate as u64 / 1000) as usize;
    let start = (state.trim_start_ms as u64 * state.source.sample_rate as u64 / 1000) as usize;
    let end = (state.trim_end_ms as u64 * state.source.sample_rate as u64 / 1000) as usize;
    let frequency = state.source.frequency_hz as f32;
    let sample_rate = state.source.sample_rate as f32;
    let samples_per_frame = state.source.channels as usize;
    let mut samples = Vec::with_capacity((end - start) * samples_per_frame);
    for frame in start..end {
        if frame >= total_samples {
            break;
        }
        let value = SOURCE_AMPLITUDE
            * (std::f32::consts::TAU * frequency * frame as f32 / sample_rate).sin();
        samples.extend(std::iter::repeat_n(value, samples_per_frame));
    }
    Ok(samples)
}

fn samples_to_le_bytes(samples: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(samples.len() * 4);
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

fn raw_caps(rate: u32, channels: u16, format: &str) -> gst::Caps {
    gst::Caps::builder("audio/x-raw")
        .field("format", format)
        .field("rate", rate as i32)
        .field("channels", channels as i32)
        .field("layout", "interleaved")
        .build()
}

fn wav_duration_ms(bytes: &[u8], sample_rate: u32, channels: u16) -> Result<u64> {
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        bail!("missing RIFF/WAVE header");
    }
    let mut offset = 12;
    while offset + 8 <= bytes.len() {
        let chunk_id = &bytes[offset..offset + 4];
        let chunk_size =
            u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize;
        let data_start = offset + 8;
        if chunk_id == b"data" {
            let frame_bytes = channels as usize * 2;
            if frame_bytes == 0 {
                bail!("invalid channel count");
            }
            return Ok(chunk_size as u64 * 1000 / (sample_rate as u64 * frame_bytes as u64));
        }
        offset = data_start
            .checked_add(chunk_size)
            .and_then(|next| next.checked_add(chunk_size % 2))
            .context("invalid WAV chunk offset")?;
    }
    bail!("WAV has no data chunk")
}

pub fn evidence_path(evidence_dir: &Path, relative: &str) -> PathBuf {
    evidence_dir.join(relative)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SourceSpec;

    #[test]
    fn renders_real_wav_with_expected_duration() {
        let state = ClipState {
            clip_id: "render-test".into(),
            source: SourceSpec {
                frequency_hz: 440,
                duration_ms: 1000,
                sample_rate: 16000,
                channels: 1,
            },
            trim_start_ms: 200,
            trim_end_ms: 800,
            gain_milli: 700,
        };
        let directory = tempfile::tempdir().unwrap();
        let info = render_into_dir(&state, directory.path(), "clip").unwrap();
        assert_eq!(info.duration_ms, 600);
        assert!(info.bytes > 44);
        assert!(info.media_sha256.starts_with("sha256:"));
    }
}
