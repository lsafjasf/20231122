mod export;
mod fixtures;
mod model;
mod scanner;
mod ui;
mod util;

use anyhow::{Context, Result};
use gstreamer as gst;
use std::env;
use std::path::PathBuf;

use model::{ScanReport, SourceState};
use scanner::ScanOptions;

fn main() -> Result<()> {
    gst::init().context("初始化 GStreamer 失败")?;
    let root = workspace_root()?;
    let mut args = env::args().skip(1);
    let command = args.next().unwrap_or_else(|| "ui".to_string());
    match command.as_str() {
        "demo" => {
            let report = scanner::demo(&root)?;
            print_summary(&report);
        }
        "scan" => {
            let report = scanner::scan(
                &ScanOptions {
                    root: root.clone(),
                    pause_after: None,
                    fresh: true,
                },
                false,
            )?
            .context("扫描未产生报告")?;
            print_summary(&report);
        }
        "resume" => {
            let report = scanner::resume(&root)?;
            print_summary(&report);
        }
        "report" => {
            let report = scanner::load_report(&root)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        "ui" => ui::run(&root)?,
        other => anyhow::bail!("未知命令 {other}; 可用: demo | scan | resume | report | ui"),
    }
    Ok(())
}

fn workspace_root() -> Result<PathBuf> {
    if let Ok(value) = env::var("SOURCE_LEDGER_ROOT") {
        return Ok(PathBuf::from(value));
    }
    let cwd = env::current_dir()?;
    if cwd.join("Cargo.toml").is_file() {
        Ok(cwd)
    } else {
        anyhow::bail!("请在仓库根目录运行，或设置 SOURCE_LEDGER_ROOT")
    }
}

fn print_summary(report: &ScanReport) {
    println!("来源检查完成：{}", report.project.package_identity);
    println!(
        "自动通过 {} / 待人工 {} / 失败 {} / 恢复重放 {}",
        report.counts.auto_passed,
        report.counts.needs_manual,
        report.counts.failures,
        report.counts.replays
    );
    for source in &report.sources {
        let marker = match source.state {
            SourceState::Verified | SourceState::Replayed => "PASS",
            SourceState::Unknown => "MANUAL",
            SourceState::Ambiguous => "BOUNDARY",
            SourceState::Failed => "FAIL",
        };
        println!("  {marker:8} {}  {}", source.spec.label, source.spec.path);
    }
    println!("报告：artifacts/report.json");
}
