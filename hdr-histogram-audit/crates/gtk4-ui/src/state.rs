//! UI-side application state.
//!
//! Holds the deterministic scenario run together with the truncation record
//! and replay report so every widget renders from the same audited data.

use audit_core::model::SampleRecord;
use audit_core::pipeline::PipelineRun;
use audit_core::replay::{ReplayReport, TruncationRecord};
use audit_core::report::AuditReport;
use gtk::Box;
use gtk::glib::WeakRef;

#[derive(Default)]
pub struct AppState {
    pub run: Option<PipelineRun>,
    pub report: Option<AuditReport>,
    pub truncation: Option<TruncationRecord>,
    pub replay: Option<ReplayReport>,
    pub selected_record: usize,
    pub chart_holder: Option<WeakRef<Box>>,
    pub detail_holder: Option<WeakRef<Box>>,
}

impl AppState {
    pub fn records(&self) -> &[SampleRecord] {
        self.run.as_ref().map(|run| run.records.as_slice()).unwrap_or(&[])
    }

    pub fn selected(&self) -> Option<&SampleRecord> {
        self.records().get(self.selected_record)
    }
}
