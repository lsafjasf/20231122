use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::model::{now_rfc3339, Degradation, SampleResult, SamplingPlan};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Checkpoint {
    pub version: u32,
    pub run_id: String,
    pub plan_sha256: String,
    pub source_sha256: String,
    pub scale_percent: u8,
    pub next_index: usize,
    pub total: usize,
    pub results: Vec<SampleResult>,
    pub decoder: String,
    pub degradation: Degradation,
    pub started_at: String,
    pub updated_at: String,
    pub done: bool,
}

impl Checkpoint {
    pub fn new(
        run_id: String,
        plan_sha256: String,
        plan: &SamplingPlan,
        degradation: Degradation,
    ) -> Self {
        let now = now_rfc3339();
        Self {
            version: 1,
            run_id,
            plan_sha256,
            source_sha256: plan.source.sha256.clone(),
            scale_percent: degradation.effective_scale_percent,
            next_index: 0,
            total: plan.samples.len(),
            results: Vec::new(),
            decoder: degradation.decoder.clone(),
            degradation,
            started_at: now.clone(),
            updated_at: now,
            done: false,
        }
    }

    pub fn push(&mut self, result: SampleResult) -> Result<()> {
        if result.seq != self.results.len() {
            bail!(
                "cannot append sample {} after {} results",
                result.seq,
                self.results.len()
            );
        }
        self.results.push(result);
        self.next_index = self.results.len();
        self.updated_at = now_rfc3339();
        self.done = self.next_index == self.total;
        Ok(())
    }

    pub fn validate(&self, plan: &SamplingPlan, plan_hash: &str) -> Result<()> {
        if self.version != 1 {
            bail!("unsupported checkpoint version {}", self.version);
        }
        if self.source_sha256 != plan.source.sha256 {
            bail!(
                "checkpoint source hash {} does not match source {}",
                self.source_sha256,
                plan.source.sha256
            );
        }
        if self.plan_sha256 != plan_hash {
            bail!("checkpoint plan hash does not match the requested extreme parameters");
        }
        if self.total != plan.samples.len() {
            bail!(
                "checkpoint total {} does not match plan total {}",
                self.total,
                plan.samples.len()
            );
        }
        if self.next_index != self.results.len() || self.next_index > self.total {
            bail!("checkpoint cursor is inconsistent with its result list");
        }
        Ok(())
    }
}

pub fn path_for_dir(out_dir: &Path) -> PathBuf {
    out_dir.join("checkpoint.json")
}

pub fn save(out_dir: &Path, checkpoint: &Checkpoint) -> Result<PathBuf> {
    std::fs::create_dir_all(out_dir)?;
    let path = path_for_dir(out_dir);
    let tmp = out_dir.join(format!("checkpoint.json.tmp-{}", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec_pretty(checkpoint)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

pub fn load(out_dir: &Path) -> Result<Checkpoint> {
    let path = path_for_dir(out_dir);
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("checkpoint not found at {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("invalid checkpoint at {}", path.display()))
}

pub fn exists(out_dir: &Path) -> bool {
    path_for_dir(out_dir).exists()
}

pub fn remove(out_dir: &Path) -> Result<()> {
    let path = path_for_dir(out_dir);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}
