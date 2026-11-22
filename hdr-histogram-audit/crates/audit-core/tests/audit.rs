use audit_core::model::{StepState, Verdict};
use audit_core::replay::{capture_truncation, recover_and_replay};
use audit_core::{build_report, run, scenario};

fn find<'a>(records: &'a [audit_core::SampleRecord], id: &str) -> &'a audit_core::SampleRecord {
    records.iter().find(|r| r.raw.sample_id == id).expect("sample present")
}

#[test]
fn every_verdict_is_represented_and_traced() {
    let config = scenario::default_config();
    let samples = scenario::fixture_samples();
    let result = run(&samples, &config);

    let truncated = find(&result.records, "s-03");
    assert_eq!(truncated.verdict, Verdict::Truncated);
    let clip = truncated
        .steps
        .iter()
        .find(|s| s.name == "clip_headroom")
        .unwrap();
    assert_eq!(clip.state, StepState::Truncated);
    assert!(clip.evidence.is_some());
    assert_eq!(clip.evidence.as_ref().unwrap().source_sample_id, "s-03");

    let boundary = find(&result.records, "s-04");
    assert_eq!(boundary.verdict, Verdict::Indeterminate);
    let guard = boundary
        .steps
        .iter()
        .find(|s| s.name == "edge_guard")
        .unwrap();
    assert_eq!(guard.state, StepState::Indeterminate);
    let facts = guard.evidence.as_ref().unwrap().facts.clone();
    assert!(facts["left_bin"].is_number());
    assert!(facts["right_bin"].is_number());

    let broken = find(&result.records, "s-05");
    assert_eq!(broken.verdict, Verdict::Error);
    assert_eq!(broken.first_problem_step(), Some(0));

    let normal = find(&result.records, "s-01");
    assert!(matches!(normal.verdict, Verdict::Binned { .. }));
}

#[test]
fn first_error_points_to_exact_record_and_step() {
    let config = scenario::default_config();
    let report = build_report(&run(&scenario::fixture_samples(), &config));

    // s-03 truncation is processed before the s-04/s-05 problems.
    let first = report.first_error.expect("a problem exists");
    assert_eq!(first.sample_id, "s-03");
    assert_eq!(first.step_name, "clip_headroom");
    assert_eq!(first.state, StepState::Truncated);
    assert_eq!(first.record_index, 2);

    // The hard error is independently reachable, not hidden by the score.
    assert_eq!(report.score.errors, 1);
    assert!(report.problem_records.contains(&4));
}

#[test]
fn replay_after_recovery_reproduces_the_journal() {
    let config = scenario::default_config();
    let samples = scenario::fixture_samples();
    let initial = run(&samples, &config);

    // Interrupt after the first three records were journaled.
    let record = capture_truncation(
        "pipeline flushing on caps change",
        &samples,
        initial.records[..3].to_vec(),
        &config,
    );
    assert_eq!(record.pending_inputs.len(), 3);

    let report = recover_and_replay(&record);
    assert!(report.matches, "replay diverged: {:?}", report.mismatches);
    assert_eq!(report.replayed_sample_count, 3);

    // The recovered run still contains the indeterminate boundary with
    // evidence attached.
    let boundary = find(&report.recovered_run.records, "s-04");
    assert_eq!(boundary.verdict, Verdict::Indeterminate);
    assert!(boundary
        .steps
        .iter()
        .any(|s| s.state == StepState::Indeterminate && s.evidence.is_some()));
}

#[test]
fn histogram_only_counts_binned_samples() {
    let config = scenario::default_config();
    let result = run(&scenario::fixture_samples(), &config);
    let total: u64 = result.histogram.counts.iter().sum();
    assert_eq!(total, 3, "only s-01, s-02 and s-06 are binned");
    assert_eq!(result.histogram.bin_edges.len(), config.bin_count + 1);
}
