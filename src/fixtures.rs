use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::compare;
use crate::engine::{self, RunConfig, RunOutcome};
use crate::evidence::EvidenceStore;
use crate::model::{EdgeParams, Playlist, PlaylistEntry};

/// Deterministic pseudo-random media bytes (xorshift64), so runs are
/// reproducible and hashes comparable across pause/resume.
fn gen_media(path: &Path, seed: u64, size: usize) -> Result<()> {
    let mut state = seed.max(1);
    let mut buf = Vec::with_capacity(size);
    while buf.len() < size {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        buf.extend_from_slice(&state.to_le_bytes());
    }
    buf.truncate(size);
    std::fs::write(path, buf)?;
    Ok(())
}

const SIZES: [usize; 8] = [
    100_000, 100_000, 100_000, 0, 200_000, 100_000, 150_000, 100_000,
];

fn entry(dir: &Path, i: usize, edge: EdgeParams) -> PlaylistEntry {
    PlaylistEntry {
        id: format!("seg-{i:03}"),
        title: format!("Segment {i}"),
        uri: dir.join(format!("seg-{i:03}.bin")).to_string_lossy().into(),
        edge,
    }
}

/// Build the two playlist versions. The missing segment is index 3
/// (seg-003, never written to disk); the entries around it carry extreme
/// parameters.
pub fn generate(root: &Path) -> Result<(Playlist, Playlist)> {
    let media = root.join("media");
    std::fs::create_dir_all(&media)?;
    for (i, size) in SIZES.iter().enumerate() {
        if i == 3 {
            continue; // the missing segment
        }
        gen_media(&media.join(format!("seg-{i:03}.bin")), i as u64 + 1, *size)?;
    }
    // v2 rewrites the entry right after the boundary.
    let v2_media = root.join("media_v2");
    std::fs::create_dir_all(&v2_media)?;
    for (i, size) in SIZES.iter().enumerate() {
        if i == 3 {
            continue;
        }
        let seed = if i == 4 { 999 } else { i as u64 + 1 };
        gen_media(&v2_media.join(format!("seg-{i:03}.bin")), seed, *size)?;
    }

    let extreme_before = EdgeParams {
        chunk_bytes: u64::MAX, // clamped by the engine
        ..EdgeParams::normal()
    };
    let extreme_missing = EdgeParams {
        chunk_bytes: 0,
        max_entry_bytes: 0,
        timeout_ms: 0,
        expect_duration_ns: i64::MAX,
    };
    let extreme_after = EdgeParams {
        max_entry_bytes: 4096, // forces the degraded path
        ..EdgeParams::normal()
    };
    let extreme_timeout = EdgeParams {
        timeout_ms: u64::MAX,
        ..EdgeParams::normal()
    };

    let build = |dir: &Path, version: &str, extra: bool| {
        let mut entries = vec![
            entry(dir, 0, EdgeParams::normal()),
            entry(dir, 1, EdgeParams::normal()),
            entry(dir, 2, extreme_before.clone()),
            entry(dir, 3, extreme_missing.clone()),
            entry(dir, 4, extreme_after.clone()),
            entry(dir, 5, extreme_timeout.clone()),
            entry(dir, 6, EdgeParams::normal()),
        ];
        if extra {
            entries.push(entry(dir, 7, EdgeParams::normal()));
        }
        Playlist {
            version: version.to_string(),
            entries,
        }
    };
    let v1 = build(&media, "v1", false);
    let v2 = build(&v2_media, "v2", true);
    std::fs::write(
        root.join("playlist_v1.json"),
        serde_json::to_string_pretty(&v1)?,
    )?;
    std::fs::write(
        root.join("playlist_v2.json"),
        serde_json::to_string_pretty(&v2)?,
    )?;
    Ok((v1, v2))
}

pub fn cmd(args: Vec<String>) -> Result<()> {
    let dir = args
        .first()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("artifacts/demo"));
    let (v1, v2) = generate(&dir)?;
    println!(
        "fixtures ready in {} (v1: {} entries, v2: {} entries)",
        dir.display(),
        v1.entries.len(),
        v2.entries.len()
    );
    Ok(())
}

