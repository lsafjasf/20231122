//! Top-level experiment orchestration: snapshots, scenarios, evidence.

use super::engine::{Engine, ProcessOrder, SnapshotReport};
use super::journal::Journal;
use crate::gst_backend;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Declarative description of one replayable experiment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunConfig {
    /// `(label, path)` pairs, including the truncated control fixture.
    pub snapshots: Vec<(String, PathBuf)>,
    /// Record ids that are submitted twice (duplicate-submission scenario).
    pub duplicate_ids: Vec<String>,
    /// If true, the healthy snapshots are also processed in reverse order.
    pub verify_reversed_order: bool,
    /// Directory for durable evidence (journal, report, gst probe output).
    pub evidence_dir: PathBuf,
    /// Scratch directory removed at the end of a successful run.
    pub temp_dir: PathBuf,
    /// Run GStreamer pipeline probes in addition to pure diagnosis.
    pub run_gst_probes: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GstProbeEvidence {
    pub record_id: String,
    pub manifest_url: String,
    pub element_factory: String,
    pub probe_ok: bool,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RunOutcome {
    pub run_id: String,
    pub reports: Vec<SnapshotReport>,
    pub gst_probes: Vec<GstProbeEvidence>,
    pub journal_path: PathBuf,
    pub report_path: PathBuf,
    pub totals: Totals,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Totals {
    pub snapshots_loaded: usize,
    pub snapshots_failed: usize,
    pub records_seen: usize,
    pub accepted: usize,
    pub duplicates: usize,
    pub invalid: usize,
}

/// Execute the experiment described by `config`, returning structured
/// evidence. Every step is journaled before this function returns.
pub fn run(config: &RunConfig, run_id: &str) -> std::io::Result<RunOutcome> {
    std::fs::create_dir_all(&config.evidence_dir)?;
    std::fs::create_dir_all(&config.temp_dir)?;

    let journal_path = config.evidence_dir.join(format!("{run_id}.journal.jsonl"));
    let report_path = config.evidence_dir.join(format!("{run_id}.report.json"));
    let mut journal = Journal::create(&journal_path)?;

    journal
        .record(
            "run_start",
            serde_json::to_value(config).map_err(io_data)?,
            serde_json::json!({"run_id": run_id}),
            serde_json::json!({"status": "started"}),
        )
        .map_err(io_other)?;

    let mut reports = Vec::new();
    let mut gst_probes = Vec::new();

    for (label, path) in &config.snapshots {
        let mut engine = Engine::new(&mut journal);
        match engine.load_snapshot(label, path) {
            Ok(snapshot) => {
                let forward = engine.process_snapshot(
                    &snapshot,
                    ProcessOrder::Submission,
                    &config.duplicate_ids,
                );
                if config.verify_reversed_order {
                    // Fresh dedup state so the reversed pass demonstrates the
                    // changed processing order rather than idempotency.
                    let mut reverse_engine = Engine::new(&mut journal);
                    let reversed =
                        reverse_engine.process_snapshot(&snapshot, ProcessOrder::Reversed, &[]);
                    reverse_engine.record_order_comparison(&forward, &reversed);
                }

                if config.run_gst_probes {
                    for record in &snapshot.records {
                        let probe = gst_backend::probe_resume(record, &config.temp_dir);
                        let mut probe_engine = Engine::new(&mut journal);
                        probe_engine.record_probe(record, &probe, &probe.element_factory);
                        gst_probes.push(probe);
                    }
                }
                reports.push(forward);
            }
            Err(failed_report) => {
                // Truncated/invalid snapshot: keep processing the remaining
                // objects instead of aborting the whole run.
                reports.push(failed_report);
            }
        }
    }

    let totals = compute_totals(&reports);
    let outcome = RunOutcome {
        run_id: run_id.to_string(),
        reports,
        gst_probes,
        journal_path: journal_path.clone(),
        report_path: report_path.clone(),
        totals,
    };

    let report_bytes = serde_json::to_string_pretty(&outcome).map_err(io_data)?;
    std::fs::write(&report_path, report_bytes)?;

    journal
        .record(
            "run_end",
            serde_json::json!({"run_id": run_id}),
            serde_json::to_value(&outcome.totals).map_err(io_data)?,
            serde_json::json!({"status": "completed", "report": report_path.display().to_string()}),
        )
        .map_err(io_other)?;

    Ok(outcome)
}
fn compute_totals(reports: &[SnapshotReport]) -> Totals {
    let mut totals = Totals::default();
    for report in reports {
        if report.load_ok {
            totals.snapshots_loaded += 1;
        } else {
            totals.snapshots_failed += 1;
        }
        for result in &report.results {
            totals.records_seen += 1;
            match result.status.as_str() {
                "accepted" => totals.accepted += 1,
                "duplicate" => totals.duplicates += 1,
                _ => totals.invalid += 1,
            }
        }
    }
    totals
}

fn io_data(err: serde_json::Error) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, err)
}

fn io_other(err: std::io::Error) -> std::io::Error {
    err
}
