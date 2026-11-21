use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Serialize;

use crate::model::now_unix_ms;

/// Key-evidence store: every intermediate artifact of a scenario is kept
/// under one directory as JSON, plus an append-only event log.
pub struct EvidenceStore {
    pub root: PathBuf,
}

impl EvidenceStore {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        std::fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    pub fn write<T: Serialize>(&self, name: &str, value: &T) -> Result<PathBuf> {
        let path = self.root.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, serde_json::to_string_pretty(value)?)?;
        Ok(path)
    }

    pub fn log(&self, msg: &str) -> Result<()> {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.root.join("run.log"))?;
        writeln!(f, "[{}] {}", now_unix_ms(), msg)?;
        Ok(())
    }

    /// Preserve an existing file (e.g. a checkpoint) under evidence/.
    pub fn snapshot(&self, src: &Path, name: &str) -> Result<PathBuf> {
        let dst = self.root.join(name);
        std::fs::copy(src, &dst)?;
        Ok(dst)
    }
}
