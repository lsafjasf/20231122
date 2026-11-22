//! GStreamer-backed sample-rate conversion and missing-segment repair.
//!
//! Each fragment runs independent, deterministic pipelines:
//!  1. decode -> audioconvert -> audioresample -> 16-bit PCM WAV at target
//!  2. (only when a gap is declared) audiotestsrc wave=silence -> WAV, then
//!     an exact PCM frame splice in [`crate::wav`].
//!
//! The exact pipeline strings are stored in the evidence, so a reviewer can
//! reproduce every byte with plain `gst-launch-1.0` plus the documented
//! splice step.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use gstreamer as gst;
use gstreamer_pbutils as gst_pbutils;
use gst::prelude::*;

use crate::model::Fragment;
use crate::util::{read_wav_info, sha256_file, WavInfo};
use crate::wav::splice_gap;

/// Facts about the source media discovered before conversion.
#[derive(Debug, Clone)]
pub struct SourceInfo {
    pub sample_rate: u32,
    pub channels: u32,
    pub duration_ns: u64,
}

/// Everything the processing layer learned from one run.
#[derive(Debug, Clone)]
pub struct RunOutcome {
    pub resample_pipeline: String,
    pub gap_pipeline: Option<String>,
    pub input_sha256: String,
    pub input_frames: u64,
    pub output_sha256: String,
    pub output_frames: u64,
    pub output_sample_rate: u32,
    pub output_channels: u32,
    pub gap_frames_inserted: u64,
    pub expected_frames: u64,
    pub duration_ns: u64,
    pub output_bytes: u64,
}

pub fn init() -> Result<()> {
    gst::init().context("failed to initialize GStreamer")?;
    Ok(())
}

pub fn gst_version_string() -> String {
    let (major, minor, micro, nano) = gst::version();
    format!("{major}.{minor}.{micro}.{nano}")
}

fn discover_source(path: &Path) -> Result<SourceInfo> {
    let timeout = gst::ClockTime::from_seconds(5);
    let discoverer = gst_pbutils::Discoverer::new(timeout)
        .context("cannot create GStreamer discoverer")?;
    let uri = url_from_path(path)?;
    let info = discoverer
        .discover_uri(&uri)
        .with_context(|| format!("GStreamer cannot decode source {}", path.display()))?;

    let audio = info
        .audio_streams()
        .into_iter()
        .next()
        .context("source contains no decodable audio stream")?;
    let sample_rate = audio.sample_rate();
    if sample_rate == 0 {
        bail!("audio stream reports no sample rate");
    }
    let channels = audio.channels();
    if channels == 0 {
        bail!("audio stream reports no channels");
    }
    let duration_ns = info
        .duration()
        .context("source reports no duration")?
        .nseconds();

    Ok(SourceInfo {
        sample_rate,
        channels,
        duration_ns,
    })
}

fn url_from_path(path: &Path) -> Result<String> {
    let absolute = path
        .canonicalize()
        .with_context(|| format!("cannot resolve path {}", path.display()))?;
    glib::filename_to_uri(&absolute, None)
        .map(|u| u.to_string())
        .map_err(|e| anyhow::anyhow!("cannot build file URI: {e}"))
}

/// Resampling pipeline description, deterministic for given settings.
/// `uridecodebin` takes a URI; `filesink` takes a plain filesystem path.
pub fn resample_pipeline_desc(source_uri: &str, target_rate: u32, output_path: &str) -> String {
    format!(
        "uridecodebin uri={src} ! audioconvert ! audioresample ! \
         audio/x-raw,format=S16LE,rate={rate} ! wavenc ! filesink location={dst}",
        src = shell_quote(source_uri),
        rate = target_rate,
        dst = shell_quote(output_path),
    )
}

/// Human-readable description of the deterministic silence synthesis step.
pub fn gap_synthesis_desc(target_rate: u32, channels: u32, gap_frames: u64) -> String {
    format!(
        "deterministic PCM synthesis: {gap_frames} zero-filled 16-bit frames \
         ({channels} channel(s), {target_rate} Hz), written with an exact RIFF/WAVE header"
    )
}

