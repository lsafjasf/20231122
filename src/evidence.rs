use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::model::{
    format_timecode, BoundaryDelta, BoundarySample, Checkpoint, CompareReport, DegradationEvent,
    FrameRecord, ResumeCompare, FRAME_BYTES,
};
use crate::source::{frame_crc, write_ppm};

pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    write_atomic(path, &bytes)
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let tmp = path.with_extension(format!(
        "{}.tmp",
        path.extension().and_then(|s| s.to_str()).unwrap_or("data")
    ));
    {
        let mut file =
            fs::File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    fs::rename(&tmp, path)
        .with_context(|| format!("replace {} with {}", path.display(), tmp.display()))?;
    Ok(())
}

pub fn append_jsonl<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("open {}", path.display()))?;
    serde_json::to_writer(&mut file, value)?;
    file.write_all(b"\n")?;
    Ok(())
}

pub fn save_checkpoint(dir: &Path, checkpoint: &Checkpoint) -> Result<()> {
    write_json(&dir.join("checkpoint.json"), checkpoint)
}

pub fn load_checkpoint(dir: &Path) -> Result<Checkpoint> {
    load_state_file(&dir.join("checkpoint.json"))
}

pub fn load_final_state(dir: &Path) -> Result<Checkpoint> {
    load_state_file(&dir.join("final_state.json"))
}

fn load_state_file(path: &Path) -> Result<Checkpoint> {
    let raw = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_slice(&raw).with_context(|| format!("parse {}", path.display()))
}

pub fn load_frames(dir: &Path) -> Result<Vec<FrameRecord>> {
    let path = dir.join("frames.jsonl");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    raw.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).with_context(|| format!("parse {line}")))
        .collect()
}

pub fn load_crc_chain(dir: &Path) -> Result<u32> {
    let path = dir.join("final_state.json");
    if !path.exists() {
        return Ok(load_frames(dir)?.last().map_or(0, |frame| frame.crc));
    }
    let raw = fs::read(&path)?;
    let value: Value = serde_json::from_slice(&raw)?;
    Ok(value.get("crc_chain").and_then(Value::as_u64).unwrap_or(0) as u32)
}

pub struct Evidence {
    pub dir: PathBuf,
    event_seq: u64,
    event_prev: String,
}

impl Evidence {
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self> {
        let dir = dir.into();
        fs::create_dir_all(&dir)?;
        let mut seq = 0;
        let mut prev = "GENESIS".to_string();
        let events = dir.join("events.jsonl");
        if events.exists() {
            let raw = fs::read_to_string(&events)?;
            if let Some(last) = raw.lines().rfind(|line| !line.trim().is_empty()) {
                let value: Value = serde_json::from_str(last)?;
                seq = value.get("seq").and_then(Value::as_u64).unwrap_or(0) + 1;
                prev = value
                    .get("hash")
                    .and_then(Value::as_str)
                    .unwrap_or("GENESIS")
                    .to_string();
            }
        }
        Ok(Self {
            dir,
            event_seq: seq,
            event_prev: prev,
        })
    }

    pub fn event(&mut self, kind: &str, detail: Value) -> Result<()> {
        let payload = json!({
            "seq": self.event_seq,
            "kind": kind,
            "at_unix": unix_now(),
            "detail": detail,
            "prev": self.event_prev,
        });
        let canonical = serde_json::to_vec(&payload)?;
        let hash = format!("{:016x}", crate::model::fnv1a64(&canonical));
        let row = json!({
            "seq": payload["seq"],
            "kind": payload["kind"],
            "at_unix": payload["at_unix"],
            "detail": payload["detail"],
            "prev": payload["prev"],
            "hash": hash,
        });
        append_jsonl(&self.dir.join("events.jsonl"), &row)?;
        self.event_seq += 1;
        self.event_prev = hash;
        Ok(())
    }

    pub fn write_params(&self, config: &crate::model::Config) -> Result<()> {
        write_json(&self.dir.join("params.json"), config)
    }

    pub fn frame(&self, frame: &FrameRecord) -> Result<()> {
        append_jsonl(&self.dir.join("frames.jsonl"), frame)
    }

    pub fn degradation(&self, event: &DegradationEvent) -> Result<()> {
        append_jsonl(&self.dir.join("degradation.jsonl"), event)
    }

    pub fn boundary(
        &self,
        config: &crate::model::Config,
        before: &FrameRecord,
        after: &FrameRecord,
    ) -> Result<BoundaryDelta> {
        let before_file = "boundary_before.ppm";
        let after_file = "boundary_after.ppm";
        write_ppm(&self.dir.join(before_file), before.idx)?;
        write_ppm(&self.dir.join(after_file), after.idx)?;

        let before_sample = BoundarySample {
            side: "before".into(),
            frame_idx: before.idx,
            pts_ns: before.pts_ns,
            crc: frame_crc(before.idx),
            ppm_file: before_file.into(),
        };
        let after_sample = BoundarySample {
            side: "after".into(),
            frame_idx: after.idx,
            pts_ns: after.pts_ns,
            crc: frame_crc(after.idx),
            ppm_file: after_file.into(),
        };
        write_json(&self.dir.join("boundary_before.json"), &before_sample)?;
        write_json(&self.dir.join("boundary_after.json"), &after_sample)?;

        let delta_ns = after.pts_ns.saturating_sub(before.pts_ns);
        let crc_changed = before_sample.crc != after_sample.crc;
        let report = BoundaryDelta {
            before: before_sample,
            after: after_sample,
            pts_delta_ns: delta_ns,
            expected_step_ns: config.frame_period_ns(),
            missing_span_ns: delta_ns.saturating_sub(config.frame_period_ns()),
            crc_changed,
        };
        write_json(&self.dir.join("boundary_delta.json"), &report)?;
        Ok(report)
    }

    pub fn resume_compare(&self, compare: &ResumeCompare) -> Result<()> {
        write_json(&self.dir.join("resume_compare.json"), compare)
    }

    pub fn final_state(&self, checkpoint: &Checkpoint) -> Result<()> {
        write_json(&self.dir.join("final_state.json"), checkpoint)
    }

    pub fn manifest(&mut self, config: &crate::model::Config) -> Result<()> {
        self.event("evidence-sealed", json!({"manifest": "manifest.json"}))?;
        let mut files = Vec::new();
        let mut names: Vec<String> = fs::read_dir(&self.dir)?
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_type()
                    .map(|kind| kind.is_file())
                    .unwrap_or(false)
            })
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| name != "manifest.json")
            .collect();
        names.sort();
        for name in names {
            let path = self.dir.join(&name);
            let bytes = fs::read(&path)?;
            let digest = Sha256::digest(&bytes);
            files.push(json!({
                "file": name,
                "bytes": bytes.len(),
                "sha256": format!("{digest:x}"),
            }));
        }
        let manifest = json!({
            "version": 1,
            "params_hash": format!("{:016x}", config.params_hash()),
            "frame_bytes": FRAME_BYTES,
            "files": files,
        });
        write_json(&self.dir.join("manifest.json"), &manifest)?;
        Ok(())
    }

    pub fn write_compare(path: &Path, report: &CompareReport) -> Result<()> {
        write_json(path, report)
    }

    pub fn frame_label(frame: &FrameRecord, fps: u32) -> String {
        format!(
            "#{} {} drift={}ns crc={:08x}",
            frame.idx,
            format_timecode(frame.pts_ns, fps),
            frame.drift_ns,
            frame.crc
        )
    }
}
