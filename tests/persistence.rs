use clockbridge::compare::{compare_attempts, store_comparison};
use clockbridge::export::{
    ATTEMPTS_HEADER, COMPARISONS_HEADER, MANIFEST_FILE, SEGMENTS_HEADER, export_review_bundle,
    load_review_bundle, verify_review_bundle,
};
use clockbridge::model::{
    AttemptRecord, AttemptStatus, EvidenceEvent, SCHEMA_VERSION, Workspace, now_rfc3339,
};
use clockbridge::runner::{RunResult, sha256_file, write_run_evidence};
use clockbridge::store::{WorkspaceStore, atomic_write};
use std::fs;
use std::path::Path;
use tempfile::TempDir;

fn demo_workspace() -> Workspace {
    Workspace::demo("/media/device-a.mp4", "/media/device-b.mp4")
}

fn add_completed_attempt(store: &mut WorkspaceStore, device_index: usize) -> String {
    let root = store.root().to_path_buf();
    let (binding, attempt_no) = {
        let workspace = store.workspace();
        let segment = workspace.segments.first().unwrap();
        (
            segment.sources[device_index].clone(),
            segment.next_attempt_no(),
        )
    };
    let output = root
        .join("runs")
        .join(format!("attempt-{attempt_no}-{}.mp4", binding.device_id));
    fs::create_dir_all(output.parent().unwrap()).unwrap();
    fs::write(&output, format!("fixture-output-{attempt_no}")).unwrap();
    let hash = sha256_file(&output).unwrap();
    let mut attempt = AttemptRecord::new_pending(
        store.workspace().segments.first().unwrap(),
        &binding,
        output.to_string_lossy().as_ref(),
        "",
    );
    attempt.status = AttemptStatus::Succeeded;
    attempt.finished_at = now_rfc3339();
    attempt.output_size_bytes = Some(fs::metadata(&output).unwrap().len());
    attempt.output_sha256 = Some(hash);
    attempt.source_sha256 = Some(format!("source-hash-{attempt_no}"));
    attempt.source_size_bytes = Some(1_000 + attempt_no as u64);
    attempt.wall_time_ms = 120 + attempt_no as u64;
    attempt.pipeline = format!("fixture-pipeline-{attempt_no}");
    attempt.evidence_file = format!("evidence/attempts/{}.json", attempt.id);
    let id = attempt.id.clone();
    {
        let workspace = store.workspace_mut();
        let segment = workspace.segments.first_mut().unwrap();
        segment.attempts.push(attempt);
        workspace.touch();
    }
    id
}

fn add_comparison(store: &mut WorkspaceStore) {
    let workspace = store.workspace_mut();
    let segment = workspace.segments.first_mut().unwrap();
    let left = segment.attempts[0].clone();
    let right = segment.attempts[1].clone();
    let comparison = compare_attempts(&left, &right);
    store_comparison(segment, comparison).unwrap();
    workspace.touch();
}

fn assert_key_order(json: &str, keys: &[&str]) {
    let mut offset = 0;
    for key in keys {
        let needle = format!("\"{key}\"");
        let relative = json[offset..]
            .find(&needle)
            .unwrap_or_else(|| panic!("missing field {key}"));
        offset += relative + needle.len();
    }
}

#[test]
fn clock_offsets_resolve_each_device_independently() {
    let workspace = demo_workspace();
    let segment = workspace.segments.first().unwrap();
    assert_eq!(segment.sources[0].resolved_start_ms, 2_800);
    assert_eq!(segment.sources[0].resolved_end_ms, 4_400);
    assert_eq!(segment.sources[1].resolved_start_ms, 4_050);
    assert_eq!(segment.sources[1].resolved_end_ms, 5_650);
    assert_eq!(segment.sources[1].clock_offset_ms, 1_250);
}

