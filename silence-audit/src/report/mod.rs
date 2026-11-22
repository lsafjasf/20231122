//! 可复核报告：运行汇总、证据文件清单与 SHA256 证据链。

use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::model::ObjectResult;

/// 一次批处理运行的完整复核报告。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunReport {
    /// 运行唯一标识。
    pub run_id: String,
    /// UTC 运行时间（RFC3339）。
    pub started_at: String,
    /// 输入快照路径。
    pub snapshot_path: String,
    /// 输入快照内容的 SHA256，锁定本次处理的输入证据。
    pub snapshot_sha256: String,
    /// 处理对象总数。
    pub total_objects: usize,
    /// 成功数（含截断）。
    pub succeeded: usize,
    /// 失败数。
    pub failed: usize,
    /// 每个对象的明细结果。
    pub results: Vec<ObjectResult>,
    /// 证据文件哈希清单。
    pub evidence: Vec<EvidenceEntry>,
}

/// 单个证据文件的哈希记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceEntry {
    /// 相对运行目录的路径。
    pub path: String,
    pub sha256: String,
    pub size_bytes: u64,
    /// 关联的对象 id。
    pub object_id: String,
    /// 证据类型，如 `waveform-png`、`media-copy`。
    pub kind: String,
}

impl RunReport {
    pub fn new(run_id: String, snapshot_path: &Path, snapshot_sha256: String) -> Self {
        Self {
            run_id,
            started_at: Utc::now().to_rfc3339(),
            snapshot_path: snapshot_path.display().to_string(),
            snapshot_sha256,
            total_objects: 0,
            succeeded: 0,
            failed: 0,
            results: Vec::new(),
            evidence: Vec::new(),
        }
    }

    pub fn record(&mut self, result: ObjectResult) {
        self.total_objects += 1;
        match result.status {
            crate::model::ObjectStatus::Failed => self.failed += 1,
            _ => self.succeeded += 1,
        }
        self.results.push(result);
    }

    /// 写入报告 JSON，并对报告自身计算校验值一并返回。
    pub fn write_to(&self, dir: &Path) -> anyhow::Result<(PathBuf, String)> {
        let path = dir.join("report.json");
        let json = serde_json::to_string_pretty(self)?;
        fs::write(&path, json)?;
        let hash = sha256_hex(&fs::read(&path)?);
        fs::write(
            dir.join("report.json.sha256"),
            format!("{hash}  report.json\n"),
        )?;
        Ok((path, hash))
    }
}

/// 计算字节切片的 SHA256 十六进制值。
pub fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let digest = hasher.finalize();
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// 计算文件 SHA256 与大小，生成一条证据清单记录。
pub fn hash_file(
    run_dir: &Path,
    abs_path: &Path,
    object_id: &str,
    kind: &str,
) -> anyhow::Result<EvidenceEntry> {
    let data = fs::read(abs_path)?;
    let rel = abs_path
        .strip_prefix(run_dir)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| abs_path.display().to_string());
    Ok(EvidenceEntry {
        path: rel,
        sha256: sha256_hex(&data),
        size_bytes: data.len() as u64,
        object_id: object_id.to_string(),
        kind: kind.to_string(),
    })
}
