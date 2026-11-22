//! Append-only, hash-chained evidence log (`evidence.jsonl`).
//!
//! Every event carries the SHA-256 of the previous event line, so any
//! tampering, deletion, or reordering is detectable by a colleague who
//! replays the chain. The log is the key evidence trail of the run:
//! session boundaries, clock beacons, pauses, resumes, degradations,
//! and the resume-consistency comparison.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::Value;

use crate::model::{ordered_map, sha256_hex};

const GENESIS: &str = "GENESIS";

pub struct EvidenceLog {
    path: PathBuf,
    last_hash: String,
    seq: u64,
}

impl EvidenceLog {
    /// Open (or create) the log. Existing lines are replayed so appends
    /// continue the chain after a resume.
    pub fn open(dir: &Path) -> Result<Self> {
        let path = dir.join("evidence.jsonl");
        let mut last_hash = GENESIS.to_string();
        let mut seq = 0u64;
        if path.exists() {
            let f = File::open(&path).with_context(|| format!("open {path:?}"))?;
            for line in BufReader::new(f).lines() {
                let line = line?;
                if line.trim().is_empty() {
                    continue;
                }
                let v: Value = serde_json::from_str(&line)
                    .with_context(|| format!("parse evidence line: {line}"))?;
                last_hash = v["hash"]
                    .as_str()
                    .context("evidence line missing hash")?
                    .to_string();
                seq += 1;
            }
        }
        Ok(Self { path, last_hash, seq })
    }

    /// Append one event. `fields` are merged into the ordered envelope:
    /// seq, kind, capture_ts_ms, analysis_ts_ms, prev_hash, <fields>, hash.
    pub fn record(
        &mut self,
        kind: &str,
        capture_ts_ms: i64,
        analysis_ts_ms: i64,
        fields: Vec<(&str, Value)>,
    ) -> Result<String> {
        let mut obj = ordered_map(vec![
            ("seq", Value::from(self.seq)),
            ("kind", Value::from(kind)),
            ("capture_ts_ms", Value::from(capture_ts_ms)),
            ("analysis_ts_ms", Value::from(analysis_ts_ms)),
            ("prev_hash", Value::from(self.last_hash.clone())),
        ]);
        for (k, v) in fields {
            obj.insert(k.to_string(), v);
        }
        let without_hash = serde_json::to_string(&obj)?;
        let hash = sha256_hex(without_hash.as_bytes());
        obj.insert("hash".to_string(), Value::from(hash.clone()));
        let line = serde_json::to_string(&obj)?;
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .with_context(|| format!("append {:?}", self.path))?;
        f.write_all(line.as_bytes())?;
        f.write_all(b"\n")?;
        f.sync_all()?;
        self.last_hash = hash.clone();
        self.seq += 1;
        Ok(hash)
    }

    /// SHA-256 over the whole file; stored in the report so a reviewer
    /// can detect any post-export modification of the evidence.
    pub fn file_digest(&self) -> Result<String> {
        let bytes = std::fs::read(&self.path).with_context(|| format!("read {:?}", self.path))?;
        Ok(sha256_hex(&bytes))
    }
}

/// Verify a chain file: parse every line, recompute each hash, and check
/// the prev_hash linkage. Returns the number of events checked.
pub fn verify_chain(path: &Path) -> Result<u64> {
    let f = File::open(path).with_context(|| format!("open {path:?}"))?;
    let mut expect_prev = GENESIS.to_string();
    let mut count = 0u64;
    for (lineno, line) in BufReader::new(f).lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(&line)
            .with_context(|| format!("evidence line {} is not valid JSON", lineno + 1))?;
        let obj = v.as_object().context("evidence event is not an object")?;
        let hash = obj
            .get("hash")
            .and_then(Value::as_str)
            .context("evidence event missing hash")?;
        let prev = obj
            .get("prev_hash")
            .and_then(Value::as_str)
            .context("evidence event missing prev_hash")?;
        anyhow::ensure!(
            prev == expect_prev,
            "evidence chain broken at line {}: prev_hash mismatch",
            lineno + 1
        );
        let mut without = obj.clone();
        without.shift_remove("hash");
        let recomputed = sha256_hex(serde_json::to_string(&without)?.as_bytes());
        anyhow::ensure!(
            recomputed == hash,
            "evidence hash mismatch at line {}",
            lineno + 1
        );
        expect_prev = hash.to_string();
        count += 1;
    }
    Ok(count)
}
