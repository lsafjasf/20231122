//! `audit-core` is the pure, headless rule engine for the HDR luminance
//! histogram source-check page.
//!
//! It owns the provenance model, the deterministic processing pipeline, the
//! truncation/recovery replay and the audit report. No GStreamer or GTK types
//! appear here, which keeps the audit logic unit-testable on a CI machine
//! without a display server.

pub mod model;
pub mod pipeline;
pub mod replay;
pub mod report;
pub mod scenario;

pub use model::{
    Evidence, Histogram, ProcessingStep, RawSample, SampleRecord, StepState, Verdict,
};
pub use pipeline::{run, PipelineConfig, PipelineRun};
pub use replay::{
    capture_truncation, recover_and_replay, ReplayMismatch, ReplayReport, TruncationRecord,
};
pub use report::{build_report, AuditReport, FirstError, ScoreCard};
