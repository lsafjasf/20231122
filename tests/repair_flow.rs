use hdr_repair::checkpoint::{Checkpoint, JobState, PauseReason};
use hdr_repair::fixture::{generate_demo_fixture, hash_file};
use hdr_repair::metadata::{FLAG_RESOURCE_DEGRADED, FrameMetadata};
use hdr_repair::model::{AnalysisQuality, METADATA_RECORD_BYTES, RepairConfig};
use hdr_repair::session::{RunOptions, RunOutcome, Session};
use std::fs::File;
use std::os::unix::fs::{FileExt, PermissionsExt};

#[test]
fn pause_at_2400ms_resume_and_accept_exact_offsets() {
    let fixture = tempfile::tempdir().unwrap();
    let profile = generate_demo_fixture(fixture.path()).unwrap();
    let jobs = tempfile::tempdir().unwrap();
    let mut config = RepairConfig::demo(profile.clone());
    config.pause_before_frame = Some(72);
    let mut session = Session::prepare(fixture.path(), jobs.path().join("job"), config).unwrap();

    let first = session.run(&RunOptions::default()).unwrap();
    let paused = match first {
        RunOutcome::Paused { checkpoint } => checkpoint,
        other => panic!("expected pause, got {other:?}"),
    };
    assert_eq!(paused.next_frame, 72);
    assert_eq!(paused.next_pts_ns, 2_400_000_000);
    assert_eq!(paused.next_video_offset, 0x144000);
    assert_eq!(paused.next_metadata_offset, 0x1200);
    assert_eq!(paused.pause_reason, Some(PauseReason::AcceptancePoint));
    assert_eq!(paused.processed_frames, 72);
    let persisted = Checkpoint::load(&session.paths.checkpoint).unwrap();
    assert_eq!(persisted.output_prefix_sha256, paused.output_prefix_sha256);
    assert_eq!(
        File::open(&session.paths.video)
            .unwrap()
            .metadata()
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o444
    );

    let second = session.run(&RunOptions::default()).unwrap();
    let (report, completed) = match second {
        RunOutcome::Completed { report, checkpoint } => (report, checkpoint),
        other => panic!("expected completion, got {other:?}"),
    };
    assert_eq!(completed.state, JobState::Complete);
    assert_eq!(report.source_video_sha256, profile.source_video_sha256);
    assert_eq!(report.output_video_sha256, profile.source_video_sha256);
    assert!(report.checks.iter().all(|check| check.pass));
    let comparison = report.resume_comparison.as_ref().unwrap();
    assert!(comparison.prefix_verified);
    assert_eq!(comparison.paused_frame, 72);
    assert_eq!(comparison.paused_pts_ns, 2_400_000_000);
    assert_eq!(comparison.paused_video_offset, 0x144000);
    assert_eq!(comparison.paused_metadata_offset, 0x1200);

    let acceptance = report
        .checks
        .iter()
        .find(|check| check.frame == 72)
        .expect("acceptance frame");
    assert_eq!(acceptance.pts_ns, 2_400_000_000);
    assert_eq!(acceptance.video_offset, 0x144000);
    assert_eq!(acceptance.metadata_offset, 0x1200);
    assert_eq!(acceptance.before.max_cll, u16::MAX);
    assert!(acceptance.after.is_repaired());
    assert!(acceptance.repeated_object_present);

    let mut before_bytes = [0u8; METADATA_RECORD_BYTES];
    let mut after_bytes = [0u8; METADATA_RECORD_BYTES];
    File::open(&session.paths.metadata)
        .unwrap()
        .read_exact_at(&mut before_bytes, 0x1200)
        .unwrap();
    File::open(&session.paths.repaired_metadata)
        .unwrap()
        .read_exact_at(&mut after_bytes, 0x1200)
        .unwrap();
    let before = FrameMetadata::decode(&before_bytes).unwrap();
    let after = FrameMetadata::decode(&after_bytes).unwrap();
    assert_eq!(before.max_cll, 65_535);
    assert!(after.max_cll >= 9_900);
    assert_eq!(before.pts_ns, after.pts_ns);
    assert_eq!(before.source_offset, after.source_offset);
    assert_eq!(
        hash_file(&session.paths.video).unwrap(),
        profile.source_video_sha256
    );
}

