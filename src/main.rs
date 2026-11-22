//! srcrepair — audio sample-rate conversion local repair tool.
//!
//! Runs a GTK4 GUI by default; `srcrepair cli ...` exposes the same engine
//! headlessly for testing and scripted review.

mod cli;
mod engine;
mod media;
mod model;
mod review;
mod storage;
mod ui;
mod util;
mod wav;

use std::path::PathBuf;

use anyhow::Result;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("cli") {
        return run_cli(&args[2..]);
    }
    ui::run()
}

fn run_cli(args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("init") => {
            let dir = PathBuf::from(args.get(1).cloned().unwrap_or_else(|| ".".into()));
            let rate: u32 = args.get(2).map(|s| s.parse()).transpose()?.unwrap_or(48000);
            cli::create_project(dir, rate)
        }
        Some("add") => {
            let parsed = parse_add(args)?;
            cli::add_fragment(parsed).map(|_| ())
        }
        Some("run") => {
            let dir = PathBuf::from(args.get(1).cloned().unwrap_or_else(|| ".".into()));
            let id = args.get(2).cloned();
            cli::run(dir, id)
        }
        Some("verify") => {
            let dir = PathBuf::from(args.get(1).cloned().unwrap_or_else(|| ".".into()));
            cli::verify(dir)
        }
        Some("export") => {
            let dir = PathBuf::from(args.get(1).cloned().unwrap_or_else(|| ".".into()));
            cli::export(dir)
        }
        other => {
            eprintln!(
                "usage: srcrepair [gui] | cli init <dir> [rate] | cli add <dir> \
                 <label> <source> <src_rate> <gap_start_ns> <gap_dur_ns> <reason> \
                 | cli run <dir> [id] | cli verify <dir> | cli export <dir>\n\
                 unknown subcommand: {:?}",
                other
            );
            std::process::exit(2);
        }
    }
}

fn parse_add(args: &[String]) -> Result<cli::AddArgs> {
    if args.len() < 8 {
        anyhow::bail!(
            "usage: cli add <dir> <label> <source> <src_rate> <gap_start_ns> <gap_dur_ns> <reason>"
        );
    }
    Ok(cli::AddArgs {
        project_dir: PathBuf::from(&args[1]),
        device_label: args[2].clone(),
        source_path: args[3].clone(),
        source_rate: args[4].parse()?,
        gap_start_ns: args[5].parse()?,
        gap_duration_ns: args[6].parse()?,
        gap_reason: args[7].clone(),
    })
}
