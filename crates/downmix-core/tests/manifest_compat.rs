use downmix_core::manifest::SnapshotManifest;
use downmix_core::model::Revision;

const LEGACY_DOC: &str = r#"{
  "snapshot_revision": "legacy",
  "recorded_at": "1700000000",
  "source_path": "/data/object.bin",
  "source_sha256": "abcd1234",
  "source_size": 42,
  "unknown_extension": true,
  "recipe": "legacy-broadcast-v1",
  "matrix": [1.0, 0.0, 0.0, 1.0],
  "input_channels": 2,
  "output_channels": 2,
  "sample_rate": 48000,
  "sample_format": "F32LE",
  "pipeline": "filesrc ! wavenc",
  "status": "completed",
  "output_path": "downmix.legacy.wav",
  "output_sha256": "deadbeef",
  "output_size": 99,
  "correlation": [
    {
      "pts_ns": 0,
      "time_ms": 0,
      "frame_index": 0,
      "samples_per_frame": 1024,
      "peak": 0.5,
      "rms": 0.1,
      "frame_sha256": "0000000000000000000000000000000000000000000000000000000000000000"
    }
  ],
  "error": null
}"#;

#[test]
fn parses_legacy_wire_format() {
    let manifest = SnapshotManifest::parse_slice(LEGACY_DOC.as_bytes()).unwrap();
    assert_eq!(manifest.revision, Revision::Legacy);
    assert!(manifest.object.is_unknown_extension());
    assert_eq!(manifest.object.size_bytes, 42);
    assert_eq!(manifest.params.coefficient_matrix, vec![1.0, 0.0, 0.0, 1.0]);
    assert_eq!(manifest.correlation.len(), 1);
    let evidence = manifest.output.unwrap();
    assert_eq!(evidence.sha256, "deadbeef");
}

#[test]
fn round_trips_both_envelope_formats() {
    let manifest = SnapshotManifest::parse_slice(LEGACY_DOC.as_bytes()).unwrap();

    let dir = tempfile::tempdir().unwrap();
    let path = manifest.write(dir.path()).unwrap();
    assert!(path.ends_with("manifest.legacy.json"));
    let reread = SnapshotManifest::load_any(&path).unwrap();
    assert_eq!(reread.revision, Revision::Legacy);
    assert_eq!(reread.correlation, manifest.correlation);
}

#[test]
fn rejects_unknown_envelope_schema() {
    let doc = r#"{"schema": 99, "envelope": "x", "manifest": {}}"#;
    let err = SnapshotManifest::parse_slice(doc.as_bytes()).unwrap_err();
    assert!(err.to_string().contains("unrecognized"));
}
