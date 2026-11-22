use std::path::{Path, PathBuf};

use bframe_addressing_lab::evidence::{run_builtin, write_evidence};
use bframe_addressing_lab::journal::parse_journal;
use bframe_addressing_lab::replay::replay_entries;

fn main() {
    if let Err(error) = run() {
        eprintln!("bframe-evidence: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "--replay") {
        let path = args
            .get(1)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("evidence"));
        return replay_saved(&path);
    }
    let output_dir = args
        .first()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("evidence"));
    let run = run_builtin()?;
    write_evidence(&run, &output_dir)?;

    println!(
        "scenarios={} entries={} checks_passed={}/{} evidence={} cleanup_removed={}",
        run.bundle.scenarios.len(),
        run.bundle
            .scenarios
            .iter()
            .map(|scenario| scenario.entries.len())
            .sum::<usize>(),
        run.bundle
            .checks
            .iter()
            .filter(|check| check.passed)
            .count(),
        run.bundle.checks.len(),
        output_dir.display(),
        run.bundle.cleanup.temporary_directory_removed,
    );
    if let Some(gst) = &run.bundle.gstreamer {
        println!(
            "gstreamer_version={} roundtrip_observed={} timestamps_preserved={}",
            gst["version"].as_str().unwrap_or("unknown"),
            gst["observed_frames"].as_u64().unwrap_or(0),
            gst["timestamps_preserved"].as_bool().unwrap_or(false),
        );
    }
    if !run.bundle.checks.iter().all(|check| check.passed) {
        anyhow::bail!("one or more key-evidence checks failed");
    }
    Ok(())
}

fn replay_saved(path: &Path) -> anyhow::Result<()> {
    let mut files = if path.is_dir() {
        std::fs::read_dir(path)?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
            .collect::<Vec<_>>()
    } else {
        vec![path.to_path_buf()]
    };
    files.sort();
    anyhow::ensure!(
        !files.is_empty(),
        "no JSONL journals found in {}",
        path.display()
    );

    let mut failed = false;
    for file in files {
        let raw = std::fs::read_to_string(&file)?;
        let entries = parse_journal(&raw)?;
        let check = replay_entries(&entries);
        println!(
            "{} entries={} matched={} mismatch={}",
            file.display(),
            check.checked_entries,
            check.matched,
            check
                .first_mismatch
                .as_ref()
                .map(|mismatch| format!(
                    "seq {} {} expected {} actual {}",
                    mismatch.seq, mismatch.field, mismatch.expected, mismatch.actual
                ))
                .unwrap_or_else(|| "none".to_string()),
        );
        failed |= !check.matched;
    }
    anyhow::ensure!(!failed, "at least one journal did not replay exactly");
    Ok(())
}
