//! Headless runner for the replayable sound-effect batch experiment.

mod runner;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use svbatch::audio::init_gstreamer;
use svbatch::{canonical_jobs, reordered_jobs};

#[derive(Parser)]
#[command(name = "svcli", version, about = "Sound-effect batch variant experiment CLI")]
struct Cli {
    /// Evidence root (runs/, outputs/, reports/ are created underneath).
    #[arg(long, global = true, default_value = "./evidence")]
    evidence: PathBuf,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the complete six-scenario experiment and write all evidence.
    Experiment {
        /// Artificial per-job latency in milliseconds.
        #[arg(long, default_value_t = 120)]
        work_delay_ms: u64,
    },
    /// Print the canonical and reordered job lists (order-change inspection).
    InspectJobs {
        #[arg(long, default_value_t = 120)]
        work_delay_ms: u64,
    },
    /// Render the human review checklist next to the machine conclusion.
    RenderReview,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    init_gstreamer().context("GStreamer initialisation failed")?;

    match cli.command {
        Command::Experiment { work_delay_ms } => {
            let outcome = runner::run_experiment(&cli.evidence, work_delay_ms)?;
            let passed = outcome.auto_checks.iter().filter(|c| c.passed).count();
            println!("experiment finished: {}/{} auto checks passed", passed, outcome.auto_checks.len());
            for check in &outcome.auto_checks {
                println!(
                    "  [{}] {} — {}",
                    if check.passed { "PASS" } else { "FAIL" },
                    check.id,
                    check.claim
                );
            }
            println!("evidence written under {}", cli.evidence.display());
            if passed != outcome.auto_checks.len() {
                std::process::exit(2);
            }
        }
        Command::InspectJobs { work_delay_ms } => {
            let canonical = canonical_jobs(work_delay_ms);
            let reordered = reordered_jobs(work_delay_ms);
            println!("canonical order:");
            for (i, job) in canonical.iter().enumerate() {
                println!("  {i:2}. {} [key={}]", job.job_id, job.idempotency_key());
            }
            println!("reordered order:");
            for (i, job) in reordered.iter().enumerate() {
                println!("  {i:2}. {} [key={}]", job.job_id, job.idempotency_key());
            }
        }
        Command::RenderReview => {
            let path = cli.evidence.join("reports").join("manual-review.md");
            render_review(&cli.evidence, &path)?;
            println!("manual review template: {}", path.display());
        }
    }
    Ok(())
}

fn render_review(root: &std::path::Path, dest: &std::path::Path) -> Result<()> {
    use std::fmt::Write;
    let auto_path = root.join("reports").join("auto-conclusion.json");
    let auto: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&auto_path)
            .with_context(|| format!("run the experiment first; missing {}", auto_path.display()))?,
    )?;
    let mut md = String::new();
    writeln!(md, "# 人工确认清单（Manual Review）")?;
    writeln!(md)?;
    writeln!(md, "> 本文件只收集人工判断；自动判定见 `reports/auto-conclusion.json`，二者分开呈现。")?;
    writeln!(md)?;
    let checks = auto.get("checks").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    for check in &checks {
        let id = check.get("id").and_then(|v| v.as_str()).unwrap_or("?");
        let claim = check.get("claim").and_then(|v| v.as_str()).unwrap_or("?");
        let auto_passed = check.get("passed").and_then(|v| v.as_bool()).unwrap_or(false);
        writeln!(md, "## {id}")?;
        writeln!(md, "- 自动结论：{}", if auto_passed { "PASS" } else { "FAIL" })?;
        writeln!(md, "- 断言：{claim}")?;
        writeln!(md, "- 人工复核（请勾选）：")?;
        writeln!(md, "  - [ ] 已核对对应 `runs/*.json` 的 steps/snapshots")?;
        writeln!(md, "  - [ ] 已试听/检查 `outputs/` 下相关 WAV")?;
        writeln!(md, "  - [ ] 人工结论：[ ] 同意自动结论  [ ] 不同意（说明理由）")?;
        writeln!(md, "- 复核人：____________ 日期：____________")?;
        writeln!(md)?;
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(dest, md)?;
    Ok(())
}
