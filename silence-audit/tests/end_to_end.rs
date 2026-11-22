mod common;

use std::fs;

extern crate gstreamer as gst;

use common::{standard_spec, write_wav};
use silence_audit::model::ObjectStatus;
use silence_audit::model::SilenceParams;
use silence_audit::runner;

fn test_env() -> tempdir_lite::TempDir {
    tempdir_lite::TempDir::new()
}

mod tempdir_lite {
    use std::fs;
    use std::path::PathBuf;

    pub struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        pub fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("silence-audit-test-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }
        pub fn path(&self) -> &std::path::Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

#[test]
fn batch_handles_legacy_and_v2_with_truncation_and_failures() {
    gst::init().unwrap();
    let dir = test_env();
    let media = dir.path().join("media");
    fs::create_dir_all(&media).unwrap();
    write_wav(&media.join("a.wav"), &standard_spec()).unwrap();
    write_wav(&media.join("b.wav"), &standard_spec()).unwrap();
    write_wav(&media.join("c.wav"), &standard_spec()).unwrap();

    // 旧格式：L1 完整，L2 截断（无 closed/end_time），L3 指向缺失文件。
    let legacy = serde_json::json!({
        "records": [
            {"id": "L1", "file": "a.wav", "track": "vocals", "frame": 0,
             "closed": true, "end_time": 5.0},
            {"id": "L2", "file": "b.wav", "track": "mic", "frame": 1280},
            {"id": "L3-MISSING", "file": "nope.wav", "track": "x", "frame": 0}
        ]
    });
    let legacy_path = dir.path().join("legacy.json");
    fs::write(&legacy_path, legacy.to_string()).unwrap();

    let params = SilenceParams {
        threshold_db: -50.0,
        on_millis: 200,
        off_millis: 200,
        interval_millis: 50,
        resample_rate: 48_000,
        waveform_buckets: 256,
    };

    let out = runner::run_batch(
        &legacy_path,
        &dir.path().join("runs"),
        &params,
        Some(&media),
    )
    .unwrap();
    assert_eq!(out.report.total_objects, 3);
    assert_eq!(out.report.succeeded, 2);
    assert_eq!(out.report.failed, 1);

    let l1 = out
        .report
        .results
        .iter()
        .find(|r| r.object_id == "L1")
        .unwrap();
    assert_eq!(l1.status, ObjectStatus::Ok);
    assert_eq!(l1.source_format, "legacy");
    assert!(
        l1.segments.len() >= 2,
        "应识别出至少两段静音，实际 {}",
        l1.segments.len()
    );
    assert!(l1.segments.iter().all(|s| !s.truncated));
    // 时间点与帧必须互相吻合（帧 ≈ 时间 * 采样率）。
    for seg in &l1.segments {
        let expect_frame = (seg.start_secs * 48_000.0).round() as u64;
        assert!(
            seg.start_frame.abs_diff(expect_frame) <= 48,
            "帧/时间不对应"
        );
        assert!(l1.track_ref.contains("track=vocals"));
    }

    let l2 = out
        .report
        .results
        .iter()
        .find(|r| r.object_id == "L2")
        .unwrap();
    assert_eq!(l2.status, ObjectStatus::Truncated);

    let l3 = out
        .report
        .results
        .iter()
        .find(|r| r.object_id == "L3-MISSING")
        .unwrap();
    assert_eq!(l3.status, ObjectStatus::Failed);
    assert!(l3.error.as_ref().unwrap().contains("不存在"));

    // 证据文件存在且已登记哈希；临时目录已清理。
    assert!(!out.run_dir.join("tmp").exists(), "临时目录应被清理");
    assert!(out.report_path.exists());
    for entry in &out.report.evidence {
        assert!(out.run_dir.join(&entry.path).is_file());
        assert_eq!(entry.sha256.len(), 64);
    }
    assert!(out.report.evidence.iter().any(|e| e.kind == "waveform-png"));
    assert!(out.report.evidence.iter().any(|e| e.kind == "media-copy"));

    // 新格式 v2 同样可处理。
    let v2 = serde_json::json!({
        "version": 2,
        "objects": [
            {"object_id": "N1", "state": "complete",
             "media": {"path": "c.wav", "track": "master",
                       "start_frame": 0, "end_frame": 240000}}
        ]
    });
    let v2_path = dir.path().join("v2.json");
    fs::write(&v2_path, v2.to_string()).unwrap();
    let out2 =
        runner::run_batch(&v2_path, &dir.path().join("runs"), &params, Some(&media)).unwrap();
    assert_eq!(out2.report.total_objects, 1);
    assert_eq!(out2.report.results[0].source_format, "v2");
    assert_eq!(out2.report.results[0].status, ObjectStatus::Ok);
}

#[test]
fn truncated_recording_keeps_final_silence_open() {
    gst::init().unwrap();
    let dir = test_env();
    let media = dir.path().join("media");
    fs::create_dir_all(&media).unwrap();
    // 末尾 1 秒保持静音且不恢复。
    let spec = common::AudioSpec {
        duration_secs: 3.0,
        sample_rate: 48_000,
        windows: vec![(0.0, 1.5, false), (1.5, 3.0, true)],
    };
    write_wav(&media.join("tail.wav"), &spec).unwrap();

    let snap = serde_json::json!({
        "records": [
            {"id": "T1", "file": "tail.wav", "track": "mic", "frame": 0}
        ]
    });
    let snap_path = dir.path().join("snap.json");
    fs::write(&snap_path, snap.to_string()).unwrap();

    let out = runner::run_batch(
        &snap_path,
        &dir.path().join("runs"),
        &SilenceParams::default(),
        Some(&media),
    )
    .unwrap();
    let result = &out.report.results[0];
    assert_eq!(result.status, ObjectStatus::Truncated);
    let last = result.segments.last().expect("应存在末尾开放静音段");
    assert!(last.truncated);
    assert!(last.end_secs.is_none());
    assert!(last.end_frame.is_none());
}
