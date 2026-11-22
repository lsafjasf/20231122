//! 字幕字体覆盖报告 — pausable subtitle font coverage pipeline.
//!
//! Usage:
//!   subtitle-font-coverage                      launch the GTK4 UI
//!   subtitle-font-coverage analyze <object> [--out DIR] [--pause-after N]
//!   subtitle-font-coverage resume  <object> [--out DIR] [--pause-after N]
//!   subtitle-font-coverage verify  [--dir DIR]

mod clocks;
mod coverage;
mod evidence;
mod extract;
mod gui;
mod model;
mod pipeline;
mod report;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use anyhow::{Context, Result};

use pipeline::{Pipeline, ResourceBudget, RunOutcome};

fn main() {
    std::process::exit(real_main());
}

fn real_main() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args[0] == "gui" {
        if let Err(e) = extract::init() {
            eprintln!("GStreamer init failed: {e:#}");
            return 1;
        }
        return gui::run_gui();
    }
    match run_cli(&args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            1
        }
    }
}

fn opt_value(args: &[String], name: &str) -> Option<String> {
    args.windows(2)
        .find(|w| w[0] == name)
        .map(|w| w[1].clone())
}

fn run_cli(args: &[String]) -> Result<i32> {
    extract::init()?;
    let cmd = args[0].as_str();
    match cmd {
        "analyze" | "resume" => {
            let object = args
                .get(1)
                .map(PathBuf::from)
                .context("usage: {analyze|resume} <media-object> [--out DIR] [--pause-after N]")?;
            let out = opt_value(args, "--out")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("sfc-output"));
            let pause_after = opt_value(args, "--pause-after").and_then(|v| v.parse().ok());
            pipeline::ensure_same_object(&out, &object)?;
            let budget = ResourceBudget::default();
            let flag = Arc::new(AtomicBool::new(false));
            let mut pipe = if cmd == "analyze" {
                Pipeline::new(&object, &out, budget, flag, pause_after)?
            } else {
                Pipeline::resume(&object, &out, budget, flag, pause_after)?
            };
            let outcome = pipe.run(|p| {
                eprintln!("[{}/{}] {}", p.processed, p.total, p.message);
            })?;
            match outcome {
                RunOutcome::Completed => {
                    println!("completed; report at {}", out.join("report.json").display());
                    if let Some(cmp) = pipe.resume_comparison() {
                        println!(
                            "resume comparison: window={} identical={}",
                            cmp.window_size, cmp.identical
                        );
                    }
                }
                RunOutcome::Paused => {
                    println!(
                        "paused; checkpoint at {} — resume with: resume {} --out {}",
                        out.join("checkpoint.json").display(),
                        object.display(),
                        out.display()
                    );
                }
            }
            Ok(0)
        }
        "verify" => {
            let dir = opt_value(args, "--dir")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("sfc-output"));
            let outcome = report::verify(&dir)?;
            for check in &outcome.checks {
                println!("ok: {check}");
            }
            println!("verification passed ({} checks)", outcome.checks.len());
            Ok(0)
        }
        other => {
            anyhow::bail!("unknown command {other:?}; expected analyze|resume|verify|gui")
        }
    }
}
