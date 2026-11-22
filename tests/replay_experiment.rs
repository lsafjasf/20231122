use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use media_meta_replay::SCHEMA;
use media_meta_replay::evidence::sha256_bytes;
use media_meta_replay::experiment::Experiment;
use media_meta_replay::fixture::{
    GSTREAMER_PROBE_JSON, demo_record, load_truncated_bytes, load_truncated_fixture, probe_fields,
    truncated_fixture_bytes,
};
use media_meta_replay::model::{Phase, StepOutcome};

#[test]
fn recovered_control_completes_without_rule_error() {
    let experiment = Experiment::builtin().expect("built-in experiment");
    assert_eq!(experiment.control.traces.len(), 7);
    assert!(experiment.control.first_error.is_none());
    assert_eq!(experiment.control.final_state.phase, Phase::Verified);
    assert!(
        experiment
            .control
            .traces
            .iter()
            .all(|trace| matches!(trace.outcome, StepOutcome::Accepted { .. }))
    );
}

#[test]
fn truncated_prefix_stops_before_submit_without_inventing_an_error() {
    let experiment = Experiment::builtin().expect("built-in experiment");
    let truncated = experiment.scenario("truncated-prefix").expect("scenario");
    assert_eq!(truncated.run.traces.len(), 5);
    assert!(truncated.run.first_error.is_none());
    assert_eq!(truncated.run.final_state.phase, Phase::Staged);
    assert_eq!(experiment.truncated.stats.incomplete_tail_bytes, 190);
    assert_eq!(experiment.truncated.stats.complete_inputs, 5);
    assert_eq!(experiment.truncated.header.intended_input_count, 7);

    let divergence = truncated
        .comparison
        .first_divergence
        .as_ref()
        .expect("truncation divergence");
    assert_eq!(divergence.index, 5);
    assert_eq!(divergence.reason, "input_missing");
    assert_eq!(divergence.control_step_id, "evt-submit-0006");
    assert!(divergence.observed_step_id.is_none());
}

#[test]
fn reordered_replay_freezes_at_the_first_wrong_state() {
    let experiment = Experiment::builtin().expect("built-in experiment");
    let scenario = experiment.scenario("reordered").expect("scenario");
    let error = scenario.run.first_error.as_ref().expect("first error");
    assert_eq!(error.step_index, 2);
    assert_eq!(error.rule, "sequence_order");
    assert_eq!(error.step_id, "evt-stage-0004");
    assert_eq!(scenario.run.traces.len(), 3);
    assert!(scenario.run.stopped_after_first_error);
    assert_eq!(scenario.run.final_state.phase, Phase::Rejected);
    assert_eq!(scenario.run.final_state.cursor, 2);

    let trace = &scenario.run.traces[2];
    assert_eq!(trace.state_before.phase, Phase::Probed);
    assert_eq!(trace.state_before.cursor, 2);
    assert_eq!(trace.state_before.revision, 0);
    assert!(trace.state_before.first_error.is_none());
    assert_eq!(trace.state_after.phase, Phase::Rejected);
    assert_eq!(trace.state_after.cursor, 2);
    assert!(trace.state_after.first_error.is_some());
    assert!(matches!(trace.outcome, StepOutcome::Rejected { .. }));
}

#[test]
fn duplicate_submission_is_isolated_from_sequence_order() {
    let experiment = Experiment::builtin().expect("built-in experiment");
    let scenario = experiment
        .scenario("duplicate-submission")
        .expect("scenario");
    let error = scenario.run.first_error.as_ref().expect("first error");
    assert_eq!(error.step_index, 6);
    assert_eq!(error.rule, "duplicate_submission");
    assert_eq!(error.observed, "sub-commit-0006");
    assert_eq!(scenario.run.final_state.phase, Phase::Rejected);
    assert_eq!(scenario.run.traces.len(), 7);

    let trace = &scenario.run.traces[6];
    assert_eq!(trace.input.sequence, 6);
    assert_eq!(trace.state_before.phase, Phase::Committed);
    assert_eq!(trace.state_before.cursor, 6);
    assert_eq!(
        trace.state_before.submission_id.as_deref(),
        Some("sub-commit-0006")
    );
}

#[test]
fn evidence_is_a_linked_hash_chain() {
    let experiment = Experiment::builtin().expect("built-in experiment");
    let mut previous = media_meta_replay::evidence::GENESIS_EVIDENCE.to_owned();
    for trace in &experiment.control.traces {
        assert_eq!(trace.evidence.previous_evidence_sha256, previous);
        assert_eq!(trace.evidence.evidence_sha256.len(), 64);
        assert!(trace.evidence.input_sha256.len() == 64);
        assert!(trace.evidence.state_after_sha256.len() == 64);
        previous = trace.evidence.evidence_sha256.clone();
    }
    assert_eq!(
        previous,
        experiment
            .control
            .traces
            .last()
            .unwrap()
            .evidence
            .evidence_sha256
    );
    assert_eq!(
        experiment.control.traces[0].evidence.state_before_sha256,
        experiment.scenarios[1].run.traces[0]
            .evidence
            .state_before_sha256
    );
    assert_ne!(
        experiment.control.traces[2].evidence.input_sha256,
        experiment.scenarios[1].run.traces[2].evidence.input_sha256
    );
}

