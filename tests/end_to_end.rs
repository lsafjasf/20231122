use std::f32::consts::TAU;
use std::fs;

use alc_review::evidence::{self, AssessOptions};

#[test]
fn new_manifest_with_unknown_extension_produces_verifiable_evidence() {
    let temp = tempfile::tempdir().unwrap();
    let audio_path = temp.path().join("recovered_replay.rplx");
    write_wav(&audio_path);
    let manifest_path = temp.path().join("manifest.json");
    fs::write(
        &manifest_path,
        r#"{
          "format": "audio-plc-assessment",
          "format_version": 2,
          "case_id": "e2e-unknown-extension",
          "audio": {"path": "recovered_replay.rplx"},
          "tracks": [{"id": "mic", "index": 0, "frame_size": 960}],
          "processing": [{"id": "plc", "parameters": {"fade_ms": 12}}],
          "events": [{"id": "loss-1", "timestamp_ms": 500, "duration_ms": 80,
            "track_id": "mic", "frame": 25, "processing_id": "plc"}]
        }"#,
    )
    .unwrap();

    let evidence_dir = temp.path().join("evidence");
    let mut outcome = evidence::assess(&AssessOptions {
        manifest: Some(manifest_path),
        audio: None,
        out: Some(evidence_dir.clone()),
        copy_audio: true,
        force: false,
        max_seconds: 30,
    })
    .unwrap();

    assert!(outcome.success, "{:?}", outcome.failure);
    assert_eq!(
        outcome
            .automated
            .as_ref()
            .unwrap()
            .unknown_extension
            .as_deref(),
        Some("rplx")
    );
    assert!(
        evidence_dir
            .join("inputs/audio/recovered_replay.rplx")
            .is_file()
    );
    assert!(evidence_dir.join("output-waveform.wav").is_file());
    assert!(evidence_dir.join("waveform.svg").is_file());
    assert!(evidence_dir.join("timeline.csv").is_file());
    let inventory = fs::read_to_string(evidence_dir.join("input-inventory.json")).unwrap();
    assert!(inventory.contains("\"gstreamer_caps\""));
    assert!(inventory.contains("audio/x-raw"));
    let markdown = fs::read_to_string(evidence_dir.join("report.md")).unwrap();
    assert!(markdown.contains("## 自动结论"));
    assert!(markdown.contains("## 人工确认"));
    assert!(markdown.contains("recovered_replay.rplx"));
    let verdicts: Vec<_> = outcome
        .manual
        .as_ref()
        .unwrap()
        .items
        .iter()
        .map(|item| {
            (
                item.id.clone(),
                alc_review::report::ManualVerdict::Pass,
                "已听辨".to_string(),
            )
        })
        .collect();
    evidence::mark_manual_review(
        &mut outcome,
        "tester".to_string(),
        "人工听辨完成".to_string(),
        &verdicts,
    )
    .unwrap();
    let markdown = fs::read_to_string(evidence_dir.join("report.md")).unwrap();
    assert!(markdown.contains("**状态：通过**"));
    assert!(markdown.contains("人工听辨完成"));
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(evidence_dir.join("report.json")).unwrap()).unwrap();
    assert_eq!(report["automated"]["verdict"], "attention");
    assert_eq!(report["manual"]["status"], "pass");
    let index: serde_json::Value =
        serde_json::from_slice(&fs::read(evidence_dir.join("evidence-index.json")).unwrap())
            .unwrap();
    assert!(index["input_audio_sha256"].is_string());
    let verification = evidence::verify_evidence(&evidence_dir).unwrap();
    assert!(verification.ok(), "{:?}", verification.issues);
}

fn write_wav(path: &std::path::Path) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).unwrap();
    for index in 0..48_000 {
        let sample = (TAU * 220.0 * index as f32 / 48_000.0).sin() * 0.25;
        writer
            .write_sample((sample * i16::MAX as f32) as i16)
            .unwrap();
    }
    writer.finalize().unwrap();
}

#[test]
fn missing_audio_preserves_failure_evidence_and_action() {
    let temp = tempfile::tempdir().unwrap();
    let manifest_path = temp.path().join("manifest.json");
    fs::write(
        &manifest_path,
        r#"{
          "format": "audio-plc-assessment",
          "format_version": 2,
          "case_id": "failure-case",
          "audio": {"path": "not-present.rplx"},
          "tracks": [{"id": "mic", "index": 0}],
          "events": [{"id": "loss-1", "timestamp_ms": 1000, "track_id": "mic"}]
        }"#,
    )
    .unwrap();
    let evidence_dir = temp.path().join("failure-evidence");
    let outcome = evidence::assess(&AssessOptions {
        manifest: Some(manifest_path),
        audio: None,
        out: Some(evidence_dir.clone()),
        copy_audio: true,
        force: false,
        max_seconds: 30,
    })
    .unwrap();

    assert!(!outcome.success);
    assert!(outcome.failure.is_some());
    assert!(evidence_dir.join("inputs/original-manifest.json").is_file());
    assert!(evidence_dir.join("automated-conclusions.json").is_file());
    assert!(evidence_dir.join("manual-confirmation.json").is_file());
    let markdown = fs::read_to_string(evidence_dir.join("report.md")).unwrap();
    assert!(markdown.contains("## 自动结论"));
    assert!(markdown.contains("## 人工确认"));
    assert!(markdown.contains("后续操作"));
    assert!(markdown.contains("--audio"));
    let run: serde_json::Value =
        serde_json::from_slice(&fs::read(evidence_dir.join("run.json")).unwrap()).unwrap();
    assert_eq!(run["status"], "failed");
}
