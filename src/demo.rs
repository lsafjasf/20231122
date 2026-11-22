use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::engine::{compare_runs, run, RunOptions, RunOutcome};
use crate::model::{format_timecode, Config};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DemoReport {
    pub continuous_dir: String,
    pub pausable_dir: String,
    pub pause_frame: u64,
    pub before_timecode: String,
    pub after_timecode: String,
    pub compare_report: String,
    pub identical: bool,
    pub verdict: String,
}

pub fn run_demo(out: &Path, pause_frame: u64) -> Result<DemoReport> {
    let config = Config::default();
    config.validate()?;
    if pause_frame == 0 || pause_frame >= config.frame_count {
        anyhow::bail!("pause frame must be in 1..{}", config.frame_count);
    }
    let continuous = out.join("continuous");
    let pausable = out.join("pausable");
    let compare_file = out.join("compare").join("compare_report.json");

    let continuous_outcome = run(
        &config,
        &continuous,
        RunOptions {
            resume: false,
            pause_at_frame: None,
            gst_probe: true,
        },
    )?;
    ensure_completed(continuous_outcome, "continuous run")?;

    let first = run(
        &config,
        &pausable,
        RunOptions {
            resume: false,
            pause_at_frame: Some(pause_frame),
            gst_probe: true,
        },
    )?;
    if first != RunOutcome::Paused {
        anyhow::bail!("pausable run did not pause at frame {pause_frame}");
    }
    let second = run(
        &config,
        &pausable,
        RunOptions {
            resume: true,
            pause_at_frame: None,
            gst_probe: true,
        },
    )?;
    ensure_completed(second, "resumed run")?;

    let report = compare_runs(&continuous, &pausable, &compare_file)?;
    let before_pts = config.nominal_pts_ns(config.boundary_before_idx());
    let after_pts = config.nominal_pts_ns(config.boundary_after_idx());
    let summary = DemoReport {
        continuous_dir: continuous.display().to_string(),
        pausable_dir: pausable.display().to_string(),
        pause_frame,
        before_timecode: format_timecode(before_pts, config.fps),
        after_timecode: format_timecode(after_pts, config.fps),
        compare_report: compare_file.display().to_string(),
        identical: report.identical_sequence,
        verdict: report.verdict,
    };
    crate::evidence::write_json(&out.join("summary.json"), &summary)?;
    Ok(summary)
}

fn ensure_completed(outcome: RunOutcome, label: &str) -> Result<()> {
    if outcome == RunOutcome::Completed {
        Ok(())
    } else {
        Err(anyhow::anyhow!("{label} unexpectedly paused"))
    }
}

pub fn default_output() -> PathBuf {
    PathBuf::from("artifacts/evidence")
}

pub fn load_or_default(config_path: Option<&Path>) -> Result<Config> {
    match config_path {
        Some(path) => crate::model::load_config(path)
            .with_context(|| format!("load config {}", path.display())),
        None => Ok(Config::default()),
    }
}
