use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::SCHEMA;
use crate::evidence::{sha256_bytes, sha256_json};
use crate::model::{Action, RecordedInput, RecordedMedia, RecoveredRecord};

pub const DEMO_RECORD_ID: &str = "take-001-recovered";
pub const DEMO_RECOVERED_AT: &str = "2026-11-22T01:20:00Z";
pub const DEMO_MEDIA_REF: &str = "fixtures/media/take-001.wav";
pub const GSTREAMER_PROBE_JSON: &str = include_str!("../fixtures/gstreamer-probe.json");

pub fn demo_record() -> RecoveredRecord {
    let wav = demo_wave_bytes();
    let fields = probe_fields();
    let probe_sha256 = sha256_json(&fields);

    let mut staged = fields.clone();
    let stages = [
        ("title", "Harbor Lights (restored take)"),
        ("artist", "Lin & The Night Shift"),
        ("album", "Recovery Sessions"),
    ];
    let mut inputs = vec![
        input(
            "evt-open-0001",
            0,
            "sub-open-0001",
            Action::Open {
                uri: DEMO_MEDIA_REF.to_owned(),
                asset_sha256: sha256_bytes(&wav),
                observed_bytes: wav.len() as u64,
            },
        ),
        input(
            "evt-probe-0002",
            1,
            "sub-probe-0002",
            Action::Probe {
                fields: fields.clone(),
                probe_sha256,
            },
        ),
    ];
    for (offset, (field, value)) in stages.into_iter().enumerate() {
        let revision = offset as u64 + 1;
        staged.insert(field.to_owned(), value.to_owned());
        inputs.push(input(
            format!("evt-stage-{:04}", offset + 3),
            revision + 1,
            format!("sub-stage-{:04}", offset + 3),
            Action::StageMetadata {
                field: field.to_owned(),
                value: value.to_owned(),
                revision,
            },
        ));
    }
    inputs.push(input(
        "evt-submit-0006",
        5,
        "sub-commit-0006",
        Action::Submit {
            endpoint: "memory://metadata-index/take-001".to_owned(),
            payload_sha256: sha256_json(&staged),
        },
    ));
    inputs.push(input(
        "evt-verify-0007",
        6,
        "sub-verify-0007",
        Action::Verify {
            expected_submission_id: "sub-commit-0006".to_owned(),
        },
    ));

    RecoveredRecord::new(
        DEMO_RECORD_ID,
        "20231122/camera-a/truncated-capture.jsonl",
        DEMO_RECOVERED_AT,
        RecordedMedia {
            uri: DEMO_MEDIA_REF.to_owned(),
            size_bytes: wav.len() as u64,
            sha256: sha256_bytes(&wav),
            duration_ms: 600,
            probe_engine: "gstreamer-pbutils Discoverer (recovered pass)".to_owned(),
        },
        inputs,
    )
}

fn input(
    event_id: impl Into<String>,
    sequence: u64,
    submission_id: impl Into<String>,
    action: Action,
) -> RecordedInput {
    RecordedInput {
        event_id: event_id.into(),
        sequence,
        submission_id: submission_id.into(),
        action,
    }
}

pub fn probe_fields() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("album".to_owned(), "Recovery Sessions".to_owned()),
        ("artist".to_owned(), "Lin & The Night Shift".to_owned()),
        (
            "audio_codec".to_owned(),
            "Uncompressed 16-bit PCM audio".to_owned(),
        ),
        ("channels".to_owned(), "1".to_owned()),
        ("container_format".to_owned(), "WAV".to_owned()),
        ("duration_ms".to_owned(), "600".to_owned()),
        ("sample_rate".to_owned(), "8000".to_owned()),
        ("title".to_owned(), "Harbor Lights".to_owned()),
    ])
}

pub fn write_demo_media(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, demo_wave_bytes())
}

pub fn demo_wave_bytes() -> Vec<u8> {
    const SAMPLE_RATE: u32 = 8_000;
    const SAMPLES: u32 = SAMPLE_RATE * 6 / 10;
    const AMPLITUDE: f32 = 5_500.0;
    let mut pcm = Vec::with_capacity(SAMPLES as usize * 2);
    for index in 0..SAMPLES {
        let phase = 2.0 * std::f32::consts::PI * 440.0 * index as f32 / SAMPLE_RATE as f32;
        let sample = (phase.sin() * AMPLITUDE) as i16;
        pcm.extend_from_slice(&sample.to_le_bytes());
    }

    let info = [
        info_chunk(b"INAM", "Harbor Lights"),
        info_chunk(b"IART", "Lin & The Night Shift"),
        info_chunk(b"IPRD", "Recovery Sessions"),
    ]
    .concat();
    let mut list_payload = b"INFO".to_vec();
    list_payload.extend_from_slice(&info);

    let riff_size = 4 + (8 + 16) + (8 + list_payload.len() as u32) + (8 + pcm.len() as u32);
    let mut wav = Vec::with_capacity(riff_size as usize + 8);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&riff_size.to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    wav.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"LIST");
    wav.extend_from_slice(&(list_payload.len() as u32).to_le_bytes());
    wav.extend_from_slice(&list_payload);
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    wav.extend_from_slice(&pcm);
    wav
}

