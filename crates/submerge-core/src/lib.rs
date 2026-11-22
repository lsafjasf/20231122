//! submerge-core: merge multilingual subtitle tracks (zh/en/ja) from sources
//! running at 24/25/30 fps onto one unified nanosecond timeline, flag
//! overlaps, suspicious gaps and out-of-range cues, and support partial
//! reordering of cue ranges.

pub mod conflict;
pub mod demo;
pub mod event;
pub mod fmt;
pub mod fps;
pub mod timeline;

pub use conflict::{Conflict, ConflictKind};
pub use demo::demo_timeline;
pub use event::{Language, SubtitleEvent};
pub use fps::FrameRate;
pub use timeline::{Timeline, DEFAULT_GAP_THRESHOLD_NS};
