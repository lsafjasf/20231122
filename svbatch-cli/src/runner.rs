//! Orchestrates the full replayable experiment and writes evidence.

use anyhow::{Context, Result};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use svbatch::audio::CancelToken;
use svbatch::batch::{run_batch, write_report, BatchConfig, SubmissionRegistry};
use svbatch::model::JobState;
use svbatch::{canonical_jobs, duplicate_submission_jobs, reordered_jobs, BatchReport};

#[allow(dead_code)]
pub struct ExperimentPaths {
    pub root: PathBuf,
    pub runs: PathBuf,
    pub outputs: PathBuf,
    pub report_dir: PathBuf,
}

impl ExperimentPaths {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        let paths = Self {
            runs: root.join("runs"),
            outputs: root.join("outputs"),
            report_dir: root.join("reports"),
            root,
        };
        for dir in [&paths.runs, &paths.outputs, &paths.report_dir] {
            fs::create_dir_all(dir)?;
        }
        Ok(paths)
    }
}

/// One scenario inside the experiment.
struct Scenario {
    run_id: String,
    mode: String,
    cancel_after_ms: Option<u64>,
    jobs_builder: fn(u64) -> Vec<svbatch::JobSpec>,
}

fn scenarios() -> Vec<Scenario> {
    vec![
        Scenario {
            run_id: "S1-baseline".into(),
            mode: "baseline_full_batch".into(),
            cancel_after_ms: None,
            jobs_builder: canonical_jobs,
        },
        Scenario {
            run_id: "S2-cancel-mid".into(),
            mode: "mid_run_cancel".into(),
            cancel_after_ms: Some(350),
            jobs_builder: canonical_jobs,
        },
        Scenario {
            run_id: "S3-control-no-cancel".into(),
            mode: "no_cancel_control".into(),
            cancel_after_ms: None,
            jobs_builder: canonical_jobs,
        },
        Scenario {
            run_id: "S4-duplicate-submit".into(),
            mode: "duplicate_submission".into(),
            cancel_after_ms: None,
            jobs_builder: duplicate_submission_jobs,
        },
        Scenario {
            run_id: "S5-reordered".into(),
            mode: "reordered_batch".into(),
            cancel_after_ms: None,
            jobs_builder: reordered_jobs,
        },
        Scenario {
            run_id: "S6-replay".into(),
            mode: "replay_recheck".into(),
            cancel_after_ms: None,
            jobs_builder: canonical_jobs,
        },
    ]
}

fn hash_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path)?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Ok(hex::encode(hasher.finalize()))
}

#[allow(dead_code)]
pub struct ExperimentOutcome {
    pub reports: Vec<(String, BatchReport)>,
    pub auto_checks: Vec<AutoCheck>,
}

#[derive(Debug, serde::Serialize)]
pub struct AutoCheck {
    pub id: String,
    pub claim: String,
    pub passed: bool,
    pub evidence: serde_json::Value,
    pub remediation: String,
}

pub fn run_experiment(root: &Path, work_delay_ms: u64) -> Result<ExperimentOutcome> {
    let paths = ExperimentPaths::new(root)?;
    let mut reports: Vec<(String, BatchReport)> = Vec::new();

    for scenario in scenarios() {
        let jobs = (scenario.jobs_builder)(work_delay_ms);
        let mut registry = SubmissionRegistry::new();
        let token = CancelToken::new();
        if let Some(after_ms) = scenario.cancel_after_ms {
            let token_clone = token.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(after_ms));
                token_clone.cancel();
            });
        }
        let cfg = BatchConfig {
            run_id: scenario.run_id.clone(),
            mode: scenario.mode.clone(),
            output_dir: paths.outputs.join(&scenario.run_id),
        };
        let report = run_batch(&cfg, &jobs, &token, &mut registry, None)
            .with_context(|| format!("scenario {} failed", scenario.run_id))?;
        write_report(&paths.runs.join(format!("{}.json", scenario.run_id)), &report)?;
        reports.push((scenario.run_id.clone(), report));
    }

    let auto_checks = evaluate(&paths, &reports)?;
    let summary = json!({
        "generated_at_unix": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
        "stack": ["Rust 1.100", "GStreamer 1.28", "GTK 4.24"],
        "checks": auto_checks,
    });
    fs::write(
        paths.report_dir.join("auto-conclusion.json"),
        serde_json::to_string_pretty(&summary)?,
    )?;
    Ok(ExperimentOutcome { reports, auto_checks })
}

