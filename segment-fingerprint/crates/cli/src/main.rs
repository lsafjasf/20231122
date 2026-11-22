//! Command line front-end.

use clap::{Parser, Subcommand};
use segment_fingerprint_core::digest::canonical_json;
use segment_fingerprint_core::pipeline::{resume_run, start_run, RunOptions, StopReason};
use segment_fingerprint_core::verify::{verify, write_report, VerificationReport};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser, Debug)]
#[command(
    name = "segment-fingerprint",
    version,
    about = "Pausable segment content fingerprint pipeline (Rust + GStreamer + GTK4)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Start a new run from a case manifest, optionally pausing mid-way.
    Run {
        #[arg(long)]
        case: PathBuf,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        pause_after: Option<usize>,
        #[arg(long)]
        force_degrade: bool,
    },
    /// Resume a paused run from its checkpoint.
    Resume {
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        case: Option<PathBuf>,
        #[arg(long)]
        force_degrade: bool,
    },
    /// Independently verify an exported run record.
    Verify {
        #[arg(long)]
        record: PathBuf,
        #[arg(long)]
        deep: bool,
    },
    /// Convenience wrapper: run with --pause-after then resume --force-degrade.
    Demo {
        #[arg(long)]
        case: PathBuf,
        #[arg(long)]
        out: PathBuf,
        #[arg(long, default_value_t = 4)]
        pause_after: usize,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Run {
            case,
            out,
            pause_after,
            force_degrade,
        } => execute(start_run(RunOptions {
            case_path: case,
            output_dir: out,
            pause_after,
            force_degrade,
            command_line: std::env::args().collect(),
        })),
        Command::Resume {
            out,
            case,
            force_degrade,
        } => execute(resume_run(RunOptions {
            case_path: case.unwrap_or_else(|| out.join("input/case.json")),
            output_dir: out,
            pause_after: None,
            force_degrade,
            command_line: std::env::args().collect(),
        })),
        Command::Verify { record, deep } => execute_verify(&record, deep),
        Command::Demo {
            case,
            out,
            pause_after,
        } => {
            let first = start_run(RunOptions {
                case_path: case.clone(),
                output_dir: out.clone(),
                pause_after: Some(pause_after),
                force_degrade: false,
                command_line: std::env::args().collect(),
            });
            if let Err(error) = first {
                eprintln!("initial run failed: {error:#}");
                return ExitCode::FAILURE;
            }
            println!("checkpoint written after {pause_after} segments; resuming in reduced mode");
            execute(resume_run(RunOptions {
                case_path: case,
                output_dir: out,
                pause_after: None,
                force_degrade: true,
                command_line: std::env::args().collect(),
            }))
        }
    }
}

fn execute(
    result: anyhow::Result<(segment_fingerprint_core::model::RunRecord, StopReason)>,
) -> ExitCode {
    match result {
        Ok((record, reason)) => {
            match reason {
                StopReason::Complete => println!(
                    "run {} complete: {} matches, {} missing, digest={}",
                    record.run.run_id,
                    record.summary.matches,
                    record.summary.missing,
                    record.summary.result_digest
                ),
                StopReason::PauseRequested | StopReason::ResourcePause => {
                    println!(
                        "run {} paused at cursor={:?} ({} completed segments)",
                        record.run.run_id,
                        record.run.cursor_segment_id,
                        record.segments.len()
                    );
                }
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("pipeline error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn execute_verify(record: &std::path::Path, deep: bool) -> ExitCode {
    match verify(record, deep) {
        Ok(report) => {
            let report_path = record
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .join("verification.json");
            if let Err(error) = write_report(&report_path, &report) {
                eprintln!("cannot write verification report: {error:#}");
                return ExitCode::FAILURE;
            }
            println!("{}", canonical_json(&serde_json::json!({
                "verdict": report.verdict,
                "checks_total": report.checks.len(),
                "checks_failed": report.checks.iter().filter(|check| !check.passed).count(),
                "report": report_path.display().to_string(),
            })));
            if report.verdict == "pass" {
                ExitCode::SUCCESS
            } else {
                for failed in report.checks.iter().filter(|check| !check.passed) {
                    eprintln!("FAILED {}: {}", failed.check_id, failed.detail);
                }
                ExitCode::FAILURE
            }
        }
        Err(error) => {
            eprintln!("verification error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

#[allow(dead_code)]
fn _assert_report_shape(report: &VerificationReport) -> String {
    canonical_json(report)
}
