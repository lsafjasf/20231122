use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app::{AppSink, AppSrc, AppStreamType};

use crate::engine::{RenderOutcome, Renderer, RendererInfo, RunRequest};
use crate::error::{io_error, AppError, AppResult};
use crate::model::{DEFAULT_SAMPLE_RATE, OUTPUT_WAV};
use crate::plan::{RenderPlan, RenderSegment, SegmentRenderResult};
use crate::wav::Pcm16Writer;

static GST_INIT: OnceLock<Result<(), String>> = OnceLock::new();

#[derive(Debug, Default)]
pub struct GStreamerRenderer;

impl GStreamerRenderer {
    pub fn new() -> AppResult<Self> {
        ensure_gst()?;
        for factory in [
            "filesrc",
            "wavparse",
            "appsrc",
            "appsink",
            "pitch",
            "audioconvert",
        ] {
            if gst::ElementFactory::find(factory).is_none() {
                return Err(AppError::GStreamer(format!(
                    "required GStreamer factory '{factory}' is not installed"
                )));
            }
        }
        Ok(Self)
    }
}

impl Renderer for GStreamerRenderer {
    fn info(&self) -> RendererInfo {
        RendererInfo {
            backend: "GStreamer".to_string(),
            plugin_version: gst::version_string().to_string(),
            plugin_chain: vec![
                "filesrc".to_string(),
                "wavparse".to_string(),
                "audioconvert".to_string(),
                "audioresample".to_string(),
                "appsrc".to_string(),
                "capsfilter(audio/x-raw,F32LE)".to_string(),
                "pitch(tempo,pitch)".to_string(),
                "audioconvert".to_string(),
                "appsink".to_string(),
                "wav16le-writer".to_string(),
            ],
        }
    }

    fn render(
        &mut self,
        plan: &RenderPlan,
        request: &RunRequest,
        output_wav: &Path,
    ) -> AppResult<RenderOutcome> {
        ensure_gst()?;
        let mut decoded = decode_to_f32(
            &request.audio_path,
            plan.sample_rate_hz,
            request.limits.max_decoded_pcm_bytes,
        )?;
        let output_channels = decoded.channels;
        let mut writer = Pcm16Writer::create(output_wav, DEFAULT_SAMPLE_RATE, output_channels)?;
        let mut segment_results = Vec::with_capacity(plan.segments.len());

        for segment in &plan.segments {
            let output_frames = render_segment(
                &mut decoded.file,
                decoded.bytes_per_frame,
                segment,
                DEFAULT_SAMPLE_RATE,
                output_channels,
                &mut writer,
            )?;
            segment_results.push(SegmentRenderResult {
                index: segment.index,
                output_frames,
            });
        }
        writer.finish(output_wav)?;
        Ok(RenderOutcome {
            segments: segment_results,
            output_sample_rate_hz: DEFAULT_SAMPLE_RATE,
            output_channels,
            plugin_version: gst::version_string().to_string(),
            plugin_chain: self.info().plugin_chain,
        })
    }
}

struct DecodedPcm {
    file: File,
    channels: u16,
    bytes_per_frame: usize,
}

