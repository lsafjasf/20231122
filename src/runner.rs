use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde::Serialize;

use crate::checkpoint::{self, Checkpoint};
use crate::evidence::EvidenceLog;
use crate::gst_sampler::{self, DecoderChoice};
use crate::model::{Degradation, SampleResult, SamplingPlan};
use crate::plan::{choose_degradation, effective_point, plan_sha256};

#[derive(Clone, Debug)]
pub struct RunConfig {
    pub out_dir: PathBuf,
    pub resume: bool,
    pub pause_after: Option<usize>,
    pub pause_flag: Option<Arc<AtomicBool>>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunEvent {
    Resumed { from_index: usize, total: usize },
    Sample { result: SampleResult },
    CheckpointSaved { next_index: usize },
    Paused { next_index: usize, reason: String },
    Completed { samples: usize },
}

#[derive(Clone, Debug)]
pub enum RunOutcome {
    Paused(Checkpoint),
    Completed(Checkpoint),
}

impl RunOutcome {
    pub fn checkpoint(&self) -> &Checkpoint {
        match self {
            Self::Paused(checkpoint) | Self::Completed(checkpoint) => checkpoint,
        }
    }
}

pub fn run(
    plan: &SamplingPlan,
    config: &RunConfig,
    evidence: &mut EvidenceLog,
    mut on_event: impl FnMut(RunEvent),
) -> Result<RunOutcome> {
    gst_sampler::init()?;
    let plan_hash = plan_sha256(plan)?;
    let decoder = gst_sampler::choose_decoder();

    let mut checkpoint_data = if config.resume && checkpoint::exists(&config.out_dir) {
        let loaded = checkpoint::load(&config.out_dir)?;
        loaded.validate(plan, &plan_hash)?;
        evidence.append(
            "resumed",
            serde_json::json!({
                "from_index": loaded.next_index,
                "run_id": loaded.run_id,
                "total": loaded.total,
            }),
        )?;
        on_event(RunEvent::Resumed {
            from_index: loaded.next_index,
            total: loaded.total,
        });
        loaded
    } else {
        checkpoint::remove(&config.out_dir)?;
        let run_id = new_run_id();
        let degradation = choose_degradation(
            plan.source.width,
            plan.source.height,
            plan.params.requested_scale_percent,
            plan.params.memory_budget_bytes,
            &decoder.name,
        );
        let cp = Checkpoint::new(run_id.clone(), plan_hash.clone(), plan, degradation.clone());
        evidence.append(
            "run_started",
            serde_json::json!({
                "run_id": run_id,
                "plan_sha256": plan_hash,
                "source": plan.source.path,
                "source_sha256": plan.source.sha256,
                "total_samples": plan.samples.len(),
                "mem_budget": plan.params.memory_budget_bytes,
                "requested_scale_percent": plan.params.requested_scale_percent,
                "pause_after": plan.params.pause_after,
                "extreme": {
                    "seek_timeout_ms": plan.params.seek_timeout_ms,
                    "final_seek_timeout_ms": plan.params.final_seek_timeout_ms,
                    "repeat_count": plan.params.repeat_count,
                }
            }),
        )?;
        record_startup_evidence(&decoder, &degradation, evidence, &config.out_dir, &run_id)?;
        checkpoint::save(&config.out_dir, &cp)?;
        cp
    };

    if checkpoint_data.done {
        evidence.append(
            "run_finished",
            serde_json::json!({
                "run_id": checkpoint_data.run_id,
                "samples": checkpoint_data.results.len(),
                "already_done": true,
            }),
        )?;
        on_event(RunEvent::Completed {
            samples: checkpoint_data.results.len(),
        });
        return Ok(RunOutcome::Completed(checkpoint_data));
    }

    if !config.resume {
        record_pipeline_ready(&checkpoint_data, evidence)?;
    } else {
        evidence.append(
            "pipeline_ready",
            serde_json::json!({
                "decoder": checkpoint_data.decoder,
                "scale_percent": checkpoint_data.scale_percent,
                "resumed": true,
            }),
        )?;
    }

    let start = checkpoint_data.next_index;
    let mut processed_this_run = 0usize;
    let mut group_timestamps: Vec<u64> = plan.samples[start..]
        .iter()
        .map(|sample| sample.timestamp_ms)
        .collect();
    group_timestamps.dedup();

    for timestamp_ms in group_timestamps {
        let pending: Vec<_> = plan.samples[start..]
            .iter()
            .filter(|sample| sample.timestamp_ms == timestamp_ms)
            .cloned()
            .collect();
        if pending.is_empty() {
            continue;
        }
        if should_pause(config) {
            return pause(
                config,
                &checkpoint_data,
                evidence,
                &mut on_event,
                "pause-requested",
            );
        }
        let timeout_ms = if timestamp_ms == plan.samples.last().unwrap().timestamp_ms {
            plan.params.final_seek_timeout_ms
        } else {
            plan.params.seek_timeout_ms
        };
        let frame = gst_sampler::sample_frame(
            Path::new(&plan.source.path),
            timestamp_ms,
            checkpoint_data.scale_percent,
            timeout_ms,
            &checkpoint_data.decoder,
        )
        .with_context(|| {
            format!(
                "sampling frame at {timestamp_ms} ms, source offset start {}",
                pending[0].raw_offset
            )
        })?;

        for spec in pending {
            let (effective_x, effective_y) = effective_point(
                spec.x,
                spec.y,
                plan.source.width,
                plan.source.height,
                checkpoint_data.scale_percent,
            );
            let rgb = frame.rgb_at(effective_x, effective_y)?;
            let effective_offset =
                (effective_y as u64 * frame.width as u64 + effective_x as u64) * 4;
            let degraded_reason = if checkpoint_data.scale_percent
                < plan.params.requested_scale_percent
                || !checkpoint_data.degradation.budget_satisfied
            {
                Some(checkpoint_data.degradation.reason.clone())
            } else {
                None
            };
            let result = SampleResult {
                seq: spec.seq,
                timestamp_ms: spec.timestamp_ms,
                landed_timestamp_ms: frame.landed_timestamp_ms,
                x: spec.x,
                y: spec.y,
                raw_offset: spec.raw_offset,
                effective_x,
                effective_y,
                scale_percent: checkpoint_data.scale_percent,
                effective_offset,
                rgb,
                frame_sha256: frame.frame_sha256.clone(),
                degraded_reason,
            };
            checkpoint_data.push(result.clone())?;
            checkpoint::save(&config.out_dir, &checkpoint_data)?;
            evidence.append("sample", &result)?;
            on_event(RunEvent::Sample {
                result: result.clone(),
            });
            on_event(RunEvent::CheckpointSaved {
                next_index: checkpoint_data.next_index,
            });
            processed_this_run += 1;
            if should_pause(config)
                || config
                    .pause_after
                    .is_some_and(|limit| processed_this_run >= limit)
                    && checkpoint_data.next_index < checkpoint_data.total
            {
                return pause(
                    config,
                    &checkpoint_data,
                    evidence,
                    &mut on_event,
                    "pause_after",
                );
            }
        }
    }

    checkpoint_data.done = true;
    checkpoint_data.updated_at = crate::model::now_rfc3339();
    checkpoint::save(&config.out_dir, &checkpoint_data)?;
    evidence.append(
        "run_finished",
        serde_json::json!({
            "run_id": checkpoint_data.run_id,
            "samples": checkpoint_data.results.len(),
        }),
    )?;
    on_event(RunEvent::Completed {
        samples: checkpoint_data.results.len(),
    });
    Ok(RunOutcome::Completed(checkpoint_data))
}

fn record_startup_evidence(
    decoder: &DecoderChoice,
    degradation: &Degradation,
    evidence: &mut EvidenceLog,
    out_dir: &Path,
    run_id: &str,
) -> Result<()> {
    if let Some(reason) = &decoder.fallback_reason {
        evidence.append("fallback", serde_json::json!({ "reason": reason }))?;
    }
    if degradation.effective_scale_percent < degradation.requested_scale_percent {
        evidence.append(
            "degraded",
            serde_json::json!({
                "reason": "mem_budget",
                "mem_budget": degradation.memory_budget_bytes,
                "requested_scale_percent": degradation.requested_scale_percent,
                "scale_percent": degradation.effective_scale_percent,
                "effective_size": [degradation.effective_width, degradation.effective_height],
                "frame_buffer_bytes": degradation.frame_buffer_bytes,
                "budget_satisfied": degradation.budget_satisfied,
                "note": "frames downscaled before buffering; offsets scaled accordingly",
            }),
        )?;
    }
    if !degradation.budget_satisfied {
        evidence.append(
            "resource_shortage",
            serde_json::json!({
                "run_id": run_id,
                "out_dir": out_dir,
                "mem_budget": degradation.memory_budget_bytes,
                "minimum_frame_bytes": degradation.effective_width as u64
                    * degradation.effective_height as u64 * 4,
                "policy": "continue at minimum scale and mark every sample degraded",
            }),
        )?;
    }
    Ok(())
}

fn record_pipeline_ready(checkpoint: &Checkpoint, evidence: &mut EvidenceLog) -> Result<()> {
    evidence.append(
        "pipeline_ready",
        serde_json::json!({
            "decoder": checkpoint.decoder,
            "scale_percent": checkpoint.scale_percent,
            "effective_size": [
                checkpoint.degradation.effective_width,
                checkpoint.degradation.effective_height
            ],
        }),
    )
}

fn should_pause(config: &RunConfig) -> bool {
    config
        .pause_flag
        .as_ref()
        .is_some_and(|flag| flag.load(Ordering::SeqCst))
}

fn pause(
    _config: &RunConfig,
    checkpoint_data: &Checkpoint,
    evidence: &mut EvidenceLog,
    on_event: &mut impl FnMut(RunEvent),
    reason: &str,
) -> Result<RunOutcome> {
    evidence.append(
        "paused",
        serde_json::json!({
            "next_index": checkpoint_data.next_index,
            "reason": reason,
            "total": checkpoint_data.total,
        }),
    )?;
    on_event(RunEvent::Paused {
        next_index: checkpoint_data.next_index,
        reason: reason.to_string(),
    });
    Ok(RunOutcome::Paused(checkpoint_data.clone()))
}

fn new_run_id() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    format!("run-{millis}")
}

pub fn write_results(out_dir: &Path, checkpoint_data: &Checkpoint) -> Result<PathBuf> {
    std::fs::create_dir_all(out_dir)?;
    let path = out_dir.join("results.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&checkpoint_data.results)?)?;
    Ok(path)
}

pub fn require_complete(outcome: RunOutcome, expected: usize) -> Result<Checkpoint> {
    match outcome {
        RunOutcome::Paused(cp) => bail!(
            "processing paused at {}/{}; resume to finish",
            cp.next_index,
            expected
        ),
        RunOutcome::Completed(cp) if cp.results.len() == expected => Ok(cp),
        RunOutcome::Completed(cp) => bail!(
            "processing completed with {} results, expected {expected}",
            cp.results.len()
        ),
    }
}
