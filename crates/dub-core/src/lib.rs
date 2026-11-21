//! dub-core: pausable dubbing-language-switch pipeline.
//!
//! A [`model::DubJob`] is the single repeated object the whole flow is
//! organised around: it is created once, processed segment by segment,
//! checkpointed on pause, resumed (possibly many times) and finally
//! exported for independent review.
//!
//! Two unsynchronised device clocks (see [`clocks`]) timestamp every
//! evidence record; [`evidence`] keeps a hash-chained append-only log so a
//! colleague can re-verify the run with `dub-cli verify` alone.

pub mod clocks;
pub mod degrade;
pub mod evidence;
pub mod media;
pub mod model;
pub mod pipeline;
pub mod report;
pub mod state;

pub use model::{DubJob, JobState, SegmentResult, SegmentSpec};
