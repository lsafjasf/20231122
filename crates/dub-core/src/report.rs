//! Export and independent review.
//!
//! `export_report` writes `report.json` — a self-contained, field-ordered
//! document a colleague can archive together with the segment files and
//! `evidence.jsonl`. `verify_workdir` re-checks everything from disk alone:
//! checkpoint field order, evidence hash chain, and every segment hash.

use std::path::Path;

use serde_json::{json, Map, Value};

use crate::evidence::{self, hex_sha256};
use crate::pipeline::compare_with_checkpoint;
use crate::state::{self, REPORT_FILE};

/// Pinned top-level key order of the exported report.
pub const REPORT_FIELD_ORDER: [&str; 7] = [
    "report_schema",
    "job",
    "files",
    "clock_sync",
    "degradations",
    "evidence",
    "generated_by",
];

/// Build the report document with pinned field order.
pub fn build_report(workdir: &Path) -> anyhow::Result<Map<String, Value>> {
    let job = state::load_state(workdir)?;
    let ev = evidence::verify_file(&workdir.join(state::EVIDENCE_FILE))?;

    let files: Vec<Value> = job
        .results
        .iter()
        .map(|r| {
            json!({
                "index": r.index,
                "file": r.file,
                "language": r.language,
                "mode": r.mode,
                "bytes": r.bytes,
                "sha256": r.sha256,
            })
        })
        .collect();

    let mut report = Map::new();
    report.insert("report_schema".into(), json!(1));
    report.insert(
        "job".into(),
        json!({
            "job_id": job.job_id,
            "input": job.input,
            "output_dir": job.output_dir,
            "segments_total": job.segments.len(),
            "segments_done": job.results.len(),
            "complete": job.is_complete(),
        }),
    );
    report.insert("files".into(), json!(files));
    report.insert("clock_sync".into(), serde_json::to_value(&job.clock_sync)?);
    report.insert(
        "degradations".into(),
        serde_json::to_value(&job.degradations)?,
    );
    report.insert(
        "evidence".into(),
        json!({
            "file": state::EVIDENCE_FILE,
            "records": ev.records,
            "tip_sha256": ev.tip,
            "chain_ok": ev.ok,
        }),
    );
    report.insert(
        "generated_by".into(),
        json!(concat!("dub-core ", env!("CARGO_PKG_VERSION"))),
    );
    Ok(report)
}

/// Write `report.json` into the workdir; returns its path.
pub fn export_report(workdir: &Path) -> anyhow::Result<std::path::PathBuf> {
    let report = build_report(workdir)?;
    let path = workdir.join(REPORT_FILE);
    std::fs::write(&path, serde_json::to_string_pretty(&report)? + "\n")?;
    Ok(path)
}

#[derive(Debug)]
pub struct ReviewReport {
    pub ok: bool,
    pub checks: Vec<(String, bool, String)>,
}

impl ReviewReport {
    fn push(&mut self, name: &str, ok: bool, detail: String) {
        self.checks.push((name.to_string(), ok, detail));
        if !ok {
            self.ok = false;
        }
    }
}

/// Independent review: trust nothing, recompute everything from disk.
pub fn verify_workdir(workdir: &Path) -> anyhow::Result<ReviewReport> {
    let mut review = ReviewReport {
        ok: true,
        checks: Vec::new(),
    };

    // 1. Checkpoint loads with fields in the pinned order.
    let job = match state::load_state(workdir) {
        Ok(j) => {
            review.push(
                "checkpoint-field-order",
                true,
                "fields and order preserved".into(),
            );
            j
        }
        Err(e) => {
            review.push("checkpoint-field-order", false, e.to_string());
            return Ok(review);
        }
    };

    // 2. Evidence hash chain.
    let ev = evidence::verify_file(&workdir.join(state::EVIDENCE_FILE))?;
    review.push(
        "evidence-chain",
        ev.ok,
        if ev.ok {
            format!("{} records, tip {}", ev.records, &ev.tip[..12])
        } else {
            ev.problems.join("; ")
        },
    );

    // 3. Every segment artifact hashes to the checkpointed value.
    let rows = compare_with_checkpoint(&job);
    let bad: Vec<u32> = rows.iter().filter(|r| !r.3).map(|r| r.0).collect();
    review.push(
        "segment-hashes",
        bad.is_empty(),
        if bad.is_empty() {
            format!("{}/{} artifacts match", rows.len(), job.results.len())
        } else {
            format!("mismatched segments: {bad:?}")
        },
    );

    // 4. Clock-sync table is usable for cross-device translation.
    let sync_ok = job.clock_sync.offset_b_minus_a_ns().is_some();
    review.push(
        "clock-sync",
        sync_ok,
        if sync_ok {
            format!(
                "offset(b-a)={} ns",
                job.clock_sync.offset_b_minus_a_ns().unwrap()
            )
        } else {
            "no clock samples recorded".into()
        },
    );

    // 5. Segment coverage: results are exactly segments [0, next_segment).
    let coverage_ok = job.results.iter().map(|r| r.index).eq(0..job.next_segment);
    review.push(
        "segment-coverage",
        coverage_ok,
        format!(
            "next_segment={} results={}",
            job.next_segment,
            job.results.len()
        ),
    );

    // 6. Evidence log hashes to a stable digest for archival.
    let ev_bytes = std::fs::read(workdir.join(state::EVIDENCE_FILE))?;
    review.push("evidence-digest", true, hex_sha256(&ev_bytes));

    Ok(review)
}
