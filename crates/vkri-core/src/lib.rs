//! vkri-core: domain model, durable storage, evidence handling, GStreamer
//! extraction, pairwise comparison and self-verifying exports for the
//! Video Keyframe Rebuild Index ("vkri") local correction tool.

pub mod audit;
pub mod compare;
pub mod error;
pub mod export;
pub mod extract;
pub mod hash;
pub mod media;
pub mod model;
pub mod service;
pub mod storage;
pub mod time;

pub use error::{Error, Result};
pub use model::project::{
    AppInfo, Comparison, ComparisonEntry, Device, ExtractionConfig, Keyframe, MediaSegment,
    Project, RunRecord, SegmentStatus,
};
pub use service::ProjectService;
pub use storage::ProjectStore;
