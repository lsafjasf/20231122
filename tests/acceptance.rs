use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use timecode_drift_replay::engine::{compare_runs, run, RunOptions, RunOutcome};
use timecode_drift_replay::evidence;
use timecode_drift_replay::model::Config;
use timecode_drift_replay::source::frame_crc;

fn scratch(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "timecode-drift-{label}-{}-{nonce}",
        std::process::id()
    ))
}

fn offline_run(
    config: &Config,
    dir: &Path,
    resume: bool,
    pause_at_frame: Option<u64>,
) -> RunOutcome {
    run(
        config,
        dir,
        RunOptions {
            resume,
            pause_at_frame,
            gst_probe: false,
        },
    )
    .unwrap()
}

#[test]
fn pause_resume_matches_continuous_and_preserves_boundary_evidence() {
    let root = scratch("main");
    let continuous = root.join("continuous");
    let pausable = root.join("pausable");
    let config = Config::default();

    assert_eq!(
        offline_run(&config, &continuous, false, None),
        RunOutcome::Completed
    );
    assert_eq!(
        offline_run(&config, &pausable, false, Some(config.gap_start_frame())),
        RunOutcome::Paused
    );

    let checkpoint = evidence::load_checkpoint(&pausable).unwrap();
    assert_eq!(checkpoint.next_frame_idx, config.gap_start_frame());
    assert_eq!(checkpoint.processed + checkpoint.dropped, 120);
    assert_eq!(checkpoint.params_hash, config.params_hash());
    assert_eq!(checkpoint.params_hash, 8_174_572_386_738_060_988);

    assert_eq!(
        offline_run(&config, &pausable, true, None),
        RunOutcome::Completed
    );
    let resume: serde_json::Value =
        serde_json::from_slice(&std::fs::read(pausable.join("resume_compare.json")).unwrap())
            .unwrap();
    assert_eq!(resume["action"], "continue");
    assert_eq!(resume["params_hash_match"], true);
    assert_eq!(resume["frame_records_match"], true);

    let report = compare_runs(
        &continuous,
        &pausable,
        &root.join("compare").join("compare_report.json"),
    )
    .unwrap();
    assert!(report.identical_sequence);
    assert_eq!(report.frames_a, 255);
    assert_eq!(report.frames_a, report.frames_b);
    assert_eq!(report.crc_chain_a, report.crc_chain_b);

    let frames = evidence::load_frames(&pausable).unwrap();
    assert_eq!(frames.len(), 255);
    assert!(frames.iter().all(|frame| !(121..=165).contains(&frame.idx)));
    let final_state = evidence::load_final_state(&pausable).unwrap();
    assert_eq!(final_state.processed + final_state.dropped, 255);
    assert_eq!(final_state.drift_events, 1);
    let events = std::fs::read_to_string(pausable.join("events.jsonl")).unwrap();
    assert!(events.contains("\"kind\":\"missing-segment\""));

    let delta: serde_json::Value =
        serde_json::from_slice(&std::fs::read(pausable.join("boundary_delta.json")).unwrap())
            .unwrap();
    assert_eq!(delta["before"]["frame_idx"], 119);
    assert_eq!(delta["after"]["frame_idx"], 166);
    assert_eq!(delta["crc_changed"], true);
    assert!(delta["missing_span_ns"].as_u64().unwrap() > 1_400_000_000);
    assert_eq!(
        delta["before"]["crc"].as_u64().unwrap() as u32,
        frame_crc(119)
    );
    assert_eq!(
        delta["after"]["crc"].as_u64().unwrap() as u32,
        frame_crc(166)
    );
    let before_ppm = std::fs::read(pausable.join("boundary_before.ppm")).unwrap();
    let after_ppm = std::fs::read(pausable.join("boundary_after.ppm")).unwrap();
    assert_ne!(before_ppm, after_ppm);
    assert!(pausable.join("manifest.json").exists());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn degradation_enters_and_recovers_within_budget() {
    let root = scratch("degrade");
    let config = Config::default();
    offline_run(&config, &root, false, None);
    let events: Vec<serde_json::Value> = std::fs::read_to_string(root.join("degradation.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(events.iter().any(|event| event["kind"] == "enter-degraded"));
    assert!(events.iter().any(|event| event["kind"] == "exit-degraded"));
    let state = evidence::load_final_state(&root).unwrap();
    assert!(!state.degraded);
    assert!(state.pool_bytes <= config.max_pool_bytes);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn resume_rejects_changed_checkpoint_parameters() {
    let root = scratch("tamper");
    let config = Config::default();
    offline_run(&config, &root, false, Some(120));
    let mut checkpoint: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("checkpoint.json")).unwrap()).unwrap();
    checkpoint["params_hash"] = serde_json::json!(0);
    std::fs::write(
        root.join("checkpoint.json"),
        serde_json::to_vec_pretty(&checkpoint).unwrap(),
    )
    .unwrap();
    let error = run(
        &config,
        &root,
        RunOptions {
            resume: true,
            pause_at_frame: None,
            gst_probe: false,
        },
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("resume rejected"));
    let compare: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("resume_compare.json")).unwrap()).unwrap();
    assert_eq!(compare["action"], "reject");
    assert!(compare["rejection"]
        .as_str()
        .unwrap()
        .contains("parameter hash"));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn gstreamer_pipeline_round_trips_boundary_frames() {
    let root = scratch("gst");
    let config = Config::default();
    let probe = timecode_drift_replay::gst_bridge::probe(&config, &root).unwrap();
    assert_eq!(probe.status, "ok");
    assert!(probe.crc_match);
    assert_eq!(probe.input_frames, 3);
    assert_eq!(probe.output_frames, 3);
    assert!(probe
        .elements
        .iter()
        .any(|element| element == "videoconvert"));
    let _ = std::fs::remove_dir_all(root);
}
