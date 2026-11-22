use std::fs;
use std::io;
use std::path::Path;

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::model::{RecordedInput, ReplayState, StepEvidence, StepOutcome};

pub fn canonical_json<T: Serialize>(value: &T) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec(value)
}

pub fn sha256_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    to_hex(&digest)
}

pub fn sha256_text(text: &str) -> String {
    sha256_bytes(text.as_bytes())
}

pub fn sha256_json<T: Serialize>(value: &T) -> String {
    let bytes = canonical_json(value).expect("serializable evidence payload");
    sha256_bytes(&bytes)
}

pub fn file_sha256(path: &Path) -> io::Result<String> {
    Ok(sha256_bytes(&fs::read(path)?))
}

fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[derive(Serialize)]
struct EvidenceMaterial<'a> {
    index: usize,
    previous_evidence_sha256: &'a str,
    input_sha256: &'a str,
    state_before_sha256: &'a str,
    state_after_sha256: &'a str,
    result_sha256: &'a str,
}

pub fn make_step_evidence(
    index: usize,
    previous_evidence_sha256: &str,
    input: &RecordedInput,
    state_before: &ReplayState,
    state_after: &ReplayState,
    outcome: &StepOutcome,
) -> StepEvidence {
    let input_sha256 = sha256_json(input);
    let state_before_sha256 = sha256_json(state_before);
    let state_after_sha256 = sha256_json(state_after);
    let result_sha256 = sha256_json(outcome);
    let material = EvidenceMaterial {
        index,
        previous_evidence_sha256,
        input_sha256: &input_sha256,
        state_before_sha256: &state_before_sha256,
        state_after_sha256: &state_after_sha256,
        result_sha256: &result_sha256,
    };
    let evidence_sha256 = sha256_json(&material);

    StepEvidence {
        input_sha256,
        state_before_sha256,
        state_after_sha256,
        result_sha256,
        previous_evidence_sha256: previous_evidence_sha256.to_owned(),
        evidence_sha256,
    }
}

pub const GENESIS_EVIDENCE: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";
