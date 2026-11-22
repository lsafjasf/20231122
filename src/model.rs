use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;

pub const SOURCE_WIDTH: u32 = 320;
pub const SOURCE_HEIGHT: u32 = 240;
pub const SOURCE_RGB_BPP: u64 = 3;
pub const SOURCE_DURATION_MS: u64 = 6_000;
pub const DEFAULT_MEM_BUDGET: u64 = 100_000;
pub const DEFAULT_PAUSE_AFTER: usize = 8;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SourceInfo {
    pub path: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub width: u32,
    pub height: u32,
    pub duration_ms: u64,
    pub frame_rate: String,
}

impl SourceInfo {
    pub fn from_path(path: &Path) -> anyhow::Result<Self> {
        let bytes = std::fs::read(path)?;
        Ok(Self {
            path: path.display().to_string(),
            sha256: sha256_hex(&bytes),
            size_bytes: bytes.len() as u64,
            width: SOURCE_WIDTH,
            height: SOURCE_HEIGHT,
            duration_ms: SOURCE_DURATION_MS,
            frame_rate: "10/1".to_string(),
        })
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ExtremeParams {
    pub requested_scale_percent: u8,
    pub memory_budget_bytes: u64,
    pub seek_timeout_ms: u64,
    pub final_seek_timeout_ms: u64,
    pub pause_after: usize,
    pub repeat_count: usize,
}

impl ExtremeParams {
    pub fn demo() -> Self {
        Self {
            requested_scale_percent: 100,
            memory_budget_bytes: DEFAULT_MEM_BUDGET,
            seek_timeout_ms: 5_000,
            final_seek_timeout_ms: u64::MAX,
            pause_after: DEFAULT_PAUSE_AFTER,
            repeat_count: 5,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SampleSpec {
    pub seq: usize,
    pub timestamp_ms: u64,
    pub x: u32,
    pub y: u32,
    pub raw_offset: u64,
    pub group_index: usize,
    pub point_index: usize,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SamplingPlan {
    pub version: u32,
    pub source: SourceInfo,
    pub params: ExtremeParams,
    pub samples: Vec<SampleSpec>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SampleResult {
    pub seq: usize,
    pub timestamp_ms: u64,
    pub landed_timestamp_ms: u64,
    pub x: u32,
    pub y: u32,
    pub raw_offset: u64,
    pub effective_x: u32,
    pub effective_y: u32,
    pub scale_percent: u8,
    pub effective_offset: u64,
    pub rgb: [u8; 3],
    pub frame_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub degraded_reason: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Degradation {
    pub requested_scale_percent: u8,
    pub effective_scale_percent: u8,
    pub effective_width: u32,
    pub effective_height: u32,
    pub frame_buffer_bytes: u64,
    pub memory_budget_bytes: u64,
    pub budget_satisfied: bool,
    pub reason: String,
    pub decoder: String,
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    hex(&digest)
}

pub fn sha256_file(path: &Path) -> anyhow::Result<String> {
    Ok(sha256_hex(&std::fs::read(path)?))
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}
