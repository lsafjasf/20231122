use crate::fixture::hex_lower;
use crate::metadata::FrameMetadata;
use crate::model::{ResourcePlan, SessionPaths};
use anyhow::Context;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum EvidenceEvent {
    JobPrepared {
        job_id: String,
        source_video_sha256: String,
        source_metadata_sha256: String,
        original_copy: String,
        acceptance_frame: u32,
        acceptance_pts_ns: u64,
    },
    ResourcePlan {
        frame: u32,
        plan: ResourcePlan,
    },
    ChunkCommitted {
        first_frame: u32,
        next_frame: u32,
        next_pts_ns: u64,
        next_video_offset: u64,
        next_metadata_offset: u64,
        output_prefix_sha256: String,
    },
    Paused {
        frame: u32,
        pts_ns: u64,
        video_offset: u64,
        metadata_offset: u64,
        reason: String,
        checkpoint_seq: u64,
    },
    Resumed {
        frame: u32,
        pts_ns: u64,
        video_offset: u64,
        metadata_offset: u64,
        checkpoint_seq: u64,
        output_prefix_verified: bool,
    },
    FrameObservation {
        frame: u32,
        pts_ns: u64,
        video_offset: u64,
        metadata_offset: u64,
        peak_nits: u16,
        avg_nits: u16,
        object_present: bool,
        sampled_pixels: u64,
        total_pixels: u64,
        before: FrameMetadata,
        after: FrameMetadata,
        before_record_hex: String,
        after_record_hex: String,
        source_frame_sha256: String,
        key_evidence_dir: Option<String>,
    },
    Completed {
        output_sha256: String,
        output_video_sha256: String,
        processed_frames: u32,
    },
}

pub struct EvidenceWriter {
    path: PathBuf,
    file: BufWriter<File>,
    seq: u64,
}

impl EvidenceWriter {
    pub fn create(paths: &SessionPaths) -> anyhow::Result<Self> {
        fs::create_dir_all(&paths.evidence_dir)?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&paths.evidence)
            .with_context(|| format!("create {}", paths.evidence.display()))?;
        Ok(Self {
            path: paths.evidence.clone(),
            file: BufWriter::new(file),
            seq: 0,
        })
    }

    pub fn reopen(paths: &SessionPaths, seq: u64) -> anyhow::Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&paths.evidence)?;
        Ok(Self {
            path: paths.evidence.clone(),
            file: BufWriter::new(file),
            seq,
        })
    }

    pub fn append(&mut self, event: &EvidenceEvent) -> anyhow::Result<()> {
        self.seq = self.seq.saturating_add(1);
        let line = serde_json::json!({
            "seq": self.seq,
            "unix_ms": unix_ms(),
            "data": event,
        });
        serde_json::to_writer(&mut self.file, &line)?;
        self.file.write_all(b"\n")?;
        self.file.flush()?;
        Ok(())
    }

    pub fn sync(&mut self) -> anyhow::Result<()> {
        self.file.flush()?;
        self.file.get_ref().sync_data()?;
        Ok(())
    }

    pub fn seq(&self) -> u64 {
        self.seq
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

pub struct KeyEvidence {
    pub dir: PathBuf,
    pub source_frame_sha256: String,
}

pub fn write_key_evidence(
    paths: &SessionPaths,
    frame: u32,
    source_frame: &[u8],
    before: &[u8],
    after: &[u8],
) -> anyhow::Result<KeyEvidence> {
    let dir = paths
        .evidence_dir
        .join("key_frames")
        .join(format!("{frame:05}"));
    fs::create_dir_all(&dir)?;
    write_atomic(&dir.join("source.gray16"), source_frame)?;
    write_atomic(&dir.join("metadata.before.bin"), before)?;
    write_atomic(&dir.join("metadata.after.bin"), after)?;
    let record = serde_json::json!({
        "frame": frame,
        "video_offset": u64::from(frame) * source_frame.len() as u64,
        "metadata_offset": u64::from(frame) * before.len() as u64,
        "source_frame_sha256": hex_lower(&Sha256::digest(source_frame)),
        "metadata_before_sha256": hex_lower(&Sha256::digest(before)),
        "metadata_after_sha256": hex_lower(&Sha256::digest(after)),
    });
    write_atomic(
        &dir.join("manifest.json"),
        &serde_json::to_vec_pretty(&record)?,
    )?;
    Ok(KeyEvidence {
        dir,
        source_frame_sha256: record["source_frame_sha256"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    })
}

fn write_atomic(path: &Path, data: &[u8]) -> anyhow::Result<()> {
    let tmp = path.with_extension("tmp");
    let mut file = File::create(&tmp)?;
    file.write_all(data)?;
    file.sync_all()?;
    fs::rename(tmp, path)?;
    Ok(())
}

fn unix_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