#[test]
fn earliest_error_points_to_a_state_not_a_score() {
    let experiment = Experiment::builtin().expect("built-in experiment");
    let first = experiment
        .first_error_across_scenarios()
        .expect("first error");
    assert_eq!(first.scenario, "reordered");
    assert_eq!(first.step_index, 2);
    assert_eq!(first.rule, "sequence_order");
    assert_eq!(first.state_before.phase, Phase::Probed);
    assert_eq!(first.state_after.phase, Phase::Rejected);
    assert_eq!(first.state_after.cursor, 2);
    assert_eq!(
        first
            .state_after
            .first_error
            .as_ref()
            .map(|error| error.rule.as_str()),
        Some("sequence_order")
    );
    assert_eq!(first.state_before.metadata_sha256.len(), 64);
    assert_eq!(first.evidence_sha256.len(), 64);
}

#[test]
fn recovered_record_must_match_its_open_media_identity() {
    let mut record = demo_record();
    record.media.sha256 = "0".repeat(64);
    let error = Experiment::from_record(record).expect_err("tampered record");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
}

#[test]
fn truncated_fixture_roundtrips_from_bytes_and_disk() {
    let record = demo_record();
    let bytes = truncated_fixture_bytes(&record, 5).expect("fixture bytes");
    let in_memory = load_truncated_bytes(&bytes).expect("in-memory fixture");
    assert_eq!(in_memory.inputs.len(), 5);
    assert!(!in_memory.stats.incomplete_tail_preview.is_empty());
    assert_eq!(in_memory.stats.total_bytes, bytes.len());
    assert_eq!(in_memory.stats.file_sha256, sha256_bytes(&bytes));

    let dir = temp_dir("truncated-roundtrip");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("truncated.jsonl");
    fs::write(&path, &bytes).unwrap();
    let from_disk = load_truncated_fixture(&path).expect("disk fixture");
    assert_eq!(from_disk, in_memory);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn committed_fixtures_match_generated_bytes() {
    let record = demo_record();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let recovered = fs::read(root.join("fixtures/recovered-record.json")).expect("recovered");
    let parsed: media_meta_replay::model::RecoveredRecord =
        serde_json::from_slice(&recovered).expect("recovered JSON");
    assert_eq!(parsed.schema, SCHEMA);
    assert_eq!(parsed, record);

    let truncated = fs::read(root.join("fixtures/truncated-record.jsonl")).expect("truncated");
    assert_eq!(
        truncated,
        truncated_fixture_bytes(&record, 5).expect("regenerated fixture")
    );
    let media = fs::read(root.join("fixtures/media/take-001.wav")).expect("media");
    assert_eq!(sha256_bytes(&media), record.media.sha256);

    let probe: serde_json::Value =
        serde_json::from_str(GSTREAMER_PROBE_JSON).expect("probe snapshot");
    assert_eq!(probe["facts"]["sha256"], record.media.sha256);
    assert_eq!(probe["facts"]["duration_ms"], 600);
    assert_eq!(probe["facts"]["discoverer_result"], "Ok");
    assert!(
        probe["gstreamer_version"]
            .as_str()
            .unwrap()
            .starts_with("GStreamer ")
    );
    let actual_fields: BTreeMap<String, String> =
        serde_json::from_value(probe["facts"]["fields"].clone()).expect("probe fields");
    assert_eq!(actual_fields, probe_fields());
}

#[test]
fn artifact_manifest_hashes_every_promised_file() {
    let experiment = Experiment::builtin().expect("built-in experiment");
    let dir = temp_dir("artifact-manifest");
    let bundle = experiment.write_artifacts(&dir).expect("write artifacts");

    let manifest_bytes = fs::read(dir.join("manifest.sha256.json")).expect("manifest");
    let manifest: media_meta_replay::experiment::ArtifactManifest =
        serde_json::from_slice(&manifest_bytes).expect("manifest JSON");
    assert_eq!(manifest.schema, SCHEMA);
    assert!(manifest.artifacts.len() >= 11);
    for (relative, expected) in &manifest.artifacts {
        let bytes = fs::read(dir.join(relative)).expect("manifest artifact");
        assert_eq!(sha256_bytes(&bytes), *expected, "{relative}");
    }

    let summary_bytes = fs::read(dir.join("summary.json")).expect("summary");
    let summary: media_meta_replay::experiment::ExperimentSummary =
        serde_json::from_slice(&summary_bytes).expect("summary JSON");
    assert_eq!(summary.record_id, bundle.summary.record_id);
    assert!(summary.control.all_accepted);
    assert_eq!(
        summary
            .first_error_across_scenarios
            .as_ref()
            .map(|error| error.step_index),
        Some(2)
    );

    let key: BTreeMap<String, serde_json::Value> =
        serde_json::from_slice(&fs::read(dir.join("key-evidence.json")).expect("key evidence"))
            .expect("key evidence JSON");
    assert_eq!(
        key["truncated_fixture_sha256"],
        summary.truncation.file_sha256
    );
    assert_eq!(key["control_trace_sha256"], summary.control.trace_sha256);
    assert_eq!(
        key["gstreamer_probe_sha256"],
        sha256_bytes(GSTREAMER_PROBE_JSON.as_bytes())
    );

    fs::remove_dir_all(dir).unwrap();
}

fn temp_dir(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "media-meta-replay-{label}-{}-{nonce}",
        std::process::id()
    ))
}
