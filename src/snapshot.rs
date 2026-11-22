use serde::{Deserialize, Serialize};
use std::path::Path;

/// A versioned pipeline snapshot used as an experiment control.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub version: String,
    pub description: String,
    pub pipeline: String,
    pub duration_ms: u64,
    pub expect: SnapshotExpect,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotExpect {
    pub max_mean_latency_ms: f64,
}

pub fn load(path: &Path) -> Result<Snapshot, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read snapshot {}: {e}", path.display()))?;
    serde_json::from_str(&text)
        .map_err(|e| format!("cannot parse snapshot {}: {e}", path.display()))
}

/// Field-by-field diff of the two control snapshots, kept as evidence.
pub fn diff(a: &Snapshot, b: &Snapshot) -> serde_json::Value {
    let mut changes = Vec::new();
    if a.pipeline != b.pipeline {
        changes.push(serde_json::json!({
            "field": "pipeline",
            "v1": a.pipeline,
            "v2": b.pipeline,
        }));
    }
    if a.duration_ms != b.duration_ms {
        changes.push(serde_json::json!({
            "field": "duration_ms",
            "v1": a.duration_ms,
            "v2": b.duration_ms,
        }));
    }
    if a.expect.max_mean_latency_ms != b.expect.max_mean_latency_ms {
        changes.push(serde_json::json!({
            "field": "expect.max_mean_latency_ms",
            "v1": a.expect.max_mean_latency_ms,
            "v2": b.expect.max_mean_latency_ms,
        }));
    }
    serde_json::json!({
        "v1": a.version,
        "v2": b.version,
        "changes": changes,
    })
}
