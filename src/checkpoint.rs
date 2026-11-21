use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::model::Checkpoint;

pub fn path_for(out_dir: &Path) -> PathBuf {
    out_dir.join("checkpoint.json")
}

pub fn save(out_dir: &Path, cp: &Checkpoint) -> Result<PathBuf> {
    std::fs::create_dir_all(out_dir)?;
    let path = path_for(out_dir);
    let tmp = out_dir.join("checkpoint.json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(cp)?)?;
    std::fs::rename(&tmp, &path)?; // atomic replace: never a torn checkpoint
    Ok(path)
}

pub fn load(out_dir: &Path) -> Result<Checkpoint> {
    let path = path_for(out_dir);
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("no checkpoint at {}", path.display()))?;
    Ok(serde_json::from_str(&text)?)
}

pub fn exists(out_dir: &Path) -> bool {
    path_for(out_dir).exists()
}
