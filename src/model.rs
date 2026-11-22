//! 数据模型：运行记录、溯源步骤、频谱结论、人工确认、日志持久化。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const JOURNAL_PATH: &str = "artifacts/session.json";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum RunStatus {
    Completed,
    Cancelled,
    Failed,
}

impl RunStatus {
    pub fn label(&self) -> &'static str {
        match self {
            RunStatus::Completed => "已完成",
            RunStatus::Cancelled => "中途取消",
            RunStatus::Failed => "失败",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum Verdict {
    Clean,
    Noise,
    Inconclusive,
}

impl Verdict {
    pub fn label(&self) -> &'static str {
        match self {
            Verdict::Clean => "干净",
            Verdict::Noise => "噪声",
            Verdict::Inconclusive => "无法判断",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum ReviewDecision {
    Pending,
    Confirmed,
    Rejected,
}

impl ReviewDecision {
    pub fn label(&self) -> &'static str {
        match self {
            ReviewDecision::Pending => "待确认",
            ReviewDecision::Confirmed => "已确认",
            ReviewDecision::Rejected => "已驳回",
        }
    }
}

/// 一条处理步骤（溯源链的一环）。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProvenanceStep {
    pub id: String,
    pub kind: String,
    pub description: String,
    pub params: BTreeMap<String, String>,
    pub started_ts: u64,
    pub ended_ts: u64,
}

/// 单个频段的自动结论 + 关键证据 + 溯源引用。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BandConclusion {
    pub band_index: usize,
    pub freq_lo_hz: f64,
    pub freq_hi_hz: f64,
    pub magnitude_db: f64,
    pub noise_floor_db: f64,
    pub threshold_db: f64,
    pub margin_db: f64,
    pub verdict: Verdict,
    /// 关键证据（名称, 值），无法判断的边界频段必须保留。
    pub evidence: Vec<(String, String)>,
    /// 该结论由哪些步骤产生（步骤 id）。
    pub step_refs: Vec<String>,
}

/// 人工确认记录，与自动结论分开存放。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HumanReview {
    pub band_index: usize,
    pub reviewer: String,
    pub decision: ReviewDecision,
    pub note: String,
    pub ts: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunRecord {
    pub id: String,
    pub input_path: String,
    pub input_sha256: String,
    pub input_size_bytes: u64,
    pub created_ts: u64,
    pub status: RunStatus,
    /// 0.0..=1.0，取消/失败时保留中断点。
    pub progress: f64,
    pub cancel_reason: Option<String>,
    pub error: Option<String>,
    /// 失败/取消时的可执行后续操作。
    pub next_actions: Vec<String>,
    pub steps: Vec<ProvenanceStep>,
    pub conclusions: Vec<BandConclusion>,
    pub reviews: Vec<HumanReview>,
}

impl RunRecord {
    pub fn review_for(&self, band_index: usize) -> Option<&HumanReview> {
        self.reviews.iter().find(|r| r.band_index == band_index)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Journal {
    pub runs: Vec<RunRecord>,
}

impl Journal {
    pub fn load(path: &Path) -> Journal {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
            Err(_) => Journal::default(),
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        std::fs::write(path, text)
    }
}

pub fn journal_path() -> PathBuf {
    PathBuf::from(JOURNAL_PATH)
}

pub fn now_ts() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 将 unix 秒格式化为 UTC 时间字符串（不依赖 chrono）。
pub fn fmt_ts(ts: u64) -> String {
    let days = (ts / 86_400) as i64;
    let secs = ts % 86_400;
    let (y, m, d) = civil_from_days(days);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}Z",
        y,
        m,
        d,
        secs / 3600,
        (secs / 60) % 60,
        secs % 60
    )
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}
