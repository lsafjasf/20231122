//! Pausable segment content fingerprint pipeline.
//!
//! Crate layout:
//! * [`model`] — stable, field-ordered JSON schemas for every artifact;
//! * [`decode`] — GStreamer `uridecodebin`/`appsink` decoding with WAV fallback;
//! * [`fingerprint`] — deterministic `band-energy-v1` audio fingerprint;
//! * [`alignment`] — cross-device clock offset estimation;
//! * [`compare`] — per-segment similarity, coverage and missing semantics;
//! * [`pipeline`] — run / pause / checkpoint / resume orchestration;
//! * [`resources`] — RSS and disk pressure handling;
//! * [`store`] — atomic checkpoint writes and evidence bundle;
//! * [`verify`] — independent replay and deep re-decode verification.

pub mod alignment;
pub mod compare;
pub mod decode;
pub mod digest;
pub mod fingerprint;
pub mod model;
pub mod pipeline;
pub mod resources;
pub mod store;
pub mod verify;

pub use model::FingerprintFrame;
