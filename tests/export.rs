use queue_priority_corrector::export::{export_bundle, verify_bundle};
use queue_priority_corrector::model::{APP_VERSION, RunRecord, RunStatus, SegmentStatus};
use queue_priority_corrector::util::sha256_file;
use std::fs;

#[test]
fn export_is_self_contained_ordered_and_verifiable() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = queue_priority_corrector::demo::seed_session(dir.path());
    let output = dir.path().join("runs/seg-01/run-0001.webm");
    let preview = dir.path().join("runs/seg-01/run-0001.preview.png");
    fs::create_dir_all(output.parent().unwrap()).unwrap();
    fs::write(&output, b"reviewable-media").unwrap();
    fs::write(&preview, b"\x89PNG\r\n\x1a\npreview").unwrap();
    let output_sha = sha256_file(&output).unwrap();
    session.runs.push(RunRecord {
        run_id: "run-0001".to_owned(),
        segment_id: "seg-01".to_owned(),
        attempt: 1,
        in_ms: 0,
        out_ms: 2_400,
        device_offset_ms: 1_500,
        effective_priority: 40,
        status: RunStatus::Succeeded,
        started_at: "2026-11-22T00:00:00.000Z".to_owned(),
        finished_at: "2026-11-22T00:00:02.000Z".to_owned(),
        duration_ms: 2_000,
        input_path: session.segment("seg-01").unwrap().source_path.clone(),
        input_sha256: Some("input-hash".to_owned()),
        output_path: Some(output.to_string_lossy().into_owned()),
        output_sha256: Some(output_sha.clone()),
        output_bytes: Some(15),
        preview_path: Some(preview.to_string_lossy().into_owned()),
        pipeline: "filesrc location=input ! decodebin ! webmmux ! filesink".to_owned(),
        gst_version: "1.28.2".to_owned(),
        tool_version: APP_VERSION.to_owned(),
        error: None,
    });
    session.segment_mut("seg-01").unwrap().status = SegmentStatus::Ready;
    session.segment_mut("seg-01").unwrap().latest_run_id = Some("run-0001".to_owned());
    session.next_run_seq = 2;

    let bundle_dir = dir.path().join("bundle");
    let bundle = export_bundle(&session, &bundle_dir).unwrap();
    assert_eq!(bundle.file_count, 8);
    assert_eq!(bundle.media_count, 2);
    assert!(bundle_dir.join("media/seg-01/run-0001.webm").is_file());
    assert!(bundle_dir.join("previews/seg-01/run-0001.png").is_file());

    let verification = verify_bundle(&bundle_dir).unwrap();
    assert!(verification.is_valid(), "{:?}", verification.errors);
    assert!(verification.checked >= 7);

    let segments_csv = fs::read_to_string(bundle_dir.join("segments.csv")).unwrap();
    let rows: Vec<&str> = segments_csv.lines().collect();
    assert!(rows[0].starts_with("order,segment_id,label,device,source_path"));
    assert!(rows[1].starts_with("1,seg-01,"));
    assert!(rows[6].starts_with("6,seg-06,"));

    let manifest: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(bundle_dir.join("manifest.json")).unwrap())
            .unwrap();
    assert_eq!(manifest["segment_order"][0], "seg-01");
    assert_eq!(manifest["segment_order"][5], "seg-06");
    assert_eq!(manifest["queue"][0]["segment_id"], "seg-01");
    assert_eq!(manifest["files"][0]["path"], "session.json");

    let review = fs::read_to_string(bundle_dir.join("REVIEW.md")).unwrap();
    assert!(review.contains("corrected_start_ms = raw_capture_start_ms - offset_ms + in_ms"));
    assert!(review.contains("sha256sum -c checksums.sha256"));

    let checksums = fs::read_to_string(bundle_dir.join("checksums.sha256")).unwrap();
    assert!(checksums.contains(&format!("{output_sha}  media/seg-01/run-0001.webm")));
    let media_index = fs::read_to_string(bundle_dir.join("media_index.csv")).unwrap();
    assert!(media_index.contains(&sha256_file(&preview).unwrap()));

    fs::write(bundle_dir.join("segments.csv"), "tampered").unwrap();
    let tampered = verify_bundle(&bundle_dir).unwrap();
    assert!(!tampered.is_valid());
    assert!(
        tampered
            .errors
            .iter()
            .any(|error| error.contains("segments.csv"))
    );
}

#[test]
fn export_refuses_missing_success_artifact() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = queue_priority_corrector::demo::seed_session(dir.path());
    session.runs.push(RunRecord {
        run_id: "run-0001".to_owned(),
        segment_id: "seg-01".to_owned(),
        attempt: 1,
        in_ms: 0,
        out_ms: 2_400,
        device_offset_ms: 1_500,
        effective_priority: 40,
        status: RunStatus::Succeeded,
        started_at: "2026-11-22T00:00:00.000Z".to_owned(),
        finished_at: "2026-11-22T00:00:01.000Z".to_owned(),
        duration_ms: 1_000,
        input_path: "/missing/input.webm".to_owned(),
        input_sha256: None,
        output_path: Some("/missing/output.webm".to_owned()),
        output_sha256: Some("recorded-hash".to_owned()),
        output_bytes: Some(10),
        preview_path: None,
        pipeline: "test".to_owned(),
        gst_version: "test".to_owned(),
        tool_version: APP_VERSION.to_owned(),
        error: None,
    });
    session.next_run_seq = 2;
    let error = export_bundle(&session, &dir.path().join("bundle")).unwrap_err();
    assert!(error.to_string().contains("无法生成自包含复核包"));
}
