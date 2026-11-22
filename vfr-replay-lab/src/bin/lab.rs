//! Headless experiment driver for the VFR replay lab.
//!
//! Subcommands:
//! - `fixture <out.webm>`      synthesize a VFR clip via GStreamer
//! - `run-scenario <dir>`      execute the full recorded experiment
//! - `replay <journal.jsonl>`  re-verify a recorded journal
//! - `tamper <journal.jsonl>`  corrupt one entry to prove chain detection

use std::fs;
use std::path::PathBuf;
use vfr_replay_lab::media;
use vfr_replay_lab::model::{Clip, Command, Outcome};
use vfr_replay_lab::persist::{append_entry, read_journal, write_json};
use vfr_replay_lab::replay::replay;
use vfr_replay_lab::{Engine, JournalEntry};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("fixture") => cmd_fixture(args.get(2).expect("usage: fixture <out.webm>")),
        Some("run-scenario") => cmd_run_scenario(
            args.get(2)
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("evidence/run")),
        ),
        Some("replay") => cmd_replay(args.get(2).expect("usage: replay <journal>")),
        Some("tamper") => cmd_tamper(args.get(2).expect("usage: tamper <journal>")),
        _ => {
            eprintln!("usage: vfr-lab <fixture|run-scenario|replay|tamper> [args]");
            std::process::exit(2);
        }
    }
}

fn cmd_fixture(out: &str) {
    if let Some(parent) = PathBuf::from(out).parent() {
        fs::create_dir_all(parent).expect("create fixture dir");
    }
    let pts = media::build_vfr_fixture(out, 160, 120).expect("build fixture");
    println!("fixture={out}");
    println!("frames={} pts_ns={pts:?}", pts.len());
}

