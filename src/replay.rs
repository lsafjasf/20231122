//! 重放复核：按记录中的参数重新分析媒体，逐条比对，
//! 报告第一个不一致状态（而不仅是总分）。
use crate::detect;
use crate::params::SafeFrameParams;
use crate::record::{self, Entry, Record};
use anyhow::{Result, anyhow};
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
pub struct Mismatch {
    pub seq: u64,
    pub pts_ms: i64,
    pub frame: u64,
    pub track: String,
    pub recorded_metric: f64,
    pub replayed_metric: Option<f64>,
    pub recorded_violation: bool,
    pub replayed_violation: Option<bool>,
    pub params_hash: String,
    pub kind: String,
}

#[derive(Debug, Serialize)]
pub struct ReplayReport {
    pub record_format: String,
    pub was_recovered: bool,
    pub dropped_bytes: u64,
    pub entries_checked: usize,
    pub params_hash: String,
    pub first_error: Option<Entry>,
    pub first_mismatch: Option<Mismatch>,
    pub wave_crc_ok: Option<bool>,
    pub verdict: String,
}

const EPS: f64 = 1e-12;

pub fn replay(record_path: &Path, media_override: Option<&Path>) -> Result<ReplayReport> {
    let rec = record::load(record_path)?;
    let (params, media) = resolve(&rec, media_override)?;
    let replayed = detect::analyze(&media, &params, |_, _, _, _| {})?;
    let mut first_mismatch = None;
    let n = rec.entries.len().max(replayed.len());
    for i in 0..n {
        let r = rec.entries.get(i);
        let p = replayed.get(i);
        let bad = match (r, p) {
            (Some(r), Some(p)) => (r.metric - p.metric).abs() > EPS || r.violation != p.violation,
            (Some(_), None) => true,
            (None, Some(_)) => true,
            (None, None) => false,
        };
        if bad {
            let m = match (r, p) {
                (Some(r), p) => Mismatch {
                    seq: r.seq,
                    pts_ms: r.pts_ms,
                    frame: r.frame,
                    track: r.track.clone(),
                    recorded_metric: r.metric,
                    replayed_metric: p.map(|x| x.metric),
                    recorded_violation: r.violation,
                    replayed_violation: p.map(|x| x.violation),
                    params_hash: r.params_hash.clone(),
                    kind: if p.is_none() {
                        "missing_in_replay"
                    } else {
                        "metric_diverged"
                    }
                    .into(),
                },
                (None, Some(p)) => Mismatch {
                    seq: i as u64,
                    pts_ms: p.pts_ms,
                    frame: p.frame,
                    track: "video0".into(),
                    recorded_metric: f64::NAN,
                    replayed_metric: Some(p.metric),
                    recorded_violation: false,
                    replayed_violation: Some(p.violation),
                    params_hash: params.hash(),
                    kind: "missing_in_record".into(),
                },
                _ => unreachable!(),
            };
            first_mismatch = Some(m);
            break;
        }
    }
    let verdict = if first_mismatch.is_some() {
        "MISMATCH"
    } else if rec.tail_garbage_bytes > 0 {
        "TRUNCATED_UNRECOVERED"
    } else {
        "OK"
    };
    Ok(ReplayReport {
        record_format: format!("{:?}", rec.format),
        was_recovered: rec.recovered.is_some(),
        dropped_bytes: rec.recovered.as_ref().map(|m| m.dropped_bytes).unwrap_or(0),
        entries_checked: rec.entries.len().min(replayed.len()),
        params_hash: params.hash(),
        first_error: rec.first_error().cloned(),
        first_mismatch,
        wave_crc_ok: rec.wave_crc_ok,
        verdict: verdict.into(),
    })
}

fn resolve(rec: &Record, media_override: Option<&Path>) -> Result<(SafeFrameParams, PathBuf)> {
    let params = rec
        .header
        .as_ref()
        .map(|h| h.params.clone())
        .unwrap_or_default();
    let media = media_override
        .map(|p| p.to_path_buf())
        .or_else(|| rec.header.as_ref().map(|h| PathBuf::from(&h.media)));
    let media = media.ok_or_else(|| anyhow!("v1 记录不含媒体路径，请用 --media 指定"))?;
    if !media.exists() {
        return Err(anyhow!("媒体不存在: {}", media.display()));
    }
    Ok((params, media))
}
