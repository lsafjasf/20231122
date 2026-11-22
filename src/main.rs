use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use frame_loss_review::fixtures;
use frame_loss_review::input::read_jsonl;
use frame_loss_review::model::{Session, import_legacy};
#[cfg(not(feature = "gstreamer"))]
use frame_loss_review::replay::SyntheticProcessor;
use frame_loss_review::replay::{FrameProcessor, ReplayReport, replay};
use frame_loss_review::review::{
    ExperimentInputs, compare_reports, records_jsonl, session_from_records, verify_manifest,
    write_experiment,
};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Parser)]
#[command(
    name = "frame-loss-review",
    about = "Auditable video frame-loss compensation replay (Rust + GStreamer + GTK4)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Generate deterministic fixtures, replay the recovered and negative cases, and write evidence.
    Demo {
        #[arg(long, default_value = "fixtures")]
        fixtures: PathBuf,
        #[arg(long, default_value = "artifacts/experiment")]
        out: PathBuf,
    },
    /// Replay a record-v2 JSONL file.
    Replay {
        input: PathBuf,
        #[arg(long)]
        trace: Option<PathBuf>,
    },
    /// Migrate a v1 JSON session and replay the generated v2 records.
    ReplayLegacy {
        input: PathBuf,
        #[arg(long)]
        trace: Option<PathBuf>,
    },
    /// Verify every file listed in a SHA-256 manifest.
    VerifyManifest { manifest: PathBuf },
    /// Recompute one trace's state links and evidence chain.
    VerifyTrace { trace: PathBuf },
    /// Open the GTK4 review window for an experiment directory.
    #[cfg(feature = "desktop")]
    Desktop { directory: PathBuf },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Demo { fixtures, out } => run_demo(&fixtures, &out),
        Command::Replay { input, trace } => run_replay(&input, None, trace.as_deref()),
        Command::ReplayLegacy { input, trace } => {
            run_replay(&input, Some("legacy-v1"), trace.as_deref())
        }
        Command::VerifyManifest { manifest } => {
            let failures = verify_manifest(&manifest)?;
            if failures.is_empty() {
                println!("manifest verified: {}", manifest.display());
                Ok(())
            } else {
                for failure in &failures {
                    eprintln!("{failure}");
                }
                bail!("{} manifest check(s) failed", failures.len())
            }
        }
        Command::VerifyTrace { trace } => {
            let failures = frame_loss_review::review::verify_trace(&trace)?;
            if failures.is_empty() {
                println!("trace verified: {}", trace.display());
                Ok(())
            } else {
                for failure in &failures {
                    eprintln!("{failure}");
                }
                bail!("{} trace check(s) failed", failures.len())
            }
        }
        #[cfg(feature = "desktop")]
        Command::Desktop { directory } => frame_loss_review::ui::run(&directory),
    }
}

fn run_demo(fixture_dir: &Path, out_dir: &Path) -> Result<()> {
    fs::create_dir_all(fixture_dir)
        .with_context(|| format!("create fixture directory {}", fixture_dir.display()))?;
    let recovered = fixtures::recovered_bytes()?;
    let truncated = fixtures::truncated_bytes()?;
    let legacy = fixtures::legacy_v1_bytes()?;
    let tampered = fixtures::tampered_bytes()?;
    fs::write(fixture_dir.join("recovered-record.jsonl"), &recovered)?;
    fs::write(fixture_dir.join("truncated-record.jsonl"), &truncated)?;
    fs::write(fixture_dir.join("legacy-v1.json"), &legacy)?;

    let recovered_read = read_jsonl(&recovered);
    let truncated_read = read_jsonl(&truncated);
    let legacy_session = import_legacy(&legacy, "fixtures/legacy-v1.json")?;
    let legacy_bytes = records_jsonl(&legacy_session.records)?;
    let legacy_read = read_jsonl(&legacy_bytes);
    let tampered_read = read_jsonl(&tampered);
    let recovered_report = replay_records(
        "recovered-replay",
        "Replay the complete recovered stream as the control run.",
        &recovered_read,
    )?;
    let truncated_report = replay_records(
        "truncated-prefix",
        "Replay only the bytes physically present before the cut; do not repair the tail.",
        &truncated_read,
    )?;
    let legacy_report = replay_records(
        "legacy-v1-import",
        "Import version 1 fields and replay the migrated v2 records without losing raw evidence.",
        &legacy_read,
    )?;
    let tampered_report = replay_records(
        "tampered-params",
        "Declare a fingerprint that does not match the processing fields; stop on the first semantic error.",
        &tampered_read,
    )?;
    let recovered_records = recovered_read
        .records
        .iter()
        .map(|input| {
            frame_loss_review::model::Record::from_value(input.value.clone())
                .expect("fixture record is valid")
        })
        .collect::<Vec<_>>();
    let recovered_session = session_from_records(
        "studio-take-recovered-01",
        "fixtures/recovered-record.jsonl",
        &recovered,
        &recovered_records,
        vec![
            "physical truncation retained in truncated-record.jsonl".to_string(),
            "recovered replay is the control; no score is emitted".to_string(),
        ],
    )?;
    let inputs = ExperimentInputs {
        recovered_session: &recovered_session,
        recovered_bytes: &recovered,
        truncated_bytes: &truncated,
        truncated_read: &truncated_read,
        legacy_bytes: &legacy,
        legacy_session: &legacy_session,
        recovered_report: &recovered_report,
        truncated_report: &truncated_report,
        legacy_report: &legacy_report,
        tampered_report: &tampered_report,
    };
    write_experiment(out_dir, &inputs)?;
    print_report(&recovered_report, &truncated_report);
    println!(
        "legacy first divergence: {:?}",
        compare_reports(&recovered_report, &legacy_report).map(|item| item.reason)
    );
    println!(
        "tampered first error: {:?}",
        tampered_report
            .first_error
            .as_ref()
            .map(|error| &error.rule)
    );
    println!("evidence written to {}", out_dir.display());
    Ok(())
}

