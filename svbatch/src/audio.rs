//! GStreamer-backed audio synthesis and variant processing.

use crate::model::{JobSpec, JobState};
use anyhow::{anyhow, Context, Result};
use glib::prelude::Cast;
use gstreamer::prelude::*;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Handle used by callers (UI, CLI, experiment harness) to request a cancel.
#[derive(Clone, Default)]
pub struct CancelToken {
    cancelled: Arc<AtomicBool>,
}

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// Sleeps up to `total_ms`, returning early (false) when cancelled.
    pub fn interruptible_sleep(&self, total_ms: u64) -> bool {
        let mut remaining = total_ms;
        while remaining > 0 {
            if self.is_cancelled() {
                return false;
            }
            let step = remaining.min(20);
            std::thread::sleep(Duration::from_millis(step));
            remaining = remaining.saturating_sub(step);
        }
        !self.is_cancelled()
    }
}

fn hash_file(path: &Path) -> Result<(String, u64)> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let len = bytes.len() as u64;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Ok((hex::encode(hasher.finalize()), len))
}

/// Builds the deterministic GStreamer pipeline description for one job.
/// `output_path` must be absolute so the WAV lands in the evidence directory
/// regardless of the process working directory.
///
/// `audiotestsrc` synthesises the source in-process (no network / mic),
/// `volume` applies gain and the `speed` resampler changes playback speed
/// (and pitch); `wavenc` writes a reproducible WAV container.
pub fn pipeline_description(job: &JobSpec, output_path: &Path) -> String {
    let samples = job.source.duration_ms as u64 * 441 / 10; // 44.1 kHz, 16-bit mono
    format!(
        "audiotestsrc wave={wave} freq={freq} samplesperbuffer=441 num-buffers={buffers} \
         ! audioconvert ! audioresample \
         ! volume volume={gain} \
         ! speed speed={speed} \
         ! audioconvert ! audioresample \
         ! capsfilter caps=audio/x-raw,format=S16LE,rate=44100,channels=1 \
         ! wavenc ! filesink location={location}",
        wave = job.source.wave,
        freq = job.source.freq_hz,
        buffers = (samples / 441).max(1),
        gain = job.variant.gain,
        speed = job.variant.speed,
        location = shell_escape(&output_path.display().to_string()),
    )
}

fn shell_escape(name: &str) -> String {
    // Job ids are restricted to [a-z0-9_-]; quote defensively anyway.
    format!("\"{}\"", name.replace('\\', "\\\\").replace('"', "\\\""))
}

pub struct Engine {
    output_dir: PathBuf,
}

/// Successful artifact: (output filename, sha256 hex, byte length).
pub type Artifact = (String, String, u64);

impl Engine {
    pub fn new(output_dir: impl Into<PathBuf>) -> Result<Self> {
        let output_dir = output_dir.into();
        fs::create_dir_all(&output_dir)
            .with_context(|| format!("create output dir {}", output_dir.display()))?;
        Ok(Self { output_dir })
    }

    pub fn output_dir(&self) -> &Path {
        &self.output_dir
    }

    /// Runs one job to completion or until the token is cancelled.
    pub fn run_job(
        &self,
        job: &JobSpec,
        token: &CancelToken,
    ) -> Result<(JobState, Option<Artifact>)> {
        let target = self.output_dir.join(job.output_filename());
        // A stale file must never be mistaken for fresh evidence.
        let _ = fs::remove_file(&target);

        // Pre-work cancel point: a cancel already requested before start.
        if !token.interruptible_sleep(job.work_delay_ms) {
            return Ok((JobState::Cancelled, None));
        }

        let desc = pipeline_description(job, &target);
        let pipeline = gstreamer::parse::launch(&desc)
            .map_err(|e| anyhow!("pipeline parse failed: {e}"))?
            .dynamic_cast::<gstreamer::Pipeline>()
            .map_err(|_| anyhow!("launch did not produce a pipeline"))?;

        pipeline
            .set_state(gstreamer::State::Playing)
            .map_err(|e| anyhow!("set_state(Playing) failed: {e:?}"))?;

        let bus = pipeline.bus().ok_or_else(|| anyhow!("pipeline has no bus"))?;
        let outcome = loop {
            if token.is_cancelled() {
                pipeline
                    .set_state(gstreamer::State::Null)
                    .map_err(|e| anyhow!("set_state(Null) failed: {e:?}"))?;
                break JobState::Cancelled;
            }
            if let Some(msg) = bus.timed_pop_filtered(
                gstreamer::ClockTime::from_mseconds(50),
                &[
                    gstreamer::MessageType::Error,
                    gstreamer::MessageType::Eos,
                ],
            ) {
                match msg.view() {
                    gstreamer::MessageView::Eos(..) => break JobState::Succeeded,
                    gstreamer::MessageView::Error(err) => {
                        pipeline.set_state(gstreamer::State::Null).ok();
                        return Err(anyhow!(
                            "GStreamer error: {} ({:?})",
                            err.error(),
                            err.debug()
                        ));
                    }
                    _ => {}
                }
            }
        };

        pipeline
            .set_state(gstreamer::State::Null)
            .map_err(|e| anyhow!("set_state(Null) failed: {e:?}"))?;

        if outcome == JobState::Cancelled {
            let _ = fs::remove_file(&target);
            return Ok((JobState::Cancelled, None));
        }

        let (sha, bytes) = hash_file(&target)
            .with_context(|| format!("hashing output {}", target.display()))?;
        Ok((JobState::Succeeded, Some((job.output_filename(), sha, bytes))))
    }
}

/// Initialises GStreamer once per process.
pub fn init_gstreamer() -> Result<()> {
    gstreamer::init().map_err(|e| anyhow!("gstreamer::init failed: {e}"))
}
