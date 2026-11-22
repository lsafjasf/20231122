use rotprov_core::model::*;
use rotprov_core::pipeline;
use std::path::PathBuf;

fn manifests() -> Vec<InputManifest> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let text = std::fs::read_to_string(root.join("fixtures/manifest.json")).unwrap();
    let list: Vec<InputManifest> = serde_json::from_str(&text).unwrap();
    list.into_iter()
        .map(|mut m| {
            m.media_path = root.join(&m.media_path).to_string_lossy().to_string();
            m.sidecar_path = root.join(&m.sidecar_path).to_string_lossy().to_string();
            m
        })
        .collect()
}

fn entry<'a>(report: &'a RunReport, id: &str) -> &'a ProvenanceEntry {
    report
        .entries
        .iter()
        .find(|e| e.manifest.id == id)
        .unwrap()
}

#[test]
fn batch_results_match_expectations() {
    let report = pipeline::run(&manifests());
    assert_eq!(report.entries.len(), 5);

    assert!(matches!(
        entry(&report, "alpha").conclusion,
        Conclusion::Fixed { from: Rotation::R90 }
    ));
    assert!(matches!(
        entry(&report, "beta").conclusion,
        Conclusion::NoFixNeeded
    ));
    // 无法判断的边界：结论为 Undecidable，且冲突证据被保留。
    let gamma = entry(&report, "gamma");
    assert!(matches!(gamma.conclusion, Conclusion::Undecidable { .. }));
    assert!(gamma
        .evidence
        .iter()
        .any(|e| e.kind == EvidenceKind::ConflictingValues
            && e.content.contains("rotate-90")
            && e.content.contains("180")));
    // 异常对象记为 Failed，且其余对象仍被处理。
    assert!(matches!(
        entry(&report, "delta").conclusion,
        Conclusion::Failed { .. }
    ));
    assert!(entry(&report, "delta")
        .evidence
        .iter()
        .any(|e| e.kind == EvidenceKind::ErrorContext));
    assert!(matches!(
        entry(&report, "epsilon").conclusion,
        Conclusion::Fixed { from: Rotation::R270 }
    ));
}

#[test]
fn two_snapshots_per_successful_object() {
    let report = pipeline::run(&manifests());
    for id in ["alpha", "beta", "gamma", "epsilon"] {
        let e = entry(&report, id);
        assert_eq!(e.snapshots.len(), 2, "{id} 应有两个版本快照");
        assert_eq!(e.snapshots[0].version, 1);
        assert_eq!(e.snapshots[0].stage, "ingest");
        assert_eq!(e.snapshots[1].version, 2);
        assert_eq!(e.snapshots[1].stage, "normalize");
    }
}

#[test]
fn long_record_is_truncated_but_verifiable() {
    let report = pipeline::run(&manifests());
    let epsilon = entry(&report, "epsilon");
    let record = &epsilon.record;
    assert!(record.truncated);
    assert!(record.steps.len() <= pipeline::MAX_KEPT_STEPS);
    assert!(record.original_step_count > pipeline::MAX_KEPT_STEPS);
    let hash = record.original_hash.as_ref().expect("截断记录必须保留哈希");
    assert_eq!(hash.len(), 16);
    assert!(record.steps.iter().any(|s| s.name.contains("截断")));
    // 未截断的记录不带哈希。
    assert!(!entry(&report, "alpha").record.truncated);
    assert!(entry(&report, "alpha").record.original_hash.is_none());
}

#[test]
fn temp_workspace_is_cleaned_up() {
    let report = pipeline::run(&manifests());
    assert!(report.temp_cleaned, "临时目录应已清理: {}", report.temp_dir);
    assert!(!std::path::Path::new(&report.temp_dir).exists());
}

#[test]
fn gst_probe_identifies_container() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let caps = rotprov_core::gstprobe::probe_container(
        &root.join("fixtures/media/alpha.mp4"),
    )
    .expect("typefind 应识别最小 MP4");
    assert!(caps.contains("video/quicktime"), "caps: {caps}");
}

#[test]
fn gst_taglist_normalizes_orientation() {
    assert_eq!(
        rotprov_core::gstprobe::normalize_orientation_tag("rotate-90"),
        Some(Rotation::R90)
    );
    assert_eq!(
        rotprov_core::gstprobe::normalize_orientation_tag("bogus"),
        None
    );
}
