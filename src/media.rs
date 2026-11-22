use std::path::Path;
use std::sync::OnceLock;

use anyhow::{Context, Result, anyhow, bail};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app::{AppSink, AppSrc};

pub const DECODE_PIPELINE: &str = "filesrc name=src ! decodebin ! audioconvert ! audioresample ! capsfilter name=filter caps=audio/x-raw,format=F32LE,layout=interleaved,channels=1 ! appsink name=sink sync=false max-buffers=8";
pub const ENCODE_PIPELINE: &str = "appsrc name=src ! audioconvert ! wavenc ! filesink name=sink";

#[derive(Debug, Clone)]
pub struct DecodedAudio {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub channels: u32,
    pub format: String,
    pub pipeline: String,
}

pub fn gstreamer_version() -> String {
    init_gstreamer()
        .map(|_| gst::version_string().to_string())
        .unwrap_or_else(|error| format!("unavailable: {error}"))
}

pub fn decode(path: &Path, sample_rate: u32, max_seconds: f64) -> Result<DecodedAudio> {
    init_gstreamer()?;
    if sample_rate == 0 {
        bail!("decode sample rate must be > 0");
    }
    let pipeline = gst::parse::launch(DECODE_PIPELINE)
        .context("failed to create GStreamer decode pipeline")?
        .downcast::<gst::Pipeline>()
        .map_err(|_| anyhow!("parsed GStreamer decode element is not a pipeline"))?;
    let source = pipeline
        .by_name("src")
        .ok_or_else(|| anyhow!("decode source element missing"))?;
    source.set_property("location", path.to_string_lossy().to_string());
    let filter = pipeline
        .by_name("filter")
        .ok_or_else(|| anyhow!("decode capsfilter element missing"))?;
    let caps = gst::Caps::builder("audio/x-raw")
        .field("format", "F32LE")
        .field("layout", "interleaved")
        .field("channels", 1_i32)
        .field("rate", sample_rate as i32)
        .build();
    filter.set_property("caps", &caps);
    let sink = pipeline
        .by_name("sink")
        .ok_or_else(|| anyhow!("decode appsink element missing"))?
        .downcast::<AppSink>()
        .map_err(|_| anyhow!("decode sink is not an appsink"))?;
    pipeline
        .set_state(gst::State::Playing)
        .context("failed to start GStreamer decode pipeline")?;
    let bus = pipeline
        .bus()
        .ok_or_else(|| anyhow!("decode pipeline has no bus"))?;
    let max_frames = (max_seconds * sample_rate as f64).ceil().max(1.0) as usize;
    let mut samples = Vec::<f32>::new();
    let mut decoded_rate = sample_rate;
    let mut decoded_channels = 1_u32;
    let mut decoded_format = "F32LE".to_string();

    loop {
        if let Some(sample) = sink.try_pull_sample(gst::ClockTime::from_mseconds(250)) {
            let sample: gst::Sample = sample;
            let caps: Option<&gst::CapsRef> = sample.caps();
            if let Some(caps) = caps {
                let structure: Option<&gst::StructureRef> = caps.structure(0);
                if let Some(structure) = structure {
                    decoded_rate =
                        structure.get::<i32>("rate").unwrap_or(decoded_rate as i32) as u32;
                    decoded_channels = structure
                        .get::<i32>("channels")
                        .unwrap_or(decoded_channels as i32)
                        as u32;
                    decoded_format = structure
                        .get::<&str>("format")
                        .unwrap_or("F32LE")
                        .to_string();
                }
            }
            let buffer = sample
                .buffer()
                .ok_or_else(|| anyhow!("decoded sample has no buffer"))?;
            let map = buffer
                .map_readable()
                .map_err(|error| anyhow!("failed to map decoded buffer: {error}"))?;
            if map.size() % 4 != 0 {
                let _ = pipeline.set_state(gst::State::Null);
                bail!("decoded buffer size {} is not a multiple of 4", map.size());
            }
            for chunk in map.as_slice().chunks_exact(4) {
                if samples.len() >= max_frames {
                    let _ = pipeline.set_state(gst::State::Null);
                    bail!(
                        "max_audio_seconds exceeded while decoding: decoded_frames={} limit_frames={}",
                        samples.len() + 1,
                        max_frames
                    );
                }
                samples.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
            }
            continue;
        }

        let mut reached_eos = false;
        while let Some(message) =
            bus.pop_filtered(&[gst::MessageType::Error, gst::MessageType::Eos])
        {
            match message.view() {
                gst::MessageView::Error(error) => {
                    let _ = pipeline.set_state(gst::State::Null);
                    bail!(
                        "GStreamer decode error: {} ({})",
                        error.error(),
                        error
                            .debug()
                            .map(|value| value.to_string())
                            .unwrap_or_default()
                    );
                }
                gst::MessageView::Eos(_) => reached_eos = true,
                _ => {}
            }
        }
        if reached_eos {
            break;
        }
    }
    let _ = pipeline.set_state(gst::State::Null);
    if samples.is_empty() {
        bail!("decoded audio contains zero samples");
    }
    Ok(DecodedAudio {
        samples,
        sample_rate: decoded_rate,
        channels: decoded_channels,
        format: decoded_format,
        pipeline: DECODE_PIPELINE.to_string(),
    })
}

