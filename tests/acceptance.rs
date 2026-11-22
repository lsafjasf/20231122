use loudness_segment_calibrator::engine::{self, ProcessOptions};
use loudness_segment_calibrator::evidence;
use loudness_segment_calibrator::io::file_sha256;
use loudness_segment_calibrator::model::CalibrationConfig;
use std::path::PathBuf;
use tempfile::tempdir;

fn source() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/original/loudness_original.wav")
}

fn pause(index: usize) -> ProcessOptions {
    ProcessOptions {
        pause_after: Some(index),
        ..ProcessOptions::default()
    }
}

#[test]
fn pause_resume_and_continuous_outputs_are_identical() {
    let workspace = tempdir().unwrap();
    let source_hash = file_sha256(&source()).unwrap();

    let continuous_root = workspace.path().join("continuous");
    let (continuous_paths, continuous_state) =
        engine::init_run(&source(), &continuous_root, CalibrationConfig::default()).unwrap();
    let continuous = engine::process(
        &continuous_paths,
        continuous_state,
        ProcessOptions::default(),
    )
    .unwrap();
    assert!(continuous.complete);

    let paused_root = workspace.path().join("paused");
    let (paused_paths, state) =
        engine::init_run(&source(), &paused_root, CalibrationConfig::default()).unwrap();
    let first = engine::process(&paused_paths, state, pause(0)).unwrap();
    assert!(first.paused);
    assert_eq!(first.state.completed.len(), 1);
    let anchor = &first.state.completed[0];
    assert_eq!(anchor.anchor_id, "A");
    assert_eq!(anchor.anchor_ns, 5_000_000_000);
    assert_eq!(anchor.anchor_frame, 240_000);
    assert_eq!(anchor.original_anchor_file_byte, 480_044);
    assert_eq!(anchor.payload_end_byte, 480_000);
    assert!(anchor.target_error_db.abs() <= 0.75);
    assert!(paused_paths.state.exists());
    assert!(paused_paths.checkpoints.join("anchor-A.json").exists());

    let second = engine::resume(&paused_paths, pause(1)).unwrap();
    assert!(second.paused);
    assert_eq!(second.state.resume_count, 1);
    assert_eq!(second.state.completed.len(), 2);
    assert_eq!(second.state.completed[1].anchor_ns, 10_000_000_000);
    assert_eq!(second.state.completed[1].original_anchor_file_byte, 960_044);

    let final_state = engine::resume(&paused_paths, ProcessOptions::default()).unwrap();
    assert!(final_state.complete);
    assert_eq!(final_state.state.resume_count, 2);
    assert_eq!(final_state.state.completed[2].anchor_ns, 15_000_000_000);
    assert_eq!(
        final_state.state.completed[2].original_anchor_file_byte,
        1_440_044
    );

    let verification = evidence::verify_run(&paused_paths).unwrap();
    assert!(verification.ok, "{:?}", verification.checks);
    assert_eq!(file_sha256(&source()).unwrap(), source_hash);
    assert_eq!(
        continuous.state.final_output.as_ref().unwrap().sha256,
        final_state.state.final_output.as_ref().unwrap().sha256
    );

    let comparison_path = workspace.path().join("equivalence.json");
    let equivalence =
        evidence::compare_runs(&continuous_paths, &paused_paths, &comparison_path).unwrap();
    assert!(equivalence.ok);
    assert!(equivalence.outputs_identical);
    assert_eq!(equivalence.anchors.len(), 3);
}

#[test]
fn forced_degradation_keeps_anchor_evidence_and_finishes_at_half_rate() {
    let workspace = tempdir().unwrap();
    let root = workspace.path().join("degraded");
    let (paths, state) = engine::init_run(
        &source(),
        &root,
        CalibrationConfig {
            force_degrade: true,
            ..CalibrationConfig::default()
        },
    )
    .unwrap();
    assert_eq!(state.resource.mode, "degraded");
    let degradation = state.degradation.as_ref().unwrap();
    assert_eq!(degradation.output_sample_rate, 24_000);
    assert_eq!(degradation.anchor_probe_bytes, 512);
    let paused = engine::process(&paths, state, pause(0)).unwrap();
    assert!(paused.paused);
    assert_eq!(paused.state.completed[0].raw_probe_bytes, 512);
    assert_eq!(paused.state.completed[0].output_sample_rate, 24_000);
    let complete = engine::resume(&paths, ProcessOptions::default()).unwrap();
    assert!(complete.complete);
    let final_output = complete.state.final_output.as_ref().unwrap();
    assert_eq!(final_output.sample_rate, 24_000);
    assert_eq!(final_output.frame_count, 360_000);
    assert_eq!(final_output.duration_ns, 15_000_000_000);
    let verification = evidence::verify_run(&paths).unwrap();
    assert!(verification.ok, "{:?}", verification.checks);
}

#[test]
fn tampered_segment_blocks_resume_and_keeps_preflight_evidence() {
    let workspace = tempdir().unwrap();
    let root = workspace.path().join("tampered");
    let (paths, state) = engine::init_run(&source(), &root, CalibrationConfig::default()).unwrap();
    let paused = engine::process(&paths, state, pause(0)).unwrap();
    assert!(paused.paused);
    let segment = PathBuf::from(&paused.state.completed[0].output_wav);
    let mut bytes = std::fs::read(&segment).unwrap();
    bytes[44] ^= 0x01;
    std::fs::write(&segment, bytes).unwrap();

    let error = engine::resume(&paths, ProcessOptions::default()).unwrap_err();
    assert!(format!("{error:#}").contains("恢复前校验失败"));
    let preflight = paths.evidence.join("resume-preflight-1.json");
    assert!(preflight.exists());
    let text = std::fs::read_to_string(preflight).unwrap();
    assert!(text.contains("segment_output_hash_mismatch"));
    assert!(paths.state.exists());
}
