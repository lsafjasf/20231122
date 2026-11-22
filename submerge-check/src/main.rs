//! 多语言字幕时间轴合并 — 来源检查页。
//!
//! 运行模式：
//! - 默认（无参数）：执行检查流水线并弹出 GTK4 来源检查页；
//! - `--selftest`：无界面模式，执行完整流水线并打印摘要，用于 CI/无显示环境；
//! - `--report-dir <DIR>`：指定报告输出目录（默认 `./artifacts`）。

mod audit;
mod gst_check;
mod merger;
mod model;
mod parser;
mod sample;
mod ui;
mod workspace;

use std::path::PathBuf;

fn parse_args() -> (bool, PathBuf) {
    let mut selftest = false;
    let mut report_dir = PathBuf::from("artifacts");
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--selftest" => selftest = true,
            "--report-dir" => {
                if let Some(value) = args.next() {
                    report_dir = PathBuf::from(value);
                }
            }
            other => eprintln!("忽略未知参数: {other}"),
        }
    }
    (selftest, report_dir)
}

fn main() {
    let (selftest, report_dir) = parse_args();
    let report = audit::run_pipeline(&report_dir).unwrap_or_else(|err| {
        eprintln!("检查流水线失败: {err}");
        std::process::exit(1);
    });

    println!(
        "检查完成：{} 条结论，报告已写入 {:?}",
        report.findings.len(),
        report.report_path
    );
    println!(
        "临时工作区清理：{} → {}",
        report.temp_root_existed_before_cleanup, report.temp_root_exists_after_cleanup
    );

    if selftest {
        ui::print_selftest_summary(&report);
        return;
    }
    ui::run_window(report);
}
