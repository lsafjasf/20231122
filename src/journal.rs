use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

/// One replayable experiment step: what went in, what state we were in,
/// and what came out. Every step is appended as one JSON line.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepRecord {
    pub seq: u64,
    pub step: String,
    pub input: serde_json::Value,
    pub state: serde_json::Value,
    pub result: StepResult,
    pub input_sha256: String,
    pub result_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum StepResult {
    Ok { output: serde_json::Value },
    Failed { error: String },
    Duplicate { original_seq: u64 },
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex_encode(&hasher.finalize())
}

pub fn sha256_json(value: &serde_json::Value) -> String {
    sha256_hex(value.to_string().as_bytes())
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Append-only JSONL journal. The journal is the replay source of truth.
pub struct Journal {
    file: File,
    pub path: PathBuf,
    seq: u64,
    seen_inputs: std::collections::HashMap<String, u64>,
}

impl Journal {
    pub fn create(path: &Path) -> std::io::Result<Journal> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().write(true).create_new(true).open(path)?;
        Ok(Journal {
            file,
            path: path.to_path_buf(),
            seq: 0,
            seen_inputs: std::collections::HashMap::new(),
        })
    }

    /// Record a step. If the same input hash was already submitted, the
    /// submission is recorded as a duplicate instead of being re-executed
    /// by the caller — the caller checks `is_duplicate` first.
    pub fn is_duplicate(&self, input: &serde_json::Value) -> Option<u64> {
        self.seen_inputs.get(&sha256_json(input)).copied()
    }

    pub fn record(
        &mut self,
        step: &str,
        input: serde_json::Value,
        state: serde_json::Value,
        result: StepResult,
    ) -> std::io::Result<StepRecord> {
        self.seq += 1;
        let input_sha256 = sha256_json(&input);
        let result_sha256 = sha256_json(&serde_json::to_value(&result).unwrap_or_default());
        let record = StepRecord {
            seq: self.seq,
            step: step.to_string(),
            input,
            state,
            result,
            input_sha256: input_sha256.clone(),
            result_sha256,
        };
        if !matches!(record.result, StepResult::Duplicate { .. }) {
            self.seen_inputs.insert(input_sha256, record.seq);
        }
        let line = serde_json::to_string(&record)?;
        writeln!(self.file, "{line}")?;
        self.file.flush()?;
        Ok(record)
    }
}

pub fn read_all(path: &Path) -> std::io::Result<Vec<StepRecord>> {
    let reader = BufReader::new(File::open(path)?);
    let mut records = Vec::new();
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let record: StepRecord = serde_json::from_str(&line)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        records.push(record);
    }
    Ok(records)
}
