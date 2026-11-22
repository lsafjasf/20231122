use hdr_meta_fix::fix::RepairSession;
use hdr_meta_fix::{gst, report, rules, synth};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("--selftest") => selftest(),
        Some("--analyze") => {
            let Some(path) = args.get(2) else {
                eprintln!("用法：hdr-meta-fix --analyze <视频文件>");
                std::process::exit(2);
            };
            analyze_file(path);
        }
        Some("--help") | Some("-h") => usage(),
        _ => {
            std::process::exit(hdr_meta_fix::ui::run().into());
        }
    }
}

fn usage() {
    println!("用法：");
    println!("  hdr-meta-fix                 启动 GTK4 图形界面");
    println!("  hdr-meta-fix --selftest      用合成帧（正常/缺字段/冲突）验证并导出报告");
    println!("  hdr-meta-fix --analyze FILE  无头分析视频文件逐帧元数据");
}

/// 无头自检：三类合成帧 → 分析 → 自动应用全部建议 → 导出对照清单。
fn selftest() {
    let groups = [
        ("正常帧", synth::normal_frames()),
        ("缺字段帧", synth::missing_field_frames()),
        ("冲突帧", synth::conflicting_frames()),
    ];
    let mut session = RepairSession::new(Vec::new());
    for (name, frames) in &groups {
        println!("== {name} ==");
        for f in frames {
            let issues = rules::analyze(f);
            println!("  帧 {}：{} 个问题", f.frame_no, issues.len());
            for i in &issues {
                println!("    [{}] {}", i.kind.label(), i.description);
            }
            session.frames.push(f.clone());
        }
    }
    // 逐帧逐字段应用全部建议（模拟用户在 GUI 中点"应用"）。
    for idx in 0..session.frames.len() {
        loop {
            let issues = session.issues(idx);
            let Some(sug) = issues.iter().flat_map(|i| i.suggestions.iter()).next().cloned()
            else {
                break;
            };
            if !session.apply(idx, &sug) {
                break;
            }
        }
    }
    println!("\n修复后剩余问题：{}", session.remaining_issues());
    std::fs::create_dir_all("artifacts").ok();
    std::fs::write("artifacts/report.md", report::to_markdown(&session)).unwrap();
    std::fs::write("artifacts/report.csv", report::to_csv(&session)).unwrap();
    println!("报告已写入 artifacts/report.md 与 artifacts/report.csv");
}

/// 无头分析真实视频文件。
fn analyze_file(path: &str) {
    match gst::extract_frames(path, 300) {
        Ok(frames) => {
            println!("提取到 {} 帧", frames.len());
            for f in &frames {
                let issues = rules::analyze(f);
                if issues.is_empty() {
                    println!("帧 {}：正常", f.frame_no);
                }
                for i in issues {
                    println!(
                        "帧 {} [{}] {} → 建议 {:?}",
                        f.frame_no,
                        i.kind.label(),
                        i.description,
                        i.suggestions
                            .iter()
                            .map(|s| format!("{}={}", s.field.label(), s.value))
                            .collect::<Vec<_>>()
                    );
                }
            }
        }
        Err(e) => {
            eprintln!("分析失败：{e:#}");
            std::process::exit(1);
        }
    }
}
