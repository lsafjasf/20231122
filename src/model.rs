use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameKind {
    I,
    P,
    B,
}

impl FrameKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::I => "I",
            Self::P => "P",
            Self::B => "B",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AddressPolicy {
    DecodeOrdinal,
    PtsStable,
}

impl AddressPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DecodeOrdinal => "decode-ordinal",
            Self::PtsStable => "pts-stable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameRecord {
    pub object_id: String,
    pub frame_kind: FrameKind,
    pub pts_ns: u64,
    pub dts_ns: u64,
    pub duration_ns: u64,
    #[serde(default)]
    pub references: Vec<String>,
    pub payload: String,
    pub checksum: String,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RecordError {
    #[error("object_id is empty")]
    EmptyObjectId,
    #[error("duration_ns must be greater than zero")]
    ZeroDuration,
    #[error("I-frame must not have references")]
    IFrameHasReferences,
    #[error("P/B-frame must have at least one reference")]
    MissingReferences,
    #[error("payload is empty")]
    EmptyPayload,
    #[error("checksum must be 64 lowercase hex characters")]
    BadChecksumFormat,
    #[error("checksum mismatch: expected {expected}, computed {actual}")]
    ChecksumMismatch { expected: String, actual: String },
}

impl FrameRecord {
    pub fn payload_digest(&self) -> String {
        sha256_hex(self.payload.as_bytes())
    }

    pub fn validate(&self) -> Result<(), RecordError> {
        if self.object_id.trim().is_empty() {
            return Err(RecordError::EmptyObjectId);
        }
        if self.duration_ns == 0 {
            return Err(RecordError::ZeroDuration);
        }
        match self.frame_kind {
            FrameKind::I if !self.references.is_empty() => {
                return Err(RecordError::IFrameHasReferences);
            }
            FrameKind::P | FrameKind::B if self.references.is_empty() => {
                return Err(RecordError::MissingReferences);
            }
            _ => {}
        }
        if self.payload.is_empty() {
            return Err(RecordError::EmptyPayload);
        }
        if self.checksum.len() != 64
            || !self
                .checksum
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(RecordError::BadChecksumFormat);
        }
        let actual = self.payload_digest();
        if self.checksum != actual {
            return Err(RecordError::ChecksumMismatch {
                expected: self.checksum.clone(),
                actual,
            });
        }
        Ok(())
    }

    pub fn content_digest(&self) -> String {
        let bytes = serde_json::to_vec(self).expect("FrameRecord is serializable");
        sha256_hex(&bytes)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub schema_version: u32,
    pub version: String,
    pub label: String,
    pub address_policy: AddressPolicy,
    pub frames: Vec<FrameRecord>,
}

impl Snapshot {
    pub fn digest(&self) -> String {
        let bytes = serde_json::to_vec(self).expect("Snapshot is serializable");
        sha256_hex(&bytes)
    }

    pub fn frame(&self, object_id: &str) -> Option<&FrameRecord> {
        self.frames
            .iter()
            .find(|frame| frame.object_id == object_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddressEntry {
    pub object_id: String,
    pub address: String,
    pub frame_kind: FrameKind,
    pub pts_ns: u64,
    pub dts_ns: u64,
    pub submission_index: u64,
    pub content_digest: String,
    pub references: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateView {
    pub version: String,
    pub policy: Option<AddressPolicy>,
    pub submission_seq: u64,
    pub accepted: usize,
    pub duplicates: usize,
    pub conflicts: usize,
    pub invalid: usize,
    pub entries: BTreeMap<String, AddressEntry>,
}

impl StateView {
    pub fn empty() -> Self {
        Self {
            version: String::new(),
            policy: None,
            submission_seq: 0,
            accepted: 0,
            duplicates: 0,
            conflicts: 0,
            invalid: 0,
            entries: BTreeMap::new(),
        }
    }

    pub fn digest(&self) -> String {
        let bytes = serde_json::to_vec(self).expect("StateView is serializable");
        sha256_hex(&bytes)
    }

    pub fn b_frame_addresses(&self) -> BTreeMap<String, String> {
        self.entries
            .iter()
            .filter(|(_, entry)| entry.frame_kind == FrameKind::B)
            .map(|(id, entry)| (id.clone(), entry.address.clone()))
            .collect()
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write;
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

pub fn stable_json<T: Serialize>(value: &T) -> serde_json::Value {
    serde_json::to_value(value).expect("value is serializable")
}
