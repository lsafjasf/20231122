use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use credits_core::project::parse_time_literal;
#[cfg(feature = "gui")]
use credits_core::report::load_report;
use credits_engine::runner::{run_review, ReviewOptions};

#[cfg(feature = "gui")]
mod gui;

#[derive(Debug, Parser)]
#[command(
    name = "credits-review",
    version,
    about = "Reviewable end-credit roll typesetting with GStreamer evidence"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Analyze a legacy/v2 project or a bare media file and write review evidence.
    Review {
        /// Project JSON, or a media file with any (including unknown) extension.
        input: PathBuf,
        /// Directory that receives review.json and evidence/.
        #[arg(long, default_value = "review-out")]
        out: PathBuf,
        /// Override every object's review point. Repeatable.
        #[arg(long = "checkpoint")]
        checkpoints: Vec<String>,
        /// Exit with status 3 when any object fails or is partial.
        #[arg(long)]
        strict: bool,
    },
    /// Verify the report checksum and every retained evidence file.
    Verify {
        /// review.json or its containing directory.
        report: PathBuf,
    },
    /// Open the GTK4 reviewer, running analysis first when needed.
    Gui {
        /// Existing review report, or a project/media input to analyze.
        input: PathBuf,
        /// Output directory used when analysis is required.
        #[arg(long, default_value = "review-out")]
        out: PathBuf,
        /// Override every object's review point before analysis. Repeatable.
        #[arg(long = "checkpoint")]
        checkpoints: Vec<String>,
    },
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("credits-review: {error:#}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<ExitCode> {
    let cli = Cli::parse();
    match cli.command {
        Command::Review {
            input,
            out,
            checkpoints,
            strict,
        } => {
            let checkpoints = parse_checkpoints(&checkpoints)?;
            let mut report = run_review(
                &input,
                &ReviewOptions {
                    out_dir: out.clone(),
                    checkpoints,
                },
            )
            .with_context(|| format!("review failed for {}", input.display()))?;
            annotate_gtk(&mut report, &out)?;
            print_summary(&out.join("review.json"), &report);
            if strict && (report.summary.failed_count > 0 || report.summary.partial_count > 0) {
                return Ok(ExitCode::from(3));
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Verify { report } => {
            let summary = match credits_core::report::verify_report(&report) {
                Ok(summary) => summary,
                Err(error) => {
                    eprintln!("verification failed: {error:#}");
                    return Ok(ExitCode::from(4));
                }
            };
            print_verification(&summary);
            if summary.ok() {
                Ok(ExitCode::SUCCESS)
            } else {
                Ok(ExitCode::from(4))
            }
        }
        Command::Gui {
            input,
            out,
            checkpoints,
        } => {
            #[cfg(feature = "gui")]
            {
                let checkpoints = parse_checkpoints(&checkpoints)?;
                let report_path = ensure_report(&input, &out, checkpoints)?;
                gui::run(&report_path)?;
                Ok(ExitCode::SUCCESS)
            }
            #[cfg(not(feature = "gui"))]
            {
                let _ = (input, out, checkpoints);
                anyhow::bail!("this build was compiled without the GTK4 gui feature")
            }
        }
    }
}

fn parse_checkpoints(raw: &[String]) -> Result<Vec<f64>> {
    raw.iter()
        .map(|value| parse_time_literal(value).map_err(anyhow::Error::from))
        .collect()
}

#[cfg(feature = "gui")]
fn ensure_report(input: &Path, out: &Path, checkpoints: Vec<f64>) -> Result<PathBuf> {
    if load_report(input).is_ok() {
        return Ok(if input.is_dir() {
            input.join("review.json")
        } else {
            input.to_path_buf()
        });
    }
    let mut report = run_review(
        input,
        &ReviewOptions {
            out_dir: out.to_path_buf(),
            checkpoints,
        },
    )
    .with_context(|| format!("review failed for {}", input.display()))?;
    annotate_gtk(&mut report, out)?;
    let report_path = out.join("review.json");
    print_summary(&report_path, &report);
    Ok(report_path)
}

fn print_summary(path: &Path, report: &credits_core::report::ReviewReport) {
    println!("report: {}", path.display());
    println!(
        "format: {} | objects: {} ok, {} partial, {} failed | correlations: {}",
        report.project_format.as_str(),
        report.summary.ok_count,
        report.summary.partial_count,
        report.summary.failed_count,
        report.summary.correlation_count
    );
    println!(
        "temp cleanup: {} (removed={})",
        report.cleanup.temp_path, report.cleanup.removed
    );
    for failure in &report.failures {
        eprintln!(
            "  failure [{}] {}: {}",
            failure.stage, failure.object_id, failure.message
        );
    }
}

fn print_verification(summary: &credits_core::report::VerificationSummary) {
    println!("verified: {}", summary.report_path.display());
    println!("checked evidence files: {}", summary.checked_files);
    for failure in &summary.failures {
        eprintln!("  integrity failure: {failure}");
    }
}

fn annotate_gtk(report: &mut credits_core::report::ReviewReport, out: &Path) -> Result<()> {
    #[cfg(feature = "gui")]
    {
        report.tool.gtk_version = Some(format!(
            "{}.{}.{}",
            gtk4::major_version(),
            gtk4::minor_version(),
            gtk4::micro_version()
        ));
        report.write(out)?;
    }
    #[cfg(not(feature = "gui"))]
    {
        let _ = (report, out);
    }
    Ok(())
}
