use crate::input::truncate_after_record;
use crate::model::{
    Compensation, Frame, FrameState, ProcessingParams, RECORD_V2, Record, ReferenceFrame, Track,
};
use crate::review::records_jsonl;
use anyhow::Result;
use serde_json::json;

pub const TRACK_ID: &str = "video-main";
pub const TRUNCATED_PREFIX_RECORDS: usize = 7;
pub const TRUNCATED_TAIL_BYTES: usize = 269;

pub fn params() -> ProcessingParams {
    ProcessingParams {
        mode: "blend".to_string(),
        before_frames: 1,
        after_frames: 1,
        blend: 0.35,
        min_confidence: 0.55,
        max_conceal_frames: 3,
    }
}

pub fn track() -> Track {
    Track {
        id: TRACK_ID.to_string(),
        index: 0,
        kind: "video".to_string(),
        caps: Some("video/x-raw,format=I420,width=640,height=360,framerate=30/1".to_string()),
    }
}

pub fn recovered_records() -> Result<Vec<Record>> {
    let params = params();
    let fingerprint = params.fingerprint();
    let track = track();
    let mut records = Vec::new();
    for index in 0..11u64 {
        let time_ms = ((index as f64) * 1000.0 / 30.0).round() as u64;
        let (state, source_luma) = match index {
            0 => (FrameState::Present, Some(0.18)),
            1 => (FrameState::Present, Some(0.22)),
            2 => (FrameState::Present, Some(0.27)),
            3 => (FrameState::Present, Some(0.32)),
            4 | 5 | 9 => (FrameState::Missing, None),
            6 => (FrameState::Present, Some(0.42)),
            7 => (FrameState::Present, Some(0.46)),
            8 => (FrameState::Present, Some(0.50)),
            10 => (FrameState::Present, Some(0.58)),
            _ => unreachable!(),
        };
        let compensation = match index {
            4 => Some(compensation(
                1,
                2,
                0.91,
                reference(3, 0.32, 100),
                reference(6, 0.42, 200),
            )?),
            5 => Some(compensation(
                2,
                2,
                0.86,
                reference(3, 0.32, 100),
                reference(6, 0.42, 200),
            )?),
            9 => Some(compensation(
                1,
                1,
                0.84,
                reference(8, 0.50, 267),
                reference(10, 0.58, 333),
            )?),
            _ => None,
        };
        records.push(Record {
            schema: RECORD_V2.to_string(),
            record_id: format!("flr-{index:04}"),
            sequence: index,
            track: track.clone(),
            frame: Frame {
                frame: index,
                pts_ns: (time_ms as i64) * 1_000_000,
                time_ms,
                duration_ms: 33,
                state,
                source_luma,
            },
            params: params.clone(),
            params_fingerprint: fingerprint.clone(),
            compensation,
        });
    }
    Ok(records)
}

fn compensation(
    gap_position: u32,
    gap_length: u32,
    confidence: f64,
    before: ReferenceFrame,
    after: ReferenceFrame,
) -> Result<Compensation> {
    let blend = params().blend;
    let output_luma = before.luma + (after.luma - before.luma) * blend;
    Ok(Compensation {
        applied: true,
        mode: "blend".to_string(),
        gap_position,
        gap_length,
        confidence,
        reference_before: before,
        reference_after: after,
        blend,
        output_luma,
    })
}

fn reference(frame: u64, luma: f64, time_ms: u64) -> ReferenceFrame {
    ReferenceFrame {
        frame,
        time_ms,
        luma,
    }
}

pub fn recovered_bytes() -> Result<Vec<u8>> {
    records_jsonl(&recovered_records()?)
}

pub fn truncated_bytes() -> Result<Vec<u8>> {
    let complete = recovered_bytes()?;
    Ok(truncate_after_record(
        &complete,
        TRUNCATED_PREFIX_RECORDS,
        TRUNCATED_TAIL_BYTES,
    ))
}

pub fn legacy_v1_bytes() -> Result<Vec<u8>> {
    let value = json!({
        "file": "fixtures/studio-take.recx",
        "fps": 30.0,
        "frames": [
            {"frame_number": 0, "luma": 0.18, "present": true, "timestamp_ms": 0},
            {"frame_number": 1, "luma": 0.22, "present": true, "timestamp_ms": 33},
            {"frame_number": 2, "luma": 0.27, "present": true, "timestamp_ms": 67},
            {"frame_number": 3, "luma": 0.32, "present": true, "timestamp_ms": 100},
            {"frame_number": 4, "lost": true, "timestamp_ms": 133},
            {"frame_number": 5, "lost": true, "timestamp_ms": 167},
            {"frame_number": 6, "luma": 0.42, "present": true, "timestamp_ms": 200}
        ],
        "notes": [
            "imported from the 2023 field recorder volume",
            "unknown .recx extension is preserved"
        ],
        "params": {
            "after": 1,
            "before": 1,
            "confidence": 0.55,
            "max_gap": 3,
            "mix": 0.35,
            "mode": "blend"
        },
        "project": "studio-frame-loss-review",
        "track_id": TRACK_ID,
        "version": 1
    });
    let mut bytes = serde_json::to_vec_pretty(&value)?;
    bytes.push(b'\n');
    Ok(bytes)
}

pub fn tampered_bytes() -> Result<Vec<u8>> {
    let mut records = recovered_records()?;
    if let Some(record) = records.get_mut(8) {
        record.params_fingerprint = "0000000000000000".to_string();
    }
    records_jsonl(&records)
}
