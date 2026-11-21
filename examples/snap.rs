//! 手动验证：对测试片跑一次片段分析，保留证据目录到 /tmp/ssr-evidence。
use subtitle_safe_review::analysis::{run_segment, Analyzer};
use subtitle_safe_review::model::Segment;
use subtitle_safe_review::session::Project;

fn main() {
    let root = std::path::PathBuf::from("/tmp/ssr-evidence");
    let _ = std::fs::remove_dir_all(&root);
    let project = Project::open_or_create(&root).unwrap();
    let analyzer = Analyzer::new("file:///tmp/ssr-test.ogg").unwrap();
    let seg = Segment::new("seg001".into(), "缺失片段1".into(), 1000, 3000);
    let run = run_segment(&project, &analyzer, &seg).unwrap();
    println!(
        "frames={} violating={} max={:.4}",
        run.frames_scanned, run.violating_frames, run.max_ink_ratio
    );
    if let Some(bp) = &run.entry_boundary {
        println!(
            "entry: before={} after={} delta={:+.4}",
            bp.before.snapshot, bp.after.snapshot, bp.ink_ratio_delta
        );
    }
}
