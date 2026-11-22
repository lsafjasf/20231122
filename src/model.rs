use std::fmt;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

pub const WIDTH: u32 = 160;
pub const HEIGHT: u32 = 120;
pub const FRAME_BYTES: usize = (WIDTH * HEIGHT * 3) as usize;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Gap {
    pub start_ns: u64,
    pub end_ns: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Extreme {
    pub window_pad_ns: u64,
    pub jitter_ns: u64,
    pub drift_tolerance_ns: u64,
    pub queue_max_buffers: u32,
    pub queue_max_bytes: u64,
    pub degrade_keep_divisor: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub fps: u32,
    pub frame_count: u64,
    pub gap: Gap,
    pub extreme: Extreme,
    pub max_pool_bytes: u64,
    pub pause_at_frame: Option<u64>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            fps: 30,
            frame_count: 300,
            gap: Gap {
                start_ns: 4_000_000_000,
                end_ns: 5_500_000_000,
            },
            extreme: Extreme {
                window_pad_ns: 1_000_000_000,
                jitter_ns: 20_000_000,
                drift_tolerance_ns: 100_000_000,
                queue_max_buffers: 2,
                queue_max_bytes: 65_536,
                degrade_keep_divisor: 4,
            },
            max_pool_bytes: 409_600,
            pause_at_frame: None,
        }
    }
}

impl Config {
    pub fn frame_period_ns(&self) -> u64 {
        1_000_000_000 / u64::from(self.fps)
    }

    pub fn nominal_pts_ns(&self, frame_idx: u64) -> u64 {
        frame_idx.saturating_mul(self.frame_period_ns())
    }

    pub fn gap_start_frame(&self) -> u64 {
        self.gap.start_ns / self.frame_period_ns()
    }

    pub fn gap_end_frame(&self) -> u64 {
        self.gap.end_ns / self.frame_period_ns()
    }

    pub fn extreme_start_frame(&self) -> u64 {
        self.gap_start_frame()
            .saturating_sub(self.extreme.window_pad_ns / self.frame_period_ns())
    }

    pub fn extreme_end_frame(&self) -> u64 {
        self.gap_end_frame()
            .saturating_add(self.extreme.window_pad_ns / self.frame_period_ns())
    }

    /// The missing segment uses an open start and closed end. This keeps the
    /// frame immediately before the discontinuity available as boundary
    /// evidence while excluding every frame in the declared missing span.
    pub fn is_missing(&self, frame_idx: u64) -> bool {
        frame_idx > self.gap_start_frame() && frame_idx <= self.gap_end_frame()
    }

    pub fn in_extreme_window(&self, frame_idx: u64) -> bool {
        frame_idx > self.extreme_start_frame() && frame_idx <= self.extreme_end_frame()
    }

    pub fn validate(&self) -> Result<()> {
        if self.fps == 0 {
            bail!("fps must be greater than zero");
        }
        if self.frame_period_ns() == 0 {
            bail!("fps is too high for nanosecond timecodes");
        }
        if self.frame_count == 0 {
            bail!("frame_count must be greater than zero");
        }
        if self.gap.start_ns >= self.gap.end_ns {
            bail!("gap.start_ns must be before gap.end_ns");
        }
        if self.gap.end_ns > self.nominal_pts_ns(self.frame_count) {
            bail!("gap lies outside the configured source frames");
        }
        if self.extreme.jitter_ns > self.extreme.window_pad_ns {
            bail!("jitter must be smaller than the extreme-window padding");
        }
        if self.extreme.queue_max_buffers == 0 {
            bail!("queue_max_buffers must be greater than zero");
        }
        if self.extreme.queue_max_bytes < 4_096 {
            bail!("queue_max_bytes is too small to represent a frame");
        }
        if self.extreme.degrade_keep_divisor < 2 {
            bail!("degrade_keep_divisor must be at least two");
        }
        if self.max_pool_bytes < u64::from(self.extreme.queue_max_buffers) * 4_096 {
            bail!("max_pool_bytes cannot hold the configured queue minimum");
        }
        if self
            .pause_at_frame
            .is_some_and(|idx| idx >= self.frame_count)
        {
            bail!("pause_at_frame must be inside the source frame range");
        }
        Ok(())
    }

