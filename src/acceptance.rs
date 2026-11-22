use crate::fixture::hex_lower;
use crate::metadata::FrameMetadata;
use crate::model::{METADATA_RECORD_BYTES, SessionPaths};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Write;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcceptanceCheck {
    pub name: String,
    pub pass: bool,
    pub frame: u32,
    pub pts_ns: u64,
    pub video_offset: u64,
    pub video_offset_hex: String,
    pub metadata_offset: u64,
    pub metadata_offset_hex: String,
    pub repeated_object_present: bool,
    pub before: FrameMetadata,
    pub after: FrameMetadata,
    pub before_record_sha256: String,
    pub after_record_sha256: String,
    pub key_evidence_dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResumeComparison {
    pub paused_frame: u32,
    pub paused_pts_ns: u64,
    pub paused_video_offset: u64,
    pub paused_metadata_offset: u64,
    pub resumed_from_frame: u32,
    pub prefix_verified: bool,
    pub uninterrupted_output_sha256: Option<String>,
    pub resumed_output_sha256: String,
    pub output_equal: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcceptanceReport {
    pub schema_version: u32,
    pub job_id: String,
    pub generated_unix_ms: u128,
    pub source_video_sha256: String,
    pub source_metadata_sha256: String,
    pub output_metadata_sha256: String,
    pub output_video_sha256: String,
    pub checks: Vec<AcceptanceCheck>,
    pub resource_plan: Option<crate::model::ResourcePlan>,
    pub resumed_count: u32,
    pub resume_comparison: Option<ResumeComparison>,
}

impl AcceptanceReport {
    pub fn write(&self, paths: &SessionPaths) -> anyhow::Result<()> {
        let tmp = paths.acceptance.with_extension("json.tmp");
        let mut file = File::create(&tmp)?;
        serde_json::to_writer_pretty(&mut file, self)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::rename(tmp, &paths.acceptance)?;
        Ok(())
    }
}

pub fn make_check(
    name: impl Into<String>,
    before: &FrameMetadata,
    after: &FrameMetadata,
    before_bytes: &[u8],
    after_bytes: &[u8],
    key_evidence_dir: &std::path::Path,
) -> AcceptanceCheck {
    AcceptanceCheck {
        name: name.into(),
        pass: before.is_camera_fault()
            && after.is_repaired()
            && before.frame == after.frame
            && before.pts_ns == after.pts_ns
            && before.source_offset == after.source_offset
            && before.max_cll != after.max_cll,
        frame: after.frame,
        pts_ns: after.pts_ns,
        video_offset: after.source_offset,
        video_offset_hex: format!("0x{:X}", after.source_offset),
        metadata_offset: u64::from(after.frame) * METADATA_RECORD_BYTES as u64,
        metadata_offset_hex: format!(
            "0x{:X}",
            u64::from(after.frame) * METADATA_RECORD_BYTES as u64
        ),
        repeated_object_present: after.has_repeated_object(),
        before: before.clone(),
        after: after.clone(),
        before_record_sha256: hex_lower(&Sha256::digest(before_bytes)),
        after_record_sha256: hex_lower(&Sha256::digest(after_bytes)),
        key_evidence_dir: key_evidence_dir.display().to_string(),
    }
}
