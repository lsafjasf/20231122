use crate::fixture::{hash_file, hex_lower};
use crate::model::{CameraProfile, METADATA_RECORD_BYTES, ResourcePlan, SessionPaths};
use anyhow::Context;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub const CHECKPOINT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum JobState {
    Prepared,
    Running,
    Paused,
    ResourcePaused,
    Complete,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum PauseReason {
    UserRequested,
    AcceptancePoint,
    ResourceBudget,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub schema_version: u32,
    pub checkpoint_seq: u64,
    pub evidence_seq: u64,
    pub job_id: String,
    pub source_video_sha256: String,
    pub source_metadata_sha256: String,
    pub next_frame: u32,
    pub next_pts_ns: u64,
    pub next_video_offset: u64,
    pub next_metadata_offset: u64,
    pub processed_frames: u32,
    pub total_frames: u32,
    pub output_prefix_sha256: String,
    pub output_metadata_len: u64,
    pub resumed_count: u32,
    pub resource_plan: Option<ResourcePlan>,
    pub state: JobState,
    pub pause_reason: Option<PauseReason>,
    pub updated_unix_ms: u128,
}

impl Checkpoint {
    pub fn prepared(
        job_id: impl Into<String>,
        profile: &CameraProfile,
        video_sha256: impl Into<String>,
        metadata_sha256: impl Into<String>,
    ) -> Self {
        Self {
            schema_version: CHECKPOINT_SCHEMA_VERSION,
            checkpoint_seq: 0,
            evidence_seq: 0,
            job_id: job_id.into(),
            source_video_sha256: video_sha256.into(),
            source_metadata_sha256: metadata_sha256.into(),
            next_frame: 0,
            next_pts_ns: 0,
            next_video_offset: 0,
            next_metadata_offset: 0,
            processed_frames: 0,
            total_frames: profile.frame_count,
            output_prefix_sha256: empty_sha256(),
            output_metadata_len: 0,
            resumed_count: 0,
            resource_plan: None,
            state: JobState::Prepared,
            pause_reason: None,
            updated_unix_ms: unix_ms(),
        }
    }

    pub fn advance(
        &mut self,
        profile: &CameraProfile,
        next_frame: u32,
        plan: &ResourcePlan,
        output_prefix_sha256: String,
    ) {
        self.checkpoint_seq = self.checkpoint_seq.saturating_add(1);
        self.next_frame = next_frame;
        self.next_pts_ns = profile.pts_ns(next_frame);
        self.next_video_offset = profile.video_offset(next_frame);
        self.next_metadata_offset = profile.metadata_offset(next_frame);
        self.processed_frames = next_frame;
        self.output_prefix_sha256 = output_prefix_sha256;
        self.output_metadata_len = u64::from(next_frame) * METADATA_RECORD_BYTES as u64;
        self.resource_plan = Some(plan.clone());
        self.updated_unix_ms = unix_ms();
    }

    pub fn mark_paused(&mut self, reason: PauseReason) {
        self.state = if reason == PauseReason::ResourceBudget {
            JobState::ResourcePaused
        } else {
            JobState::Paused
        };
        self.pause_reason = Some(reason);
        self.updated_unix_ms = unix_ms();
    }

    pub fn validate(
        &self,
        profile: &CameraProfile,
        video_sha256: &str,
        metadata_sha256: &str,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.schema_version == CHECKPOINT_SCHEMA_VERSION,
            "unsupported checkpoint schema"
        );
        anyhow::ensure!(
            self.source_video_sha256 == video_sha256,
            "source video changed"
        );
        anyhow::ensure!(
            self.source_metadata_sha256 == metadata_sha256,
            "source metadata changed"
        );
        anyhow::ensure!(
            self.total_frames == profile.frame_count,
            "frame count changed"
        );
        anyhow::ensure!(
            self.next_frame <= profile.frame_count,
            "checkpoint past EOF"
        );
        anyhow::ensure!(
            self.next_pts_ns == profile.pts_ns(self.next_frame),
            "checkpoint PTS does not match frame index"
        );
        anyhow::ensure!(
            self.next_video_offset == profile.video_offset(self.next_frame),
            "checkpoint video offset does not match frame index"
        );
        anyhow::ensure!(
            self.next_metadata_offset == profile.metadata_offset(self.next_frame),
            "checkpoint metadata offset does not match frame index"
        );
        anyhow::ensure!(
            self.output_metadata_len == u64::from(self.next_frame) * METADATA_RECORD_BYTES as u64,
            "checkpoint output length does not match cover"
        );
        Ok(())
    }

    pub fn save(&self, paths: &SessionPaths) -> anyhow::Result<()> {
        fs::create_dir_all(&paths.state_dir)?;
        let tmp = paths.state_dir.join(".checkpoint.json.tmp");
        let data = serde_json::to_vec_pretty(self)?;
        let mut file = File::create(&tmp)?;
        file.write_all(&data)?;
        file.sync_all()?;
        fs::rename(&tmp, &paths.checkpoint)?;
        if let Ok(dir) = File::open(&paths.state_dir) {
            let _ = dir.sync_all();
        }
        Ok(())
    }

    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let data = fs::read(path).with_context(|| format!("read {}", path.display()))?;
        Ok(serde_json::from_slice(&data)?)
    }

    pub fn verify_output_prefix(&self, paths: &SessionPaths) -> anyhow::Result<()> {
        let actual = hash_prefix(&paths.repaired_metadata, self.output_metadata_len)?;
        anyhow::ensure!(
            actual == self.output_prefix_sha256,
            "repaired metadata prefix changed across pause/resume"
        );
        Ok(())
    }
}

pub fn hash_prefix(path: &Path, length: u64) -> anyhow::Result<String> {
    let mut file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let file_len = file.metadata()?.len();
    anyhow::ensure!(length <= file_len, "prefix is longer than file");
    let mut remaining = length;
    let mut buffer = [0u8; 16 * 1024];
    let mut hasher = Sha256::new();
    while remaining > 0 {
        let take = buffer.len().min(remaining as usize);
        file.read_exact(&mut buffer[..take])?;
        hasher.update(&buffer[..take]);
        remaining -= take as u64;
    }
    Ok(hex_lower(&hasher.finalize()))
}

pub fn source_identity(paths: &SessionPaths) -> anyhow::Result<(String, String)> {
    Ok((hash_file(&paths.video)?, hash_file(&paths.metadata)?))
}

fn empty_sha256() -> String {
    hex_lower(&Sha256::digest([]))
}

fn unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::generate_demo_fixture;

    #[test]
    fn checkpoint_round_trip_is_atomic_and_geometry_checked() {
        let dir = tempfile::tempdir().unwrap();
        let profile = generate_demo_fixture(dir.path()).unwrap();
        let paths = SessionPaths::new(dir.path().join("job"));
        fs::create_dir_all(&paths.state_dir).unwrap();
        let mut checkpoint = Checkpoint::prepared(
            "test",
            &profile,
            profile.source_video_sha256.clone(),
            "metadata-hash",
        );
        let plan = crate::resource::plan_resource(&profile, 8, 1024 * 1024);
        checkpoint.advance(&profile, 72, &plan, empty_sha256());
        checkpoint.save(&paths).unwrap();
        let loaded = Checkpoint::load(&paths.checkpoint).unwrap();
        loaded
            .validate(&profile, &profile.source_video_sha256, "metadata-hash")
            .unwrap();
        assert_eq!(loaded.next_pts_ns, 2_400_000_000);
        assert_eq!(loaded.next_video_offset, 0x144000);
        assert_eq!(loaded.next_metadata_offset, 0x1200);
    }
}
