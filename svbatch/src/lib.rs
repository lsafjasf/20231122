//! Replayable batch sound-effect variant experiment core.
//!
//! Fixed stack: Rust + GStreamer + GTK4. This crate is UI-free so the same
//! engine drives both the headless CLI (`svcli`) and the GTK4 panel (`svgui`).

pub mod audio;
pub mod batch;
pub mod model;
pub mod scenarios;

pub use audio::{init_gstreamer, CancelToken, Engine};
pub use batch::{run_batch, write_report, BatchConfig, SubmitOutcome, SubmissionRegistry};
pub use model::{
    BatchReport, BatchSnapshot, JobResult, JobSpec, JobState, SourceSpec, StepRecord, VariantSpec,
};
pub use scenarios::{canonical_jobs, duplicate_submission_jobs, reordered_jobs, sources, variants};
