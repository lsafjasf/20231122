use std::fs;

use encode_panel::evidence::EvidenceBundle;
use encode_panel::model::{
    EncodeSample, Metric, MetricLevel, ProcessingParams, SampleFlag, StatisticsProcessor,
};
use encode_panel::scenarios::DEFAULT_HUGE_SAMPLES;
use encode_panel::{ScenarioKind, run_scenario, waveform, write_evidence};

#[test]
fn empty_input_is_explicit_and_waveform_is_well_formed() {
    let run = run_scenario(ScenarioKind::Empty, 1);
    assert_eq!(run.generated_samples, 0);
    assert!(!run.snapshot.has_data);
    assert!(run.snapshot.waveform.is_empty());
    assert_eq!(run.snapshot.boundary_events_total, 0);

    let svg = waveform::render_svg(&[], &run.parameters, "empty", 1200.0, 560.0);
    assert!(svg.contains("NO DATA"));
    assert!(svg.starts_with("<svg"));
    assert!(svg.ends_with("</svg>\n"));
}

#[test]
fn boundary_event_keeps_the_sample_before_and_after() {
    let run = run_scenario(ScenarioKind::Boundary, 1);
    let fps_event = run
        .boundaries
        .iter()
        .find(|event| event.metric == Metric::Fps && event.to_level == MetricLevel::Warning)
        .expect("fps warning crossing");
    let before = fps_event.before.as_ref().expect("before sample");
    let changes = fps_event.changes.as_ref().expect("recorded change");

    assert!(before.fps > run.parameters.fps_warn);
    assert!(fps_event.after.fps < run.parameters.fps_warn);
    assert!(changes.fps_delta < 0.0);
    assert_eq!(changes.frame_delta, 1);
    assert_eq!(changes.timestamp_delta_ns, 33_333_333);
    assert_eq!(fps_event.after.track, 7);
    assert_eq!(fps_event.after.frame, 102);
}

#[test]
fn all_three_threshold_families_produce_boundaries() {
    let run = run_scenario(ScenarioKind::Boundary, 1);
    for metric in [Metric::Fps, Metric::Bitrate, Metric::Dropped] {
        assert!(
            run.boundaries.iter().any(|event| event.metric == metric),
            "missing boundary for {metric:?}"
        );
    }
    assert!(
        run.boundaries
            .iter()
            .filter(|event| event.before.is_some())
            .all(|event| event.changes.is_some())
    );
}

#[test]
fn non_finite_and_u64_max_inputs_are_sanitized() {
    let mut processor = StatisticsProcessor::new(ProcessingParams::default());
    let point = processor
        .ingest(EncodeSample::new(
            1,
            9,
            1,
            f64::NAN,
            u64::MAX,
            u64::MAX,
            u64::MAX,
        ))
        .expect("sample accepted");

    assert_eq!(point.fps, 0.0);
    assert!(point.flags.contains(&SampleFlag::NonFiniteFps));
    assert!(point.flags.contains(&SampleFlag::ClampedBitrate));
    assert!(point.flags.contains(&SampleFlag::ClampedDropped));
    assert_eq!(point.bitrate_level, MetricLevel::Critical);
    assert_eq!(point.dropped_level, MetricLevel::Critical);
    assert_eq!(point.waveform.fps_normalized, 0.0);
    assert_eq!(point.waveform.bitrate_normalized, 1.0);
    assert_eq!(point.waveform.dropped_normalized, 1.0);
}

#[test]
fn regressions_are_flagged_without_underflow() {
    let mut processor = StatisticsProcessor::new(ProcessingParams::default());
    processor
        .ingest(EncodeSample::new(1_000, 1, 10, 30.0, 4_000_000, 9, 10_000))
        .unwrap();
    let point = processor
        .ingest(EncodeSample::new(500, 1, 9, -1.0, 3_000_000, 0, 1))
        .unwrap();

    for flag in [
        SampleFlag::TimestampRegression,
        SampleFlag::FrameRegression,
        SampleFlag::DropCounterReset,
        SampleFlag::EncodedBytesRegression,
        SampleFlag::ClampedFps,
    ] {
        assert!(point.flags.contains(&flag), "missing {flag:?}");
    }
    assert_eq!(point.dropped_delta, 0);
    assert_eq!(point.fps, 0.0);
}

