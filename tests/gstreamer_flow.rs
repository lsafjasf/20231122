use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use color_sample_flow::checkpoint;
use color_sample_flow::compare;
use color_sample_flow::evidence::EvidenceLog;
use color_sample_flow::fixture;
use color_sample_flow::model::{sha256_file, ExtremeParams};
use color_sample_flow::plan::{build_demo_plan, validate_demo_plan};
use color_sample_flow::runner::{self, RunConfig, RunOutcome};

#[test]
fn pause_resume_matches_fresh_gstreamer_pass() {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let source_path = workspace.join(fixture::SOURCE_PATH);
    let manifest_path = workspace.join(fixture::MANIFEST_PATH);
    let source = fixture::ensure_original(&source_path, &manifest_path).unwrap();
    let before_hash = sha256_file(&source_path).unwrap();
    let plan = build_demo_plan(source, ExtremeParams::demo());
    validate_demo_plan(&plan).unwrap();

    let root = std::env::temp_dir().join(format!(
        "color-sample-flow-e2e-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mut evidence = EvidenceLog::create(root.join("evidence.jsonl")).unwrap();
    let paused = runner::run(
        &plan,
        &RunConfig {
            out_dir: root.clone(),
            resume: false,
            pause_after: Some(plan.params.pause_after),
            pause_flag: None,
        },
        &mut evidence,
        |_| {},
    )
    .unwrap();
    let paused_cp = match paused {
        RunOutcome::Paused(cp) => cp,
        _ => panic!("first pass should pause"),
    };
    assert_eq!(paused_cp.next_index, 8);

    let resumed = runner::run(
        &plan,
        &RunConfig {
            out_dir: root.clone(),
            resume: true,
            pause_after: None,
            pause_flag: None,
        },
        &mut evidence,
        |_| {},
    )
    .unwrap();
    let resumed_cp = runner::require_complete(resumed, plan.samples.len()).unwrap();

    let fresh_dir = root.join("fresh");
    let fresh = runner::run(
        &plan,
        &RunConfig {
            out_dir: fresh_dir,
            resume: false,
            pause_after: None,
            pause_flag: None,
        },
        &mut evidence,
        |_| {},
    )
    .unwrap();
    let fresh_cp = runner::require_complete(fresh, plan.samples.len()).unwrap();

    let report = compare::compare_results(&resumed_cp.results, &fresh_cp.results);
    assert!(report.pause_resume_equivalent, "{report:#?}");
    assert_eq!(resumed_cp.scale_percent, 25);
    assert_eq!(resumed_cp.results[0].raw_offset, 0);
    assert_eq!(resumed_cp.results[0].landed_timestamp_ms, 0);
    assert_eq!(resumed_cp.results[24].raw_offset, 115_680);
    assert_eq!(resumed_cp.results[24].landed_timestamp_ms, 5_900);
    assert_eq!(sha256_file(&source_path).unwrap(), before_hash);

    let loaded = checkpoint::load(&root).unwrap();
    assert!(loaded.done);
    let mut tampered = plan.clone();
    tampered.source.sha256 = "0".repeat(64);
    assert!(loaded.validate(&tampered, &plan_sha(&plan)).is_err());
    std::fs::remove_dir_all(root).ok();
}

fn plan_sha(plan: &color_sample_flow::model::SamplingPlan) -> String {
    color_sample_flow::plan::plan_sha256(plan).unwrap()
}