/// Global read budget: deliberately too small for the whole playlist, so the
/// tail entries hit the resource-insufficiency path (evidence of degradation).
const DEMO_BUDGET_BYTES: u64 = 500_096;

fn run_logged(
    playlist: &Playlist,
    cfg: &RunConfig,
    resume: bool,
    ev: &EvidenceStore,
    tag: &str,
) -> Result<RunOutcome> {
    engine::run(
        playlist,
        cfg,
        resume,
        |event| {
            let line = serde_json::to_string(&event).unwrap();
            let _ = ev.log(&format!("[{tag}] {line}"));
        },
    )
}

/// Full scenario: pause mid-run, resume, compare versions, keep evidence.
pub fn cmd_demo(args: Vec<String>) -> Result<()> {
    let root = args
        .iter()
        .position(|a| a == "--root")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("artifacts/demo"));
    let ev = EvidenceStore::new(root.join("evidence"))?;
    ev.log("demo: generating fixtures")?;
    let (v1, v2) = generate(&root)?;
    ev.write("playlist_v1.json", &v1)?;
    ev.write("playlist_v2.json", &v2)?;

    let mk = |name: &str, pause_after: Option<usize>| RunConfig {
        out_dir: root.join(name),
        pause_after,
        budget_bytes: DEMO_BUDGET_BYTES,
        pause_flag: None,
    };

    // 1. v1, paused after 2 entries -> intermediate state on disk.
    ev.log("demo: run v1 with pause-after=2")?;
    let out = run_logged(&v1, &mk("run_v1", Some(2)), false, &ev, "v1-pause")?;
    anyhow::ensure!(matches!(out, RunOutcome::Paused(2)), "expected pause at 2");
    ev.snapshot(
        &crate::checkpoint::path_for(&root.join("run_v1")),
        "checkpoint_v1_paused.json",
    )?;

    // 2. Resume from the checkpoint and finish.
    ev.log("demo: resume v1")?;
    let out = run_logged(&v1, &mk("run_v1", None), true, &ev, "v1-resume")?;
    let results_v1 = match out {
        RunOutcome::Completed(r) => r,
        _ => anyhow::bail!("resume did not complete"),
    };
    ev.snapshot(&root.join("run_v1/results.json"), "results_v1.json")?;
    ev.snapshot(
        &crate::checkpoint::path_for(&root.join("run_v1")),
        "checkpoint_v1_final.json",
    )?;

    // 3. Uninterrupted reference run of v1 for the consistency check.
    ev.log("demo: reference run v1")?;
    let out = run_logged(&v1, &mk("run_v1_ref", None), false, &ev, "v1-ref")?;
    let results_ref = match out {
        RunOutcome::Completed(r) => r,
        _ => unreachable!(),
    };
    let consistency = compare::consistency(&results_v1, &results_ref);
    ev.write("consistency_report.json", &consistency)?;
    ev.log(&format!("demo: pause/resume consistent = {}", consistency.consistent))?;

    // 4. v2 full run.
    ev.log("demo: run v2")?;
    let out = run_logged(&v2, &mk("run_v2", None), false, &ev, "v2")?;
    let results_v2 = match out {
        RunOutcome::Completed(r) => r,
        _ => unreachable!(),
    };
    ev.snapshot(&root.join("run_v2/results.json"), "results_v2.json")?;

    // 5. Version diff with boundary samples around the missing segment.
    let report = compare::diff(&v1, &results_v1, &v2, &results_v2);
    ev.write("diff_report.json", &report)?;
    ev.write("boundary_samples.json", &report.boundary_samples)?;
    ev.log("demo: done")?;

    println!("evidence in {}", ev.root.display());
    println!("pause/resume consistent: {}", consistency.consistent);
    println!(
        "boundary: index {:?} id {:?}, samples: {}",
        report.boundary_index,
        report.boundary_entry_id,
        report.boundary_samples.len()
    );
    Ok(())
}
