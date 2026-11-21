//! Checkpoint persistence with canonical, order-preserving JSON.
//!
//! Saving is atomic (write temp + rename). Loading validates that the
//! document's top-level fields appear in exactly the pinned
//! [`STATE_FIELD_ORDER`], so a reloaded artifact provably keeps both its
//! fields and their order.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context};

use crate::model::{DubJob, STATE_FIELD_ORDER};

pub const CHECKPOINT_FILE: &str = "checkpoint.json";
pub const EVIDENCE_FILE: &str = "evidence.jsonl";
pub const REPORT_FILE: &str = "report.json";

pub fn checkpoint_path(workdir: &Path) -> PathBuf {
    workdir.join(CHECKPOINT_FILE)
}

/// Serialise canonically: pretty JSON, fields in declaration order.
pub fn to_canonical(job: &DubJob) -> anyhow::Result<String> {
    Ok(serde_json::to_string_pretty(job)? + "\n")
}

/// Atomic save: write to a temp file in the same directory, then rename.
pub fn save_state(workdir: &Path, job: &DubJob) -> anyhow::Result<PathBuf> {
    std::fs::create_dir_all(workdir)?;
    let target = checkpoint_path(workdir);
    let tmp = workdir.join(".checkpoint.json.tmp");
    std::fs::write(&tmp, to_canonical(job)?)?;
    std::fs::rename(&tmp, &target)?;
    Ok(target)
}

/// Extract top-level keys of a JSON object in document order.
fn top_level_keys(text: &str) -> anyhow::Result<Vec<String>> {
    let value: serde_json::Value = serde_json::from_str(text)?;
    match value {
        serde_json::Value::Object(map) => Ok(map.keys().cloned().collect()),
        _ => bail!("checkpoint is not a JSON object"),
    }
}

/// Load a checkpoint, enforcing the pinned field order.
pub fn load_state(workdir: &Path) -> anyhow::Result<DubJob> {
    let path = checkpoint_path(workdir);
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let keys = top_level_keys(&text)?;
    let expected: Vec<String> = STATE_FIELD_ORDER.iter().map(|s| s.to_string()).collect();
    anyhow::ensure!(
        keys == expected,
        "checkpoint field order mismatch: got {keys:?}, expected {expected:?}"
    );
    let job: DubJob = serde_json::from_str(&text)?;
    anyhow::ensure!(
        job.schema == crate::model::STATE_SCHEMA,
        "unsupported checkpoint schema {}",
        job.schema
    );
    // Segment order must be strictly by index.
    anyhow::ensure!(
        job.segments.windows(2).all(|w| w[0].index < w[1].index),
        "segment order not preserved"
    );
    Ok(job)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_job() -> DubJob {
        DubJob::new("j", "in.mkv", "out", 9_000, &["eng".into(), "zho".into()]).unwrap()
    }

    #[test]
    fn roundtrip_preserves_fields_and_order() {
        let dir = std::env::temp_dir().join(format!("dub-state-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let job = sample_job();
        save_state(&dir, &job).unwrap();
        let raw = std::fs::read_to_string(checkpoint_path(&dir)).unwrap();
        let keys = top_level_keys(&raw).unwrap();
        assert_eq!(keys, STATE_FIELD_ORDER.to_vec());
        let loaded = load_state(&dir).unwrap();
        assert_eq!(loaded.segments, job.segments);
        // Re-serialising the loaded job is byte-identical: order preserved.
        assert_eq!(to_canonical(&loaded).unwrap(), raw);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reordered_fields_are_rejected() {
        let dir = std::env::temp_dir().join(format!("dub-state-reorder-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        save_state(&dir, &sample_job()).unwrap();
        let raw = std::fs::read_to_string(checkpoint_path(&dir)).unwrap();
        // Swap the first two top-level keys in the raw text.
        let swapped = raw.replacen("\"schema\"", "\"job_id__TMP\"", 1);
        let swapped = swapped.replacen("\"job_id\"", "\"schema\"", 1);
        let swapped = swapped.replacen("\"job_id__TMP\"", "\"job_id\"", 1);
        std::fs::write(checkpoint_path(&dir), swapped).unwrap();
        assert!(load_state(&dir).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
