//! End-to-end replayable experiment tests: two snapshot versions, the
//! truncated control, order changes, duplicate submissions and cleanup.

use hls_resume_lab::cleanup;
use hls_resume_lab::core::journal::Journal;
use hls_resume_lab::core::runner::{self, RunConfig};
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn fixtures_dir() -> PathBuf {
    repo_root().join("fixtures/snapshots")
}

fn test_config(work: &Path, with_gst: bool) -> RunConfig {
    let f = fixtures_dir();
    RunConfig {
        snapshots: vec![
            ("v1".into(), f.join("v1_2026-11-20.json")),
            ("v2".into(), f.join("v2_2026-11-21.json")),
            (
                "truncated-control".into(),
                f.join("v2_truncated_control.json"),
            ),
        ],
        duplicate_ids: vec!["r-1001".into(), "r-2002".into()],
        verify_reversed_order: true,
        evidence_dir: work.join("evidence"),
        temp_dir: work.join("scratch"),
        run_gst_probes: with_gst,
    }
}

#[test]
fn full_experiment_records_every_step_and_keeps_going_after_truncation() {
    let work = repo_root().join("target/test-work/full");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).unwrap();
    let config = test_config(&work, false);

    let outcome = runner::run(&config, "test-full").expect("run completes");

    // Two healthy snapshots load; the truncated one fails but does not abort.
    assert_eq!(outcome.totals.snapshots_loaded, 2);
    assert_eq!(outcome.totals.snapshots_failed, 1);
    assert!(outcome.totals.duplicates >= 2, "duplicates observed");

    let truncated = outcome
        .reports
        .iter()
        .find(|r| r.snapshot_id == "truncated-control")
        .expect("truncated report present");
    assert!(!truncated.load_ok);
    assert!(
        truncated.load_error.as_ref().unwrap().contains("truncated"),
        "truncation classified explicitly"
    );

    // Every step is journaled and replayable.
    let entries = Journal::replay(&outcome.journal_path).unwrap();
    let phases: Vec<&str> = entries.iter().map(|e| e.phase.as_str()).collect();
    assert!(phases.contains(&"load_snapshot"));
    assert!(phases.contains(&"diagnose"));
    assert!(phases.contains(&"plan_snapshot"));
    assert!(phases.first() == Some(&"run_start"));
    assert!(phases.last() == Some(&"run_end"));

    // Order-change comparison evidence exists on disk.
    let journal_text = std::fs::read_to_string(&outcome.journal_path).unwrap();
    assert!(journal_text.contains("order_change"));
    assert!(journal_text.contains("reversed_order"));

    // Report file is durable JSON.
    let report: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&outcome.report_path).unwrap()).unwrap();
    assert_eq!(report["run_id"], "test-full");
}

#[test]
fn temp_state_is_cleaned_while_evidence_survives() {
    let work = repo_root().join("target/test-work/cleanup");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).unwrap();
    let config = test_config(&work, false);

    let outcome = runner::run(&config, "test-cleanup").unwrap();
    assert!(config.temp_dir.exists());
    let message = cleanup::remove_temp_dir(&config.temp_dir);
    assert!(message.contains("已删除"));
    assert!(!config.temp_dir.exists());
    // Evidence must remain after cleanup.
    assert!(outcome.journal_path.exists());
    assert!(outcome.report_path.exists());
}

#[test]
fn duplicate_runs_are_deterministic_given_same_inputs() {
    let work_a = repo_root().join("target/test-work/det-a");
    let work_b = repo_root().join("target/test-work/det-b");
    let _ = std::fs::remove_dir_all(&work_a);
    let _ = std::fs::remove_dir_all(&work_b);
    std::fs::create_dir_all(&work_a).unwrap();
    std::fs::create_dir_all(&work_b).unwrap();

    let a = runner::run(&test_config(&work_a, false), "det").unwrap();
    let b = runner::run(&test_config(&work_b, false), "det").unwrap();
    assert_eq!(a.totals, b.totals);
}
