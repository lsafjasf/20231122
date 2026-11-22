use std::path::Path;

use anyhow::{bail, Context, Result};
use serde_json::json;

use crate::evidence::{self, Evidence};
use crate::model::{
    Checkpoint, CompareReport, Config, DegradationEvent, FrameRecord, ResumeCompare, FRAME_BYTES,
};
use crate::source::{frame_crc, source_frame};

const ALLOC_OVERHEAD_BYTES: u64 = 15;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    Completed,
    Paused,
}

#[derive(Debug, Clone, Copy)]
pub struct RunOptions {
    pub resume: bool,
    pub pause_at_frame: Option<u64>,
    pub gst_probe: bool,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            resume: false,
            pause_at_frame: None,
            gst_probe: true,
        }
    }
}

struct Runtime {
    next_frame_idx: u64,
    last_pts_ns: Option<u64>,
    processed: u64,
    dropped: u64,
    drift_events: u64,
    crc_chain: u32,
    pool_bytes: u64,
    degraded: bool,
    boundary_before: Option<FrameRecord>,
    boundary_after: Option<FrameRecord>,
    missing_segment_logged: bool,
}

impl Runtime {
    fn fresh() -> Self {
        Self {
            next_frame_idx: 0,
            last_pts_ns: None,
            processed: 0,
            dropped: 0,
            drift_events: 0,
            crc_chain: 0,
            pool_bytes: 0,
            degraded: false,
            boundary_before: None,
            boundary_after: None,
            missing_segment_logged: false,
        }
    }

    fn from_checkpoint(config: &Config, checkpoint: &Checkpoint, frames: &[FrameRecord]) -> Self {
        Self {
            next_frame_idx: checkpoint.next_frame_idx,
            last_pts_ns: frames.last().map(|frame| frame.pts_ns),
            processed: checkpoint.processed,
            dropped: checkpoint.dropped,
            drift_events: checkpoint.drift_events,
            crc_chain: checkpoint.crc_chain,
            pool_bytes: checkpoint.pool_bytes,
            degraded: checkpoint.degraded,
            boundary_before: frames
                .iter()
                .find(|frame| frame.idx == config.boundary_before_idx())
                .cloned(),
            boundary_after: frames
                .iter()
                .find(|frame| frame.idx == config.boundary_after_idx())
                .cloned(),
            missing_segment_logged: checkpoint.next_frame_idx > config.gap_end_frame(),
        }
    }

    fn checkpoint(&self, config: &Config) -> Checkpoint {
        Checkpoint {
            version: 1,
            next_frame_idx: self.next_frame_idx,
            last_pts_ns: self.last_pts_ns.unwrap_or(0),
            processed: self.processed,
            dropped: self.dropped,
            drift_events: self.drift_events,
            crc_chain: self.crc_chain,
            pool_bytes: self.pool_bytes,
            degraded: self.degraded,
            params_hash: config.params_hash(),
            taken_at_unix: evidence::unix_now(),
        }
    }
}

fn extend_chain(chain: u32, crc: u32) -> u32 {
    chain
        .rotate_left(5)
        .wrapping_add(crc)
        .wrapping_mul(0x0100_0193)
}

fn replay_chain(frames: &[FrameRecord]) -> u32 {
    frames
        .iter()
        .fold(0_u32, |chain, frame| extend_chain(chain, frame.crc))
}

fn prepare_fresh_dir(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    for name in [
        "frames.jsonl",
        "events.jsonl",
        "degradation.jsonl",
        "checkpoint.json",
        "final_state.json",
        "resume_compare.json",
        "boundary_before.json",
        "boundary_after.json",
        "boundary_delta.json",
        "boundary_before.ppm",
        "boundary_after.ppm",
        "pipeline.json",
        "manifest.json",
    ] {
        let path = dir.join(name);
        if path.exists() {
            std::fs::remove_file(&path)
                .with_context(|| format!("remove stale {}", path.display()))?;
        }
    }
    Ok(())
}

fn expected_prefix_len(config: &Config, next_frame_idx: u64) -> usize {
    (0..next_frame_idx)
        .filter(|idx| !config.is_missing(*idx))
        .count()
}

fn validate_resume(
    config: &Config,
    checkpoint: &Checkpoint,
    frames: &[FrameRecord],
) -> Result<ResumeCompare> {
    let validation_error = checkpoint.validate_for(config).err();
    let next_in_range = checkpoint.next_frame_idx <= config.frame_count;
    let expected = if next_in_range {
        expected_prefix_len(config, checkpoint.next_frame_idx)
    } else {
        0
    };
    let replayed_chain = replay_chain(frames);
    let params_hash_match = checkpoint.params_hash == config.params_hash();
    let frame_records_match = next_in_range
        && frames.len() == expected
        && frames
            .iter()
            .all(|frame| frame.idx < checkpoint.next_frame_idx);
    let chain_match = replayed_chain == checkpoint.crc_chain;
    let action =
        if validation_error.is_none() && params_hash_match && frame_records_match && chain_match {
            "continue"
        } else {
            "reject"
        };
    let rejection = if action == "continue" {
        None
    } else if let Some(error) = validation_error {
        Some(format!("{error:#}"))
    } else {
        Some(format!(
            "params_match={params_hash_match}, frames_match={frame_records_match}, chain_match={chain_match}"
        ))
    };
    let compare = ResumeCompare {
        checkpoint_next_frame: checkpoint.next_frame_idx,
        existing_frames: frames.len(),
        expected_frames_before_pause: expected as u64,
        params_hash_match,
        frame_records_match,
        checkpoint_crc_chain: checkpoint.crc_chain,
        replayed_crc_chain: replayed_chain,
        action: action.into(),
        rejection,
    };
    Ok(compare)
}

