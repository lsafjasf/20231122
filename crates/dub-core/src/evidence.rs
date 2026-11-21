//! Key-evidence log: an append-only JSONL file whose records form a hash
//! chain. Every record carries timestamps from *both* unsynchronised device
//! clocks. A colleague can verify a run end-to-end with `verify_file`
//! without trusting any application state.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::clocks::ClockPair;

pub const GENESIS: &str = "genesis";

/// One evidence record. Field order is fixed by declaration order and is
/// part of the hashed canonical form.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceRecord {
    pub seq: u64,
    pub a_ns: i64,
    pub b_ns: i64,
    pub kind: String,
    pub fields: Map<String, Value>,
    pub prev_hash: String,
    pub hash: String,
}

impl EvidenceRecord {
    /// Canonical bytes that are hashed: the record without its `hash` field.
    fn digest_input(&self) -> Vec<u8> {
        let body = json!({
            "seq": self.seq,
            "a_ns": self.a_ns,
            "b_ns": self.b_ns,
            "kind": self.kind,
            "fields": self.fields,
            "prev_hash": self.prev_hash,
        });
        serde_json::to_vec(&body).expect("evidence body serialises")
    }

    pub fn compute_hash(&self) -> String {
        hex_sha256(&self.digest_input())
    }
}

pub fn hex_sha256(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

/// Append-only writer keeping the chain tip in memory.
pub struct EvidenceLog {
    path: PathBuf,
    seq: u64,
    tip: String,
}

impl EvidenceLog {
    /// Open (or create) the log; if it exists, the chain is verified first
    /// and appending continues from the verified tip.
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        if path.exists() {
            let report = verify_file(path)?;
            anyhow::ensure!(report.ok, "refusing to append to broken chain: {report:?}");
            Ok(Self {
                path: path.into(),
                seq: report.records,
                tip: report.tip,
            })
        } else {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            File::create(path)?;
            Ok(Self {
                path: path.into(),
                seq: 0,
                tip: GENESIS.into(),
            })
        }
    }

    /// Append one record, stamping both device clocks.
    pub fn record(
        &mut self,
        clocks: &ClockPair,
        kind: &str,
        fields: Map<String, Value>,
    ) -> anyhow::Result<EvidenceRecord> {
        let mut rec = EvidenceRecord {
            seq: self.seq,
            a_ns: clocks.a.now_ns(),
            b_ns: clocks.b.now_ns(),
            kind: kind.into(),
            fields,
            prev_hash: self.tip.clone(),
            hash: String::new(),
        };
        rec.hash = rec.compute_hash();
        let mut f = OpenOptions::new().append(true).open(&self.path)?;
        f.write_all(serde_json::to_string(&rec)?.as_bytes())?;
        f.write_all(b"\n")?;
        f.sync_all()?;
        self.seq += 1;
        self.tip = rec.hash.clone();
        Ok(rec)
    }
}

#[derive(Debug)]
pub struct VerifyReport {
    pub ok: bool,
    pub records: u64,
    pub tip: String,
    pub problems: Vec<String>,
}

/// Independent verification: recompute every hash and the chain links.
pub fn verify_file(path: &Path) -> anyhow::Result<VerifyReport> {
    let f = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut problems = Vec::new();
    let mut prev = GENESIS.to_string();
    let mut count = 0u64;
    for (lineno, line) in BufReader::new(f).lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let rec: EvidenceRecord = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                problems.push(format!("line {}: unparseable: {e}", lineno + 1));
                break;
            }
        };
        if rec.seq != count {
            problems.push(format!(
                "line {}: seq {} != expected {count}",
                lineno + 1,
                rec.seq
            ));
        }
        if rec.prev_hash != prev {
            problems.push(format!("line {}: broken chain link", lineno + 1));
        }
        if rec.hash != rec.compute_hash() {
            problems.push(format!("line {}: hash mismatch", lineno + 1));
        }
        prev = rec.hash.clone();
        count += 1;
    }
    Ok(VerifyReport {
        ok: problems.is_empty(),
        records: count,
        tip: prev,
        problems,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chain_verifies_and_tamper_is_detected() {
        let dir = std::env::temp_dir().join(format!("dub-ev-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("evidence.jsonl");
        let _ = std::fs::remove_file(&path);

        let clocks = ClockPair::default();
        let mut log = EvidenceLog::open(&path).unwrap();
        log.record(&clocks, "job-start", Map::new()).unwrap();
        log.record(&clocks, "segment-done", Map::new()).unwrap();
        drop(log);

        let rep = verify_file(&path).unwrap();
        assert!(rep.ok, "{:?}", rep.problems);
        assert_eq!(rep.records, 2);

        // Reopening continues the chain instead of restarting it.
        let mut log = EvidenceLog::open(&path).unwrap();
        log.record(&clocks, "job-resume", Map::new()).unwrap();
        drop(log);
        let rep = verify_file(&path).unwrap();
        assert!(rep.ok && rep.records == 3);

        // Tamper with the file: verification must fail.
        let mut content = std::fs::read_to_string(&path).unwrap();
        content = content.replacen("job-start", "job-START", 1);
        std::fs::write(&path, content).unwrap();
        let rep = verify_file(&path).unwrap();
        assert!(!rep.ok);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
