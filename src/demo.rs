use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::Serialize;

use crate::acceptance;
use crate::checkpoint;
use crate::compare::{self, ComparisonReport};
use crate::evidence::EvidenceLog;
use crate::fixture;
use crate::model::{ExtremeParams, SampleResult, SamplingPlan, DEFAULT_PAUSE_AFTER};
use crate::plan::{build_demo_plan, validate_demo_plan};
use crate::runner::{self, RunConfig, RunOutcome};

#[derive(Serialize)]
struct ComparisonArtifact {
    before: &'static str,
    after: &'static str,
    paused_prefix: ComparisonReport,
    resumed_vs_fresh: ComparisonReport,
}

#[derive(Serialize)]
struct Summary {
    run_id: String,
    paused_at: usize,
    resumed_from: usize,
    total: usize,
    scale_percent: u8,
    first_anchor: Anchor,
    last_anchor: Anchor,
    pass: bool,
}

#[derive(Serialize)]
struct Anchor {
    timestamp_ms: u64,
    raw_offset: u64,
    effective_offset: u64,
    rgb: [u8; 3],
}

pub fn run_demo(workspace: &Path) -> Result<bool> {
    let (source_path, manifest_path) = fixture::default_paths();
    let source_path = workspace.join(source_path);
    let manifest_path = workspace.join(manifest_path);
    let source = fixture::ensure_original(&source_path, &manifest_path)?;

    let artifacts = workspace.join("artifacts");
    std::fs::create_dir_all(&artifacts)?;
    let plan = build_demo_plan(source, ExtremeParams::demo());
    validate_demo_plan(&plan)?;

    let evidence_path = artifacts.join("evidence.jsonl");
    let mut evidence = EvidenceLog::create(&evidence_path)?;
    let cfg = RunConfig {
        out_dir: artifacts.clone(),
        resume: false,
        pause_after: Some(DEFAULT_PAUSE_AFTER),
        pause_flag: None,
    };
    let paused = runner::run(&plan, &cfg, &mut evidence, |_| {})?;
    let paused_cp = match paused {
        RunOutcome::Paused(cp) => cp,
        RunOutcome::Completed(_) => bail!("demo expected the first pass to pause"),
    };
    let paused_snapshot = artifacts.join("checkpoint.paused.json");
    std::fs::write(&paused_snapshot, serde_json::to_vec_pretty(&paused_cp)?)?;
    std::fs::write(
        artifacts.join("results.paused.json"),
        serde_json::to_vec_pretty(&paused_cp.results)?,
    )?;

    let resumed_cfg = RunConfig {
        out_dir: artifacts.clone(),
        resume: true,
        pause_after: None,
        pause_flag: None,
    };
    let completed = runner::run(&plan, &resumed_cfg, &mut evidence, |_| {})?;
    let final_cp = runner::require_complete(completed, plan.samples.len())?;
    runner::write_results(&artifacts, &final_cp)?;

    let fresh_dir = artifacts.join("fresh");
    let fresh_cfg = RunConfig {
        out_dir: fresh_dir.clone(),
        resume: false,
        pause_after: None,
        pause_flag: None,
    };
    let fresh_cp = runner::require_complete(
        runner::run(&plan, &fresh_cfg, &mut evidence, |_| {})?,
        plan.samples.len(),
    )?;
    runner::write_results(&fresh_dir, &fresh_cp)?;
    std::fs::write(
        artifacts.join("results.fresh.json"),
        serde_json::to_vec_pretty(&fresh_cp.results)?,
    )?;

    let mut prefix = compare::compare_results(
        &paused_cp.results,
        &final_cp.results[..paused_cp.results.len().min(final_cp.results.len())],
    );
    prefix.pause_resume_equivalent =
        prefix.mismatch_count == 0 && paused_cp.results.len() <= final_cp.results.len();
    prefix.prefix_preserved = prefix.pause_resume_equivalent;
    let mut resumed_vs_fresh = compare::compare_results(&final_cp.results, &fresh_cp.results);
    resumed_vs_fresh.prefix_preserved = prefix.pause_resume_equivalent;
    let comparison = ComparisonArtifact {
        before: "results.paused.json (first 8 persisted results)",
        after: "results.json (resumed full pass)",
        paused_prefix: prefix,
        resumed_vs_fresh: resumed_vs_fresh.clone(),
    };
    std::fs::write(
        artifacts.join("comparison.json"),
        serde_json::to_vec_pretty(&comparison)?,
    )?;

    let acceptance = acceptance::verify(
        &plan,
        &final_cp.results,
        &fresh_cp.results,
        &resumed_vs_fresh,
        &evidence_path,
        &manifest_path,
        final_cp.scale_percent,
    )?;
    acceptance::write(&artifacts.join("acceptance.json"), &acceptance)?;

    let first = final_cp.results.first().context("no first anchor")?;
    let last = final_cp.results.last().context("no last anchor")?;
    let summary = Summary {
        run_id: final_cp.run_id.clone(),
        paused_at: paused_cp.next_index,
        resumed_from: paused_cp.next_index,
        total: final_cp.total,
        scale_percent: final_cp.scale_percent,
        first_anchor: anchor(first),
        last_anchor: anchor(last),
        pass: acceptance.all_ok(),
    };
    std::fs::write(
        artifacts.join("summary.json"),
        serde_json::to_vec_pretty(&summary)?,
    )?;
    copy_checkpoint_evidence(&artifacts)?;
    Ok(acceptance.all_ok())
}

fn anchor(sample: &SampleResult) -> Anchor {
    Anchor {
        timestamp_ms: sample.timestamp_ms,
        raw_offset: sample.raw_offset,
        effective_offset: sample.effective_offset,
        rgb: sample.rgb,
    }
}

fn copy_checkpoint_evidence(artifacts: &Path) -> Result<()> {
    let cp = checkpoint::load(artifacts)?;
    std::fs::write(
        artifacts.join("checkpoint.json"),
        serde_json::to_vec_pretty(&cp)?,
    )?;
    Ok(())
}

pub fn read_results(path: impl AsRef<Path>) -> Result<Vec<SampleResult>> {
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}

pub fn make_plan(workspace: &Path) -> Result<SamplingPlan> {
    let (source_path, manifest_path) = fixture::default_paths();
    let source =
        fixture::ensure_original(&workspace.join(source_path), &workspace.join(manifest_path))?;
    let plan = build_demo_plan(source, ExtremeParams::demo());
    validate_demo_plan(&plan)?;
    Ok(plan)
}
