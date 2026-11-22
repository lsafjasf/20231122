mod audio;
mod engine;
mod evidence;
mod gstutil;
mod resources;
mod state;
mod ui;

use anyhow::bail;
use engine::{EngineConfig, RunOutcome};
use resources::SimulatedLimits;
use serde_json::json;
use std::path::{Path, PathBuf};

fn workspace() -> PathBuf {
    std::env::var("LOOP_WORKSPACE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

fn pristine_sha256(ws: &Path) -> anyhow::Result<String> {
    let text = std::fs::read_to_string(ws.join("materials/ORIGINAL.sha256"))?;
    let line = text
        .lines()
        .find(|l| l.contains("original.pristine.wav"))
        .ok_or_else(|| anyhow::anyhow!("pristine hash line missing"))?;
    Ok(line.split_whitespace().next().unwrap().to_string())
}

fn parse_simulate(s: &str) -> SimulatedLimits {
    let mut sim = SimulatedLimits::default();
    for kv in s.split(',') {
        if let Some((k, v)) = kv.split_once('=') {
            match k.trim() {
                "mem" => sim.mem_mb = v.trim().parse().ok(),
                "disk" => sim.disk_mb = v.trim().parse().ok(),
                _ => {}
            }
        }
    }
    sim
}

fn base_config(ws: &Path, run_id: &str) -> anyhow::Result<EngineConfig> {
    Ok(EngineConfig {
        input: ws.join("materials/original.wav"),
        pristine_sha256: pristine_sha256(ws)?,
        run_root: ws.join("artifacts/runs"),
        run_id: run_id.to_string(),
        pause_after: None,
        simulate: SimulatedLimits::default(),
        needed_disk_mb: 32,
    })
}

fn cmd_run(args: &[String]) -> anyhow::Result<()> {
    let ws = workspace();
    let mut run_id = "default".to_string();
    let mut pause_after: Option<usize> = None;
    let mut simulate = SimulatedLimits::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--run-id" => {
                run_id = args.get(i + 1).cloned().unwrap_or(run_id);
                i += 1;
            }
            "--pause-after" => {
                let a = args.get(i + 1).map(String::as_str).unwrap_or("");
                pause_after = Some(state::ANCHORS.iter().position(|x| x == &a || x.starts_with(a))
                    .ok_or_else(|| anyhow::anyhow!("unknown anchor {a}"))?);
                i += 1;
            }
            "--simulate" => {
                simulate = parse_simulate(args.get(i + 1).map(String::as_str).unwrap_or(""));
                i += 1;
            }
            other => bail!("unknown run arg {other}"),
        }
        i += 1;
    }
    let mut cfg = base_config(&ws, &run_id)?;
    cfg.pause_after = pause_after;
    cfg.simulate = simulate;
    match engine::run(&cfg)? {
        RunOutcome::Completed { run_dir } => println!("COMPLETED {}", run_dir.display()),
        RunOutcome::Paused { run_dir, after_anchor } => {
            println!("PAUSED after {} ({})", state::ANCHORS[after_anchor], run_dir.display())
        }
    }
    Ok(())
}

fn cmd_compare(a: &str, b: &str) -> anyhow::Result<()> {
    let ws = workspace();
    let report = engine::compare_runs(
        &ws.join("artifacts/runs").join(a),
        &ws.join("artifacts/runs").join(b),
    )?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    if report["identical"] == json!(true) {
        Ok(())
    } else {
        bail!("runs differ")
    }
}

