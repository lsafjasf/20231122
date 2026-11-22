//! Persisted domain model.
//!
//! Field declaration order is the on-disk JSON key order (serde serializes
//! struct fields in declaration order and `preserve_order` keeps maps ordered
//! on reload), so a reloaded artifact keeps identical fields and ordering.

pub mod ids;
pub mod project;