pub fn run(config: &Config, dir: &Path, options: RunOptions) -> Result<RunOutcome> {
    run_controlled(config, dir, options, &|| false)
}

pub fn run_controlled(
    config: &Config,
    dir: &Path,
    options: RunOptions,
    external_pause: &(dyn Fn() -> bool + Sync),
) -> Result<RunOutcome> {
    config.validate()?;
    let pause_at_frame = options.pause_at_frame.or(if options.resume {
        None
    } else {
        config.pause_at_frame
    });
    if !options.resume {
        prepare_fresh_dir(dir)?;
    }
    let mut evidence = Evidence::open(dir)?;
    let mut runtime;
    if options.resume {
        let checkpoint =
            evidence::load_checkpoint(dir).context("resume requires checkpoint.json")?;
        let frames = evidence::load_frames(dir)?;
        let compare = validate_resume(config, &checkpoint, &frames)?;
        evidence.resume_compare(&compare)?;
        evidence.event("resume-compare", serde_json::to_value(&compare)?)?;
        if compare.action != "continue" {
            bail!(
                "resume rejected: params_match={} frames_match={} chain={:08x}/{:08x}",
                compare.params_hash_match,
                compare.frame_records_match,
                compare.replayed_crc_chain,
                compare.checkpoint_crc_chain
            );
        }
        runtime = Runtime::from_checkpoint(config, &checkpoint, &frames);
        evidence.event(
            "run-resume",
            json!({
                "next_frame_idx": checkpoint.next_frame_idx,
                "processed": checkpoint.processed,
                "dropped": checkpoint.dropped,
            }),
        )?;
    } else {
        evidence.write_params(config)?;
        runtime = Runtime::fresh();
        evidence.event(
            "run-start",
            json!({
                "params_hash": format!("{:016x}", config.params_hash()),
                "gap_start_ns": config.gap.start_ns,
                "gap_end_ns": config.gap.end_ns,
                "extreme_start_frame": config.extreme_start_frame(),
                "extreme_end_frame": config.extreme_end_frame(),
            }),
        )?;
    }

    while runtime.next_frame_idx < config.frame_count {
        if external_pause() || pause_at_frame.is_some_and(|pause| runtime.next_frame_idx >= pause) {
            let checkpoint = runtime.checkpoint(config);
            evidence::save_checkpoint(dir, &checkpoint)?;
            evidence.event(
                "run-paused",
                json!({
                    "next_frame_idx": checkpoint.next_frame_idx,
                    "last_pts_ns": checkpoint.last_pts_ns,
                    "crc_chain": format!("{:08x}", checkpoint.crc_chain),
                }),
            )?;
            return Ok(RunOutcome::Paused);
        }

        let idx = runtime.next_frame_idx;
        let Some(source) = source_frame(config, idx) else {
            if !runtime.missing_segment_logged {
                evidence.event(
                    "missing-segment",
                    json!({
                        "start_ns": config.gap.start_ns,
                        "end_ns": config.gap.end_ns,
                        "first_missing_frame": idx,
                        "after_resume_or_gap": true,
                    }),
                )?;
                runtime.missing_segment_logged = true;
            }
            runtime.next_frame_idx += 1;
            continue;
        };

        let drift_ns = runtime
            .last_pts_ns
            .map(|previous| {
                i128::from(source.pts_ns)
                    - i128::from(previous)
                    - i128::from(config.frame_period_ns())
            })
            .unwrap_or(0)
            .clamp(i64::MIN as i128, i64::MAX as i128) as i64;

        let incoming = FRAME_BYTES as u64 + ALLOC_OVERHEAD_BYTES;
        let over_budget = runtime.pool_bytes.saturating_add(incoming) > config.max_pool_bytes;
        if over_budget && !runtime.degraded {
            runtime.degraded = true;
            let event = DegradationEvent {
                at_pts_ns: source.pts_ns,
                kind: "enter-degraded".into(),
                pool_bytes: runtime.pool_bytes,
                budget_bytes: config.max_pool_bytes,
                detail: format!(
                    "pool would exceed budget; keeping 1/{} frames, queue={} buffers/{} bytes",
                    config.extreme.degrade_keep_divisor,
                    config.extreme.queue_max_buffers,
                    config.extreme.queue_max_bytes
                ),
            };
            evidence.degradation(&event)?;
            evidence.event("degrade", serde_json::to_value(&event)?)?;
        }

        let keep = !runtime.degraded
            || source
                .idx
                .is_multiple_of(u64::from(config.extreme.degrade_keep_divisor));
        let crc = if keep { frame_crc(source.idx) } else { 0 };
        let record = FrameRecord {
            idx: source.idx,
            pts_ns: source.pts_ns,
            jitter_ns: source.jitter_ns,
            drift_ns,
            crc,
            degraded: runtime.degraded,
            in_extreme_window: source.in_extreme_window,
        };
        evidence.frame(&record)?;
        runtime.crc_chain = extend_chain(runtime.crc_chain, crc);
        if keep {
            runtime.processed += 1;
        } else {
            runtime.dropped += 1;
        }

        let drain = if runtime.degraded && !source.in_extreme_window {
            incoming.saturating_mul(2)
        } else if source.in_extreme_window {
            incoming / 4
        } else {
            incoming
        };
        runtime.pool_bytes = runtime
            .pool_bytes
            .saturating_add(incoming)
            .saturating_sub(drain);

        if runtime.degraded
            && !source.in_extreme_window
            && runtime.pool_bytes <= config.max_pool_bytes / 2
        {
            runtime.degraded = false;
            let event = DegradationEvent {
                at_pts_ns: source.pts_ns,
                kind: "exit-degraded".into(),
                pool_bytes: runtime.pool_bytes,
                budget_bytes: config.max_pool_bytes,
                detail: "pool drained below low-water mark; full replay restored".into(),
            };
            evidence.degradation(&event)?;
            evidence.event("degrade-recovered", serde_json::to_value(&event)?)?;
        }

        if drift_ns.unsigned_abs() > config.extreme.drift_tolerance_ns {
            runtime.drift_events += 1;
            evidence.event(
                "drift-threshold",
                json!({
                    "frame_idx": record.idx,
                    "drift_ns": drift_ns,
                    "tolerance_ns": config.extreme.drift_tolerance_ns,
                }),
            )?;
        }
        if record.idx == config.boundary_before_idx() {
            runtime.boundary_before = Some(record.clone());
        }
        if record.idx == config.boundary_after_idx() {
            runtime.boundary_after = Some(record.clone());
        }
        runtime.last_pts_ns = Some(source.pts_ns);
        runtime.next_frame_idx += 1;
    }

    let checkpoint = runtime.checkpoint(config);
    evidence.final_state(&checkpoint)?;
    if let (Some(before), Some(after)) = (
        runtime.boundary_before.as_ref(),
        runtime.boundary_after.as_ref(),
    ) {
        let delta = evidence.boundary(config, before, after)?;
        evidence.event("boundary-samples", serde_json::to_value(&delta)?)?;
    }
    evidence.event(
        "run-complete",
        json!({
            "processed": checkpoint.processed,
            "dropped": checkpoint.dropped,
            "drift_events": checkpoint.drift_events,
            "crc_chain": format!("{:08x}", checkpoint.crc_chain),
        }),
    )?;
    if options.gst_probe {
        match crate::gst_bridge::probe(config, dir) {
            Ok(probe) => {
                evidence.event(
                    "gst-pipeline",
                    json!({
                        "status": probe.status,
                        "caps": probe.caps,
                        "crc_match": probe.crc_match,
                        "frames": probe.output_frames,
                    }),
                )?;
            }
            Err(error) => {
                let fallback = json!({
                    "status": "degraded",
                    "backend": "Rust deterministic replay",
                    "error": format!("{error:#}"),
                    "reason": "GStreamer probe unavailable; simulation evidence remains valid",
                });
                evidence::write_json(&dir.join("pipeline.json"), &fallback)?;
                evidence.event("gst-degraded", fallback)?;
            }
        }
    }
    evidence.manifest(config)?;
    Ok(RunOutcome::Completed)
}

