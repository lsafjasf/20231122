//! 端到端：对真实媒体文件重跑一个片段，验证边界采样与证据落盘。
//! 需要 /tmp/ssr-test.ogg（可用 gst-launch 生成，见 README）。文件缺失时跳过。
use subtitle_safe_review::analysis::{run_segment, Analyzer};
use subtitle_safe_review::model::Segment;
use subtitle_safe_review::session::Project;
use std::path::Path;

#[test]
fn rerun_segment_and_collect_evidence() {
    let media = Path::new("/tmp/ssr-test.ogg");
    if !media.exists() {
        eprintln!("跳过：/tmp/ssr-test.ogg 不存在");
        return;
    }
    let root = std::env::temp_dir().join(format!("ssr-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let project = Project::open_or_create(&root).unwrap();
    let analyzer = Analyzer::new("file:///tmp/ssr-test.ogg").unwrap();
    assert!(analyzer.duration_ms() >= 3000, "测试片约 4 秒");
    let seg = Segment::new("seg001".into(), "缺失片段1".into(), 1000, 3000);
    let run = run_segment(&project, &analyzer, &seg).unwrap();
    assert!(run.frames_scanned > 30, "应扫描到帧: {}", run.frames_scanned);
    assert!(run.violating_frames > 0, "字幕应触发违规");
    for pair in [&run.entry_boundary, &run.exit_boundary] {
        let bp = pair.as_ref().expect("应有边界样本");
        assert!(bp.before.timestamp_ms < bp.after.timestamp_ms);
        for s in [&bp.before, &bp.after] {
            let p = project.evidence_dir().join(&s.snapshot);
            assert!(p.exists(), "证据缺失: {}", p.display());
            assert!(std::fs::metadata(&p).unwrap().len() > 1000);
        }
    }
    let entry = run.entry_boundary.as_ref().unwrap();
    assert!(entry.ink_ratio_delta.abs() < 0.2);
    let _ = std::fs::remove_dir_all(&root);
}
