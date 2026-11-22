use frame_loss_review::fixtures;
use frame_loss_review::input::read_jsonl;
use frame_loss_review::model::{FrameState, import_legacy};
use frame_loss_review::replay::{Phase, ReplayReport, SyntheticProcessor, replay};
use frame_loss_review::review::{
    ExperimentInputs, compare_reports, records_jsonl, session_from_records, verify_manifest,
    write_experiment,
};

fn run(name: &str, bytes: &[u8]) -> (frame_loss_review::input::JsonlRead, ReplayReport) {
    let read = read_jsonl(bytes);
    let report = replay(name, name, &read, &mut SyntheticProcessor);
    (read, report)
}

#[test]
fn recovered_replay_has_timestamp_track_frame_and_waveform_links() {
    let bytes = fixtures::recovered_bytes().unwrap();
    let (_, report) = run("recovered", &bytes);
    assert_eq!(report.final_phase, Phase::Verified);
    assert_eq!(report.accepted_steps, 11);
    assert_eq!(report.first_error, None);
    assert_eq!(report.waveform.len(), 11);
    assert_eq!(report.waveform[4].source_state, FrameState::Missing);
    assert_eq!(report.waveform[4].time_ms, 133);
    assert_eq!(report.waveform[4].track_id, "video-main");
    assert_eq!(report.waveform[4].frame, 4);
    assert_eq!(report.waveform[4].record_id, "flr-0004");
    assert_eq!(report.waveform[3].time_ms, 100);
    assert_eq!(report.waveform[7].time_ms, 233);
    for point in &report.waveform {
        assert_eq!(point.pts_ns, point.time_ms as i64 * 1_000_000);
        assert_eq!(point.evidence_sha256.len(), 64);
        assert_eq!(point.params_fingerprint.len(), 16);
    }
}

#[test]
fn truncated_record_stops_at_the_physical_tail_with_state_before_and_after() {
    let bytes = fixtures::truncated_bytes().unwrap();
    assert_ne!(bytes.last(), Some(&b'\n'));
    let (read, report) = run("truncated-prefix", &bytes);
    assert_eq!(read.records.len(), 7);
    assert_eq!(read.error.as_ref().unwrap().raw.len(), 269);
    let error = report.first_error.as_ref().expect("first error");
    assert_eq!(error.rule, "input_truncated");
    assert_eq!(error.step_index, 7);
    assert_eq!(error.state_before.phase, Phase::Streaming);
    assert_eq!(error.state_before.accepted, 7);
    assert!(error.state_before.first_error.is_none());
    assert_eq!(error.state_after.phase, Phase::Faulted);
    assert_eq!(error.state_after.accepted, 7);
    assert_eq!(
        error.state_after.first_error.as_ref().unwrap().rule,
        "input_truncated"
    );
    assert_eq!(report.steps.len(), 8);
}

#[test]
fn tampered_fingerprint_stops_on_the_first_semantic_error() {
    let (_, report) = run("tampered-params", &fixtures::tampered_bytes().unwrap());
    let error = report.first_error.as_ref().expect("first error");
    assert_eq!(error.step_index, 8);
    assert_eq!(error.record_id.as_deref(), Some("flr-0008"));
    assert_eq!(error.rule, "params_fingerprint_mismatch");
    assert_eq!(error.state_before.accepted, 8);
    assert_eq!(error.state_after.accepted, 8);
    assert_eq!(report.steps.len(), 9);
    assert_eq!(report.accepted_steps, 8);
}