pub fn compare_runs(run_a: &Path, run_b: &Path, report_path: &Path) -> Result<CompareReport> {
    let frames_a = evidence::load_frames(run_a)?;
    let frames_b = evidence::load_frames(run_b)?;
    let first_mismatch = frames_a
        .iter()
        .zip(&frames_b)
        .find_map(|(left, right)| (left != right).then_some(left.idx))
        .or_else(|| {
            (frames_a.len() != frames_b.len()).then(|| frames_a.len().min(frames_b.len()) as u64)
        });
    let chain_a = evidence::load_crc_chain(run_a)?;
    let chain_b = evidence::load_crc_chain(run_b)?;
    let identical_sequence =
        frames_a == frames_b && frames_a.len() == frames_b.len() && chain_a == chain_b;
    let report = CompareReport {
        run_a: run_a.display().to_string(),
        run_b: run_b.display().to_string(),
        frames_a: frames_a.len(),
        frames_b: frames_b.len(),
        identical_sequence,
        first_mismatch,
        crc_chain_a: chain_a,
        crc_chain_b: chain_b,
        verdict: if identical_sequence {
            "MATCH: pause/resume processing is equivalent to uninterrupted".into()
        } else {
            "MISMATCH: inspect first_mismatch and boundary evidence".into()
        },
    };
    Evidence::write_compare(report_path, &report)?;
    Ok(report)
}
