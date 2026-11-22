use std::fs;
use std::io::{Seek, SeekFrom, Write};
use std::os::unix::fs::PermissionsExt;

use mask_edge_review::evidence::{read_journal, sha256_file};
use mask_edge_review::model::{MediaInfo, RunStatus, SEGMENT_COUNT};
use mask_edge_review::project::{create_project, inspect_project, open_project, save_session};
use mask_edge_review::render::RenderOutcome;
use mask_edge_review::service::{acceptance_value, begin_run, compare_latest_runs, finish_run};
use tempfile::tempdir;

fn media() -> MediaInfo {
    MediaInfo {
        duration_ns: 12_008_000_000,
        width: 640,
        height: 360,
        fps_num: 30,
        fps_den: 1,
        container: "file/webm".into(),
        video_codec: "video/x-vp8".into(),
        probe_engine: "test".into(),
    }
}

fn setup() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = tempdir().unwrap();
    let source = temp.path().join("source.webm");
    let bytes: Vec<u8> = (0_u8..255).cycle().take(32 * 1024).collect();
    fs::write(&source, &bytes).unwrap();
    let project = temp.path().join("review-project");
    (temp, source, project)
}

#[test]
fn import_preserves_original_and_three_exact_ranges() {
    let (_temp, source, project_dir) = setup();
    let source_hash = sha256_file(&source).unwrap();
    let opened = create_project(&project_dir, &source, "demo".into(), media()).unwrap();
    assert!(opened.integrity.verified);
    assert_eq!(opened.session.segments.len(), SEGMENT_COUNT);

    let stored = opened
        .session
        .project_dir
        .join(&opened.session.original.stored_relative_path);
    assert_eq!(sha256_file(&stored).unwrap(), source_hash);
    assert_eq!(sha256_file(&source).unwrap(), source_hash);
    assert_eq!(
        fs::metadata(&stored).unwrap().permissions().mode() & 0o777,
        0o444
    );
    assert_eq!(
        fs::metadata(&source).unwrap().permissions().mode() & 0o200,
        0o200
    );

    let anchors: Vec<u64> = opened
        .session
        .segments
        .iter()
        .map(|segment| segment.anchor_ns)
        .collect();
    assert!(anchors[0] < anchors[1] && anchors[1] < anchors[2]);
    let first = opened.session.range_for(0).unwrap();
    assert_eq!(first.anchor_ns, 2_001_333_333);
    assert_eq!(first.in_offset_ns, -1_000_000_000);
    assert_eq!(first.out_offset_ns, 1_000_000_000);
    assert_eq!(first.start_ns, 1_001_333_333);
    assert_eq!(first.end_ns, 3_001_333_333);

    let report = acceptance_value(&opened.session);
    assert_eq!(report["segments"].as_array().unwrap().len(), SEGMENT_COUNT);
    assert_eq!(report["segments"][0]["in_offset_ns"], -1_000_000_000_i64);
    assert_eq!(
        report["segments"][0]["range"]["start_ns"],
        1_001_333_333_u64
    );
    assert_eq!(report["original"]["integrity"]["verified"], true);
}

#[test]
fn interrupted_run_is_recovered_and_journaled() {
    let (_temp, source, project_dir) = setup();
    let mut opened = create_project(&project_dir, &source, "demo".into(), media()).unwrap();
    let job = begin_run(&mut opened.session, 0).unwrap();
    assert_eq!(job.run_number, 1);
    assert_eq!(
        opened.session.segments[0].runs[0].status,
        RunStatus::Running
    );
    save_session(&opened.session).unwrap();
    drop(opened);

    let reopened = open_project(&project_dir).unwrap();
    let run = &reopened.session.segments[0].runs[0];
    assert_eq!(run.status, RunStatus::Interrupted);
    assert!(run.error.as_deref().unwrap().contains("退出"));
    let events = read_journal(&project_dir);
    assert!(events.iter().any(|event| event.kind == "run_started"));
    assert!(events.iter().any(|event| event.kind == "runs_interrupted"));
    assert!(events.iter().any(|event| event.kind == "session_opened"));
}

#[test]
fn inspect_is_read_only_and_detects_tampering() {
    let (_temp, source, project_dir) = setup();
    create_project(&project_dir, &source, "demo".into(), media()).unwrap();
    let journal_before = fs::read_to_string(project_dir.join("journal.jsonl")).unwrap();
    let inspected = inspect_project(&project_dir).unwrap();
    assert!(inspected.integrity.verified);
    let journal_after = fs::read_to_string(project_dir.join("journal.jsonl")).unwrap();
    assert_eq!(journal_before, journal_after);

    let stored = inspected
        .session
        .project_dir
        .join(&inspected.session.original.stored_relative_path);
    let mut permissions = fs::metadata(&stored).unwrap().permissions();
    permissions.set_mode(0o644);
    fs::set_permissions(&stored, permissions).unwrap();
    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&stored)
        .unwrap();
    file.seek(SeekFrom::End(0)).unwrap();
    file.write_all(b"tamper").unwrap();
    file.sync_all().unwrap();

    let tampered = inspect_project(&project_dir).unwrap();
    assert!(!tampered.integrity.verified);
    assert_eq!(
        tampered.integrity.expected_sha256,
        inspected.integrity.expected_sha256
    );
    assert_ne!(
        tampered.integrity.actual_sha256,
        Some(tampered.integrity.expected_sha256.clone())
    );
}

#[test]
fn offset_clamping_and_invalid_ranges_are_visible() {
    let (_temp, source, project_dir) = setup();
    let mut opened = create_project(&project_dir, &source, "demo".into(), media()).unwrap();
    let segment = &mut opened.session.segments[0];
    segment.anchor_ns = 500_000_000;
    segment.in_offset_ns = -1_000_000_000;
    segment.out_offset_ns = 1_000_000_000;
    let range = segment.range(opened.session.media.duration_ns).unwrap();
    assert_eq!(range.start_ns, 0);
    assert!(range.start_clamped);
    assert_eq!(range.requested_start_ns, -500_000_000);
    assert_eq!(range.in_offset_ns, -1_000_000_000);

    segment.in_offset_ns = 2_000_000_000;
    segment.out_offset_ns = -2_000_000_000;
    assert!(segment.range(opened.session.media.duration_ns).is_err());
}

#[test]
fn latest_successful_runs_can_be_compared() {
    let (_temp, source, project_dir) = setup();
    let mut opened = create_project(&project_dir, &source, "demo".into(), media()).unwrap();
    for hash_byte in *b"ab" {
        let job = begin_run(&mut opened.session, 0).unwrap();
        let outcome = RenderOutcome {
            output_path: job.output_path.clone(),
            output_sha256: String::from_utf8(vec![hash_byte; 64]).unwrap(),
            measured_start_ns: 1_001_333_333,
            measured_end_ns: 2_968_000_000,
            evidence: Vec::new(),
        };
        finish_run(&mut opened.session, 0, &job.run_id, Ok(outcome)).unwrap();
    }
    let comparison = compare_latest_runs(&opened.session, 0).unwrap();
    assert!(comparison.contains("对比片段 1"));
    assert!(comparison.contains("第 1 次 → 第 2 次"));
    assert!(comparison.contains("证据帧哈希一致"));
}
