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
    pub fn create(path: &Path) -> std::io::Result<Journal> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)?;
        Ok(Journal {
            path: path.to_path_buf(),
            file,
            seq: 0,
        })
    }

    pub fn append(
        &mut self,
        step: &str,
        purpose: &str,
        input: serde_json::Value,
        state_before: &str,
        state_after: &str,
        result: serde_json::Value,
        evidence: Vec<String>,
    ) -> std::io::Result<StepRecord> {
        self.seq += 1;
        let rec = StepRecord {
            seq: self.seq,
            step: step.to_string(),
            purpose: purpose.to_string(),
            input,
            state_before: state_before.to_string(),
            state_after: state_after.to_string(),
            result,
            evidence,
            ts_unix_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0),
        };
        let line = serde_json::to_string(&rec).expect("journal record serializes");
        writeln!(self.file, "{line}")?;
        self.file.flush()?;
        Ok(rec)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

pub fn read_journal(path: &Path) -> std::io::Result<Vec<StepRecord>> {
    let f = File::open(path)?;
    let mut out = Vec::new();
    for line in BufReader::new(f).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let rec: StepRecord = serde_json::from_str(&line)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        out.push(rec);
    }
    Ok(out)
}