#[test]
fn legacy_v1_import_preserves_raw_evidence_and_replays_semantically() {
    let legacy = fixtures::legacy_v1_bytes().unwrap();
    let session = import_legacy(&legacy, "fixtures/legacy-v1.json").unwrap();
    assert_eq!(session.records.len(), 7);
    let compatibility = session.compatibility.as_ref().unwrap();
    assert_eq!(compatibility.source_schema, "frame-loss-review/v1");
    assert!(compatibility.migrated);
    assert_eq!(
        compatibility.raw_sha256,
        frame_loss_review::model::sha256_hex(&legacy)
    );
    assert!(compatibility.warnings.len() >= 8);

    let migrated = records_jsonl(&session.records).unwrap();
    let (_, legacy_report) = run("legacy-v1-import", &migrated);
    let (_, recovered_report) = run("recovered", &fixtures::recovered_bytes().unwrap());
    assert_eq!(legacy_report.accepted_steps, 7);
    assert_eq!(legacy_report.first_error, None);
    let divergence = compare_reports(&recovered_report, &legacy_report).unwrap();
    assert_eq!(divergence.index, 7);
    assert_eq!(divergence.reason, "input_missing");
}

#[test]
fn evidence_chain_is_linked_and_experiment_manifest_verifies() {
    let recovered_bytes = fixtures::recovered_bytes().unwrap();
    let truncated_bytes = fixtures::truncated_bytes().unwrap();
    let legacy_bytes = fixtures::legacy_v1_bytes().unwrap();
    let tampered_bytes = fixtures::tampered_bytes().unwrap();
    let (recovered_read, recovered_report) = run("recovered-replay", &recovered_bytes);
    let (truncated_read, truncated_report) = run("truncated-prefix", &truncated_bytes);
    let legacy_session = import_legacy(&legacy_bytes, "fixtures/legacy-v1.json").unwrap();
    let legacy_migrated = records_jsonl(&legacy_session.records).unwrap();
    let (_, legacy_report) = run("legacy-v1-import", &legacy_migrated);
    let (_, tampered_report) = run("tampered-params", &tampered_bytes);
    for (index, pair) in recovered_report.steps.windows(2).enumerate() {
        assert_eq!(
            pair[1].evidence.previous_evidence_sha256, pair[0].evidence.evidence_sha256,
            "chain break at {index}"
        );
    }
    let records: Vec<_> = recovered_read
        .records
        .iter()
        .map(|input| frame_loss_review::model::Record::from_value(input.value.clone()).unwrap())
        .collect();
    let recovered_session = session_from_records(
        "test-recovered",
        "fixtures/recovered-record.jsonl",
        &recovered_bytes,
        &records,
        Vec::new(),
    )
    .unwrap();
    let inputs = ExperimentInputs {
        recovered_session: &recovered_session,
        recovered_bytes: &recovered_bytes,
        truncated_bytes: &truncated_bytes,
        truncated_read: &truncated_read,
        legacy_bytes: &legacy_bytes,
        legacy_session: &legacy_session,
        recovered_report: &recovered_report,
        truncated_report: &truncated_report,
        legacy_report: &legacy_report,
        tampered_report: &tampered_report,
    };
    let dir = tempfile::tempdir().unwrap();
    write_experiment(dir.path(), &inputs).unwrap();
    let summary: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("summary.json")).unwrap()).unwrap();
    assert_eq!(summary["has_total_score"], false);
    assert_eq!(
        summary["first_error_across_scenarios"]["rule"],
        "input_truncated"
    );
    assert!(
        verify_manifest(&dir.path().join("manifest.sha256.json"))
            .unwrap()
            .is_empty()
    );
    let trace_path = dir.path().join("recovered.trace.jsonl");
    let trace = std::fs::read_to_string(&trace_path).unwrap();
    let mut first: serde_json::Value = serde_json::from_str(trace.lines().next().unwrap()).unwrap();
    first["outcome"]["effect"] = serde_json::Value::String("tampered".to_string());
    let mut lines: Vec<String> = trace.lines().map(str::to_string).collect();
    lines[0] = serde_json::to_string(&first).unwrap();
    std::fs::write(&trace_path, lines.join("\n") + "\n").unwrap();
    let failures = frame_loss_review::review::verify_trace(&trace_path).unwrap();
    assert!(
        failures
            .iter()
            .any(|failure| failure.contains("do not reproduce"))
    );
}