fn info_chunk(id: &[u8; 4], value: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(id);
    let mut text = value.as_bytes().to_vec();
    text.push(0);
    if !text.len().is_multiple_of(2) {
        text.push(0);
    }
    bytes.extend_from_slice(&(text.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&text);
    bytes
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TruncatedHeader {
    pub schema: String,
    pub kind: String,
    pub record_id: String,
    pub source_take: String,
    pub recovered_at: String,
    pub media: RecordedMedia,
    pub intended_input_count: usize,
    pub truncation_reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TruncationStats {
    pub total_bytes: usize,
    pub valid_prefix_bytes: usize,
    pub incomplete_tail_bytes: usize,
    pub complete_lines: usize,
    pub complete_inputs: usize,
    pub file_sha256: String,
    pub valid_prefix_sha256: String,
    pub incomplete_tail_preview: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedTruncatedRecord {
    pub header: TruncatedHeader,
    pub inputs: Vec<RecordedInput>,
    pub stats: TruncationStats,
}

impl LoadedTruncatedRecord {
    pub fn replay_inputs(&self) -> Vec<RecordedInput> {
        self.inputs.clone()
    }
}

pub fn write_truncated_fixture(
    record: &RecoveredRecord,
    path: &Path,
    complete_inputs: usize,
) -> io::Result<()> {
    let bytes = truncated_fixture_bytes(record, complete_inputs)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, bytes)
}

pub fn truncated_fixture_bytes(
    record: &RecoveredRecord,
    complete_inputs: usize,
) -> io::Result<Vec<u8>> {
    if complete_inputs >= record.inputs.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "truncation must remove at least part of one input",
        ));
    }
    let header = TruncatedHeader {
        schema: SCHEMA.to_owned(),
        kind: "truncated_capture_header".to_owned(),
        record_id: format!("{}-truncated", record.record_id),
        source_take: record.source_take.clone(),
        recovered_at: record.recovered_at.clone(),
        media: record.media.clone(),
        intended_input_count: record.inputs.len(),
        truncation_reason: "process stopped while flushing the submit line".to_owned(),
    };
    let mut bytes = serde_json::to_vec(&header).map_err(io::Error::other)?;
    bytes.push(b'\n');
    for input in record.inputs.iter().take(complete_inputs) {
        bytes.extend_from_slice(&serde_json::to_vec(input).map_err(io::Error::other)?);
        bytes.push(b'\n');
    }

    let mut tail = serde_json::to_vec(&record.inputs[complete_inputs]).map_err(io::Error::other)?;
    let cut = tail.len().saturating_sub(44).max(1);
    tail.truncate(cut);
    bytes.extend_from_slice(&tail);
    Ok(bytes)
}

pub fn load_truncated_fixture(path: &Path) -> io::Result<LoadedTruncatedRecord> {
    let bytes = fs::read(path)?;
    load_truncated_bytes(&bytes)
}

pub fn load_truncated_bytes(bytes: &[u8]) -> io::Result<LoadedTruncatedRecord> {
    let text = String::from_utf8(bytes.to_vec()).map_err(io::Error::other)?;
    let mut header = None;
    let mut inputs = Vec::new();
    let mut valid_prefix_bytes = 0;
    let mut complete_lines = 0;
    let mut incomplete_tail = "";

    for segment in text.split_inclusive('\n') {
        if !segment.ends_with('\n') {
            incomplete_tail = segment;
            break;
        }
        valid_prefix_bytes += segment.len();
        complete_lines += 1;
        let line = segment.trim();
        if line.is_empty() {
            continue;
        }
        if header.is_none() {
            let parsed: TruncatedHeader = serde_json::from_str(line).map_err(io::Error::other)?;
            if parsed.schema != SCHEMA || parsed.kind != "truncated_capture_header" {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "truncated capture header has an unexpected schema",
                ));
            }
            header = Some(parsed);
        } else {
            inputs.push(serde_json::from_str::<RecordedInput>(line).map_err(io::Error::other)?);
        }
    }

    let header = header.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "truncated capture is missing its complete header",
        )
    })?;
    let incomplete_tail_bytes = text.len().saturating_sub(valid_prefix_bytes);
    let incomplete_tail_preview: String = incomplete_tail.chars().take(160).collect();
    let stats = TruncationStats {
        total_bytes: text.len(),
        valid_prefix_bytes,
        incomplete_tail_bytes,
        complete_lines,
        complete_inputs: inputs.len(),
        file_sha256: sha256_bytes(bytes),
        valid_prefix_sha256: sha256_bytes(&bytes[..valid_prefix_bytes]),
        incomplete_tail_preview,
    };
    Ok(LoadedTruncatedRecord {
        header,
        inputs,
        stats,
    })
}

pub fn load_recovered_record(path: &Path) -> io::Result<RecoveredRecord> {
    let bytes = fs::read(path)?;
    let record: RecoveredRecord = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    if record.schema != SCHEMA {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "recovered record has an unexpected schema",
        ));
    }
    Ok(record)
}
