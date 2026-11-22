use std::path::PathBuf;

use mask_edge_review::media::probe_media;
use mask_edge_review::project::{create_project, inspect_project};
use mask_edge_review::service::{
    acceptance_value, begin_run, execute_run, export_segment, finish_run, record_export,
};

#[test]
#[ignore = "需要 MASK_EDGE_TEST_VIDEO 指向可解码视频，并安装 GStreamer VP8/PNG 插件"]
fn render_segment_and_validate_evidence() {
    let Some(video) = std::env::var_os("MASK_EDGE_TEST_VIDEO") else {
        return;
    };
    let temp = tempfile::tempdir().unwrap();
    let project_dir = temp.path().join("project");
    let media = probe_media(PathBuf::from(&video).as_path()).unwrap();
    let mut opened = create_project(
        &project_dir,
        PathBuf::from(&video).as_path(),
        "smoke".into(),
        media,
    )
    .unwrap();
    let job = begin_run(&mut opened.session, 0).unwrap();
    let outcome = execute_run(&job).unwrap();
    finish_run(&mut opened.session, 0, &job.run_id, Ok(outcome.clone())).unwrap();
    assert!(outcome.output_path.is_file());
    assert!(outcome.output_sha256.len() == 64);
    assert!(!outcome.evidence.is_empty());
    for frame in &outcome.evidence {
        assert!(project_dir.join(&frame.relative_path).is_file());
    }
    let inspected = inspect_project(&project_dir).unwrap();
    let report = acceptance_value(&inspected.session);
    let checks = report["segments"][0]["evidence_checks"].as_array().unwrap();
    assert!(!checks.is_empty());
    assert!(checks.iter().all(|check| check["valid"] == true));

    let export_path = temp.path().join("export.webm");
    let exported = export_segment(&opened.session, 0, &export_path).unwrap();
    assert!(export_path.is_file());
    record_export(&mut opened.session, 0, &exported).unwrap();
    assert_eq!(opened.session.exports.len(), 1);
}
