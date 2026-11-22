//! Headless helpers shared by the UI and its smoke test.
//!
//! Keeping the data assembly outside GTK widgets means the same provenance
//! data the page renders can be verified on a machine without a display and
//! exported as evidence.

use audit_core::model::RawSample;
use audit_core::replay::{capture_truncation, recover_and_replay, ReplayReport, TruncationRecord};
use audit_core::report::AuditReport;
use audit_core::{build_report, run, PipelineConfig, PipelineRun};

/// All audited artifacts for one demonstration cycle.
#[derive(Debug, serde::Serialize)]
pub struct HeadlessArtifacts {
    pub config: PipelineConfig,
    pub run: PipelineRun,
    pub report: AuditReport,
    pub truncation: TruncationRecord,
    pub replay: ReplayReport,
}

/// Runs the full source-check scenario: audit, truncation, recovery replay.
pub fn assemble(samples: Vec<RawSample>, config: PipelineConfig) -> HeadlessArtifacts {
    let pipeline_run = run(&samples, &config);
    let report = build_report(&pipeline_run);
    let completed = pipeline_run.records[..3].to_vec();
    let truncation = capture_truncation("caps 变更触发 flush", &samples, completed, &config);
    let replay = recover_and_replay(&truncation);
    HeadlessArtifacts { config, run: pipeline_run, report, truncation, replay }
}