    /// Compact JSON in field declaration order; this is the parameter
    /// fingerprint used by checkpoints and resumed runs.
    pub fn params_hash(&self) -> u64 {
        let bytes = serde_json::to_vec(self).expect("Config serialization cannot fail");
        fnv1a64(&bytes)
    }

    pub fn boundary_before_idx(&self) -> u64 {
        self.gap_start_frame().saturating_sub(1)
    }

    pub fn boundary_after_idx(&self) -> u64 {
        self.gap_end_frame() + 1
    }
}

pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 14_695_981_039_346_656_037_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(1_099_511_628_211);
    }
    hash
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceFrame {
    pub idx: u64,
    pub nominal_pts_ns: u64,
    pub pts_ns: u64,
    pub jitter_ns: i64,
    pub in_extreme_window: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FrameRecord {
    pub idx: u64,
    pub pts_ns: u64,
    pub jitter_ns: i64,
    pub drift_ns: i64,
    pub crc: u32,
    pub degraded: bool,
    pub in_extreme_window: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Checkpoint {
    pub version: u32,
    pub next_frame_idx: u64,
    pub last_pts_ns: u64,
    pub processed: u64,
    pub dropped: u64,
    pub drift_events: u64,
    pub crc_chain: u32,
    pub pool_bytes: u64,
    pub degraded: bool,
    pub params_hash: u64,
    pub taken_at_unix: u64,
}

impl Checkpoint {
    pub fn new(config: &Config) -> Self {
        Self {
            version: 1,
            next_frame_idx: 0,
            last_pts_ns: 0,
            processed: 0,
            dropped: 0,
            drift_events: 0,
            crc_chain: 0,
            pool_bytes: 0,
            degraded: false,
            params_hash: config.params_hash(),
            taken_at_unix: 0,
        }
    }

    pub fn validate_for(&self, config: &Config) -> Result<()> {
        if self.version != 1 {
            bail!("unsupported checkpoint version {}", self.version);
        }
        if self.params_hash != config.params_hash() {
            bail!(
                "checkpoint parameter hash {:016x} does not match {:016x}",
                self.params_hash,
                config.params_hash()
            );
        }
        if self.next_frame_idx > config.frame_count {
            bail!(
                "checkpoint frame {} is outside source length {}",
                self.next_frame_idx,
                config.frame_count
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DegradationEvent {
    pub at_pts_ns: u64,
    pub kind: String,
    pub pool_bytes: u64,
    pub budget_bytes: u64,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BoundarySample {
    pub side: String,
    pub frame_idx: u64,
    pub pts_ns: u64,
    pub crc: u32,
    pub ppm_file: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BoundaryDelta {
    pub before: BoundarySample,
    pub after: BoundarySample,
    pub pts_delta_ns: u64,
    pub expected_step_ns: u64,
    pub missing_span_ns: u64,
    pub crc_changed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompareReport {
    pub run_a: String,
    pub run_b: String,
    pub frames_a: usize,
    pub frames_b: usize,
    pub identical_sequence: bool,
    pub first_mismatch: Option<u64>,
    pub crc_chain_a: u32,
    pub crc_chain_b: u32,
    pub verdict: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResumeCompare {
    pub checkpoint_next_frame: u64,
    pub existing_frames: usize,
    pub expected_frames_before_pause: u64,
    pub params_hash_match: bool,
    pub frame_records_match: bool,
    pub checkpoint_crc_chain: u32,
    pub replayed_crc_chain: u32,
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rejection: Option<String>,
}

impl fmt::Display for SourceFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "frame {} pts={}ns jitter={:+}ns",
            self.idx, self.pts_ns, self.jitter_ns
        )
    }
}

pub fn format_timecode(pts_ns: u64, fps: u32) -> String {
    let period = 1_000_000_000 / u64::from(fps.max(1));
    let frame = pts_ns / period;
    let sub_ns = pts_ns % period;
    let frames = frame % u64::from(fps.max(1));
    let seconds = frame / u64::from(fps.max(1));
    format!(
        "{:02}:{:02}:{:02}:{:02}.{:03}",
        seconds / 3600,
        (seconds / 60) % 60,
        seconds % 60,
        frames,
        sub_ns / 1_000_000
    )
}

pub fn load_config(path: &std::path::Path) -> Result<Config> {
    let raw = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let config: Config =
        serde_json::from_slice(&raw).with_context(|| format!("parse {}", path.display()))?;
    config.validate()?;
    Ok(config)
}
