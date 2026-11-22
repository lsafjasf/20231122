//! Deterministic diagnosis engine: ordering, de-duplication and error
//! isolation, independent of any media framework.

use super::journal::Journal;
use super::model::{DiagnosisResult, ResumeRecord, SnapshotParseError};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Processing order for the records of a snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProcessOrder {
    #[default]
    Submission,
    Reversed,
}

#[derive(Debug, Clone, Serialize)]
struct EngineState {
    processed: usize,
    accepted: usize,
    rejected: usize,
    seen_ids: Vec<String>,
}

impl EngineState {
    fn new() -> Self {
        Self {
            processed: 0,
            accepted: 0,
            rejected: 0,
            seen_ids: Vec::new(),
        }
    }
}

/// The diagnosis engine. One instance processes exactly one run; create a
/// fresh engine for every replay.
#[derive(Debug)]
pub struct Engine<'a> {
    journal: &'a mut Journal,
    seen: HashSet<String>,
    state: EngineState,
}

/// Outcome of processing one snapshot (or one failed snapshot load).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotReport {
    pub snapshot_id: String,
    pub schema_version: Option<u32>,
    pub load_ok: bool,
    pub load_error: Option<String>,
    pub results: Vec<DiagnosisResult>,
}

impl<'a> Engine<'a> {
    pub fn new(journal: &'a mut Journal) -> Self {
        Self {
            journal,
            seen: HashSet::new(),
            state: EngineState::new(),
        }
    }

    pub fn processed_count(&self) -> usize {
        self.state.processed
    }

    /// Record an external probe (e.g. a GStreamer pipeline check) against an
    /// already-loaded record while keeping the journal borrow inside the engine.
    pub fn record_probe<T: Serialize>(&mut self, record: &ResumeRecord, probe: &T, factory: &str) {
        self.journal
            .record(
                "gst_probe",
                serde_json::to_value(record).unwrap(),
                serde_json::json!({"factory": factory}),
                serde_json::to_value(probe).unwrap(),
            )
            .expect("journal write");
    }

    /// Journal the forward-vs-reversed ordering comparison.
    pub fn record_order_comparison(&mut self, forward: &SnapshotReport, reversed: &SnapshotReport) {
        self.journal
            .record(
                "order_change",
                serde_json::json!({"snapshot_id": forward.snapshot_id}),
                serde_json::to_value(&self.state).unwrap(),
                serde_json::json!({
                    "comparison": "order_change",
                    "forward_order": forward
                        .results
                        .iter()
                        .map(|r| r.record_id.clone())
                        .collect::<Vec<_>>(),
                    "reversed_order": reversed
                        .results
                        .iter()
                        .map(|r| r.record_id.clone())
                        .collect::<Vec<_>>(),
                }),
            )
            .expect("journal write");
    }

    /// Load a snapshot file, recording the truncated control case as an
    /// isolated failure instead of aborting the run.
    pub fn load_snapshot(
        &mut self,
        label: &str,
        path: &std::path::Path,
    ) -> Result<super::model::Snapshot, SnapshotReport> {
        let input = serde_json::json!({
            "label": label,
            "path": path.display().to_string(),
        });
        let state_before = serde_json::to_value(&self.state).unwrap();
        match super::model::Snapshot::load(path) {
            Ok(snapshot) => {
                self.journal
                    .record(
                        "load_snapshot",
                        input,
                        state_before,
                        serde_json::json!({
                            "ok": true,
                            "snapshot_id": snapshot.snapshot_id,
                            "schema_version": snapshot.schema_version,
                            "record_count": snapshot.records.len(),
                        }),
                    )
                    .expect("journal write");
                Ok(snapshot)
            }
            Err(err) => {
                let report = SnapshotReport {
                    snapshot_id: label.to_string(),
                    schema_version: None,
                    load_ok: false,
                    load_error: Some(err.to_string()),
                    results: Vec::new(),
                };
                self.journal
                    .record(
                        "load_snapshot",
                        input,
                        state_before,
                        serde_json::json!({
                            "ok": false,
                            "error_kind": match &err {
                                SnapshotParseError::Truncated { .. } => "truncated",
                                SnapshotParseError::InvalidJson { .. } => "invalid_json",
                                SnapshotParseError::Io(_) => "io",
                            },
                            "error": err.to_string(),
                        }),
                    )
                    .expect("journal write");
                Err(report)
            }
        }
    }

