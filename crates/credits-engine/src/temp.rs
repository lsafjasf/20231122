use std::path::Path;

use credits_core::report::{now_unix_ms, CleanupReport};
use tempfile::TempDir;

pub struct WorkArea {
    dir: Option<TempDir>,
    finalized: Option<CleanupReport>,
}

impl WorkArea {
    pub fn create(parent: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(parent)?;
        let dir = tempfile::Builder::new()
            .prefix(".credits-review-work-")
            .tempdir_in(parent)?;
        Ok(Self {
            dir: Some(dir),
            finalized: None,
        })
    }

    pub fn finish(&mut self) -> CleanupReport {
        if let Some(report) = &self.finalized {
            return report.clone();
        }
        let Some(dir) = self.dir.take() else {
            let cleanup = CleanupReport {
                strategy: "raii tempfile guard; key evidence kept outside the temp tree"
                    .to_string(),
                temp_path: String::new(),
                removed: true,
                residue: Vec::new(),
                completed_at_unix_ms: now_unix_ms(),
            };
            self.finalized = Some(cleanup.clone());
            return cleanup;
        };
        let path = dir.path().to_path_buf();
        let cleanup = match dir.close() {
            Ok(()) => {
                let residue = if path.exists() {
                    vec![path.display().to_string()]
                } else {
                    Vec::new()
                };
                CleanupReport {
                    strategy: "raii tempfile guard; key evidence kept outside the temp tree"
                        .to_string(),
                    temp_path: path.display().to_string(),
                    removed: residue.is_empty(),
                    residue,
                    completed_at_unix_ms: now_unix_ms(),
                }
            }
            Err(error) => CleanupReport {
                strategy: "raii tempfile guard; key evidence kept outside the temp tree"
                    .to_string(),
                temp_path: path.display().to_string(),
                removed: false,
                residue: vec![format!("{}: {error}", path.display())],
                completed_at_unix_ms: now_unix_ms(),
            },
        };
        self.finalized = Some(cleanup.clone());
        cleanup
    }
}

impl Drop for WorkArea {
    fn drop(&mut self) {
        if self.finalized.is_none() {
            if let Some(dir) = self.dir.take() {
                let _ = dir.close();
            }
        }
    }
}
