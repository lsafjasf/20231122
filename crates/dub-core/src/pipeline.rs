//! The pausable processing loop.
//!
//! Each run of [`run_job`] works on the same repeated object (the job in
//! `checkpoint.json`): it processes pending segments in order, checkpoints
//! after every segment, and stops cleanly when the pause callback fires.
//! On resume it first recomputes the hashes of everything written before
//! the pause and compares them with the checkpoint (before/after
//! comparison), then continues where it left off.

use std::path::{Path, PathBuf};

use anyhow::Context;
use gstreamer::prelude::*;
use serde_json::json;

use crate::clocks::ClockPair;
use crate::degrade::{self, Mode, Policy};
use crate::evidence::{hex_sha256, EvidenceLog};
use crate::media;
use crate::model::{Degradation, DubJob, SegmentResult};
use crate::state;

/// How a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Completed,
    Paused,
}

/// Optional per-run controls.
pub struct RunOptions<'a> {
    pub policy: Policy,
    /// Checked between segments; returning true pauses the run.
    pub should_pause: Box<dyn Fn() -> bool + Send + 'a>,
    /// Called after each segment with (done, total) for progress display.
    pub on_progress: Box<dyn Fn(u32, u32) + Send + 'a>,
}

impl Default for RunOptions<'_> {
    fn default() -> Self {
        Self {
            policy: Policy::default(),
            should_pause: Box::new(|| false),
            on_progress: Box::new(|_, _| {}),
        }
    }
}

fn sha256_file(path: &Path) -> anyhow::Result<String> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    Ok(hex_sha256(&bytes))
}

/// Extract one dubbing track for a time window into `out`.
fn process_segment(
    input: &Path,
    pad_name: &str,
    start_ms: u64,
    duration_ms: u64,
    mode: Mode,
    out: &Path,
) -> anyhow::Result<()> {
    let conv = match mode {
        Mode::Full => "opusdec ! audioconvert ! audioresample ! opusenc ! oggmux".to_string(),
        Mode::LowResource => "opusdec ! audioconvert ! audioresample \
             ! audio/x-raw,rate=8000,channels=1,format=S16LE ! wavenc"
            .to_string(),
    };
    let desc = format!(
        "filesrc location={input} ! matroskademux name=d \
         d.{pad} ! queue ! {conv} ! filesink location={out}",
        input = input.display(),
        pad = pad_name,
        out = out.display()
    );
    let pipeline = gstreamer::parse::launch(&desc)?
        .downcast::<gstreamer::Pipeline>()
        .map_err(|_| anyhow::anyhow!("not a pipeline"))?;
    // Drain every pad we do not consume (e.g. video) into a fakesink,
    // otherwise the demuxer aborts with "not-linked". The queue is
    // required: without it the pipeline deadlocks while prerolling.
    let pipeline_weak = pipeline.downgrade();
    pipeline
        .by_name("d")
        .context("demuxer not found")?
        .connect_pad_added(move |_, pad| {
            if pad.is_linked() {
                return;
            }
            let Some(pipeline) = pipeline_weak.upgrade() else {
                return;
            };
            let Ok(queue) = gstreamer::ElementFactory::make("queue").build() else {
                return;
            };
            let Ok(sink) = gstreamer::ElementFactory::make("fakesink").build() else {
                return;
            };
            if pipeline.add_many([&queue, &sink]).is_err() {
                return;
            }
            if gstreamer::Element::link_many([&queue, &sink]).is_err() {
                return;
            }
            queue.sync_state_with_parent().ok();
            sink.sync_state_with_parent().ok();
            if let Some(sinkpad) = queue.static_pad("sink") {
                let _ = pad.link(&sinkpad);
            }
        });
    pipeline.set_state(gstreamer::State::Paused)?;
    let _ = pipeline.state(gstreamer::ClockTime::from_seconds(10));
    let start = gstreamer::ClockTime::from_mseconds(start_ms);
    let stop = gstreamer::ClockTime::from_mseconds(start_ms + duration_ms);
    // Seek on the demuxer: muxers do not forward seek events, so seeking
    // the whole pipeline would fail.
    let demux = pipeline.by_name("d").context("demuxer not found")?;
    demux.seek(
        1.0,
        gstreamer::SeekFlags::FLUSH | gstreamer::SeekFlags::ACCURATE,
        gstreamer::SeekType::Set,
        Some(start),
        gstreamer::SeekType::Set,
        Some(stop),
    )?;
    pipeline.set_state(gstreamer::State::Playing)?;
    let bus = pipeline.bus().context("pipeline has no bus")?;
    for msg in bus.iter_timed(gstreamer::ClockTime::from_seconds(120)) {
        use gstreamer::MessageView;
        match msg.view() {
            MessageView::Eos(_) => break,
            MessageView::Error(e) => {
                pipeline.set_state(gstreamer::State::Null).ok();
                anyhow::bail!("{}: {}", e.error(), e.debug().unwrap_or_default());
            }
            _ => {}
        }
    }
    pipeline.set_state(gstreamer::State::Null)?;
    Ok(())
}

