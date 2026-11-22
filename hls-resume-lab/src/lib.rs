//! Replayable HLS resume-point diagnosis lab.
//!
//! The crate is split into a pure [`core`] engine (deterministic, no I/O
//! side effects beyond the journal), a [`gst_backend`] adapter around
//! GStreamer and an optional GTK4 front end.

pub mod cleanup;
pub mod core;
pub mod gst_backend;

#[cfg(feature = "gtk")]
pub mod gtk_app;

pub use core::{
    engine::Engine,
    journal::{Journal, JournalEntry},
    model::{DiagnosisResult, ResumeRecord, Snapshot, SnapshotParseError},
    runner::{RunConfig, RunOutcome},
};
