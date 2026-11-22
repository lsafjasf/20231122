use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub const JOURNAL_FILE: &str = "journal.jsonl";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceEvent {
    pub seq: u64,
    pub at: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub segment_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub message: String,
    #[serde(default)]
    pub data: Value,
}

pub fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let file = fs::File::open(path)
        .with_context(|| format!("无法读取文件以计算 SHA-256：{}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        use std::io::Read;
        let read = reader
            .read(&mut buffer)
            .with_context(|| format!("读取失败：{}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

pub fn sha256_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

pub fn read_journal(project_root: &Path) -> Vec<EvidenceEvent> {
    let path = project_root.join(JOURNAL_FILE);
    let Ok(file) = fs::File::open(path) else {
        return Vec::new();
    };
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| serde_json::from_str::<EvidenceEvent>(&line).ok())
        .collect()
}

pub fn append_event(
    project_root: &Path,
    kind: &str,
    segment_index: Option<usize>,
    run_id: Option<String>,
    message: impl Into<String>,
    data: Value,
) -> Result<EvidenceEvent> {
    let path = project_root.join(JOURNAL_FILE);
    let next_seq = read_journal(project_root)
        .last()
        .map(|event| event.seq + 1)
        .unwrap_or(1);
    let event = EvidenceEvent {
        seq: next_seq,
        at: now_rfc3339(),
        kind: kind.to_string(),
        segment_index,
        run_id,
        message: message.into(),
        data,
    };
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("无法写入证据日志：{}", path.display()))?;
    serde_json::to_writer(&mut file, &event).context("证据日志序列化失败")?;
    file.write_all(b"\n").context("证据日志写入失败")?;
    file.sync_data().context("证据日志落盘失败")?;
    Ok(event)
}

pub fn relative_evidence_path(
    segment_index: usize,
    run_number: u32,
    kind: &str,
    timestamp_ns: u64,
) -> PathBuf {
    PathBuf::from("evidence")
        .join(format!("segment-{:02}", segment_index + 1))
        .join(format!("run-{run_number:04}"))
        .join(format!("{kind}-{timestamp_ns:018}.png"))
}
