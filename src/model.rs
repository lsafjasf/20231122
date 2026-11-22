use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const METADATA_RECORD_BYTES: usize = 64;
pub const METADATA_MAGIC: &[u8; 8] = b"FHDRM001";
pub const FRAME_RATE: (u32, u32) = (30, 1);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExtremeProfile {
    pub id: String,
    pub max_cll: u16,
    pub max_fall: u16,
    pub peak_nits: u16,
    pub avg_nits: u16,
    pub white_point: [u16; 2],
    pub mastering_max_nits: u16,
    pub mastering_min_nits: u16,
}

impl ExtremeProfile {
    pub fn camera_fault() -> Self {
        Self {
            id: "XTREME-65535".into(),
            max_cll: u16::MAX,
            max_fall: u16::MAX,
            peak_nits: u16::MAX,
            avg_nits: u16::MAX,
            white_point: [0, 0],
            mastering_max_nits: u16::MAX,
            mastering_min_nits: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepeatedObject {
    pub id: String,
    pub rect: Rect,
    pub peak_nits: u16,
    pub threshold_nits: u16,
    pub frames: Vec<u32>,
}

impl RepeatedObject {
    pub fn frame_offsets(&self, frame_bytes: u64) -> Vec<u64> {
        self.frames
            .iter()
            .map(|frame| u64::from(*frame) * frame_bytes)
            .collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MasteringDisplay {
    pub max_nits: u16,
    pub min_nits: u16,
    pub white_point: [u16; 2],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraProfile {
    pub schema_version: u32,
    pub camera_model: String,
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    pub frame_count: u32,
    pub video_format: String,
    pub source_video_sha256: String,
    pub fault_profile: ExtremeProfile,
    pub repeated_object: RepeatedObject,
    pub repaired_mastering: MasteringDisplay,
}

impl CameraProfile {
    pub fn frame_bytes(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height) * 2
    }

    pub fn duration_ns(&self) -> u64 {
        if self.fps_num == 0 {
            return 0;
        }
        let frames = u64::from(self.frame_count);
        frames
            .saturating_mul(u64::from(self.fps_den))
            .saturating_mul(1_000_000_000)
            / u64::from(self.fps_num)
    }

    pub fn pts_ns(&self, frame: u32) -> u64 {
        if self.fps_num == 0 {
            return 0;
        }
        u64::from(frame)
            .saturating_mul(u64::from(self.fps_den))
            .saturating_mul(1_000_000_000)
            / u64::from(self.fps_num)
    }

    pub fn video_offset(&self, frame: u32) -> u64 {
        u64::from(frame).saturating_mul(self.frame_bytes())
    }

    pub fn metadata_offset(&self, frame: u32) -> u64 {
        u64::from(frame).saturating_mul(METADATA_RECORD_BYTES as u64)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(self.width > 0 && self.height > 0, "invalid frame size");
        anyhow::ensure!(self.fps_num > 0 && self.fps_den > 0, "invalid frame rate");
        anyhow::ensure!(self.frame_count > 0, "empty camera clip");
        anyhow::ensure!(
            self.video_format.eq_ignore_ascii_case("GRAY16_LE"),
            "unsupported raw format: {}",
            self.video_format
        );
        let all_frames = (0..self.frame_count).collect::<Vec<_>>();
        anyhow::ensure!(
            self.repeated_object
                .frames
                .iter()
                .all(|f| all_frames.contains(f)),
            "repeated object lies outside the clip"
        );
        anyhow::ensure!(
            self.repeated_object.rect.x + self.repeated_object.rect.width <= self.width
                && self.repeated_object.rect.y + self.repeated_object.rect.height <= self.height,
            "repeated object lies outside the frame"
        );
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AnalysisQuality {
    Full,
    Sampled { pixel_step: u32 },
    Blocked,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourcePlan {
    pub quality: AnalysisQuality,
    pub chunk_frames: u32,
    pub pixel_step: u32,
    pub estimated_bytes: u64,
    pub budget_bytes: u64,
    pub degraded_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepairConfig {
    pub profile: CameraProfile,
    pub chunk_frames: u32,
    pub memory_budget_bytes: u64,
    pub pause_before_frame: Option<u32>,
    pub acceptance_frame: u32,
}

impl RepairConfig {
    pub fn demo(profile: CameraProfile) -> Self {
        let acceptance_frame = profile
            .repeated_object
            .frames
            .get(1)
            .copied()
            .unwrap_or(profile.frame_count.saturating_sub(1) / 2);
        Self {
            profile,
            chunk_frames: 8,
            memory_budget_bytes: 1024 * 1024,
            pause_before_frame: None,
            acceptance_frame,
        }
    }

    pub fn acceptance_pts_ns(&self) -> u64 {
        self.profile.pts_ns(self.acceptance_frame)
    }

    pub fn acceptance_video_offset(&self) -> u64 {
        self.profile.video_offset(self.acceptance_frame)
    }

    pub fn acceptance_metadata_offset(&self) -> u64 {
        self.profile.metadata_offset(self.acceptance_frame)
    }
}

#[derive(Debug, Clone)]
pub struct SessionPaths {
    pub root: PathBuf,
    pub original_dir: PathBuf,
    pub output_dir: PathBuf,
    pub evidence_dir: PathBuf,
    pub state_dir: PathBuf,
    pub manifest: PathBuf,
    pub checkpoint: PathBuf,
    pub evidence: PathBuf,
    pub acceptance: PathBuf,
    pub video: PathBuf,
    pub metadata: PathBuf,
    pub repaired_metadata: PathBuf,
}

impl SessionPaths {
    pub fn new(root: PathBuf) -> Self {
        let original_dir = root.join("original");
        let output_dir = root.join("output");
        let evidence_dir = root.join("evidence");
        let state_dir = root.join("state");
        Self {
            manifest: root.join("manifest.json"),
            checkpoint: state_dir.join("checkpoint.json"),
            evidence: evidence_dir.join("events.jsonl"),
            acceptance: root.join("acceptance.json"),
            video: original_dir.join("video.gray16"),
            metadata: original_dir.join("metadata.bin"),
            repaired_metadata: output_dir.join("metadata.repaired.bin"),
            root,
            original_dir,
            output_dir,
            evidence_dir,
            state_dir,
        }
    }
}
