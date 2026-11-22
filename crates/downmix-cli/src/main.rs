//! Command-line front end for the reviewable surround downmix flow.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use downmix_core::{clean_temp_state, discover::discover_objects, run_batch, verify_evidence, RunConfig};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "downmix",
    version,
    about = "Reviewable surround-sound object downmix (Rust + GStreamer)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Discover audio objects, including unknown-extension (sniffed) files.
    Discover {
        #[arg(long)]
        input: PathBuf,
    },
    /// Run two-revision downmix over every object and retain evidence.
    Run {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        evidence: PathBuf,
    },
    /// Re-hash outputs and validate correlation tables from stored manifests.
    Verify {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        evidence: PathBuf,
        #[arg(long)]
        report: Option<PathBuf>,
    },
    /// Remove temporary state left by an interrupted run.
    Clean {
        #[arg(long)]
        evidence: PathBuf,
    },
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let cli = Cli::parse();
    match cli.command {
        Command::Discover { input } => cmd_discover(input),
        Command::Run { input, evidence } => cmd_run(input, evidence),
        Command::Verify {
            input,
            evidence,
            report,
        } => cmd_verify(input, evidence, report),
        Command::Clean { evidence } => cmd_clean(evidence),
    }
}

fn cmd_discover(input: PathBuf) -> Result<()> {
    let objects = discover_objects(&input)
        .with_context(|| format!("discovery under {input:?} failed"))?;
    let json = serde_json::to_string_pretty(&objects)?;
    println!("{json}");
    eprintln!("discovered {} object(s)", objects.len());
    Ok(())
}

fn cmd_run(input: PathBuf, evidence: PathBuf) -> Result<()> {
    let summary = run_batch(&RunConfig {
        input_root: input,
        evidence_root: evidence.clone(),
    })
    .context("batch run failed")?;
    println!("{}", serde_json::to_string_pretty(&summary)?);
    if summary.failed > 0 {
        anyhow::bail!(
            "{} of {} object(s) failed; remaining objects were processed",
            summary.failed,
            summary.total_objects
        );
    }
    Ok(())
}

fn cmd_verify(input: PathBuf, evidence: PathBuf, report: Option<PathBuf>) -> Result<()> {
    let objects = discover_objects(&input)
        .with_context(|| format!("discovery under {input:?} failed"))?;
    let report_value = verify_evidence(&evidence, &objects).context("verification failed")?;
    let json = serde_json::to_string_pretty(&report_value)?;
    if let Some(path) = report {
        std::fs::write(&path, &json).with_context(|| format!("write report {path:?}"))?;
    }
    println!("{json}");
    if !report_value.clean {
        anyhow::bail!("evidence verification found inconsistencies");
    }
    Ok(())
}

fn cmd_clean(evidence: PathBuf) -> Result<()> {
    let removed = clean_temp_state(&evidence).context("temp cleanup failed")?;
    eprintln!("removed {removed} temporary directory/directories");
    Ok(())
}