    /// Process every record of a snapshot, continuing after per-record
    /// failures so that later objects still get diagnosed.
    pub fn process_snapshot(
        &mut self,
        snapshot: &super::model::Snapshot,
        order: ProcessOrder,
        extra_submissions: &[String],
    ) -> SnapshotReport {
        let mut ordered: Vec<&ResumeRecord> = snapshot.records.iter().collect();
        match order {
            ProcessOrder::Submission => ordered.sort_by_key(|r| r.submitted_at.clone()),
            ProcessOrder::Reversed => ordered.reverse(),
        }

        // Duplicate submissions: ids named here are handed to the engine a
        // second time after the regular pass, exercising idempotency.
        let mut queue: Vec<&ResumeRecord> = ordered.iter().copied().collect();
        for dup_id in extra_submissions {
            if let Some(record) = snapshot.records.iter().find(|r| r.record_id == *dup_id) {
                queue.push(record);
            }
        }

        self.journal
            .record(
                "plan_snapshot",
                serde_json::json!({
                    "snapshot_id": snapshot.snapshot_id,
                    "order": format!("{order:?}"),
                    "queue_ids": queue.iter().map(|r| r.record_id.clone()).collect::<Vec<_>>(),
                }),
                serde_json::to_value(&self.state).unwrap(),
                serde_json::json!({"planned": queue.len()}),
            )
            .expect("journal write");

        let mut results = Vec::new();
        for record in queue {
            results.push(self.process_record(snapshot.schema_version, record));
        }

        SnapshotReport {
            snapshot_id: snapshot.snapshot_id.clone(),
            schema_version: Some(snapshot.schema_version),
            load_ok: true,
            load_error: None,
            results,
        }
    }

    fn process_record(&mut self, schema_version: u32, record: &ResumeRecord) -> DiagnosisResult {
        let state_before = serde_json::to_value(&self.state).unwrap();
        let input = serde_json::to_value(record).unwrap();

        let (accepted, status, detail) = if self.seen.contains(&record.record_id) {
            (
                false,
                "duplicate",
                "record_id already processed in this run; submission ignored".to_string(),
            )
        } else if record.resume_position_ms == 0 {
            (
                false,
                "invalid",
                "resume position must be greater than zero".to_string(),
            )
        } else {
            self.seen.insert(record.record_id.clone());
            (
                true,
                "accepted",
                format!(
                    "resume at {} ms (segment seq {}) accepted under schema v{}",
                    record.resume_position_ms, record.segment_seq, schema_version
                ),
            )
        };

        self.state.processed += 1;
        if accepted {
            self.state.accepted += 1;
        } else {
            self.state.rejected += 1;
        }
        self.state.seen_ids = self.seen.iter().cloned().collect();
        self.state.seen_ids.sort();

        let result = DiagnosisResult {
            record_id: record.record_id.clone(),
            schema_version,
            manifest_url: record.manifest_url.clone(),
            resume_position_ms: record.resume_position_ms,
            accepted,
            status: status.to_string(),
            detail,
        };

        self.journal
            .record(
                "diagnose",
                input,
                state_before,
                serde_json::to_value(&result).unwrap(),
            )
            .expect("journal write");
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::{ResumeRecord, Snapshot};

    fn snapshot() -> Snapshot {
        Snapshot {
            schema_version: 2,
            snapshot_id: "test".into(),
            captured_at: None,
            note: None,
            records: vec![
                ResumeRecord {
                    record_id: "a".into(),
                    manifest_url: "m".into(),
                    resume_position_ms: 100,
                    segment_seq: 1,
                    integrity: None,
                    submitted_at: Some("2026-11-21T07:00:00+08:00".into()),
                },
                ResumeRecord {
                    record_id: "b".into(),
                    manifest_url: "m".into(),
                    resume_position_ms: 200,
                    segment_seq: 2,
                    integrity: None,
                    submitted_at: Some("2026-11-21T08:00:00+08:00".into()),
                },
            ],
        }
    }

    #[test]
    fn duplicate_submissions_are_rejected_but_processing_continues() {
        let mut journal = Journal::memory();
        let mut engine = Engine::new(&mut journal);
        let snap = snapshot();
        let report = engine.process_snapshot(&snap, ProcessOrder::Submission, &["a".into()]);
        assert_eq!(report.results.len(), 3);
        assert!(report.results[2].status == "duplicate");
        assert_eq!(engine.processed_count(), 3);
    }

    #[test]
    fn reversed_order_changes_processing_sequence() {
        let mut journal = Journal::memory();
        let mut engine = Engine::new(&mut journal);
        let snap = snapshot();
        let report = engine.process_snapshot(&snap, ProcessOrder::Reversed, &[]);
        assert_eq!(report.results[0].record_id, "b");
        assert_eq!(report.results[1].record_id, "a");
    }
}
