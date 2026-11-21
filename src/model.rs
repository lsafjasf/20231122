use serde::{Deserialize, Serialize};

/// Extreme/edge parameters attached to entries around the missing segment.
/// Values are deliberately allowed to be extreme (0, u64::MAX, ...); the
/// engine clamps them and records every clamp as evidence.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct EdgeParams {
    /// filesrc blocksize; 0 and u64::MAX are exercised as extremes.
    pub chunk_bytes: u64,
    /// Per-entry read budget in bytes; exceeding it forces the degraded path.
    pub max_entry_bytes: u64,
    /// Per-entry wall-clock budget; 0 = no budget at all, u64::MAX = unlimited.
    pub timeout_ms: u64,
    /// Expected duration in ns; -1 = unknown. Extreme values are kept as-is.
    pub expect_duration_ns: i64,
}

impl EdgeParams {
    pub fn normal() -> Self {
        Self {
            chunk_bytes: 64 * 1024,
            max_entry_bytes: 64 * 1024 * 1024,
            timeout_ms: 10_000,
            expect_duration_ns: -1,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PlaylistEntry {
    pub id: String,
    pub title: String,
    pub uri: String,
    pub edge: EdgeParams,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Playlist {
    pub version: String,
    pub entries: Vec<PlaylistEntry>,
}

impl Playlist {
    pub fn load(path: &std::path::Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&text)?)
    }

    /// Index of the first entry whose media file is missing on disk.
    pub fn missing_index(&self) -> Option<usize> {
        self.entries
            .iter()
            .position(|e| !std::path::Path::new(&e.uri).exists())
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum EntryStatus {
    Ok,
    /// Fell back to the degraded path; payload is the reason.
    Degraded(String),
    /// Media file does not exist (the "missing segment").
    Missing(String),
    /// GStreamer pipeline error.
    Error(String),
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct EntryResult {
    pub id: String,
    pub index: usize,
    pub status: EntryStatus,
    pub bytes_read: u64,
    pub buffers: u64,
    pub sha256: Option<String>,
    pub elapsed_ms: u64,
    /// Extreme-parameter clamps applied while processing, kept as evidence.
    pub param_notes: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ResourceBudget {
    /// Global remaining read budget in bytes; 0 forces degradation of any
    /// further non-empty entry.
    pub remaining_bytes: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Checkpoint {
    pub playlist_version: String,
    pub next_index: usize,
    pub results: Vec<EntryResult>,
    pub budget: ResourceBudget,
    pub saved_unix_ms: u128,
}

pub fn now_unix_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}
