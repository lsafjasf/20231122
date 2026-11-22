//! End-to-end workflow test.
//!
//! Requires the GStreamer command line tools to synthesize a short clip. The
//! test skips itself (rather than fails) when they are unavailable, so `cargo
//! test` stays green on minimal build hosts while exercising the full pipeline
//! on machines equipped like the target.

use std::path::PathBuf;
use std::process::Command;

use vkri_core::model::project::KeyframeMode;
use vkri_core::{export::verify_bundle, ProjectService};

fn have(tool: &str) -> bool {
    Command::new(tool).arg("--version").output().is_ok()
}

fn make_test_clip(path: &std::path::Path) {
    let status = Command::new("gst-launch-1.0")
        .args([
            "-q",
            "videotestsrc",
            "pattern=ball",
            "num-buffers=90",
            "!",
            "video/x-raw,framerate=30/1",
            "!",
            "vp8enc",
            "cpu-used=4",
            "deadline=1",
            "!",
            "webmmux",
            "!",
            "filesink",
        ])
        .arg(format!("location={}", path.display()))
        .status()
        .expect("spawn gst-launch");
    assert!(status.success(), "gst-launch failed to create clip");
}

#[test]
fn full_lifecycle_register_extract_compare_export_verify() {
    if !have("gst-launch-1.0") {
        eprintln!("skipping: gst-launch-1.0 unavailable");
        return;
    }
    vkri_core::media::init_gstreamer().expect("initialize GStreamer");
    let root = std::env::temp_dir().join(format!("vkri_e2e_{}", uuid::Uuid::new_v4().simple()));
    let media_dir = root.join("media");
    let project_dir = root.join("project");
    std::fs::create_dir_all(&media_dir).unwrap();
    let clip_a = media_dir.join("a.webm");
    let clip_b = media_dir.join("b.webm");
    make_test_clip(&clip_a);
    make_test_clip(&clip_b);

    let mut service = ProjectService::create(&project_dir, "e2e", "integration", "test").unwrap();
    let device_a = service.add_device("camera-A", "").unwrap();
    let device_b = service.add_device("camera-B", "").unwrap();
    service
        .set_device_offset(&device_b.id, 1_500_000_000, 25_000_000)
        .unwrap();

    let config = vkri_core::model::project::ExtractionConfig {
        mode: KeyframeMode::Interval,
        interval_ns: 1_000_000_000,
        thumbnail_max_edge: 120,
        jpeg_quality: 80,
    };
    let origin = 1_700_000_000_000_000_000;
    let segment_a = service
        .register_segment(
            &device_a.id,
            "frag-A",
            &clip_a,
            origin,
            "object_x",
            config.clone(),
        )
        .unwrap();
    let segment_b = service
        .register_segment(&device_b.id, "frag-B", &clip_b, origin, "object_x", config)
        .unwrap();

    let cancel = vkri_core::extract::CancellationToken::new();
    let out_a = service.run_segment(&segment_a.id, &cancel, |_| {}).unwrap();
    assert!(!out_a.interrupted);
    assert!(!out_a.keyframes.is_empty());
    let out_b = service.run_segment(&segment_b.id, &cancel, |_| {}).unwrap();
    assert!(!out_b.interrupted);

    // Re-running retains all run directories on disk.
    service.mark_rerun_requested(&segment_a.id).unwrap();
    let _ = service.run_segment(&segment_a.id, &cancel, |_| {}).unwrap();
    let evidence = project_dir.join("evidence").join(&segment_a.id);
    let run_dirs: Vec<PathBuf> = std::fs::read_dir(&evidence)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_dir()
                && path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("run-")
        })
        .collect();
    assert!(run_dirs.len() >= 2, "re-run must retain prior evidence");

    let comparison = service
        .compare(&segment_a.id, &segment_b.id, 900_000_000, "e2e")
        .unwrap();
    assert!(!comparison.entries.is_empty());

    let export_root = root.join("exports");
    let report = service.export(&export_root, "bundle").unwrap();
    assert!(report.manifest_sha256.len() == 64);

    let findings = verify_bundle(&report.bundle_dir).unwrap();
    assert!(findings.iter().any(|line| line.contains("verified OK")));

    // Reopening preserves the document.
    let reopened = ProjectService::open(&project_dir, "test").unwrap();
    assert_eq!(reopened.project.segments.len(), 2);
    assert_eq!(
        reopened.project.segments[0].keyframes.len(),
        service.project.segments[0].keyframes.len()
    );

    // External sha256sum -c must succeed on the bundle.
    let status = Command::new("sha256sum")
        .arg("-c")
        .arg("SHA256SUMS")
        .current_dir(&report.bundle_dir)
        .status()
        .expect("sha256sum");
    assert!(status.success());
}
