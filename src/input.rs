use crate::model::{canonical_json, sha256_hex};
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct InputRecord {
    pub index: usize,
    pub byte_offset: usize,
    pub raw: Vec<u8>,
    pub value: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct InputReadError {
    pub rule: &'static str,
    pub byte_offset: usize,
    pub raw: Vec<u8>,
    pub sha256: String,
    pub parse_error: String,
    pub complete_prefix_records: usize,
}

#[derive(Debug, Clone)]
pub struct JsonlRead {
    pub records: Vec<InputRecord>,
    pub error: Option<InputReadError>,
    pub total_bytes: usize,
    pub final_newline: bool,
    pub raw_sha256: String,
}

impl JsonlRead {
    pub fn input_sha256(&self) -> String {
        let mut hasher = sha2::Sha256::new();
        use sha2::Digest;
        for record in &self.records {
            hasher.update(canonical_json(&record.value));
        }
        if let Some(error) = &self.error {
            hasher.update(canonical_json(error));
        }
        hex::encode(hasher.finalize())
    }
}

pub fn read_jsonl(bytes: &[u8]) -> JsonlRead {
    let mut records = Vec::new();
    let mut error = None;
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        let remainder = &bytes[cursor..];
        let Some(relative_newline) = remainder.iter().position(|byte| *byte == b'\n') else {
            let tail = remainder.to_vec();
            if !tail.is_empty() {
                let parse_error = serde_json::from_slice::<Value>(&tail)
                    .err()
                    .map(|failure| failure.to_string())
                    .unwrap_or_else(|| "record is not terminated by a newline".to_string());
                error = Some(InputReadError {
                    rule: "input_truncated",
                    byte_offset: cursor,
                    sha256: sha256_hex(&tail),
                    raw: tail,
                    parse_error,
                    complete_prefix_records: records.len(),
                });
            }
            break;
        };
        let line_end = cursor + relative_newline;
        let raw = bytes[cursor..line_end].to_vec();
        if raw.iter().all(u8::is_ascii_whitespace) {
            error = Some(InputReadError {
                rule: "malformed_json",
                byte_offset: cursor,
                sha256: sha256_hex(&raw),
                raw,
                parse_error: "empty JSONL record".to_string(),
                complete_prefix_records: records.len(),
            });
            break;
        }
        match serde_json::from_slice::<Value>(&raw) {
            Ok(value) => records.push(InputRecord {
                index: records.len(),
                byte_offset: cursor,
                raw,
                value,
            }),
            Err(failure) => {
                error = Some(InputReadError {
                    rule: "malformed_json",
                    byte_offset: cursor,
                    sha256: sha256_hex(&raw),
                    raw,
                    parse_error: failure.to_string(),
                    complete_prefix_records: records.len(),
                });
                break;
            }
        }
        cursor = line_end + 1;
    }
    JsonlRead {
        total_bytes: bytes.len(),
        final_newline: bytes.last() == Some(&b'\n'),
        raw_sha256: sha256_hex(bytes),
        records,
        error,
    }
}

pub fn truncate_after_record(bytes: &[u8], complete_records: usize, retain_tail: usize) -> Vec<u8> {
    let mut newlines = 0usize;
    let mut offset = 0usize;
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'\n' {
            newlines += 1;
            if newlines == complete_records {
                offset = index + 1;
                break;
            }
        }
    }
    let end = (offset + retain_tail).min(bytes.len());
    bytes[..end].to_vec()
}