#[test]
fn huge_input_stays_bounded_and_retains_key_evidence() {
    let run = run_scenario(ScenarioKind::Huge, DEFAULT_HUGE_SAMPLES);
    assert_eq!(run.generated_samples, DEFAULT_HUGE_SAMPLES);
    assert_eq!(run.snapshot.samples, DEFAULT_HUGE_SAMPLES);
    assert!(run.snapshot.waveform.len() <= run.parameters.window_capacity);
    assert!(run.boundaries.len() <= run.parameters.max_evidence_events);
    assert!(run.snapshot.boundary_events_total > run.boundaries.len() as u64);
    assert!(run.snapshot.boundary_events_dropped > 0);
    assert!(run.snapshot.total_dropped_delta > 0);
    assert!(run.processed_points.len() <= run.parameters.window_capacity);
    assert_eq!(run.rejected_samples, 0);
}

#[test]
fn track_limit_is_enforced_for_high_cardinality_input() {
    let params = ProcessingParams {
        max_tracks: 2,
        ..ProcessingParams::default()
    };
    let mut processor = StatisticsProcessor::new(params);
    processor
        .ingest(EncodeSample::new(1, 0, 1, 30.0, 1, 0, 1))
        .unwrap();
    processor
        .ingest(EncodeSample::new(2, 1, 1, 30.0, 1, 0, 1))
        .unwrap();
    assert!(
        processor
            .ingest(EncodeSample::new(3, 2, 1, 30.0, 1, 0, 1))
            .is_err()
    );
    assert_eq!(processor.snapshot().rejected_samples, 1);
}

#[test]
fn waveform_correlates_points_with_time_track_frame_and_params() {
    let run = run_scenario(ScenarioKind::Boundary, 1);
    let svg = waveform::render_svg(
        &run.snapshot.waveform,
        &run.parameters,
        "boundary evidence",
        1200.0,
        560.0,
    );

    assert!(svg.contains("id=\"lane-FPS\""));
    assert!(svg.contains("id=\"lane-BITRATE\""));
    assert!(svg.contains("id=\"lane-DROPPED\""));
    assert!(svg.contains("window_capacity=96"));
    assert!(svg.contains("track=7"));
    assert!(svg.contains("frame=102"));
    assert!(svg.contains("t="));
    assert!(!svg.contains("NaN"));
}

#[test]
fn evidence_files_and_manifest_are_written() {
    let temp = tempfile::tempdir().unwrap();
    let run = run_scenario(ScenarioKind::Boundary, 1);
    let bundle = EvidenceBundle::from_scenario(&run);
    let serialized = serde_json::to_string(&bundle).unwrap();
    assert!(!serialized.contains("NaN"));

    let paths = write_evidence(temp.path(), &bundle).unwrap();
    for path in [
        &paths.evidence,
        &paths.boundaries,
        &paths.trace,
        &paths.waveform,
        &paths.manifest,
    ] {
        assert!(path.is_file(), "missing {}", path.display());
        assert!(fs::metadata(path).unwrap().len() > 0);
    }
    let manifest: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(paths.manifest).unwrap()).unwrap();
    assert_eq!(manifest["schema_version"], "encode-panel.evidence/v1");
    assert_eq!(manifest["files"].as_array().unwrap().len(), 4);
    for file in manifest["files"].as_array().unwrap() {
        assert_eq!(file["sha256"].as_str().unwrap().len(), 64);
    }
}

#[test]
fn invalid_extreme_parameters_are_clamped_and_reported() {
    let (params, changes) = ProcessingParams {
        window_capacity: 0,
        ema_alpha: f64::NAN,
        fps_warn: -1.0,
        fps_critical: 1e300,
        drop_critical: 0,
        max_evidence_events: 0,
        ..ProcessingParams::default()
    }
    .sanitized();

    assert!(params.window_capacity >= 1);
    assert!(params.ema_alpha.is_finite());
    assert!(params.fps_warn >= params.fps_critical);
    assert!(params.drop_critical >= params.drop_warn);
    assert!(params.max_evidence_events >= 1);
    assert_ne!(changes, vec!["none"]);
}
