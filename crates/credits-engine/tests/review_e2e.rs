use std::path::{Path, PathBuf};

use credits_core::project::load_project;
use credits_core::report::verify_report;
use credits_engine::runner::{run_review, ReviewOptions};
use gstreamer as gst;
use serde_json::json;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")))
}

fn fixture() -> PathBuf {
    workspace_root().join("fixtures/tail-roll.rollunk")
}

fn gst_ready() -> bool {
    if gst::init().is_err() {
        return false;
    }
    [
        "uridecodebin",
        "videoconvert",
        "audioconvert",
        "theoradec",
        "vorbisdec",
    ]
    .iter()
    .all(|name| gst::ElementFactory::find(name).is_some())
}

#[test]
fn v2_review_keeps_two_versions_and_continues_after_failure() {
    if !gst_ready() || !fixture().exists() {
        eprintln!("skipping GStreamer end-to-end test: plugins or fixture unavailable");
        return;
    }
    let work = tempfile::tempdir().unwrap();
    let project_path = work.path().join("project-v2.json");
    let out = work.path().join("out");
    let fixture = fixture();
    let project = json!({
        "schema": "credits-roll/v2",
        "title": "test",
        "timeline": {
            "fps": 24,
            "tracks": [{
                "id": "video-main",
                "kind": "video",
                "objects": [
                    {
                        "id": "unknown-tail",
                        "src": fixture,
                        "review_points": [{"label": "first", "at": "00:00:01.000"}],
                        "text": "CREDITS\n\nA. Person"
                    },
                    {
                        "id": "broken-tail",
                        "src": work.path().join("missing.rollunk")
                    },
                    {
                        "id": "after-failure",
                        "src": fixture,
                        "review_points": [{"at": "00:00:01.500"}],
                        "text": "AFTER\n\nB. Person"
                    }
                ]
            }]
        }
    });
    std::fs::write(&project_path, serde_json::to_vec_pretty(&project).unwrap()).unwrap();

    let report = run_review(
        &project_path,
        &ReviewOptions {
            out_dir: out.clone(),
            checkpoints: Vec::new(),
        },
    )
    .unwrap();

    assert_eq!(report.summary.ok_count, 2);
    assert_eq!(report.summary.failed_count, 1);
    assert_eq!(report.summary.correlation_count, 2);
    assert_eq!(report.summary.snapshot_count, 4);
    assert!(report.cleanup.removed);
    assert!(report.cleanup.residue.is_empty());
    let leftovers = std::fs::read_dir(&out)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".credits-review-work-")
        })
        .count();
    assert_eq!(leftovers, 0, "temporary work directories must be removed");

    let first = &report.objects[0];
    assert!(first.media.unknown_extension);
    assert_eq!(first.media.extension.as_deref(), Some("rollunk"));
    assert_eq!(first.correlations[0].versions.len(), 2);
    assert_eq!(first.correlations[0].time.frame, 24);
    assert_eq!(first.correlations[0].time.timecode, "00:00:01:00");
    assert!(first.waveform.available);
    assert_eq!(
        first.correlations[0].waveform_file,
        "evidence/0000-unknown-tail/waveform.json"
    );
    assert_eq!(
        first.waveform_json,
        "evidence/0000-unknown-tail/waveform.json"
    );
    assert!(!first.correlations[0].waveform_window.is_empty());
    for version in &first.correlations[0].versions {
        assert!(out.join(&version.file).exists());
    }
    assert!(out.join("evidence/0000-unknown-tail/waveform.svg").exists());
    assert!(out.join("evidence/0001-broken-tail/failure.json").exists());
    assert!(report
        .evidence_files
        .iter()
        .any(|entry| entry.path.ends_with("0001-broken-tail/failure.json")));
    assert_eq!(report.objects[1].id, "after-failure");

    let verification = verify_report(&out).unwrap();
    assert!(verification.ok(), "{:?}", verification.failures);
    assert!(verification.checked_files >= 8);
    let leftovers = std::fs::read_dir(&out)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".credits-review-work-")
        })
        .count();
    assert_eq!(leftovers, 0);
}

#[test]
fn legacy_project_is_detected_without_extension_rules() {
    let loaded = load_project(&workspace_root().join("examples/project-v1.json")).unwrap();
    assert_eq!(
        loaded.project.format,
        credits_core::model::ProjectFormat::LegacyV1
    );
    assert_eq!(loaded.project.objects.len(), 1);
    assert_eq!(loaded.project.objects[0].track, "video:1");
    assert!(!loaded.source.unknown_extension);
}
