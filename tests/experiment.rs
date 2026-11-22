use std::fs;

use bwf_replay_lab::{ExperimentReport, run_experiment};
use tempfile::tempdir;

#[test]
fn experiment_localizes_first_error_and_keeps_control_replay() {
    let temp = tempdir().expect("tempdir");
    let output = run_experiment(Some(temp.path())).expect("run experiment");
    let report = output.report;

    let first = report.first_error.as_ref().expect("global first error");
    assert_eq!(first.step_index, 2);
    assert_eq!(first.code, "data_truncated");
    assert_eq!(first.state_before.name, "asset_read");
    assert_eq!(first.error_state.name, "rejected");

    assert_eq!(report.runs.len(), 4);
    let control = report
        .runs
        .iter()
        .find(|run| run.id == "recovered_control")
        .expect("control run");
    assert!(control.passed);
    assert!(control.first_error.is_none());
    let replay = control
        .steps
        .iter()
        .find(|step| step.result.code == "gst_eos_full_duration")
        .expect("GStreamer replay step");
    let evidence = replay.evidence.as_object().expect("replay evidence");
    assert_eq!(evidence["eos"], true);
    assert_eq!(
        evidence["decoded_frames"].as_u64(),
        evidence["expected_frames"].as_u64()
    );

    let order = report
        .runs
        .iter()
        .find(|run| run.id == "order_changed")
        .expect("order run");
    assert_eq!(
        order.first_error.as_ref().map(|error| error.code.as_str()),
        Some("marker_order_violation")
    );
    assert_eq!(
        order.first_error.as_ref().map(|error| error.step_index),
        Some(3)
    );

    let duplicate = report
        .runs
        .iter()
        .find(|run| run.id == "duplicate_submission")
        .expect("duplicate run");
    assert_eq!(
        duplicate
            .first_error
            .as_ref()
            .map(|error| error.code.as_str()),
        Some("duplicate_marker")
    );
    assert_eq!(
        duplicate.first_error.as_ref().map(|error| error.step_index),
        Some(4)
    );

    let parsed: ExperimentReport =
        serde_json::from_slice(&fs::read(&output.report_path).expect("read report"))
            .expect("report is valid JSON");
    assert_eq!(parsed.schema_version, 1);
    assert_eq!(
        report.evidence.truncated.sha256,
        report.evidence.recovery.source_sha256
    );
    assert_ne!(
        report.evidence.truncated.sha256,
        report.evidence.recovery.recovered_sha256
    );
    assert!(output.evidence_dir.join("steps.jsonl").exists());
    assert!(output.evidence_dir.join("first-error.json").exists());
}