/// The complete recorded experiment. Every step is appended to the journal and
/// the GStreamer evidence is captured alongside the engine transitions.
fn cmd_run_scenario(dir: PathBuf) {
    fs::create_dir_all(&dir).expect("create run dir");
    let fixture = dir.join("vfr_sample.webm");
    let journal_path = dir.join("journal.jsonl");
    let fixture_uri = format!("file://{}", fixture.display());
    // Each scenario run starts a fresh append-only journal.
    fs::write(&journal_path, b"").expect("reset journal");

    // --- Stage 0: synthesize a real VFR fixture through GStreamer. ---
    let fixture_pts =
        media::build_vfr_fixture(fixture.to_str().unwrap(), 160, 120).expect("fixture");
    let probed_pts = media::probe_pts(&fixture_uri).expect("probe pts");

    let mut engine = Engine::new();
    let mut media_evidence = serde_json::Map::new();

    fn step<'a>(
        engine: &'a mut Engine,
        journal_path: &PathBuf,
        key: &str,
        cmd: Command,
    ) -> &'a JournalEntry {
        let entry = engine.apply(key.to_string(), cmd);
        append_entry(journal_path, entry).expect("append journal");
        entry
    }

    // --- Stage 1: normal ordered workflow. ---
    step(
        &mut engine,
        &journal_path,
        "add-vfr",
        Command::AddClip {
            clip: Clip {
                id: "vfr-1".into(),
                uri: fixture_uri.clone(),
                pts_ns: fixture_pts.clone(),
            },
        },
    );
    step(
        &mut engine,
        &journal_path,
        "place-vfr",
        Command::Place {
            clip_id: "vfr-1".into(),
            position: 0,
        },
    );
    step(
        &mut engine,
        &journal_path,
        "rate-24",
        Command::SetRate {
            fps_num: 24,
            fps_den: 1,
        },
    );
    let preview_24 = media::run_preview(&fixture_uri, 24, 1).expect("preview 24");
    media_evidence.insert(
        "preview_at_24fps".into(),
        serde_json::to_value(&preview_24).unwrap(),
    );

    // --- Stage 2: order change (swap) then re-preview at a new rate. ---
    step(
        &mut engine,
        &journal_path,
        "add-bumper",
        Command::AddClip {
            clip: Clip {
                id: "bumper".into(),
                uri: "file:///fixture/bumper.webm".into(),
                pts_ns: vec![0, 41_666_667, 83_333_334],
            },
        },
    );
    step(
        &mut engine,
        &journal_path,
        "place-bumper",
        Command::Place {
            clip_id: "bumper".into(),
            position: 1,
        },
    );
    step(
        &mut engine,
        &journal_path,
        "swap-head",
        Command::Swap {
            position_a: 0,
            position_b: 1,
        },
    );
    step(
        &mut engine,
        &journal_path,
        "rate-60",
        Command::SetRate {
            fps_num: 60,
            fps_den: 1,
        },
    );
    let preview_60 = media::run_preview(&fixture_uri, 60, 1).expect("preview 60");
    media_evidence.insert(
        "preview_at_60fps".into(),
        serde_json::to_value(&preview_60).unwrap(),
    );

    // --- Stage 3: duplicate submission (same idempotency key twice). ---
    let dup_first = step(
        &mut engine,
        &journal_path,
        "idem-remove",
        Command::Remove {
            clip_id: "bumper".into(),
        },
    )
    .outcome
    .clone();
    let dup_second = step(
        &mut engine,
        &journal_path,
        "idem-remove",
        Command::Remove {
            clip_id: "bumper".into(),
        },
    )
    .outcome
    .clone();
    assert!(matches!(dup_first, Outcome::Accepted { .. }));
    assert!(matches!(dup_second, Outcome::Duplicate { .. }));

    // --- Stage 4: unknown extension control — the control sample. ---
    let ext_outcome = step(
        &mut engine,
        &journal_path,
        "ext-x-twist",
        Command::UnknownExtension {
            name: "x-experimental-twist".into(),
            payload: "phase=invert".into(),
        },
    )
    .outcome
    .clone();
    assert!(matches!(ext_outcome, Outcome::UnknownIgnored { .. }));

    // --- Stage 5: replay from the recovered journal. ---
    let recovered = read_journal(&journal_path).expect("read journal");
    let report = replay(&recovered).expect("replay");

    let mut evidence = serde_json::Map::new();
    evidence.insert(
        "fixture_pts_ns".into(),
        serde_json::to_value(&fixture_pts).unwrap(),
    );
    evidence.insert(
        "probed_pts_ns".into(),
        serde_json::to_value(&probed_pts).unwrap(),
    );
    evidence.insert("media".into(), serde_json::Value::Object(media_evidence));
    evidence.insert(
        "journal_entries".into(),
        serde_json::to_value(engine.entries().len()).unwrap(),
    );
    evidence.insert("replay".into(), serde_json::to_value(&report).unwrap());
    evidence.insert(
        "final_state".into(),
        serde_json::to_value(engine.state()).unwrap(),
    );
    write_json(
        dir.join("evidence.json"),
        &serde_json::Value::Object(evidence),
    )
    .expect("write evidence");

    println!(
        "scenario complete: {} journal entries",
        engine.entries().len()
    );
    println!("replay_ok={}", report.is_ok());
    println!("evidence: {}", dir.join("evidence.json").display());
}

fn cmd_replay(path: &str) {
    let entries = read_journal(path).expect("read journal");
    match replay(&entries) {
        Ok(report) => {
            println!(
                "replay steps={} ok={} state_divergences={:?} outcome_divergences={:?}",
                report.steps_replayed,
                report.is_ok(),
                report.state_divergences,
                report.outcome_divergences
            );
            if !report.is_ok() {
                std::process::exit(1);
            }
        }
        Err(e) => {
            eprintln!("CHAIN FAILURE: {e}");
            std::process::exit(1);
        }
    }
}

/// Deliberately corrupt a journal line to demonstrate tamper detection.
fn cmd_tamper(path: &str) {
    let content = fs::read_to_string(path).expect("read journal");
    let mut lines: Vec<String> = content.lines().map(str::to_string).collect();
    if lines.len() < 2 {
        eprintln!("journal too short to tamper");
        std::process::exit(2);
    }
    // Flip a character inside the first entry's command payload.
    lines[0] = lines[0].replacen("vfr-1", "vfr-9", 1);
    let out = tampered_path(path);
    fs::write(&out, lines.join("\n")).expect("write tampered journal");
    println!("tampered journal: {}", out.display());
}

fn tampered_path(path: &str) -> PathBuf {
    let p = PathBuf::from(path);
    let stem = p
        .file_stem()
        .map(|s| s.to_str().unwrap().to_string())
        .unwrap_or_else(|| "journal".into());
    p.with_file_name(format!("{stem}.tampered.jsonl"))
}
