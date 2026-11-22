//! Sequential batch engine with duplicate-submit detection and replay logs.

use crate::audio::{CancelToken, Engine};
use crate::model::{BatchReport, BatchSnapshot, JobResult, JobSpec, JobState, StepRecord};
use anyhow::Result;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Instant;

/// Result of submitting a job to the registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmitOutcome {
    Accepted,
    Duplicate,
}

pub struct BatchConfig {
    pub run_id: String,
    pub mode: String,
    pub output_dir: std::path::PathBuf,
}

struct Progress<'a> {
    started: Instant,
    seq: u64,
    states: HashMap<String, JobState>,
    steps: Vec<StepRecord>,
    snapshots: Vec<BatchSnapshot>,
    snapshot_cb: Option<&'a dyn Fn(&BatchSnapshot)>,
}

impl<'a> Progress<'a> {
    fn new(jobs: &[JobSpec], snapshot_cb: Option<&'a dyn Fn(&BatchSnapshot)>) -> Self {
        let states = jobs
            .iter()
            .map(|j| (j.job_id.clone(), JobState::Pending))
            .collect();
        Self {
            started: Instant::now(),
            seq: 0,
            states,
            steps: Vec::new(),
            snapshots: Vec::new(),
            snapshot_cb,
        }
    }

    fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }

    fn elapsed_ms(&self) -> u128 {
        self.started.elapsed().as_millis()
    }

    fn set_state(&mut self, job_id: &str, state: JobState) {
        self.states.insert(job_id.to_string(), state);
    }

    fn record(
        &mut self,
        phase: &str,
        job_id: Option<&str>,
        input: serde_json::Value,
        state: serde_json::Value,
        result: serde_json::Value,
    ) {
        let seq = self.next_seq();
        let step = StepRecord {
            seq,
            elapsed_ms: self.elapsed_ms(),
            phase: phase.to_string(),
            job_id: job_id.map(|s| s.to_string()),
            input,
            state,
            result,
        };
        self.steps.push(step);
    }

    fn snapshot(&mut self) {
        let snap = BatchSnapshot {
            seq: self.next_seq(),
            elapsed_ms: self.elapsed_ms(),
            states: self
                .states
                .iter()
                .map(|(k, v)| (k.clone(), *v))
                .collect(),
        };
        if let Some(cb) = self.snapshot_cb {
            cb(&snap);
        }
        self.snapshots.push(snap);
    }
}

/// Registry shared by all batches in a process: enforces "no duplicate
/// submission" by idempotency key.
#[derive(Default)]
pub struct SubmissionRegistry {
    seen: HashSet<String>,
}

impl SubmissionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn submit(&mut self, job: &JobSpec) -> SubmitOutcome {
        let key = job.idempotency_key();
        if self.seen.insert(key) {
            SubmitOutcome::Accepted
        } else {
            SubmitOutcome::Duplicate
        }
    }

    pub fn contains(&self, job: &JobSpec) -> bool {
        self.seen.contains(&job.idempotency_key())
    }
}

