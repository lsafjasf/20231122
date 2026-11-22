mod gui;

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use timecode_drift_replay::demo::{default_output, load_or_default, run_demo};
use timecode_drift_replay::engine::{compare_runs, run, RunOptions, RunOutcome};

fn main() {
    if let Err(error) = real_main() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

fn real_main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = args.first().map(String::as_str) else {
        print_help();
        return Ok(());
    };
    match command {
        "demo" => {
            let out = value(&args, "--out")
                .map(PathBuf::from)
                .unwrap_or_else(default_output);
            let pause_at = value(&args, "--pause-at")
                .map(|value| value.parse::<u64>())
                .transpose()
                .context("--pause-at expects a frame number")?
                .unwrap_or_else(|| {
                    timecode_drift_replay::model::Config::default().gap_start_frame()
                });
            let report = run_demo(&out, pause_at)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            if !report.identical {
                bail!("pause/resume comparison failed");
            }
        }
        "run" => {
            let out = required_path(&args, "--out")?;
            let config_path = value(&args, "--config");
            let config = load_or_default(config_path.as_deref().map(Path::new))?;
            let pause_at = value(&args, "--pause-at")
                .map(|value| value.parse::<u64>())
                .transpose()
                .context("--pause-at expects a frame number")?;
            let outcome = run(
                &config,
                &out,
                RunOptions {
                    resume: false,
                    pause_at_frame: pause_at,
                    gst_probe: true,
                },
            )?;
            println!("{}: {}", out.display(), outcome_label(outcome));
        }
        "resume" => {
            let out = required_path(&args, "--out")?;
            let config_path = value(&args, "--config");
            let config = load_or_default(config_path.as_deref().map(Path::new))?;
            let outcome = run(
                &config,
                &out,
                RunOptions {
                    resume: true,
                    pause_at_frame: None,
                    gst_probe: true,
                },
            )?;
            println!("{}: {}", out.display(), outcome_label(outcome));
        }
        "compare" => {
            let run_a = required_path(&args, "--a")?;
            let run_b = required_path(&args, "--b")?;
            let report_path = value(&args, "--report")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("compare_report.json"));
            let report = compare_runs(&run_a, &run_b, &report_path)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            if !report.identical_sequence {
                bail!("runs differ; see {}", report_path.display());
            }
        }
        "gst-probe" => {
            let out = value(&args, "--out")
                .map(PathBuf::from)
                .unwrap_or_else(default_output);
            let config_path = value(&args, "--config");
            let config = load_or_default(config_path.as_deref().map(Path::new))?;
            let probe = timecode_drift_replay::gst_bridge::probe(&config, &out)?;
            println!("{}", serde_json::to_string_pretty(&probe)?);
            if !probe.crc_match {
                bail!("GStreamer output CRC differs from input");
            }
        }
        "gui" => std::process::exit(gui::run_gui().value()),
        "help" | "--help" | "-h" => print_help(),
        other => {
            print_help();
            bail!("unknown command {other}");
        }
    }
    Ok(())
}

fn value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == flag)
        .and_then(|index| args.get(index + 1))
        .cloned()
}

fn required_path(args: &[String], flag: &str) -> Result<PathBuf> {
    value(args, flag)
        .map(PathBuf::from)
        .with_context(|| format!("missing required {flag} <path>"))
}

fn outcome_label(outcome: RunOutcome) -> &'static str {
    match outcome {
        RunOutcome::Completed => "completed",
        RunOutcome::Paused => "paused, checkpoint saved",
    }
}

fn print_help() {
    println!(
        "timecode-drift-replay\n\
         \n\
         Commands:\n\
           demo [--out DIR] [--pause-at FRAME]\n\
           run --out DIR [--pause-at FRAME] [--config FILE]\n\
           resume --out DIR [--config FILE]\n\
           compare --a RUN_DIR --b RUN_DIR --report FILE\n\
           gst-probe [--out DIR] [--config FILE]\n\
           gui\n\
         \n\
         Defaults: 30 fps, 300 frames, missing span 4.0s..5.5s,\n\
         jitter +/-20ms, queue 2 buffers/65536 bytes, pool 409600 bytes,\n\
         degraded keep divisor 4."
    );
}