fn run_replay(input: &Path, legacy: Option<&str>, trace: Option<&Path>) -> Result<()> {
    let bytes = fs::read(input).with_context(|| format!("read {}", input.display()))?;
    let (name, description, replay_bytes, session_source) = if legacy == Some("legacy-v1") {
        let session = import_legacy(&bytes, &input.to_string_lossy())?;
        (
            "legacy-v1-import",
            "Import version 1 and replay migrated v2 records.",
            records_jsonl(&session.records)?,
            Some(session),
        )
    } else {
        (
            "manual-replay",
            "Replay a record-v2 JSONL stream.",
            bytes.clone(),
            None,
        )
    };
    let read = read_jsonl(&replay_bytes);
    let report = replay_records(name, description, &read)?;
    if let Some(path) = trace {
        frame_loss_review::review::write_trace(path, &report)?;
    }
    if let Some(session) = session_source {
        print_session(&session)?;
    }
    print_report(&report, &report);
    Ok(())
}

fn replay_records(
    name: &str,
    description: &str,
    input: &frame_loss_review::input::JsonlRead,
) -> Result<ReplayReport> {
    let mut processor = new_processor()?;
    Ok(replay(name, description, input, processor.as_mut()))
}

fn new_processor() -> Result<Box<dyn FrameProcessor>> {
    #[cfg(feature = "gstreamer")]
    {
        Ok(Box::new(
            frame_loss_review::waveform::GstLumaProcessor::new()?,
        ))
    }
    #[cfg(not(feature = "gstreamer"))]
    {
        Ok(Box::new(SyntheticProcessor))
    }
}

fn print_report(recovered: &ReplayReport, scenario: &ReplayReport) {
    println!(
        "{}: accepted {}/{} waveform={} evidence_tip={}",
        scenario.scenario,
        scenario.accepted_steps,
        scenario.input_steps,
        &frame_loss_review::replay::waveform_digest(&scenario.waveform)[..16],
        &scenario.evidence_tip()[..16]
    );
    if let Some(error) = &scenario.first_error {
        println!(
            "first error: step={} record={:?} rule={} expected={} observed={}",
            error.step_index, error.record_id, error.rule, error.expected, error.observed
        );
        println!(
            "state_before: phase={:?} cursor={} accepted={} first_error={:?}",
            error.state_before.phase,
            error.state_before.cursor,
            error.state_before.accepted,
            error
                .state_before
                .first_error
                .as_ref()
                .map(|item| &item.rule)
        );
        println!(
            "state_after:  phase={:?} cursor={} accepted={} first_error={:?}",
            error.state_after.phase,
            error.state_after.cursor,
            error.state_after.accepted,
            error
                .state_after
                .first_error
                .as_ref()
                .map(|item| &item.rule)
        );
    } else {
        println!("first error: none");
    }
    if let Some(divergence) = compare_reports(recovered, scenario) {
        println!(
            "first divergence: #{} {} control={:?} observed={:?}",
            divergence.index,
            divergence.reason,
            divergence.control_record,
            divergence.observed_record
        );
    }
}

fn print_session(session: &Session) -> Result<()> {
    println!(
        "migrated {} -> {}: {} records",
        session
            .compatibility
            .as_ref()
            .map_or("unknown", |compat| compat.source_schema.as_str()),
        session.schema,
        session.records.len()
    );
    Ok(())
}