/// Recompute hashes of results recorded before this run and compare them
/// with the checkpoint. Rows: (segment index, stored, recomputed, matches).
pub fn compare_with_checkpoint(job: &DubJob) -> Vec<(u32, String, String, bool)> {
    let mut rows = Vec::new();
    for res in &job.results {
        let path = Path::new(&job.output_dir).join(&res.file);
        let recomputed = sha256_file(&path).unwrap_or_else(|e| format!("error:{e}"));
        rows.push((
            res.index,
            res.sha256.clone(),
            recomputed.clone(),
            recomputed == res.sha256,
        ));
    }
    rows
}

/// Run (or resume) the job stored in `workdir`.
pub fn run_job(workdir: &Path, opts: &RunOptions) -> anyhow::Result<Outcome> {
    gstreamer::init()?;
    let mut job = state::load_state(workdir)?;
    let clocks = ClockPair::default();
    let mut log = EvidenceLog::open(&workdir.join(state::EVIDENCE_FILE))?;

    // Fresh clock-sync samples for this run; both clocks are stamped on
    // every evidence record, and the sync table maps between them.
    job.clock_sync.observe(&clocks.a, &clocks.b);
    log.record(
        &clocks,
        "run-start",
        serde_json::Map::from_iter([
            ("job_id".into(), json!(job.job_id)),
            ("resuming".into(), json!(job.next_segment > 0)),
            ("next_segment".into(), json!(job.next_segment)),
        ]),
    )?;

    // Before/after comparison on resume: verify pre-pause artifacts still
    // match the checkpoint before trusting them.
    if !job.results.is_empty() {
        let rows = compare_with_checkpoint(&job);
        let mismatches: Vec<u32> = rows.iter().filter(|r| !r.3).map(|r| r.0).collect();
        log.record(
            &clocks,
            "resume-compare",
            serde_json::Map::from_iter([
                ("checked".into(), json!(rows.len())),
                ("matched".into(), json!(rows.len() - mismatches.len())),
                ("mismatched".into(), json!(mismatches)),
            ]),
        )?;
        anyhow::ensure!(
            mismatches.is_empty(),
            "pre-pause artifacts changed on disk: segments {mismatches:?}"
        );
    }

    let tracks = media::list_audio_tracks(Path::new(&job.input))?;
    let total = job.segments.len() as u32;

    while !job.is_complete() {
        if (opts.should_pause)() {
            state::save_state(workdir, &job)?;
            log.record(
                &clocks,
                "run-paused",
                serde_json::Map::from_iter([("next_segment".into(), json!(job.next_segment))]),
            )?;
            return Ok(Outcome::Paused);
        }
        let seg = job.segments[job.next_segment as usize].clone();
        let pad = tracks
            .iter()
            .find(|(_, lang)| {
                media::normalize_language(lang) == media::normalize_language(&seg.language)
            })
            .map(|(pad, _)| pad.clone())
            .with_context(|| format!("no audio track for language {}", seg.language))?;

        // Resource probe + degradation decision for this segment.
        let report = degrade::probe(Path::new(&job.output_dir));
        let decision = opts.policy.decide(&report);
        if let Some((reason, detail)) = &decision.degraded {
            let deg = Degradation {
                segment: seg.index,
                reason: reason.clone(),
                detail: detail.clone(),
                a_ns: clocks.a.now_ns(),
                b_ns: clocks.b.now_ns(),
            };
            log.record(
                &clocks,
                "degrade",
                serde_json::Map::from_iter([
                    ("segment".into(), json!(seg.index)),
                    ("reason".into(), json!(reason)),
                    ("detail".into(), json!(detail)),
                ]),
            )?;
            job.degradations.push(deg);
        }

        let ext = match decision.mode {
            Mode::Full => "ogg",
            Mode::LowResource => "wav",
        };
        let file = format!("seg-{:03}-{}.{}", seg.index, seg.language, ext);
        let out_path: PathBuf = Path::new(&job.output_dir).join(&file);
        process_segment(
            Path::new(&job.input),
            &pad,
            seg.start_ms,
            seg.duration_ms,
            decision.mode,
            &out_path,
        )?;

        let result = SegmentResult {
            index: seg.index,
            language: seg.language.clone(),
            file: file.clone(),
            sha256: sha256_file(&out_path)?,
            bytes: std::fs::metadata(&out_path)?.len(),
            mode: decision.mode.as_str().to_string(),
            a_ns: clocks.a.now_ns(),
            b_ns: clocks.b.now_ns(),
        };
        log.record(
            &clocks,
            "segment-done",
            serde_json::Map::from_iter([
                ("index".into(), json!(result.index)),
                ("language".into(), json!(result.language)),
                ("file".into(), json!(result.file)),
                ("sha256".into(), json!(result.sha256)),
                ("mode".into(), json!(result.mode)),
            ]),
        )?;
        job.results.push(result);
        job.next_segment += 1;
        // Checkpoint after every segment: pause can happen at any point.
        state::save_state(workdir, &job)?;
        (opts.on_progress)(job.next_segment, total);
    }

    job.clock_sync.observe(&clocks.a, &clocks.b);
    state::save_state(workdir, &job)?;
    log.record(
        &clocks,
        "run-completed",
        serde_json::Map::from_iter([
            ("segments".into(), json!(job.results.len())),
            ("degraded".into(), json!(job.degradations.len())),
        ]),
    )?;
    Ok(Outcome::Completed)
}
