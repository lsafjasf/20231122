use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepRecord {
    pub seq: u32,
    pub step: String,
    pub purpose: String,
    pub input: serde_json::Value,
    pub state_before: String,
    pub state_after: String,
    pub result: serde_json::Value,
    pub evidence: Vec<String>,
    pub ts_unix_ms: u128,
}

pub struct Journal {
    path: PathBuf,
    file: File,
    seq: u32,
}

impl Journal {
    pub fn create(path: &Path) -> Result<Journal, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)
            .map_err(|e| format!("打开日志 {}: {e}", path.display()))?;
        Ok(Journal {
            path: path.to_path_buf(),
            file,
            seq: 0,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn append(
        &mut self,
        step: &str,
        purpose: &str,
        input: serde_json::Value,
        state_before: &str,
        state_after: &str,
        result: serde_json::Value,
        evidence: Vec<String>,
    ) -> Result<StepRecord, String> {
        self.seq += 1;
        let record = StepRecord {
            seq: self.seq,
            step: step.to_string(),
            purpose: purpose.to_string(),
            input,
            state_before: state_before.to_string(),
            state_after: state_after.to_string(),
            result,
            evidence,
            ts_unix_ms: crate::hash::now_unix_ms(),
        };
        let line = serde_json::to_string(&record).map_err(|e| e.to_string())?;
        writeln!(self.file, "{line}").map_err(|e| e.to_string())?;
        self.file.flush().map_err(|e| e.to_string())?;
        Ok(record)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

pub fn read_journal(path: &Path) -> Result<Vec<StepRecord>, String> {
    let file = File::open(path).map_err(|e| format!("打开日志 {}: {e}", path.display()))?;
    let mut out = Vec::new();
    for (index, line) in BufReader::new(file).lines().enumerate() {
        let line = line.map_err(|e| e.to_string())?;
        if line.trim().is_empty() {
            continue;
        }
        let record = serde_json::from_str(&line)
            .map_err(|e| format!("日志第 {} 行无法解析: {e}", index + 1))?;
        out.push(record);
    }
    Ok(out)
}

/// Timestamps and evidence paths naturally differ between runs. Everything
/// that describes the experimental state, input and outcome must match.
pub fn comparable_record(record: &StepRecord) -> serde_json::Value {
    serde_json::json!({
        "step": record.step,
        "input": record.input,
        "state_before": record.state_before,
        "state_after": record.state_after,
        "result": record.result,
    })
}
