use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Result, anyhow, bail};
use audio_repair::{Limits, RunOptions, demo, run, verify_bundle};

fn main() -> ExitCode {
    match execute() {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::from(1)
        }
    }
}

fn execute() -> Result<u8> {
    let mut args = std::env::args().skip(1);
    let Some(command) = args.next() else {
        print_help();
        return Ok(2);
    };
    let flags = parse_flags(args.collect())?;
    match command.as_str() {
        "run" => command_run(flags),
        "verify" => command_verify(flags),
        "demo" => command_demo(flags),
        "empty" => command_boundary(flags, false),
        "oversized" => command_boundary(flags, true),
        "help" | "--help" | "-h" => {
            print_help();
            Ok(0)
        }
        other => bail!("unknown command: {other}"),
    }
}

fn command_run(flags: BTreeMap<String, String>) -> Result<u8> {
    let recording = required(&flags, "--audio")?;
    let intervals = required(&flags, "--intervals")?;
    let versions = required(&flags, "--versions")?;
    let output = required(&flags, "--out")?;
    reject_unknown(
        &flags,
        &["--audio", "--intervals", "--versions", "--out", "--json"],
    )?;
    let options = RunOptions::new(recording, intervals, versions, output);
    let outcome = run(options)?;
    print_outcome(&outcome.report, &outcome.report_path);
    if flags.contains_key("--json") {
        println!("{}", serde_json::to_string_pretty(&outcome.report)?);
    }
    Ok(if outcome.succeeded() { 0 } else { 2 })
}

fn command_verify(flags: BTreeMap<String, String>) -> Result<u8> {
    reject_unknown(&flags, &["--dir"])?;
    let directory = required(&flags, "--dir")?;
    let summary = verify_bundle(&PathBuf::from(directory))?;
    println!("run_id={}", summary.run_id);
    println!("status={}", summary.status);
    println!("artifacts_verified={}", summary.artifacts_verified);
    println!(
        "inputs_verified={} inputs_missing={}",
        summary.inputs_verified, summary.inputs_missing
    );
    for warning in &summary.warnings {
        println!("warning={warning}");
    }
    println!("verification=passed");
    Ok(0)
}

fn command_demo(flags: BTreeMap<String, String>) -> Result<u8> {
    reject_unknown(&flags, &["--work", "--out"])?;
    let work = flags
        .get("--work")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("demo-input"));
    let out = flags
        .get("--out")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("demo-output"));
    let inputs = demo::write_demo_inputs(&work)?;
    let options = RunOptions::new(inputs.recording, inputs.intervals, inputs.versions, &out);
    let outcome = run(options)?;
    print_outcome(&outcome.report, &outcome.report_path);
    println!("verify_command=audio-repair-cli verify {}", out.display());
    Ok(if outcome.succeeded() { 0 } else { 2 })
}

fn command_boundary(flags: BTreeMap<String, String>, oversized: bool) -> Result<u8> {
    reject_unknown(&flags, &["--work", "--out"])?;
    let label = if oversized { "oversized" } else { "empty" };
    let work = flags
        .get("--work")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(format!("{label}-input")));
    let out = flags
        .get("--out")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(format!("{label}-output")));
    let inputs = if oversized {
        demo::write_oversized_inputs(&work, Limits::default().max_intervals)?
    } else {
        demo::write_empty_inputs(&work)?
    };
    let options = RunOptions::new(inputs.recording, inputs.intervals, inputs.versions, &out);
    let outcome = run(options)?;
    print_outcome(&outcome.report, &outcome.report_path);
    Ok(if oversized {
        if outcome.succeeded() { 2 } else { 0 }
    } else if outcome.succeeded() {
        0
    } else {
        2
    })
}

fn print_outcome(report: &audio_repair::model::RepairReport, path: &std::path::Path) {
    println!("run_id={}", report.run_id);
    println!("status={:?}", report.status);
    println!("report={}", path.display());
    println!(
        "media=rate:{} channels:{} frames:{} duration_ms:{:.3}",
        report.media.decoded_sample_rate,
        report.media.channels,
        report.media.total_frames,
        report.media.duration_ms
    );
    for clock in &report.clocks {
        println!(
            "clock={} role={} offset_ms:{:.3} drift_ppm:{:.3} skew_ms:{:.3}",
            clock.device_id,
            clock.role,
            clock.offset_ms,
            clock.drift_ppm,
            clock.max_observed_skew_ms
        );
    }
    for interval in &report.intervals {
        println!(
            "interval={} device={} canonical_ms:{:.3}..{:.3} frames:{}..{} disposition={}",
            interval.interval_id,
            interval.device_id,
            interval.canonical_start_ms,
            interval.canonical_end_ms,
            interval.applied_start_frame,
            interval.applied_end_frame,
            interval.disposition
        );
    }
    for version in &report.versions {
        println!(
            "version={} order={} output={} sha256={} warnings={}",
            version.version_id,
            version.manifest_index,
            version.output_path,
            version.output_sha256,
            version.warnings.len()
        );
    }
    for warning in report
        .evidence
        .iter()
        .filter(|entry| entry.severity == "warning")
    {
        println!("warning={}", warning.message);
    }
}

fn parse_flags(args: Vec<String>) -> Result<BTreeMap<String, String>> {
    let mut flags = BTreeMap::new();
    let mut index = 0;
    while index < args.len() {
        let key = &args[index];
        if !key.starts_with("--") {
            bail!("expected a --flag, found {key}");
        }
        if key == "--json" {
            flags.insert(key.clone(), "true".to_string());
            index += 1;
            continue;
        }
        let value = args
            .get(index + 1)
            .ok_or_else(|| anyhow!("missing value for {key}"))?;
        if flags.insert(key.clone(), value.clone()).is_some() {
            bail!("duplicate flag {key}");
        }
        index += 2;
    }
    Ok(flags)
}

fn required(flags: &BTreeMap<String, String>, key: &str) -> Result<String> {
    flags
        .get(key)
        .cloned()
        .ok_or_else(|| anyhow!("missing required {key}"))
}

fn reject_unknown(flags: &BTreeMap<String, String>, allowed: &[&str]) -> Result<()> {
    for key in flags.keys() {
        if !allowed.contains(&key.as_str()) {
            bail!("unknown flag {key}");
        }
    }
    Ok(())
}

fn print_help() {
    println!(
        "audio-repair-cli\n\
         \n\
         run     --audio <file> --intervals <json> --versions <json> --out <dir> [--json]\n\
         verify  --dir <artifact-dir>\n\
         demo    [--work <input-dir>] [--out <artifact-dir>]\n\
         empty   [--work <input-dir>] [--out <artifact-dir>]\n\
         oversized [--work <input-dir>] [--out <artifact-dir>]\n\
         \n\
         Artifacts: report.json, artifact_manifest.json, review.md, review.csv,\n\
         SHA256SUMS, media/*.wav, waveforms/*.csv"
    );
}