pub fn run_batch(
    cfg: &BatchConfig,
    jobs: &[JobSpec],
    token: &CancelToken,
    registry: &mut SubmissionRegistry,
    snapshot_cb: Option<&dyn Fn(&BatchSnapshot)>,
) -> Result<BatchReport> {
    let engine = Engine::new(&cfg.output_dir)?;
    let mut progress = Progress::new(jobs, snapshot_cb);
    let mut results: Vec<JobResult> = Vec::new();
    let started_at = now_iso8601();

    progress.record(
        "batch_start",
        None,
        json!({ "run_id": cfg.run_id, "mode": cfg.mode, "job_count": jobs.len() }),
        json!({ "token_cancelled": token.is_cancelled() }),
        json!({ "submitted_jobs": jobs.iter().map(|j| j.job_id.clone()).collect::<Vec<_>>() }),
    );
    progress.snapshot();

    for job in jobs {
        // Duplicate submission gate (also enforced across batches).
        match registry.submit(job) {
            SubmitOutcome::Duplicate => {
                progress.set_state(&job.job_id, JobState::Failed);
                progress.record(
                    "submit_rejected",
                    Some(&job.job_id),
                    json!({ "idempotency_key": job.idempotency_key() }),
                    json!({ "state": "rejected_duplicate" }),
                    json!({ "outcome": "duplicate_submission_blocked" }),
                );
                results.push(JobResult {
                    job_id: job.job_id.clone(),
                    state: JobState::Failed,
                    output_file: None,
                    sha256: None,
                    bytes: None,
                    detail: "duplicate submission blocked".to_string(),
                });
                progress.snapshot();
                continue;
            }
            SubmitOutcome::Accepted => {
                progress.record(
                    "submit_accepted",
                    Some(&job.job_id),
                    serde_json::to_value(job)?,
                    json!({ "state_before": JobState::Pending.as_str() }),
                    json!({ "idempotency_key": job.idempotency_key() }),
                );
            }
        }

        progress.set_state(&job.job_id, JobState::Running);
        progress.record(
            "job_start",
            Some(&job.job_id),
                    json!({ "pipeline": crate::audio::pipeline_description(
                        job,
                        &engine.output_dir().join(job.output_filename())
                    ) }),
            json!({ "state": JobState::Running.as_str() }),
            json!(null),
        );
        progress.snapshot();

        match engine.run_job(job, token) {
            Ok((state, artifact)) => {
                progress.set_state(&job.job_id, state);
                let result = match (state, artifact) {
                    (JobState::Succeeded, Some((file, sha, bytes))) => JobResult {
                        job_id: job.job_id.clone(),
                        state,
                        output_file: Some(file),
                        sha256: Some(sha),
                        bytes: Some(bytes),
                        detail: "ok".to_string(),
                    },
                    (JobState::Cancelled, None) => JobResult {
                        job_id: job.job_id.clone(),
                        state,
                        output_file: None,
                        sha256: None,
                        bytes: None,
                        detail: "cancelled before completion; partial output removed".to_string(),
                    },
                    other => JobResult {
                        job_id: job.job_id.clone(),
                        state: other.0,
                        output_file: None,
                        sha256: None,
                        bytes: None,
                        detail: format!("unexpected engine outcome: {:?}", other.0),
                    },
                };
                progress.record(
                    "job_end",
                    Some(&job.job_id),
                    json!({ "job_id": job.job_id }),
                    json!({ "state": state.as_str(), "token_cancelled": token.is_cancelled() }),
                    serde_json::to_value(&result)?,
                );
                results.push(result);
            }
            Err(err) => {
                progress.set_state(&job.job_id, JobState::Failed);
                let result = JobResult {
                    job_id: job.job_id.clone(),
                    state: JobState::Failed,
                    output_file: None,
                    sha256: None,
                    bytes: None,
                    detail: err.to_string(),
                };
                progress.record(
                    "job_end",
                    Some(&job.job_id),
                    json!({ "job_id": job.job_id }),
                    json!({ "state": JobState::Failed.as_str() }),
                    serde_json::to_value(&result)?,
                );
                results.push(result);
            }
        }
        progress.snapshot();

        // A cancelled token means every remaining job stays pending and the
        // batch stops immediately (mid-run cancel evidence).
        if token.is_cancelled() {
            let handled: HashSet<&str> =
                results.iter().map(|r| r.job_id.as_str()).collect();
            for pending in jobs.iter().filter(|j| !handled.contains(j.job_id.as_str())) {
                progress.set_state(&pending.job_id, JobState::Pending);
            }
            break;
        }
    }

    let cancelled = token.is_cancelled();
    let any_failed = results.iter().any(|r| r.state == JobState::Failed);
    let outcome = if cancelled {
        "cancelled"
    } else if any_failed {
        "completed_with_errors"
    } else {
        "completed"
    };

    progress.record(
        "batch_end",
        None,
        json!({ "processed": results.len(), "of": jobs.len() }),
        json!({ "token_cancelled": cancelled }),
        json!({ "outcome": outcome }),
    );
    progress.snapshot();

    Ok(BatchReport {
        run_id: cfg.run_id.clone(),
        mode: cfg.mode.clone(),
        started_at,
        finished_at: Some(now_iso8601()),
        outcome: outcome.to_string(),
        jobs: results,
        steps: progress.steps,
        snapshots: progress.snapshots,
    })
}

pub fn write_report(path: &Path, report: &BatchReport) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(report)?)?;
    Ok(())
}

fn now_iso8601() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("unix:{secs}")
}
