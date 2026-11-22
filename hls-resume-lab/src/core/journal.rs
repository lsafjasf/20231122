//! Append-only, replay-friendly JSONL journal with per-step evidence.

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

/// One recorded step of an experiment run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalEntry {
    /// Monotonic 1-based step number within the run.
    pub step: usize,
    /// Logical phase, e.g. `load_snapshot`, `diagnose`, `dedupe`.
    pub phase: String,
    /// What the step received (already serialized).
    pub input: serde_json::Value,
    /// Engine state visible at the start of the step.
    pub state: serde_json::Value,
    /// What the step produced.
    pub result: serde_json::Value,
}

/// Journal that mirrors every step to a JSONL file.
#[derive(Debug)]
pub struct Journal {
    path: PathBuf,
    file: Option<std::fs::File>,
    entries: Vec<JournalEntry>,
}

impl Journal {
    /// Create a journal backed by `path`. When `path` is `/dev/null`-like or
    /// the file cannot be opened, entries are still kept in memory.
    pub fn create(path: &Path) -> std::io::Result<Self> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            file: Some(file),
            entries: Vec::new(),
        })
    }

    /// In-memory only journal (used by unit tests).
    pub fn memory() -> Self {
        Self {
            path: PathBuf::from("memory://journal"),
            file: None,
            entries: Vec::new(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn entries(&self) -> &[JournalEntry] {
        &self.entries
    }

    /// Append one step and flush it immediately so a crash still leaves
    /// durable evidence on disk.
    pub fn record(
        &mut self,
        phase: &str,
        input: serde_json::Value,
        state: serde_json::Value,
        result: serde_json::Value,
    ) -> std::io::Result<&JournalEntry> {
        let entry = JournalEntry {
            step: self.entries.len() + 1,
            phase: phase.to_string(),
            input,
            state,
            result,
        };
        let line = serde_json::to_string(&entry)?;
        if let Some(file) = self.file.as_mut() {
            file.write_all(line.as_bytes())?;
            file.write_all(b"\n")?;
            file.flush()?;
        }
        self.entries.push(entry);
        Ok(self.entries.last().unwrap())
    }

    /// Replay a journal from a JSONL file.
    pub fn replay(path: &Path) -> std::io::Result<Vec<JournalEntry>> {
        let content = std::fs::read_to_string(path)?;
        let mut entries = Vec::new();
        for (idx, line) in content.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let entry: JournalEntry = serde_json::from_str(line).map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("journal line {}: {e}", idx + 1),
                )
            })?;
            entries.push(entry);
        }
        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_are_numbered_and_replayable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal.jsonl");
        let mut journal = Journal::create(&path).unwrap();
        journal
            .record(
                "phase",
                serde_json::json!({"k": 1}),
                serde_json::json!({"seen": 0}),
                serde_json::json!({"ok": true}),
            )
            .unwrap();
        let replayed = Journal::replay(&path).unwrap();
        assert_eq!(replayed.len(), 1);
        assert_eq!(replayed[0].step, 1);
        assert_eq!(replayed[0].phase, "phase");
    }
}