fn decode_to_f32(input: &Path, sample_rate_hz: u32, max_bytes: u64) -> AppResult<DecodedPcm> {
    let channels = crate::wav::probe_wav(input)?.channels;
    let caps = audio_caps(sample_rate_hz, channels);
    let filesrc = gst::ElementFactory::make("filesrc")
        .property("location", input.to_string_lossy().as_ref())
        .build()
        .map_err(gst_error)?;
    let wavparse = gst::ElementFactory::make("wavparse")
        .build()
        .map_err(gst_error)?;
    let audioconvert = gst::ElementFactory::make("audioconvert")
        .build()
        .map_err(gst_error)?;
    let audioresample = gst::ElementFactory::make("audioresample")
        .build()
        .map_err(gst_error)?;
    let capsfilter = gst::ElementFactory::make("capsfilter")
        .property("caps", &caps)
        .build()
        .map_err(gst_error)?;
    let appsink = AppSink::builder().sync(false).max_buffers(4).build();

    let pipeline = gst::Pipeline::new();
    pipeline
        .add_many([
            &filesrc,
            &wavparse,
            &audioconvert,
            &audioresample,
            &capsfilter,
            appsink.upcast_ref(),
        ])
        .map_err(gst_error)?;
    gst::Element::link_many([
        &filesrc,
        &wavparse,
        &audioconvert,
        &audioresample,
        &capsfilter,
        appsink.upcast_ref(),
    ])
    .map_err(gst_error)?;

    let mut temp = tempfile::tempfile().map_err(|err| io_error("decoded-pcm.tmp", err))?;
    pipeline
        .set_state(gst::State::Playing)
        .map_err(state_error)?;
    let mut decoded_bytes = 0_u64;
    loop {
        let sample = match appsink.pull_sample() {
            Ok(sample) => sample,
            Err(_) => break,
        };
        let buffer = sample
            .buffer()
            .ok_or_else(|| AppError::GStreamer("decoded sample has no buffer".to_string()))?;
        let map = buffer
            .map_readable()
            .map_err(|_| AppError::GStreamer("decoded buffer is not readable".to_string()))?;
        decoded_bytes = decoded_bytes.saturating_add(map.size() as u64);
        if decoded_bytes > max_bytes {
            let _ = pipeline.set_state(gst::State::Null);
            return Err(AppError::LimitExceeded {
                what: "decoded PCM bytes",
                actual: decoded_bytes,
                limit: max_bytes,
            });
        }
        temp.write_all(map.as_slice())
            .map_err(|err| io_error("decoded-pcm.tmp", err))?;
    }
    let _ = pipeline.set_state(gst::State::Null);
    temp.seek(SeekFrom::Start(0))
        .map_err(|err| io_error("decoded-pcm.tmp", err))?;
    Ok(DecodedPcm {
        file: temp,
        channels,
        bytes_per_frame: channels as usize * 4,
    })
}

fn render_segment(
    decoded: &mut File,
    decoded_bytes_per_frame: usize,
    segment: &RenderSegment,
    sample_rate_hz: u32,
    channels: u16,
    writer: &mut Pcm16Writer,
) -> AppResult<u64> {
    let caps = audio_caps(sample_rate_hz, channels);
    let appsrc = AppSrc::builder()
        .caps(&caps)
        .format(gst::Format::Time)
        .stream_type(AppStreamType::Stream)
        .is_live(false)
        .block(true)
        .max_bytes(4 * 1024 * 1024)
        .build();
    let capsfilter = gst::ElementFactory::make("capsfilter")
        .property("caps", &caps)
        .build()
        .map_err(gst_error)?;
    let pitch = gst::ElementFactory::make("pitch")
        .property("tempo", segment.speed_rate as f32)
        .property("pitch", segment.pitch_ratio as f32)
        .build()
        .map_err(gst_error)?;
    let audioconvert = gst::ElementFactory::make("audioconvert")
        .build()
        .map_err(gst_error)?;
    let output_caps = audio_caps(sample_rate_hz, channels);
    let output_filter = gst::ElementFactory::make("capsfilter")
        .property("caps", &output_caps)
        .build()
        .map_err(gst_error)?;
    let appsink = AppSink::builder().sync(false).max_buffers(8).build();

    let pipeline = gst::Pipeline::new();
    pipeline
        .add_many([
            appsrc.upcast_ref(),
            &capsfilter,
            &pitch,
            &audioconvert,
            &output_filter,
            appsink.upcast_ref(),
        ])
        .map_err(gst_error)?;
    gst::Element::link_many([
        appsrc.upcast_ref(),
        &capsfilter,
        &pitch,
        &audioconvert,
        &output_filter,
        appsink.upcast_ref(),
    ])
    .map_err(gst_error)?;
    pipeline
        .set_state(gst::State::Playing)
        .map_err(state_error)?;

    let source_frames = crate::plan::us_to_frames(
        segment.source_end_us - segment.source_start_us,
        sample_rate_hz,
    );
    let source_bytes = source_frames as usize * decoded_bytes_per_frame;
    let source_offset = crate::plan::us_to_frames(segment.source_start_us, sample_rate_hz)
        * decoded_bytes_per_frame as u64;
    decoded
        .seek(SeekFrom::Start(source_offset))
        .map_err(|err| io_error("decoded-pcm.tmp", err))?;
    let mut remaining = source_bytes;
    let mut pushed_frames = 0_u64;
    while remaining > 0 {
        let chunk_len = remaining.min(64 * 1024);
        let mut bytes = vec![0_u8; chunk_len];
        decoded
            .read_exact(&mut bytes)
            .map_err(|err| io_error("decoded-pcm.tmp", err))?;
        let complete = bytes.len() / decoded_bytes_per_frame * decoded_bytes_per_frame;
        if complete == 0 {
            break;
        }
        bytes.truncate(complete);
        let mut buffer = gst::Buffer::with_size(bytes.len()).map_err(gst_error)?;
        {
            let buffer_ref = buffer.get_mut().expect("fresh buffer");
            let mut map = buffer_ref
                .map_writable()
                .map_err(|_| AppError::GStreamer("cannot map appsrc buffer".to_string()))?;
            map.copy_from_slice(&bytes);
        }
        let pts_ns = segment.source_start_us.saturating_mul(1_000)
            + (pushed_frames as u128 * 1_000_000_000_u128 / sample_rate_hz as u128) as u64;
        let frames_in_chunk = (complete / decoded_bytes_per_frame) as u64;
        let buffer_ref = buffer.get_mut().expect("fresh buffer");
        buffer_ref.set_pts(gst::ClockTime::from_nseconds(pts_ns));
        buffer_ref.set_duration(gst::ClockTime::from_nseconds(
            frames_in_chunk.saturating_mul(1_000_000_000) / sample_rate_hz as u64,
        ));
        appsrc
            .push_buffer(buffer)
            .map_err(|err| AppError::GStreamer(format!("appsrc push failed: {err:?}")))?;
        pushed_frames += frames_in_chunk;
        remaining = remaining.saturating_sub(complete);
    }
    appsrc
        .end_of_stream()
        .map_err(|err| AppError::GStreamer(format!("appsrc EOS failed: {err:?}")))?;

    let deadline = Instant::now() + Duration::from_secs(60);
    let mut output_frames = 0_u64;
    loop {
        if let Some(sample) = appsink.try_pull_sample(gst::ClockTime::from_mseconds(100)) {
            let buffer = sample
                .buffer()
                .ok_or_else(|| AppError::GStreamer("rendered sample has no buffer".to_string()))?;
            let map = buffer
                .map_readable()
                .map_err(|_| AppError::GStreamer("rendered buffer is not readable".to_string()))?;
            let slice = map.as_slice();
            let complete = slice.len() / 4 * 4;
            let samples: Vec<f32> = slice[..complete]
                .chunks_exact(4)
                .map(|bytes| f32::from_le_bytes(bytes.try_into().expect("4 bytes")))
                .collect();
            writer.write_f32_interleaved(&samples)?;
            output_frames += (samples.len() / channels as usize) as u64;
        } else if appsink.is_eos() {
            break;
        } else if Instant::now() >= deadline {
            let _ = pipeline.set_state(gst::State::Null);
            return Err(AppError::GStreamer(format!(
                "timed out rendering segment {} at {} us",
                segment.index, segment.source_start_us
            )));
        }
    }
    let _ = pipeline.set_state(gst::State::Null);
    Ok(output_frames)
}