pub fn encode_wav(path: &Path, samples: &[f32], sample_rate: u32) -> Result<()> {
    init_gstreamer()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }
    let pipeline = gst::Pipeline::new();
    let caps = gst::Caps::builder("audio/x-raw")
        .field("format", "F32LE")
        .field("layout", "interleaved")
        .field("channels", 1_i32)
        .field("rate", sample_rate as i32)
        .build();
    let source = AppSrc::builder()
        .caps(&caps)
        .format(gst::Format::Time)
        .is_live(false)
        .build();
    source.set_property("block", true);
    let convert = gst::ElementFactory::make("audioconvert")
        .build()
        .context("missing audioconvert plugin")?;
    let encoder = gst::ElementFactory::make("wavenc")
        .build()
        .context("missing wavenc plugin")?;
    let sink = gst::ElementFactory::make("filesink")
        .build()
        .context("missing filesink plugin")?;
    sink.set_property("location", path.to_string_lossy().to_string());
    pipeline
        .add_many([source.upcast_ref(), &convert, &encoder, &sink])
        .map_err(|error| anyhow!("failed to assemble encode pipeline: {error}"))?;
    gst::Element::link_many([source.upcast_ref(), &convert, &encoder, &sink])
        .map_err(|error| anyhow!("failed to link encode pipeline: {error}"))?;

    pipeline
        .set_state(gst::State::Playing)
        .context("failed to start GStreamer encode pipeline")?;
    let mut bytes = Vec::with_capacity(samples.len() * 4);
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    let mut buffer = gst::Buffer::with_size(bytes.len())
        .map_err(|error| anyhow!("failed to allocate encode buffer: {error}"))?;
    {
        let buffer_ref = buffer
            .get_mut()
            .ok_or_else(|| anyhow!("failed to access encode buffer"))?;
        let mut map = buffer_ref
            .map_writable()
            .map_err(|error| anyhow!("failed to map encode buffer: {error}"))?;
        map.copy_from_slice(&bytes);
    }
    source
        .push_buffer(buffer)
        .map_err(|error| anyhow!("failed to push PCM to GStreamer: {error}"))?;
    source
        .end_of_stream()
        .map_err(|error| anyhow!("failed to finish GStreamer appsrc: {error}"))?;

    let bus = pipeline
        .bus()
        .ok_or_else(|| anyhow!("encode pipeline has no bus"))?;
    let message = bus
        .timed_pop_filtered(
            gst::ClockTime::from_seconds(30),
            &[gst::MessageType::Error, gst::MessageType::Eos],
        )
        .ok_or_else(|| anyhow!("timed out waiting for GStreamer wavenc"))?;
    let result = match message.view() {
        gst::MessageView::Error(error) => Err(anyhow!(
            "GStreamer encode error: {} ({})",
            error.error(),
            error
                .debug()
                .map(|value| value.to_string())
                .unwrap_or_default()
        )),
        gst::MessageView::Eos(_) => Ok(()),
        _ => Err(anyhow!("unexpected GStreamer encode message")),
    };
    let _ = pipeline.set_state(gst::State::Null);
    result?;
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("encoded file not found: {}", path.display()))?;
    if metadata.len() <= 44 {
        bail!(
            "encoded WAV is unexpectedly empty: {} bytes",
            metadata.len()
        );
    }
    Ok(())
}

fn init_gstreamer() -> Result<()> {
    static INIT: OnceLock<Result<(), String>> = OnceLock::new();
    match INIT.get_or_init(|| gst::init().map_err(|error| error.to_string())) {
        Ok(()) => Ok(()),
        Err(error) => bail!("failed to initialize GStreamer: {error}"),
    }
}