/// Full acceptance: continuous run vs. pause/resume run, degraded run,
/// insufficient-resource run, original-material integrity.
fn cmd_verify() -> anyhow::Result<()> {
    let ws = workspace();
    let mut checks: Vec<serde_json::Value> = Vec::new();

    // 1. continuous run
    let _ = std::fs::remove_dir_all(ws.join("artifacts/runs/accept-continuous"));
    engine::run(&base_config(&ws, "accept-continuous")?)?;

    // 2. paused run: stop after A1, resume, stop after A2, resume to end
    let _ = std::fs::remove_dir_all(ws.join("artifacts/runs/accept-paused"));
    let mut cfg = base_config(&ws, "accept-paused")?;
    cfg.pause_after = Some(0);
    engine::run(&cfg)?;
    cfg.pause_after = Some(1);
    engine::run(&cfg)?;
    cfg.pause_after = None;
    engine::run(&cfg)?;

    // 3. compare artifacts
    let cmp = engine::compare_runs(
        &ws.join("artifacts/runs/accept-continuous"),
        &ws.join("artifacts/runs/accept-paused"),
    )?;
    checks.push(json!({"check": "pause_resume_equivalence", "pass": cmp["identical"], "detail": cmp}));

    // 4. seam quality + concrete offsets from the continuous run's report
    let report: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(
        ws.join("artifacts/runs/accept-continuous/output/a3_report.json"),
    )?)?;
    checks.push(json!({
        "check": "seamless_join",
        "pass": report["seam_ok"],
        "loop_start_seconds": report["loop_start_seconds"],
        "loop_end_seconds": report["loop_end_seconds"],
        "loop_start_sample": report["loop_start_sample"],
        "loop_end_sample": report["loop_end_sample"],
        "seam_wrap_step": report["seam_wrap_step"],
        "seam_max_inner_step": report["seam_max_inner_step"],
    }));

    // 5. degraded path (forced low memory)
    let _ = std::fs::remove_dir_all(ws.join("artifacts/runs/accept-degraded"));
    let mut cfg = base_config(&ws, "accept-degraded")?;
    cfg.simulate = SimulatedLimits { mem_mb: Some(200), disk_mb: None };
    let degraded_ok = engine::run(&cfg).is_ok();
    let deg_evidence = std::fs::read_to_string(
        ws.join("artifacts/runs/accept-degraded/evidence.jsonl"),
    )
    .unwrap_or_default();
    checks.push(json!({
        "check": "degradation_path",
        "pass": degraded_ok && deg_evidence.contains("degradation_applied"),
        "evidence": "artifacts/runs/accept-degraded/evidence.jsonl",
    }));

    // 6. insufficient resources must stop cleanly with evidence
    let _ = std::fs::remove_dir_all(ws.join("artifacts/runs/accept-insufficient"));
    let mut cfg = base_config(&ws, "accept-insufficient")?;
    cfg.simulate = SimulatedLimits { mem_mb: Some(64), disk_mb: None };
    let failed = engine::run(&cfg).is_err();
    let ins_evidence = std::fs::read_to_string(
        ws.join("artifacts/runs/accept-insufficient/evidence.jsonl"),
    )
    .unwrap_or_default();
    checks.push(json!({
        "check": "insufficient_resource_handling",
        "pass": failed && ins_evidence.contains("resource_insufficient"),
        "evidence": "artifacts/runs/accept-insufficient/evidence.jsonl",
    }));

    // 7. original material untouched
    let now_hash = audio::sha256_file(&ws.join("materials/original.wav"))?;
    let pristine = pristine_sha256(&ws)?;
    checks.push(json!({
        "check": "original_unmodified",
        "pass": now_hash == pristine,
        "sha256": now_hash,
    }));

    let pass = checks.iter().all(|c| c["pass"] == json!(true));
    let summary = json!({ "acceptance": if pass { "PASS" } else { "FAIL" }, "checks": checks });
    let out = serde_json::to_string_pretty(&summary)?;
    std::fs::create_dir_all(ws.join("artifacts/acceptance"))?;
    std::fs::write(ws.join("artifacts/acceptance/summary.json"), &out)?;
    println!("{out}");
    if pass {
        Ok(())
    } else {
        bail!("acceptance failed")
    }
}

fn main() -> anyhow::Result<()> {
    gstreamer::init()?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("run") => cmd_run(&args[1..]),
        Some("compare") => cmd_compare(
            args.get(1).map(String::as_str).unwrap_or(""),
            args.get(2).map(String::as_str).unwrap_or(""),
        ),
        Some("verify") => cmd_verify(),
        Some("ui") | None => ui::run(),
        Some(other) => bail!("unknown command {other}: use run|compare|verify|ui"),
    }
}
