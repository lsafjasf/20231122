use downmix_core::model::{
    AudioObject, DetectionKind, ProcessingParams, Revision, SnapshotStatus,
};
use downmix_core::manifest::{FileEvidence, SnapshotManifest};
use downmix_core::verify::verify_evidence;
use std::fs;

fn object(id: &str, root: &std::path::Path) -> AudioObject {
    let path = root.join(format!("{id}.wav"));
    fs::write(&path, b"RIFF####WAVE").unwrap();
    AudioObject {
        id: id.to_string(),
        path,
        extension: "wav".to_string(),
        detection: DetectionKind::SniffedContent,
        size_bytes: 10,
        sha256: downmix_core::hash::sha256_file(&root.join(format!("{id}.wav"))).unwrap(),
    }
}

fn failed_manifest(revision: Revision, object: &AudioObject) -> SnapshotManifest {
    SnapshotManifest {
        schema: 2,
        revision,
        created_at: "1700000000".to_string(),
        object: object.clone(),
        params: ProcessingParams::unavailable(revision),
        status: SnapshotStatus::Failed,
        output: None,
        waveform: None,
        correlation: Vec::new(),
        error: Some("simulated pipeline failure".to_string()),
    }
}

#[test]
fn failed_but_documented_snapshots_verify_clean() {
    let root = tempfile::tempdir().unwrap();
    let evidence = root.path().join("evidence");
    let obj = object("obj", root.path());
    let dir = evidence.join(&obj.id);
    fs::create_dir_all(&dir).unwrap();
    failed_manifest(Revision::Legacy, &obj)
        .write(&dir)
        .unwrap();
    failed_manifest(Revision::Current, &obj)
        .write(&dir)
        .unwrap();

    let report = verify_evidence(&evidence, &[obj]).unwrap();
    assert!(report.clean);
}

#[test]
fn tampered_output_hash_is_detected() {
    let root = tempfile::tempdir().unwrap();
    let evidence = root.path().join("evidence");
    let obj = object("obj", root.path());
    let dir = evidence.join(&obj.id);
    fs::create_dir_all(&dir).unwrap();

    let output_path = dir.join("downmix.current.wav");
    fs::write(&output_path, vec![0u8; 128]).unwrap();
    let sha = downmix_core::hash::sha256_file(&output_path).unwrap();
    let manifest = completed_manifest(Revision::Current, &obj, &sha, 128, &dir);
    manifest.write(&dir).unwrap();
    failed_manifest(Revision::Legacy, &obj)
        .write(&dir)
        .unwrap();

    // Mutate the output without updating the manifest.
    fs::write(&output_path, vec![1u8; 128]).unwrap();
    let report = verify_evidence(&evidence, &[obj]).unwrap();
    assert!(!report.clean);
    let check = &report.object_reports[0].current;
    assert!(!check.output_hash_matches);
    assert!(check.problems.iter().any(|p| p.contains("sha256")));
}

fn completed_manifest(
    revision: Revision,
    object: &AudioObject,
    sha: &str,
    size: u64,
    dir: &std::path::Path,
) -> SnapshotManifest {
    let point = downmix_core::model::CorrelationPoint {
        pts_ns: 0,
        time_ms: 0,
        frame_index: 0,
        samples_per_frame: 2,
        peak: 0.5,
        rms: 0.25,
        frame_sha256: "a".repeat(64),
    };
    let waveform = downmix_core::model::WaveformSummary {
        total_frames: 64,
        duration_ms: 10,
        peak: 0.5,
        rms: 0.25,
        dc_offset: 0.01,
        points: vec![point.clone()],
    };
    SnapshotManifest {
        schema: 2,
        revision,
        created_at: "1700000000".to_string(),
        object: object.clone(),
        params: ProcessingParams::unavailable(revision),
        status: SnapshotStatus::Completed,
        output: Some(FileEvidence {
            path: downmix_core::model::relative(
                &dir.join("downmix.current.wav"),
                dir,
            )
            .to_string_lossy()
            .into_owned(),
            sha256: sha.to_string(),
            size_bytes: size,
        }),
        waveform: Some(waveform),
        correlation: vec![point],
        error: None,
    }
}

#[test]
fn missing_manifest_is_reported() {
    let root = tempfile::tempdir().unwrap();
    let evidence = root.path().join("evidence");
    let obj = object("obj", root.path());
    let report = verify_evidence(&evidence, &[obj]).unwrap();
    assert!(!report.clean);
    assert!(!report.object_reports[0].legacy.manifest_present);
}