#[test]
fn workspace_reload_preserves_fields_and_array_order() {
    let temp = TempDir::new().unwrap();
    let mut store = WorkspaceStore::create(temp.path(), demo_workspace()).unwrap();
    let first_id = add_completed_attempt(&mut store, 0);
    let second_id = add_completed_attempt(&mut store, 1);
    add_comparison(&mut store);
    store.save().unwrap();
    let expected = store.workspace().clone();
    let raw_before = fs::read_to_string(temp.path().join("workspace.json")).unwrap();

    let reopened = WorkspaceStore::open(temp.path().join("workspace.json")).unwrap();
    assert_eq!(reopened.workspace(), &expected);
    let segment = reopened.workspace().segments.first().unwrap();
    assert_eq!(segment.attempts[0].id, first_id);
    assert_eq!(segment.attempts[1].id, second_id);
    assert_eq!(segment.attempts[0].attempt_no, 1);
    assert_eq!(segment.attempts[1].attempt_no, 2);
    assert_eq!(segment.comparisons.len(), 1);
    assert_eq!(segment.comparisons[0].left_attempt_id, first_id);
    assert_eq!(segment.comparisons[0].right_attempt_id, second_id);

    assert_key_order(
        &raw_before,
        &[
            "schema_version",
            "workspace_id",
            "title",
            "created_at",
            "updated_at",
            "revision",
            "devices",
            "missing_segment",
            "segments",
        ],
    );
    assert_key_order(
        &raw_before,
        &[
            "id",
            "ordinal",
            "label",
            "timeline_start_ms",
            "duration_ms",
            "reason",
            "sources",
            "attempts",
            "comparisons",
            "selected_attempt_id",
        ],
    );
    assert_key_order(
        &raw_before,
        &[
            "id",
            "attempt_no",
            "segment_id",
            "device_id",
            "device_name",
            "started_at",
            "finished_at",
            "status",
            "source_path",
            "resolved_start_ms",
            "duration_ms",
            "pipeline",
            "output_path",
            "output_size_bytes",
            "output_sha256",
            "source_size_bytes",
            "source_sha256",
            "wall_time_ms",
            "error",
            "evidence_file",
        ],
    );

    let mut reopened = reopened;
    reopened.save().unwrap();
    let raw_after = fs::read_to_string(temp.path().join("workspace.json")).unwrap();
    assert_eq!(
        raw_before, raw_after,
        "reload + save must not reorder fields"
    );
}

#[test]
fn evidence_events_are_append_only_and_reload_in_sequence() {
    let temp = TempDir::new().unwrap();
    let mut store = WorkspaceStore::create(temp.path(), demo_workspace()).unwrap();
    store
        .record_event("note", None, None, "first note", vec!["a".to_string()])
        .unwrap();
    store
        .record_event("note", None, None, "second note", vec!["b".to_string()])
        .unwrap();
    drop(store);

    let reopened = WorkspaceStore::open(temp.path().join("workspace.json")).unwrap();
    let events = reopened.events();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].kind, "workspace_created");
    assert_eq!(events[1].summary, "first note");
    assert_eq!(events[2].summary, "second note");
    assert!(events[0].seq < events[1].seq && events[1].seq < events[2].seq);
    let raw = fs::read_to_string(temp.path().join("evidence/events.jsonl")).unwrap();
    let summaries: Vec<_> = raw
        .lines()
        .map(|line| serde_json::from_str::<EvidenceEvent>(line).unwrap().summary)
        .collect();
    assert_eq!(
        summaries,
        vec!["创建修正工作区", "first note", "second note"]
    );
}

#[test]
fn running_attempt_becomes_interrupted_after_reopen() {
    let temp = TempDir::new().unwrap();
    let mut store = WorkspaceStore::create(temp.path(), demo_workspace()).unwrap();
    let binding = store.workspace().segments[0].sources[0].clone();
    let mut attempt = AttemptRecord::new_pending(
        &store.workspace().segments[0],
        &binding,
        "/tmp/unfinished.mp4",
        "evidence/attempts/pending.json",
    );
    attempt.status = AttemptStatus::Running;
    store.workspace_mut().segments[0].attempts.push(attempt);
    store.workspace_mut().touch();
    store.save().unwrap();
    drop(store);

    let reopened = WorkspaceStore::open(temp.path().join("workspace.json")).unwrap();
    let attempt = &reopened.workspace().segments[0].attempts[0];
    assert_eq!(attempt.status, AttemptStatus::Interrupted);
    assert!(!attempt.finished_at.is_empty());
    assert!(attempt.error.as_deref().unwrap().contains("重新载入"));
    assert!(
        reopened
            .events()
            .iter()
            .any(|event| event.kind == "attempt_interrupted_on_reload")
    );
}

#[test]
fn completed_worker_evidence_recovers_attempt_after_abrupt_exit() {
    let temp = TempDir::new().unwrap();
    let mut store = WorkspaceStore::create(temp.path(), demo_workspace()).unwrap();
    let binding = store.workspace().segments[0].sources[0].clone();
    let mut attempt = AttemptRecord::new_pending(
        &store.workspace().segments[0],
        &binding,
        "/tmp/recovered.mp4",
        "evidence/attempts/pending.json",
    );
    attempt.status = AttemptStatus::Running;
    let attempt_id = attempt.id.clone();
    let segment_id = store.workspace().segments[0].id.clone();
    store.workspace_mut().segments[0].attempts.push(attempt);
    store.workspace_mut().touch();
    store.save().unwrap();
    let evidence_path = temp
        .path()
        .join("evidence/attempts")
        .join(format!("{attempt_id}.result.json"));
    write_run_evidence(
        &evidence_path,
        &attempt_id,
        &segment_id,
        Path::new(&binding.media_path),
        binding.resolved_start_ms,
        binding.clamped_duration_ms(),
        &RunResult {
            status: AttemptStatus::Succeeded,
            pipeline: "recovered-pipeline".to_string(),
            wall_time_ms: 345,
            output_size_bytes: Some(9_001),
            output_sha256: Some("recovered-output-hash".to_string()),
            source_size_bytes: Some(12_345),
            source_sha256: Some("recovered-source-hash".to_string()),
            error: None,
        },
    )
    .unwrap();
    drop(store);

    let reopened = WorkspaceStore::open(temp.path().join("workspace.json")).unwrap();
    let attempt = &reopened.workspace().segments[0].attempts[0];
    assert_eq!(attempt.status, AttemptStatus::Succeeded);
    assert_eq!(attempt.pipeline, "recovered-pipeline");
    assert_eq!(attempt.output_size_bytes, Some(9_001));
    assert_eq!(
        attempt.source_sha256.as_deref(),
        Some("recovered-source-hash")
    );
    assert_eq!(
        reopened.workspace().segments[0]
            .selected_attempt_id
            .as_deref(),
        Some(attempt_id.as_str())
    );
    assert!(
        reopened
            .events()
            .iter()
            .any(|event| event.kind == "attempt_recovered_from_worker_evidence")
    );
}