fn audio_caps(sample_rate_hz: u32, channels: u16) -> gst::Caps {
    gst::Caps::builder("audio/x-raw")
        .field("format", "F32LE")
        .field("rate", sample_rate_hz as i32)
        .field("channels", channels as i32)
        .field("layout", "interleaved")
        .build()
}

fn ensure_gst() -> AppResult<()> {
    let result = GST_INIT.get_or_init(|| {
        gst::init().map_err(|error| format!("GStreamer initialization failed: {error}"))
    });
    result
        .as_ref()
        .map(|_| ())
        .map_err(|error| AppError::GStreamer(error.clone()))
}

fn gst_error(error: gst::glib::BoolError) -> AppError {
    AppError::GStreamer(error.to_string())
}

fn state_error(error: gst::StateChangeError) -> AppError {
    AppError::GStreamer(format!("state change failed: {error:?}"))
}

pub fn play_artifact(output_dir: &Path) -> AppResult<gst::Element> {
    ensure_gst()?;
    let path = output_dir.join(OUTPUT_WAV);
    if !path.is_file() {
        return Err(AppError::InvalidInput(format!(
            "{} does not exist",
            path.display()
        )));
    }
    let uri = gst::glib::filename_to_uri(&path, None::<&str>)
        .map_err(|error| AppError::InvalidInput(format!("invalid output URI: {error}")))?;
    let playbin = gst::ElementFactory::make("playbin")
        .property("uri", uri)
        .build()
        .map_err(gst_error)?;
    playbin
        .set_state(gst::State::Playing)
        .map_err(state_error)?;
    Ok(playbin)
}
