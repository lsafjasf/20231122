use anyhow::{Context, Result};
use serde_json::json;

use crate::scenario::{Experiment, ScenarioReport, persist_run};
pub use crate::scenario::{ExperimentBundle, ExperimentRun};

pub fn run_builtin() -> Result<ExperimentRun> {
    let mut run = Experiment::run_builtin()?;
    let snapshot: crate::model::Snapshot = serde_json::from_str(crate::scenario::V2_SNAPSHOT_JSON)
        .context("parse v2 snapshot for GStreamer probe")?;
    let report = crate::gst_probe::probe_roundtrip(&snapshot.frames);
    let plugins_available = report.appsrc_available && report.fakesink_available;
    run.bundle.checks.push(crate::scenario::Check {
        id: "gstreamer-pipeline-integrity".to_string(),
        passed: report.initialized && (!plugins_available || report.passed()),
        expected: json!({
            "initialized": true,
            "roundtrip_if_plugins_available": true,
        }),
        actual: json!({
            "initialized": report.initialized,
            "version": report.version,
            "pipeline": report.pipeline,
            "plugins_available": plugins_available,
            "observed_frames": report.observed_frames,
            "expected_frames": report.expected_frames,
            "timestamps_preserved": report.timestamps_preserved,
            "roundtrip_passed": report.passed(),
            "error": report.error,
        }),
        note: "GStreamer initialized and, when app/coreelements are present, the appsrc ! fakesink probe verified PTS/DTS passthrough".to_string(),
    });
    run.bundle.gstreamer =
        Some(serde_json::to_value(&report).context("serialize GStreamer report")?);
    Ok(run)
}

pub fn write_evidence(run: &ExperimentRun, output_dir: &std::path::Path) -> Result<()> {
    persist_run(run, output_dir)
}

pub fn scenario<'a>(run: &'a ExperimentRun, id: &str) -> Option<&'a ScenarioReport> {
    run.bundle
        .scenarios
        .iter()
        .find(|scenario| scenario.id == id)
}
