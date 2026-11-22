#![cfg(feature = "gst")]

use std::path::PathBuf;

use tempo_pitch_debug::audit::verify_artifact;
use tempo_pitch_debug::engine::{run_with_renderer, RunRequest};
use tempo_pitch_debug::gst_engine::GStreamerRenderer;
use tempo_pitch_debug::model::{AnomalyKind, RunStatus};

#[test]
fn gstreamer_renders_missing_segment_fixture_and_verifies() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let scenario = root.join("fixtures/missing_segment/scenario.json");
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("artifact");
    let request = RunRequest::from_scenario(&scenario, Some(output.clone())).unwrap();
    let mut renderer = match GStreamerRenderer::new() {
        Ok(renderer) => renderer,
        Err(error) => {
            eprintln!("skipping GStreamer test: {error}");
            return;
        }
    };
    let record = run_with_renderer(request, &mut renderer).unwrap();
    assert_eq!(record.status, RunStatus::CompleteWithAnomalies);
    assert_eq!(record.anomalies.len(), 1);
    assert_eq!(record.anomalies[0].kind, AnomalyKind::MissingPitchSegment);
    assert_eq!(record.anomalies[0].source_span.start_us, 1_250_000);
    assert_eq!(record.anomalies[0].source_span.end_us, 1_500_000);
    assert!(record.output.frames > 0);
    assert!(!record.timeline.is_empty());
    assert!(verify_artifact(&output).unwrap().passed());
}
