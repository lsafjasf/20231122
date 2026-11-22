mod ui;

use std::env;
use std::process::ExitCode;

use anyhow::{Result, bail};
use gtk::prelude::*;

use bwf_replay_lab::run_experiment;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("bwf-replay-lab: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help();
        return Ok(());
    }
    if args.iter().any(|arg| arg == "--headless") {
        let output = run_experiment(None)?;
        println!("{}", output.report_path.display());
        print!(
            "{}",
            std::fs::read_to_string(output.evidence_dir.join("summary.txt"))?
        );
        return Ok(());
    }
    let screenshot = args
        .iter()
        .position(|arg| arg == "--screenshot")
        .map(|index| {
            args.get(index + 1)
                .map(std::path::PathBuf::from)
                .ok_or_else(|| anyhow::anyhow!("--screenshot requires a PNG path"))
        })
        .transpose()?;
    if let Some(unknown) = args.iter().find(|arg| {
        *arg != "--screenshot"
            && !screenshot
                .as_ref()
                .is_some_and(|path| path.as_os_str() == arg.as_str())
    }) {
        bail!("unknown argument {unknown:?}; use --headless or --help");
    }

    let initial = run_experiment(None)?;
    let app = gtk::Application::builder()
        .application_id("dev.bwf.ReplayLab")
        .build();
    app.connect_activate(move |app| {
        ui::install_view(app, initial.clone());
        if let Some(path) = &screenshot {
            ui::schedule_screenshot(app, path.clone());
        }
    });
    app.run_with_args(&["bwf-replay-lab"]);
    Ok(())
}

fn print_help() {
    println!(
        "BWF 时间标记校验 · 可回放实验

用法:
  bwf-replay-lab              启动 GTK4 审计界面
  bwf-replay-lab --headless   运行实验、写入证据并打印摘要
  bwf-replay-lab --screenshot out.png
                              启动界面、截图后退出
  bwf-replay-lab --help       显示帮助

证据目录: artifacts/bwf-replay-lab/evidence"
    );
}
