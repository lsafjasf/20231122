use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const STATE_FILE: &str = "state.json";
pub const ANCHORS: [&str; 3] = ["A1_DECODE", "A2_ANALYZE", "A3_RENDER"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AnchorStatus {
    Pending,
    Done,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnchorRecord {
    pub name: String,
    pub status: AnchorStatus,
    pub artifact: Option<String>,
    pub sha256: Option<String>,
    /// Raw offset domain: sample index and byte offset of the artifact payload.
    pub samples: Option<u64>,
    pub byte_offset: Option<u64>,
    /// Wall-clock completion time, RFC3339 UTC.
    pub completed_at: Option<String>,
    /// Pipeline time domain: nanoseconds since stream start.
    pub stream_position_ns: Option<u64>,
}

impl AnchorRecord {
    pub fn pending(name: &str) -> Self {
        Self {
            name: name.to_string(),
            status: AnchorStatus::Pending,
            artifact: None,
            sha256: None,
            samples: None,
            byte_offset: None,
            completed_at: None,
            stream_position_ns: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputInfo {
    pub path: PathBuf,
    pub sha256: String,
    pub pristine_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Degradation {
    pub mode: String, // "normal" | "degraded"
    pub reasons: Vec<String>,
    pub mem_available_mb: u64,
    pub disk_available_mb: u64,
    pub crossfade_samples: u64,
    pub analysis_stride: u64,
    pub chunk_samples: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopPlan {
    pub sample_rate: u32,
    pub total_samples: u64,
    pub loop_start_sample: u64,
    pub loop_end_sample: u64,
    pub crossfade_samples: u64,
    pub seam_error_max: f64,
    pub loop_start_seconds: f64,
    pub loop_end_seconds: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunState {
    pub version: u32,
    pub run_id: String,
    pub created_at: String,
    pub input: InputInfo,
    pub anchors: Vec<AnchorRecord>,
    pub degradation: Option<Degradation>,
    pub plan: Option<LoopPlan>,
    pub next_anchor: usize, // index into ANCHORS; 3 == finished
}

impl RunState {
    pub fn new(run_id: String, input: InputInfo) -> Self {
        Self {
            version: 1,
            run_id,
            created_at: crate::evidence::now_utc(),
            input,
            anchors: ANCHORS.iter().map(|a| AnchorRecord::pending(a)).collect(),
            degradation: None,
            plan: None,
            next_anchor: 0,
        }
    }

    pub fn load(run_dir: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(run_dir.join(STATE_FILE))?;
        Ok(serde_json::from_str(&text)?)
    }

    pub fn save(&self, run_dir: &Path) -> anyhow::Result<()> {
        let tmp = run_dir.join("state.json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        std::fs::rename(&tmp, run_dir.join(STATE_FILE))?;
        Ok(())
    }

    pub fn anchor_done(&self, idx: usize) -> bool {
        self.anchors
            .get(idx)
            .map(|a| a.status == AnchorStatus::Done)
            .unwrap_or(false)
    }
}
