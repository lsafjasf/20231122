//! End-to-end assertions over the three fixture containers.
//! Run `tests/make_fixtures.sh` once if `fixtures/` is empty.

use std::path::PathBuf;

use mp4_integrity_lib::{isobmff, isobmff::Repairability, repair};

fn fixture(name: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("fixtures");
    p.push(name);
    p
}

#[test]
fn normal_container_has_no_errors() {
    let report = isobmff::inspect_file(&fixture("normal.mp4")).expect("read normal.mp4");
    assert!(report.is_healthy(), "normal fixture must be error free");
    assert_eq!(report.first_error_offset(), None);
    assert_eq!(report.track_ids.len(), 2, "video + audio tracks");
    assert!(report.track_kinds.values().any(|k| k == "video"));
    assert!(report.track_kinds.values().any(|k| k == "audio"));
    let top: Vec<_> = report.boxes.iter().filter(|b| b.depth == 0).collect();
    assert!(top.iter().any(|b| b.box_type.as_str() == "ftyp"));
    assert!(top.iter().any(|b| b.box_type.as_str() == "moov"));
    assert!(top.iter().any(|b| b.box_type.as_str() == "mdat"));
}

#[test]
fn truncated_container_is_flagged_and_trimmable() {
    let report = isobmff::inspect_file(&fixture("truncated.mp4")).expect("read truncated.mp4");
    assert!(!report.is_healthy());
    assert_eq!(report.first_error_offset(), Some(2443));
    let mdat = report
        .error_boxes()
        .find(|b| b.box_type.as_str() == "mdat")
        .expect("truncated mdat reported");
    assert_eq!(mdat.repair, Repairability::TrimTail);
}

#[test]
fn unknown_container_flags_private_box() {
    let report = isobmff::inspect_file(&fixture("unknown.mp4")).expect("read unknown.mp4");
    assert!(!report.is_healthy());
    assert_eq!(report.first_error_offset(), Some(32));
    let unknown = report
        .unknown_boxes()
        .find(|b| b.box_type.as_str() == "zzzz")
        .expect("injected zzzz box found");
    assert_eq!(unknown.track_id, 0);
    assert_eq!(unknown.repair, Repairability::Droppable);
}

#[test]
fn repair_unknown_container_drops_box_and_preserves_tracks() {
    let dir = std::env::temp_dir().join(format!("mp4-integ-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("unknown-repaired.mp4");
    let outcome = repair::repair_file(&fixture("unknown.mp4"), &out).expect("repair succeeds");
    assert!(out.exists());
    assert_eq!(outcome.dropped_boxes, vec!["zzzz @ 32".to_string()]);
    let repaired = isobmff::inspect_file(&out).unwrap();
    assert!(repaired.is_healthy());
    assert_eq!(repaired.track_ids.len(), 2);
}

#[test]
fn repair_refuses_same_path_and_existing_output() {
    let input = fixture("unknown.mp4");
    let same = repair::repair_file(&input, &input);
    assert!(matches!(same, Err(repair::RepairError::SameAsInput)));

    let dir = std::env::temp_dir().join(format!("mp4-integ-existing-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("already.mp4");
    std::fs::write(&out, b"placeholder").unwrap();
    let exists = repair::repair_file(&input, &out);
    assert!(matches!(exists, Err(repair::RepairError::OutputExists(_))));
}

#[test]
fn gstreamer_reload_counts_tracks() {
    let result = mp4_integrity_lib::gst_check::reload_and_count(&fixture("normal.mp4"))
        .expect("reload runs");
    assert!(result.success, "qtdemux accepts the normal fixture");
    assert_eq!(result.track_count, 2);
    assert!(result.track_kinds.contains(&"video".to_string()));
    assert!(result.track_kinds.contains(&"audio".to_string()));
}
