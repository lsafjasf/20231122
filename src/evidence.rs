use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Serialize;
use serde_json::Value;

use crate::model::now_rfc3339;

pub struct EvidenceLog {
    path: PathBuf,
    next_seq: u64,
}

impl EvidenceLog {
    pub fn create(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, b"")?;
        Ok(Self { path, next_seq: 0 })
    }

    pub fn open_append(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let next_seq = if path.exists() {
            std::fs::read_to_string(&path)?
                .lines()
                .filter(|line| !line.trim().is_empty())
                .count() as u64
        } else {
            0
        };
        Ok(Self { path, next_seq })
    }

    pub fn append<T: Serialize>(&mut self, kind: &str, data: T) -> Result<()> {
        let event = Event {
            seq: self.next_seq,
            at: now_rfc3339(),
            kind: kind.to_string(),
            data: serde_json::to_value(data)?,
        };
        self.next_seq += 1;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        serde_json::to_writer(&mut file, &event)?;
        file.write_all(b"\n")?;
        file.flush()?;
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[derive(Serialize)]
struct Event {
    seq: u64,
    at: String,
    kind: String,
    data: Value,
}

pub fn read_kinds(path: &Path) -> Result<Vec<String>> {
    let mut kinds = Vec::new();
    for line in std::fs::read_to_string(path)?.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(line)?;
        if let Some(kind) = value.get("kind").and_then(Value::as_str) {
            kinds.push(kind.to_string());
        }
    }
    Ok(kinds)
}
