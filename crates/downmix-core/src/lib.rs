//! Reviewable surround-sound object downmix.
//!
//! Every input object is rendered into two revision snapshots (legacy recipe
//! with a legacy manifest envelope, current recipe with a versioned envelope).
//! Each snapshot ties timestamps, frame indices, processing parameters and
//! output waveform evidence together so a reviewer can re-verify the run.

pub mod discover;
pub mod error;
pub mod hash;
pub mod manifest;
pub mod model;
pub mod pipeline;
pub mod recipe;
pub mod run;
pub mod verify;

pub use error::{DownmixError, Result};
pub use model::{AudioObject, CorrelationPoint, ProcessingParams, Revision, WaveformSummary};
pub use run::{run_batch, ObjectResult, RunConfig, RunSummary};
pub use verify::{clean_temp_state, verify_evidence, VerificationReport};
