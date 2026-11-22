use audit_core::model::StepState;
use audit_core::scenario;

#[path = "../src/headless.rs"]
mod headless;

#[test]
fn full_cycle_assembles_and_serializes_with_evidence() {
    let config = scenario::default_config();
    let artifacts = headless::assemble(scenario::fixture_samples(), config);

    let first = artifacts.report.first_error.as_ref().expect("first error recorded");
    assert_eq!(first.sample_id, "s-03");
    assert_eq!(first.state, StepState::Truncated);

    assert!(artifacts.replay.matches);
    assert_eq!(artifacts.truncation.pending_inputs.len(), 3);

    let json = serde_json::to_string_pretty(&artifacts).expect("artifacts serialize");
    assert!(json.contains("clip_headroom"));
    assert!(json.contains("edge_guard"));
    assert!(json.contains("source_sample_id"));
    assert!(json.contains("无法判断") || json.contains("Indeterminate"));
}
