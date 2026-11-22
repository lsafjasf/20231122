//! GStreamer-backed keyframe extraction.
//!
//! Design:
//! * `filesrc ! decodebin3 ! videoconvertscale ! videorate` decodes each
//!   fragment independently so it can be re-run in isolation.
//! * `videorate` emits one buffer per config interval in interval mode, giving
//!   deterministic uniform sampling; keyframe-only mode bypasses the rate
//!   adjustment and keeps decoder keyframes reported via buffer flags.
//! * `appsink` hands buffers to Rust, where frames are scaled to thumbnail
//!   size and encoded to JPEG by a reusable `jpegenc` helper pipeline. Each
//!   JPEG is hashed (content address) and fsynced into the run directory.
//! * The [`CancellationToken`] supports mid-run exit: on cancellation the
//!   pipeline is torn down cleanly, partial evidence is kept, the segment is
//!   marked `interrupted`, and the audit trail records the stop point.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::model::ids::new_id;
#[allow(unused_imports)]
use crate::model::project::SegmentStatus as _SegmentStatus;
use crate::model::project::{
    ExtractionConfig, Keyframe, KeyframeMode, MediaSegment, RunRecord, SegmentStatus,
};
use crate::time::{pts_to_device_time, pts_to_reference_time, ClockOffset};

/// Progress reported after every decoded frame (cheap, callback based).
#[derive(Debug, Clone)]
pub struct Progress {
    pub frames_seen: u64,
    pub keyframes_written: u64,
    pub pts_ns: i64,
}

/// Thread-safe cancellation handle shared with the UI / CLI signal handler.
#[derive(Debug, Clone, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

/// Outcome of one extraction attempt.
#[derive(Debug, Clone)]
pub struct ExtractionOutcome {
    pub run_id: String,
    pub record: RunRecord,
    pub keyframes: Vec<Keyframe>,
    pub interrupted: bool,
}

/// Where thumbnails of one run are written.
#[derive(Debug, Clone)]
pub struct RunPaths {
    pub run_dir: PathBuf,
    pub manifest_path: PathBuf,
}

pub fn run_paths(evidence_root: &Path, segment_id: &str, run_id: &str) -> RunPaths {
    let run_dir = evidence_root.join(segment_id).join(format!("run-{run_id}"));
    RunPaths {
        manifest_path: run_dir.join("manifest.json"),
        run_dir,
    }
}

/// Run-local manifest written incrementally; its presence documents partial
/// runs and supports later forensics even if `project.json` were unavailable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunManifest {
    pub run_id: String,
    pub segment_id: String,
    pub source_sha256: String,
    pub config: ExtractionConfig,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub completed: bool,
    pub cancelled: bool,
    pub frames_seen: u64,
    pub keyframes: Vec<Keyframe>,
}

/// Decision sink shared by both extraction modes.
#[derive(Debug)]
struct FrameSelector {
    mode: KeyframeMode,
    interval_ns: i64,
    last_kept_pts: Option<i64>,
}

impl FrameSelector {
    fn new(config: &ExtractionConfig) -> Self {
        Self {
            mode: config.mode,
            interval_ns: config.interval_ns.max(1),
            last_kept_pts: None,
        }
    }

    fn should_keep(&mut self, pts_ns: i64, is_decoder_keyframe: bool) -> bool {
        match self.mode {
            KeyframeMode::KeyframeOnly => is_decoder_keyframe,
            KeyframeMode::Interval => match self.last_kept_pts {
                None => true,
                Some(previous) => pts_ns - previous >= self.interval_ns,
            },
        }
    }

    fn note_kept(&mut self, pts_ns: i64) {
        self.last_kept_pts = Some(pts_ns);
    }
}

/// Build the absolute reference time for a frame using the segment origin and
/// the owning device's clock offset.
#[allow(clippy::too_many_arguments)]
fn keyframe_from_buffer(
    index: u32,
    pts_ns: i64,
    is_decoder_keyframe: bool,
    thumbnail_sha256: String,
    thumbnail_file: String,
    thumbnail_bytes: u64,
    segment: &MediaSegment,
    offset: ClockOffset,
) -> Keyframe {
    let device_time = pts_to_device_time(segment.clock_origin_unix_ns, pts_ns);
    let reference_time = pts_to_reference_time(segment.clock_origin_unix_ns, pts_ns, offset);
    Keyframe {
        index,
        pts_ns,
        device_time_unix_ns: device_time,
        reference_time_unix_ns: reference_time,
        is_decoder_keyframe,
        thumbnail_sha256,
        thumbnail_file,
        thumbnail_bytes,
    }
}

pub use engine::extract_segment;

mod engine;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_selector_is_deterministic() {
        let config = ExtractionConfig {
            mode: KeyframeMode::Interval,
            interval_ns: 1_000,
            thumbnail_max_edge: 64,
            jpeg_quality: 80,
        };
        let mut selector = FrameSelector::new(&config);
        let mut kept = Vec::new();
        for pts in (0..5000).step_by(250) {
            if selector.should_keep(pts, true) {
                selector.note_kept(pts);
                kept.push(pts);
            }
        }
        assert_eq!(kept, vec![0, 1000, 2000, 3000, 4000]);
    }

    #[test]
    fn keyframe_only_selector_uses_flags() {
        let config = ExtractionConfig::default();
        let mut selector = FrameSelector::new(&ExtractionConfig {
            mode: KeyframeMode::KeyframeOnly,
            ..config
        });
        assert!(selector.should_keep(0, true));
        assert!(!selector.should_keep(100, false));
        assert!(selector.should_keep(200, true));
    }
}