#[test]
fn export_bundle_roundtrips_and_detects_tampering() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("workspace");
    let bundle = temp.path().join("review");
    let mut store = WorkspaceStore::create(&root, demo_workspace()).unwrap();
    add_completed_attempt(&mut store, 0);
    add_completed_attempt(&mut store, 1);
    add_comparison(&mut store);
    store
        .record_event(
            "review_note",
            Some(store.workspace().segments[0].id.clone()),
            None,
            "复核备注",
            vec!["保留切点".to_string()],
        )
        .unwrap();
    store.save().unwrap();
    let expected_workspace = store.workspace().clone();
    let expected_events = store.events().to_vec();

    let report = export_review_bundle(store.workspace(), store.events(), &bundle, true).unwrap();
    assert_eq!(report.copied_outputs, 2);
    let manifest = load_review_bundle(&bundle).unwrap();
    assert_eq!(manifest.schema_version, SCHEMA_VERSION);
    assert_eq!(manifest.workspace, expected_workspace);
    assert_eq!(manifest.evidence_events, expected_events);
    assert_eq!(manifest.exported_files.len(), 7);

    let report = verify_review_bundle(&bundle).unwrap();
    assert!(report.is_ok());
    assert_eq!(report.attempts, 2);
    assert_eq!(report.comparisons, 1);

    let manifest_raw = fs::read_to_string(bundle.join(MANIFEST_FILE)).unwrap();
    assert_key_order(
        &manifest_raw,
        &[
            "schema_version",
            "tool_name",
            "tool_version",
            "exported_at",
            "bundle_id",
            "workspace",
            "evidence_events",
            "exported_files",
        ],
    );
    let attempts_csv = fs::read_to_string(bundle.join("attempts.csv")).unwrap();
    assert!(attempts_csv.starts_with(ATTEMPTS_HEADER));
    let segments_csv = fs::read_to_string(bundle.join("segments.csv")).unwrap();
    assert!(segments_csv.starts_with(SEGMENTS_HEADER));
    let comparisons_csv = fs::read_to_string(bundle.join("comparisons.csv")).unwrap();
    assert!(comparisons_csv.starts_with(COMPARISONS_HEADER));

    let readme = bundle.join("README.md");
    let original_readme = fs::read(&readme).unwrap();
    let mut tampered = original_readme.clone();
    tampered.extend_from_slice(b"\nchanged");
    atomic_write(&readme, &tampered).unwrap();
    let report = verify_review_bundle(&bundle).unwrap();
    assert!(!report.is_ok());
    assert!(
        report
            .errors
            .iter()
            .any(|error| error.contains("README.md"))
    );

    atomic_write(&readme, &original_readme).unwrap();
    let checksums_path = bundle.join("SHA256SUMS");
    let mut checksums = fs::read_to_string(&checksums_path).unwrap();
    let first_newline = checksums.find('\n').unwrap();
    let first_line = &checksums[..first_newline];
    let path = first_line.split_once("  ").unwrap().1;
    let replacement = format!("{}  {path}\n", "0".repeat(64));
    checksums.replace_range(..=first_newline, &replacement);
    atomic_write(&checksums_path, checksums.as_bytes()).unwrap();
    let report = verify_review_bundle(&bundle).unwrap();
    assert!(!report.is_ok());
    assert!(
        report
            .errors
            .iter()
            .any(|error| error.contains("SHA256SUMS 不一致"))
    );
}

#[test]
fn export_without_outputs_keeps_records_and_is_verifiable() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("workspace");
    let bundle = temp.path().join("review-no-outputs");
    let mut store = WorkspaceStore::create(&root, demo_workspace()).unwrap();
    add_completed_attempt(&mut store, 0);
    store.save().unwrap();
    let report = export_review_bundle(store.workspace(), store.events(), &bundle, false).unwrap();
    assert_eq!(report.copied_outputs, 0);
    assert!(verify_review_bundle(&bundle).unwrap().is_ok());
}
