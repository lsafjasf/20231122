use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub const SESSION_V2: &str = "frame-loss-review/session-v2";
pub const RECORD_V2: &str = "frame-loss-review/record-v2";
pub const LEGACY_V1: &str = "frame-loss-review/v1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SourceRef {
    pub path: String,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Track {
    pub id: String,
    pub index: u32,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caps: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProcessingParams {
    pub mode: String,
    pub before_frames: u32,
    pub after_frames: u32,
    pub blend: f64,
    pub min_confidence: f64,
    pub max_conceal_frames: u32,
}

impl ProcessingParams {
    pub fn validate(&self) -> Result<()> {
        if self.mode != "blend" {
            bail!("unsupported compensation mode {:?}", self.mode);
        }
        if !(0.0..=1.0).contains(&self.blend) {
            bail!("blend must be in [0, 1]");
        }
        if !(0.0..=1.0).contains(&self.min_confidence) {
            bail!("min_confidence must be in [0, 1]");
        }
        if self.before_frames == 0 || self.after_frames == 0 {
            bail!("compensation requires at least one frame on each side");
        }
        Ok(())
    }

    pub fn fingerprint(&self) -> String {
        let canonical = serde_json::to_vec(self).expect("params are serializable");
        short_sha256(&canonical)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FrameState {
    Present,
    Missing,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Frame {
    pub frame: u64,
    pub pts_ns: i64,
    pub time_ms: u64,
    pub duration_ms: u32,
    pub state: FrameState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_luma: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReferenceFrame {
    pub frame: u64,
    pub time_ms: u64,
    pub luma: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Compensation {
    pub applied: bool,
    pub mode: String,
    pub gap_position: u32,
    pub gap_length: u32,
    pub confidence: f64,
    pub reference_before: ReferenceFrame,
    pub reference_after: ReferenceFrame,
    pub blend: f64,
    pub output_luma: f64,
}

impl Compensation {
    pub fn expected_luma(&self) -> f64 {
        let before = self.reference_before.luma;
        before + (self.reference_after.luma - before) * self.blend
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Record {
    pub schema: String,
    pub record_id: String,
    pub sequence: u64,
    pub track: Track,
    pub frame: Frame,
    pub params: ProcessingParams,
    pub params_fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compensation: Option<Compensation>,
}

impl Record {
    pub fn from_value(value: Value) -> Result<Self> {
        let record: Self = serde_json::from_value(value).context("invalid record-v2 JSON")?;
        if record.schema != RECORD_V2 {
            bail!("unsupported record schema {:?}", record.schema);
        }
        Ok(record)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Compatibility {
    pub source_schema: String,
    pub migrated: bool,
    pub raw_sha256: String,
    pub raw_size_bytes: u64,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Session {
    pub schema: String,
    pub session_id: String,
    pub source: SourceRef,
    pub track: Track,
    pub params: ProcessingParams,
    pub records: Vec<Record>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compatibility: Option<Compatibility>,
    #[serde(default)]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LegacyFile {
    pub file: String,
    pub fps: f64,
    pub frames: Vec<LegacyFrame>,
    #[serde(default)]
    pub notes: Vec<String>,
    pub params: LegacyParams,
    pub track_id: String,
    pub version: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LegacyFrame {
    pub frame_number: u64,
    pub timestamp_ms: u64,
    #[serde(default)]
    pub luma: Option<f64>,
    #[serde(default)]
    pub present: bool,
    #[serde(default)]
    pub lost: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LegacyParams {
    pub after: u32,
    pub before: u32,
    pub confidence: f64,
    pub max_gap: u32,
    pub mix: f64,
    pub mode: String,
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

pub fn short_sha256(bytes: &[u8]) -> String {
    sha256_hex(bytes)[..16].to_string()
}

pub fn canonical_json<T: Serialize>(value: &T) -> Vec<u8> {
    serde_json::to_vec(value).expect("review structures are serializable")
}

pub fn import_legacy(raw: &[u8], path: &str) -> Result<Session> {
    let legacy: LegacyFile = serde_json::from_slice(raw).context("invalid v1 JSON")?;
    if legacy.version != 1 {
        bail!("unsupported legacy version {}", legacy.version);
    }
    if !(legacy.fps.is_finite() && legacy.fps > 0.0) {
        bail!("legacy fps must be positive");
    }
    let params = ProcessingParams {
        mode: legacy.params.mode,
        before_frames: legacy.params.before,
        after_frames: legacy.params.after,
        blend: legacy.params.mix,
        min_confidence: legacy.params.confidence,
        max_conceal_frames: legacy.params.max_gap,
    };
    params.validate()?;
    let duration_ms = (1000.0 / legacy.fps).round() as u32;
    let mut warnings = Vec::new();
    let mut records = Vec::with_capacity(legacy.frames.len());

    for (index, old) in legacy.frames.iter().enumerate() {
        let state = if old.lost || !old.present {
            FrameState::Missing
        } else {
            FrameState::Present
        };
        let source_luma = old.luma.filter(|luma| (0.0..=1.0).contains(luma));
        if state == FrameState::Present && source_luma.is_none() {
            bail!(
                "legacy frame {} is present but has no valid luma",
                old.frame_number
            );
        }
        let pts_ns = (old.timestamp_ms as i128 * 1_000_000) as i64;
        warnings.push(format!(
            "frame {}: pts_ns inferred from time_ms ({})",
            old.frame_number, old.timestamp_ms
        ));
        let compensation = if state == FrameState::Missing {
            legacy_compensation(&legacy.frames, index, &params)?
        } else {
            None
        };
        records.push(Record {
            schema: RECORD_V2.to_string(),
            record_id: format!("legacy-{:04}", old.frame_number),
            sequence: index as u64,
            track: Track {
                id: legacy.track_id.clone(),
                index: 0,
                kind: "video".to_string(),
                caps: None,
            },
            frame: Frame {
                frame: old.frame_number,
                pts_ns,
                time_ms: old.timestamp_ms,
                duration_ms,
                state,
                source_luma,
            },
            params: params.clone(),
            params_fingerprint: params.fingerprint(),
            compensation,
        });
    }
    warnings.push(
        "legacy waveform was reconstructed from the v1 luma envelope; it is not decoded audio/video waveform evidence"
            .to_string(),
    );
    Ok(Session {
        schema: SESSION_V2.to_string(),
        session_id: format!("legacy-import-{}", &sha256_hex(raw)[..12]),
        source: SourceRef {
            path: path.to_string(),
            size_bytes: raw.len() as u64,
            sha256: sha256_hex(raw),
        },
        track: Track {
            id: legacy.track_id,
            index: 0,
            kind: "video".to_string(),
            caps: None,
        },
        params: params.clone(),
        records,
        compatibility: Some(Compatibility {
            source_schema: LEGACY_V1.to_string(),
            migrated: true,
            raw_sha256: sha256_hex(raw),
            raw_size_bytes: raw.len() as u64,
            warnings,
        }),
        notes: legacy.notes,
    })
}

fn legacy_compensation(
    frames: &[LegacyFrame],
    index: usize,
    params: &ProcessingParams,
) -> Result<Option<Compensation>> {
    let before = frames[..index]
        .iter()
        .rev()
        .find(|frame| frame.present && !frame.lost && frame.luma.is_some())
        .and_then(|frame| {
            frame.luma.map(|luma| ReferenceFrame {
                frame: frame.frame_number,
                time_ms: frame.timestamp_ms,
                luma,
            })
        });
    let after = frames[index + 1..]
        .iter()
        .find(|frame| frame.present && !frame.lost && frame.luma.is_some())
        .and_then(|frame| {
            frame.luma.map(|luma| ReferenceFrame {
                frame: frame.frame_number,
                time_ms: frame.timestamp_ms,
                luma,
            })
        });
    let (Some(reference_before), Some(reference_after)) = (before, after) else {
        return Ok(None);
    };
    let mut gap_start = index;
    while gap_start > 0 && !(frames[gap_start - 1].present && !frames[gap_start - 1].lost) {
        gap_start -= 1;
    }
    let mut gap_end = index + 1;
    while gap_end < frames.len() && !(frames[gap_end].present && !frames[gap_end].lost) {
        gap_end += 1;
    }
    let gap_length = (gap_end - gap_start) as u32;
    let gap_position = (index - gap_start + 1) as u32;
    let confidence = (0.95 - 0.03 * gap_position.saturating_sub(1) as f64).clamp(0.0, 1.0);
    let blend = params.blend;
    let output_luma =
        reference_before.luma + (reference_after.luma - reference_before.luma) * blend;
    Ok(Some(Compensation {
        applied: true,
        mode: params.mode.clone(),
        gap_position,
        gap_length,
        confidence,
        reference_before,
        reference_after,
        blend,
        output_luma,
    }))
}

pub fn track_map(records: &[Record]) -> BTreeMap<String, Track> {
    records
        .iter()
        .map(|record| (record.track.id.clone(), record.track.clone()))
        .collect()
}