fn report<'a>(reports: &'a [(String, BatchReport)], run_id: &str) -> &'a BatchReport {
    reports
        .iter()
        .find(|(id, _)| id == run_id)
        .map(|(_, r)| r)
        .expect("scenario report present")
}

fn succeeded_hashes(r: &BatchReport) -> std::collections::BTreeMap<String, String> {
    r.jobs
        .iter()
        .filter(|j| j.state == JobState::Succeeded)
        .filter_map(|j| Some((j.job_id.clone(), j.sha256.clone()?)))
        .collect()
}

fn evaluate(
    paths: &ExperimentPaths,
    reports: &[(String, BatchReport)],
) -> Result<Vec<AutoCheck>> {
    let s1 = report(reports, "S1-baseline");
    let s2 = report(reports, "S2-cancel-mid");
    let s3 = report(reports, "S3-control-no-cancel");
    let s4 = report(reports, "S4-duplicate-submit");
    let s5 = report(reports, "S5-reordered");
    let s6 = report(reports, "S6-replay");

    let mut checks = Vec::new();

    // C1: mid-run cancel stops the batch and leaves remaining jobs pending.
    let cancelled: Vec<_> = s2
        .jobs
        .iter()
        .filter(|j| j.state == JobState::Cancelled)
        .map(|j| j.job_id.clone())
        .collect();
    let pending_after = s2
        .snapshots
        .last()
        .map(|snap| {
            snap
        .states
        .iter()
        .filter(|(_, st)| *st == JobState::Pending)
        .count()
        })
        .unwrap_or(0);
    checks.push(AutoCheck {
        id: "C1-mid-cancel".into(),
        claim: "S2 中途取消：至少一个作业被取消，且后续作业保持 pending，批次 outcome=cancelled".into(),
        passed: s2.outcome == "cancelled" && !cancelled.is_empty() && pending_after > 0,
        evidence: json!({
            "outcome": s2.outcome,
            "cancelled_jobs": cancelled,
            "pending_jobs_in_final_snapshot": pending_after,
            "report": "runs/S2-cancel-mid.json",
        }),
        remediation: "重新执行 `svcli experiment`；若仍失败，检查 CancelToken 轮询间隔与 GStreamer bus 超时（audio.rs）。".into(),
    });

    // C2: control run processes every job successfully.
    let all_ok = s3.jobs.len() == 9 && s3.jobs.iter().all(|j| j.state == JobState::Succeeded);
    checks.push(AutoCheck {
        id: "C2-control".into(),
        claim: "S3 无取消对照：9 个作业全部 succeeded，outcome=completed".into(),
        passed: all_ok && s3.outcome == "completed",
        evidence: json!({
            "outcome": s3.outcome,
            "job_count": s3.jobs.len(),
            "states": s3.jobs.iter().map(|j| (j.job_id.clone(), j.state)).collect::<Vec<_>>(),
            "report": "runs/S3-control-no-cancel.json",
        }),
        remediation: "查看 runs/S3 报告中 job_end 的 detail 字段；用 `gst-launch-1.0` 复现失败管线（见报告 steps[].input.pipeline）。".into(),
    });

    // C3: cancelled outputs are absent; successful outputs exist and hash-match.
    let cancel_dir = paths.outputs.join("S2-cancel-mid");
    let mut leaked: Vec<String> = Vec::new();
    for job in &s2.jobs {
        let file = cancel_dir.join(format!("{}.wav", job.job_id));
        if job.state != JobState::Succeeded && file.exists() {
            leaked.push(file.display().to_string());
        }
    }
    checks.push(AutoCheck {
        id: "C3-no-partial-output".into(),
        claim: "S2 被取消/未开始的作业没有残留 WAV 文件".into(),
        passed: leaked.is_empty(),
        evidence: json!({ "leaked_files": leaked, "output_dir": cancel_dir.display().to_string() }),
        remediation: "删除 outputs/S2-cancel-mid 后重跑；检查 Engine::run_job 的取消分支是否清理目标文件。".into(),
    });

    // C4: duplicate submission is rejected.
    let rejected: Vec<_> = s4
        .jobs
        .iter()
        .filter(|j| j.detail.contains("duplicate"))
        .map(|j| j.job_id.clone())
        .collect();
    let accepted_s4 = s4.jobs.iter().filter(|j| j.state == JobState::Succeeded).count();
    checks.push(AutoCheck {
        id: "C4-duplicate-blocked".into(),
        claim: "S4 重复提交：10 个提交中 9 个唯一作业正常完成，重复的第 10 个（同 idempotency key）被拒绝且无文件".into(),
        passed: rejected.len() == 1 && accepted_s4 == 9,
        evidence: json!({
            "rejected_jobs": rejected,
            "succeeded_count": accepted_s4,
            "report": "runs/S4-duplicate-submit.json",
        }),
        remediation: "检查 SubmissionRegistry::submit 的 key 生成逻辑；用 `svcli inspect-jobs` 对比重复项的 idempotency_key。".into(),
    });

    // C5: reordering does not change per-job outputs (same hashes).
    let h1 = succeeded_hashes(s1);
    let h5 = succeeded_hashes(s5);
    let order1: Vec<_> = s1.jobs.iter().map(|j| j.job_id.clone()).collect();
    let order5: Vec<_> = s5.jobs.iter().map(|j| j.job_id.clone()).collect();
    let order_changed = order1 != order5;
    checks.push(AutoCheck {
        id: "C5-order-invariance".into(),
        claim: "S5 改变提交顺序后，每个作业的输出 SHA256 与 S1 完全一致".into(),
        passed: order_changed && h1 == h5 && h1.len() == 9,
        evidence: json!({
            "order_changed": order_changed,
            "baseline_order": order1,
            "reordered_order": order5,
            "hash_pairs": h1.iter().map(|(k, v)| json!({"job": k, "s1": v, "s5": h5.get(k)})).collect::<Vec<_>>(),
        }),
        remediation: "若哈希不一致，检查管线描述是否引入非确定性元素（时间戳/随机源）；本实验固定 audiotestsrc 与 caps。".into(),
    });

    // C6: replay reproduces identical hashes.
    let h6 = succeeded_hashes(s6);
    checks.push(AutoCheck {
        id: "C6-replay-determinism".into(),
        claim: "S6 重放：9 个作业的 SHA256 与 S1 基线逐一相等".into(),
        passed: h1 == h6 && h6.len() == 9,
        evidence: json!({
            "baseline": h1,
            "replay": h6,
        }),
        remediation: "重跑 `svcli experiment`；若仍漂移，用 `sha256sum outputs/S1-baseline/*.wav outputs/S6-replay/*.wav` 定位差异文件。".into(),
    });

    // C7: on-disk evidence files match the hashes recorded in reports.
    let mut mismatches: Vec<String> = Vec::new();
    for (run_id, report) in reports {
        for job in &report.jobs {
            if job.state == JobState::Succeeded {
                let path = paths
                    .outputs
                    .join(run_id)
                    .join(job.output_file.clone().unwrap_or_default());
                match hash_file(&path) {
                    Ok(actual) if Some(&actual) == job.sha256.as_ref() => {}
                    Ok(actual) => mismatches.push(format!(
                        "{}/{}: report={} disk={}",
                        run_id,
                        job.job_id,
                        job.sha256.clone().unwrap_or_default(),
                        actual
                    )),
                    Err(e) => mismatches.push(format!("{}/{}: unreadable: {e}", run_id, job.job_id)),
                }
            }
        }
    }
    checks.push(AutoCheck {
        id: "C7-evidence-integrity".into(),
        claim: "报告中记录的全部 SHA256 与磁盘 WAV 文件实际哈希一致".into(),
        passed: mismatches.is_empty(),
        evidence: json!({ "mismatches": mismatches }),
        remediation: "以 runs/*.json 为准重新生成输出：删除 outputs/ 后执行 `svcli experiment --fresh`。".into(),
    });

    Ok(checks)
}