#[test]
fn resumed_output_matches_uninterrupted_run() {
    let fixture = tempfile::tempdir().unwrap();
    let profile = generate_demo_fixture(fixture.path()).unwrap();
    let jobs = tempfile::tempdir().unwrap();

    let baseline_config = RepairConfig::demo(profile.clone());
    let mut baseline = Session::prepare(
        fixture.path(),
        jobs.path().join("baseline"),
        baseline_config,
    )
    .unwrap();
    let (baseline_report, _) = match baseline.run(&RunOptions::default()).unwrap() {
        RunOutcome::Completed { report, checkpoint } => (report, checkpoint),
        other => panic!("baseline did not complete: {other:?}"),
    };

    let mut paused_config = RepairConfig::demo(profile);
    paused_config.pause_before_frame = Some(72);
    let mut paused =
        Session::prepare(fixture.path(), jobs.path().join("paused"), paused_config).unwrap();
    assert!(matches!(
        paused.run(&RunOptions::default()).unwrap(),
        RunOutcome::Paused { .. }
    ));
    let (resumed_report, _) = match paused.run(&RunOptions::default()).unwrap() {
        RunOutcome::Completed { report, checkpoint } => (report, checkpoint),
        other => panic!("resumed run did not complete: {other:?}"),
    };
    assert_eq!(
        baseline_report.output_metadata_sha256,
        resumed_report.output_metadata_sha256
    );
    assert_eq!(
        baseline_report.output_video_sha256,
        resumed_report.output_video_sha256
    );
}

#[test]
fn resource_shortage_pauses_at_safe_offset_and_can_resume_degraded() {
    let fixture = tempfile::tempdir().unwrap();
    let profile = generate_demo_fixture(fixture.path()).unwrap();
    let jobs = tempfile::tempdir().unwrap();
    let mut config = RepairConfig::demo(profile.clone());
    config.memory_budget_bytes = 40 * 1024;
    let mut blocked =
        Session::prepare(fixture.path(), jobs.path().join("blocked"), config).unwrap();
    let paused = match blocked.run(&RunOptions::default()).unwrap() {
        RunOutcome::ResourcePaused { checkpoint, .. } => checkpoint,
        other => panic!("expected resource pause: {other:?}"),
    };
    assert_eq!(paused.next_frame, 0);
    assert_eq!(paused.next_pts_ns, 0);
    assert_eq!(paused.next_video_offset, 0);
    assert_eq!(paused.next_metadata_offset, 0);
    assert_eq!(paused.pause_reason, Some(PauseReason::ResourceBudget));

    let resumed = blocked
        .run(&RunOptions {
            memory_budget_override: Some(1024 * 1024),
            ..RunOptions::default()
        })
        .unwrap();
    assert!(matches!(resumed, RunOutcome::Completed { .. }));

    let mut low_config = RepairConfig::demo(profile);
    low_config.memory_budget_bytes = 80 * 1024;
    let mut low = Session::prepare(fixture.path(), jobs.path().join("low"), low_config).unwrap();
    let (report, _) = match low.run(&RunOptions::default()).unwrap() {
        RunOutcome::Completed { report, checkpoint } => (report, checkpoint),
        other => panic!("low-memory run did not complete: {other:?}"),
    };
    assert_eq!(
        report.resource_plan.as_ref().unwrap().quality,
        AnalysisQuality::Sampled { pixel_step: 2 }
    );
    let mut after_bytes = [0u8; METADATA_RECORD_BYTES];
    File::open(&low.paths.repaired_metadata)
        .unwrap()
        .read_exact_at(&mut after_bytes, 0x1200)
        .unwrap();
    let after = FrameMetadata::decode(&after_bytes).unwrap();
    assert_ne!(after.flags & FLAG_RESOURCE_DEGRADED, 0);
    assert!(after.is_repaired());
}
