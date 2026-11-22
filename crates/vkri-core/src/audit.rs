//! Append-only audit trail.
//!
//! Each segment owns a JSON Lines audit file. Every record embeds the SHA-256
//! of the previous record, forming a tamper-evident hash chain. Records are
//! only ever appended, never rewritten or deleted, so interrupted runs and
//! re-runs remain fully traceable as key evidence.

use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{Error, Result};

const ZERO_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// One immutable audit entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditRecord {
    pub seq: u64,
    pub at: String,
    pub actor: String,
    pub action: String,
    pub detail: String,
    pub run_id: Option<String>,
    pub prev_sha256: String,
    pub record_sha256: String,
}

/// Handle to one segment's append-only JSONL audit file.
#[derive(Debug)]
pub struct AuditLog {
    path: PathBuf,
    next_seq: u64,
    last_hash: String,
}

impl AuditLog {
    /// Open an existing log or create a new empty one for `segment_id`.
    pub fn open(evidence_root: &Path, segment_id: &str) -> Result<Self> {
        let dir = evidence_root.join(segment_id);
        fs::create_dir_all(&dir).map_err(|e| Error::io(&dir, e))?;
        let path = dir.join("audit.jsonl");
        let mut next_seq = 0u64;
        let mut last_hash = ZERO_HASH.to_string();

        if path.exists() {
            let file = fs::File::open(&path).map_err(|e| Error::io(&path, e))?;
            for line in BufReader::new(file).lines() {
                let line = line.map_err(|e| Error::io(&path, e))?;
                if line.trim().is_empty() {
                    continue;
                }
                let record: AuditRecord = serde_json::from_str(&line).map_err(Error::Serde)?;
                next_seq = record.seq + 1;
                last_hash = record.record_sha256;
            }
        }
        Ok(Self {
            path,
            next_seq,
            last_hash,
        })
    }

    /// Append a new entry and fsync the file before returning.
    pub fn append(
        &mut self,
        actor: &str,
        action: &str,
        detail: &str,
        run_id: Option<&str>,
    ) -> Result<AuditRecord> {
        let at = crate::time::format_unix_nanos(
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default(),
        );
        let mut record = AuditRecord {
            seq: self.next_seq,
            at,
            actor: actor.to_string(),
            action: action.to_string(),
            detail: detail.to_string(),
            run_id: run_id.map(str::to_string),
            prev_sha256: self.last_hash.clone(),
            record_sha256: String::new(),
        };
        let canonical = serde_json::to_string(&record)?;
        let digest = Sha256::digest(canonical.as_bytes());
        record.record_sha256 = hex::encode(digest);

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|e| Error::io(&self.path, e))?;
        writeln!(file, "{}", serde_json::to_string(&record)?)
            .map_err(|e| Error::io(&self.path, e))?;
        file.sync_all().map_err(|e| Error::io(&self.path, e))?;

        self.next_seq += 1;
        self.last_hash = record.record_sha256.clone();
        Ok(record)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn len(&self) -> u64 {
        self.next_seq
    }

    pub fn is_empty(&self) -> bool {
        self.next_seq == 0
    }

    /// Re-read and verify the full hash chain. Used by the export verifier.
    pub fn verify(evidence_root: &Path, segment_id: &str) -> Result<Vec<AuditRecord>> {
        let path = evidence_root.join(segment_id).join("audit.jsonl");
        let file = fs::File::open(&path).map_err(|e| Error::io(&path, e))?;
        let mut previous = ZERO_HASH.to_string();
        let mut records = Vec::new();
        for (line_no, line) in BufReader::new(file).lines().enumerate() {
            let line = line.map_err(|e| Error::io(&path, e))?;
            if line.trim().is_empty() {
                continue;
            }
            let record: AuditRecord = serde_json::from_str(&line).map_err(Error::Serde)?;
            if record.prev_sha256 != previous {
                return Err(Error::Verify(format!(
                    "audit chain broken at line {} of {}",
                    line_no + 1,
                    path.display()
                )));
            }
            let stored = record.record_sha256.clone();
            let mut without_hash = record.clone();
            without_hash.record_sha256 = String::new();
            let canonical = serde_json::to_string(&without_hash)?;
            let recomputed = hex::encode(Sha256::digest(canonical.as_bytes()));
            if recomputed != stored {
                return Err(Error::Verify(format!(
                    "audit record digest mismatch at line {} of {}",
                    line_no + 1,
                    path.display()
                )));
            }
            previous = stored;
            records.push(record);
        }
        Ok(records)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_and_verify_chain() {
        let dir = std::env::temp_dir().join(format!("vkri_audit_{}", uuid::Uuid::new_v4()));
        let mut log = AuditLog::open(&dir, "seg_x").unwrap();
        log.append("cli", "segment.registered", "first", None)
            .unwrap();
        log.append("cli", "run.started", "go", Some("run_1"))
            .unwrap();
        drop(log);

        let records = AuditLog::verify(&dir, "seg_x").unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].prev_sha256, ZERO_HASH);
        assert_eq!(records[1].prev_sha256, records[0].record_sha256);
        std::fs::remove_dir_all(&dir).ok();
    }
}