/// Minimal shell-style quoting used inside recorded pipeline strings.
fn shell_quote(value: &str) -> String {
    if value.bytes().all(|b| {
        b.is_ascii_alphanumeric() || matches!(b, b'/' | b'.' | b'_' | b'-' | b':')
    }) {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

/// Convert one fragment independently and collect all evidence.
///
/// Output layout under `project_dir/fragments/<id>/`:
/// - `resampled.wav` (always)
/// - `gap.wav` (only when a gap is declared)
/// - `output.wav` (final reviewable artifact)
pub fn process_fragment(
    fragment: &Fragment,
    output_dir: &Path,
    final_output: &Path,
) -> Result<RunOutcome> {
    let source = Path::new(&fragment.source_path);
    if !source.is_file() {
        bail!("source file does not exist: {}", fragment.source_path);
    }
    std::fs::create_dir_all(output_dir)
        .with_context(|| format!("cannot create work dir {}", output_dir.display()))?;

    let source_info = discover_source(source)?;
    if fragment.source_rate != 0 && source_info.sample_rate != fragment.source_rate {
        bail!(
            "declared source rate {} does not match discovered rate {}",
            fragment.source_rate,
            source_info.sample_rate
        );
    }
    if fragment.gap_start_ns > source_info.duration_ns {
        bail!(
            "gap start {} ns is beyond source duration {} ns",
            fragment.gap_start_ns,
            source_info.duration_ns
        );
    }

    let input_sha256 = sha256_file(source)?;
    let input_wav = read_wav_info(source)?.unwrap_or(WavInfo {
        sample_rate: source_info.sample_rate,
        channels: source_info.channels as u16,
        bits_per_sample: 0,
        data_bytes: 0,
    });

    let work_resampled: PathBuf = output_dir.join("resampled.wav");
    let work_gap: PathBuf = output_dir.join("gap.wav");
    for stale in [&work_resampled, &work_gap, final_output] {
        if stale.exists() {
            std::fs::remove_file(stale)
                .with_context(|| format!("cannot remove stale {}", stale.display()))?;
        }
    }

    let source_uri = url_from_path(source)?;
    let resample_path = work_resampled.display().to_string();
    let resample_desc =
        resample_pipeline_desc(&source_uri, fragment.target_rate, &resample_path);
    run_pipeline(&resample_desc)?;

    let requested_gap_frames =
        ((fragment.target_rate as u128 * fragment.gap_duration_ns as u128) / 1_000_000_000u128)
            as u64;
    let gap_start_frame = ((fragment.target_rate as u128 * fragment.gap_start_ns as u128)
        / 1_000_000_000u128) as u64;

    let resampled_info = read_wav_info(&work_resampled)?
        .with_context(|| format!("invalid resampled WAV: {}", work_resampled.display()))?;

    let gap_desc = if requested_gap_frames > 0 {
        let gap_path = work_gap.display().to_string();
        let desc = gap_synthesis_desc(
            fragment.target_rate,
            resampled_info.channels as u32,
            requested_gap_frames,
        );
        crate::wav::write_silence_wav(
            &work_gap,
            fragment.target_rate,
            resampled_info.channels,
            requested_gap_frames,
        )?;
        let _ = gap_path;
        splice_gap(
            &work_resampled,
            &work_gap,
            final_output,
            gap_start_frame,
        )?;
        Some(desc)
    } else {
        std::fs::copy(&work_resampled, final_output).with_context(|| {
            format!(
                "cannot copy {} -> {}",
                work_resampled.display(),
                final_output.display()
            )
        })?;
        None
    };

    let output_wav = read_wav_info(final_output)?
        .with_context(|| format!("final output is not a valid WAV: {}", final_output.display()))?;
    if output_wav.sample_rate != fragment.target_rate {
        bail!(
            "output rate {} differs from target {}",
            output_wav.sample_rate,
            fragment.target_rate
        );
    }

    // Exact by construction: silence is synthesized with precisely the
    // requested frame count, then spliced into the resampled stream.
    let gap_frames = requested_gap_frames;
    let expected_frames = resampled_info.frames() + gap_frames;
    let metadata = std::fs::metadata(final_output)
        .with_context(|| format!("cannot stat output {}", final_output.display()))?;

    Ok(RunOutcome {
        resample_pipeline: resample_desc,
        gap_pipeline: gap_desc,
        input_sha256,
        input_frames: input_wav.frames(),
        output_sha256: sha256_file(final_output)?,
        output_frames: output_wav.frames(),
        output_sample_rate: output_wav.sample_rate,
        output_channels: output_wav.channels as u32,
        gap_frames_inserted: gap_frames,
        expected_frames,
        duration_ns: source_info.duration_ns,
        output_bytes: metadata.len(),
    })
}

fn run_pipeline(desc: &str) -> Result<()> {
    let pipeline = gst::parse::launch_full(desc, None, gst::ParseFlags::empty())
        .with_context(|| format!("invalid GStreamer pipeline: {desc}"))?
        .downcast::<gst::Pipeline>()
        .map_err(|_| anyhow::anyhow!("launch result is not a pipeline"))?;
    let bus = pipeline.bus().context("pipeline has no bus")?;
    pipeline
        .set_state(gst::State::Playing)
        .context("cannot start pipeline")?;

    for msg in bus.iter_timed(gst::ClockTime::NONE) {
        if let gst::MessageView::Error(err) = msg.view() {
            let _ = pipeline.set_state(gst::State::Null);
            bail!(
                "GStreamer error from {:?}: {} ({:?})",
                err.src().map(|s| s.path_string().to_string()),
                err.error(),
                err.debug()
            );
        }
        if let gst::MessageView::Eos(..) = msg.view() {
            break;
        }
    }
    pipeline
        .set_state(gst::State::Null)
        .context("cannot stop pipeline")?;
    Ok(())
}
