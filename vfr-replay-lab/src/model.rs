//! Domain model for the variable-frame-rate (VFR) clip preview lab.
//!
//! The model is intentionally plain serializable data so every input, state
//! and result can be recorded to the journal and replayed byte-for-byte.

use serde::{Deserialize, Serialize};

/// A source clip. Frame timestamps are expressed in nanoseconds; a VFR clip is
/// one whose consecutive frame deltas are not constant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Clip {
    pub id: String,
    pub uri: String,
    /// PTS of every frame in nanoseconds; length defines the frame count.
    pub pts_ns: Vec<u64>,
}

impl Clip {
    /// Frame duration deltas; empty for clips shorter than two frames.
    pub fn frame_deltas_ns(&self) -> Vec<u64> {
        self.pts_ns.windows(2).map(|w| w[1] - w[0]).collect()
    }

    /// A clip is variable frame rate when at least two deltas differ.
    pub fn is_vfr(&self) -> bool {
        let deltas = self.frame_deltas_ns();
        deltas.windows(2).any(|w| w[0] != w[1])
    }
}

/// Commands accepted by the preview engine. These are the *inputs* recorded in
/// the journal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    /// Register a clip in the library.
    AddClip { clip: Clip },
    /// Place (or move) a clip onto the preview timeline at `position`.
    Place { clip_id: String, position: usize },
    /// Remove a clip from the timeline.
    Remove { clip_id: String },
    /// Request an output frame rate (frames per second, numerator/denominator).
    SetRate { fps_num: u32, fps_den: u32 },
    /// Reorder two timeline positions by swapping their contents.
    Swap {
        position_a: usize,
        position_b: usize,
    },
    /// Request an engine/extension capability not known to the core.
    /// This is the *unknown extension* control input.
    UnknownExtension { name: String, payload: String },
}

/// The result of applying one command — the recorded *output*.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Outcome {
    Accepted {
        detail: String,
    },
    Rejected {
        reason: String,
    },
    /// The command's idempotency key had already been applied.
    Duplicate {
        original_seq: u64,
    },
    /// The core does not understand the extension. The raw input is preserved.
    UnknownIgnored {
        name: String,
    },
}

/// Full engine state. Every field is deterministic given the command history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct EngineState {
    /// clip-id -> clip, in insertion order.
    pub library: Vec<Clip>,
    /// Timeline positions; entries hold clip ids (`None` = empty slot).
    pub timeline: Vec<Option<String>>,
    pub fps_num: u32,
    pub fps_den: u32,
    /// Ids of clips that have been placed at least once.
    pub placed: Vec<String>,
    /// Monotonic counter of applied commands.
    pub revision: u64,
}

impl EngineState {
    pub fn new() -> Self {
        Self {
            fps_num: 30,
            fps_den: 1,
            ..Default::default()
        }
    }

    pub fn find_clip(&self, clip_id: &str) -> Option<&Clip> {
        self.library.iter().find(|c| c.id == clip_id)
    }

    pub fn timeline_clip_ids(&self) -> Vec<Option<String>> {
        self.timeline.clone()
    }

    /// Effective output frame rate as a float, used by the GStreamer layer.
    pub fn fps(&self) -> f64 {
        if self.fps_den == 0 {
            0.0
        } else {
            self.fps_num as f64 / self.fps_den as f64
        }
    }
}
