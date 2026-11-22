use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// An experiment object loaded from the objects directory. The loader
/// dispatches on file extension; an unknown extension is a recoverable
/// error so the experiment continues with the remaining objects.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamObject {
    pub name: String,
    pub kind: String,
    pub payload: serde_json::Value,
    pub source_sha256: String,
}

#[derive(Debug)]
pub struct ObjectError {
    pub path: PathBuf,
    pub message: String,
}

impl std::fmt::Display for ObjectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.message)
    }
}

pub fn load(path: &Path) -> Result<StreamObject, ObjectError> {
    let fail = |message: String| ObjectError {
        path: path.to_path_buf(),
        message,
    };
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_string();
    let raw = std::fs::read(path).map_err(|e| fail(format!("read error: {e}")))?;
    let source_sha256 = crate::journal::sha256_hex(&raw);
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unnamed")
        .to_string();
    let text = String::from_utf8(raw).map_err(|e| fail(format!("not utf-8: {e}")))?;
    match ext.as_str() {
        "json" => {
            let payload: serde_json::Value =
                serde_json::from_str(&text).map_err(|e| fail(format!("invalid json: {e}")))?;
            Ok(StreamObject {
                name,
                kind: "stream-config".into(),
                payload,
                source_sha256,
            })
        }
        "csv" => {
            let rows: Vec<Vec<&str>> = text
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| l.split(',').map(str::trim).collect())
                .collect();
            Ok(StreamObject {
                name,
                kind: "metric-series".into(),
                payload: serde_json::json!({ "rows": rows }),
                source_sha256,
            })
        }
        other => Err(fail(format!(
            "unknown extension '.{other}': no loader registered"
        ))),
    }
}

pub fn list(dir: &Path) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd.filter_map(|e| e.ok().map(|e| e.path())).collect(),
        Err(_) => return Vec::new(),
    };
    entries.sort();
    entries
}
