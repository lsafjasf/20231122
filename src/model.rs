use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Info,
    Warn,
}

#[derive(Clone, Debug)]
pub struct SourceSummary {
    pub filename: String,
    pub path: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub channels: u32,
    pub sample_rate: u32,
    pub bits_per_sample: u32,
    pub duration_secs: f64,
    pub route: String,
    pub session: String,
    pub captured_at: String,
    pub modified: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepStatus {
    Ok,
    Warn,
    Failed,
}

#[derive(Clone, Debug)]
pub struct ProcessingStep {
    pub id: String,
    pub title: String,
    pub detail: String,
    pub elapsed_ms: u128,
    pub status: StepStatus,
    pub evidence_id: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FindingStatus {
    Confirmed,
    Isolated,
    Undecidable,
}

#[derive(Clone, Debug)]
pub struct AutoFinding {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub rule: String,
    pub confidence: Option<f32>,
    pub status: FindingStatus,
    pub tone: Tone,
    pub source_ref: String,
    pub processing_ref: String,
    pub evidence_ids: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManualStatus {
    Pending,
    Confirmed,
    Rejected,
}

#[derive(Clone, Debug)]
pub struct ManualCheck {
    pub id: String,
    pub title: String,
    pub detail: String,
    pub evidence_ids: Vec<String>,
    pub status: ManualStatus,
}

#[derive(Clone, Debug)]
pub struct Evidence {
    pub id: String,
    pub title: String,
    pub locator: String,
    pub digest: String,
    pub prose: String,
    pub payload: String,
}

#[derive(Clone, Debug)]
pub struct RecoveryEvent {
    pub recovery_s: f64,
    pub source_start_s: f64,
    pub replay_start_s: f64,
    pub replay_end_s: f64,
    pub boundary_sample: u64,
    pub boundary_frames: u64,
    pub confidence: f32,
}

#[derive(Clone, Debug)]
pub struct UnknownExtension {
    pub fourcc: String,
    pub offset: u64,
    pub size: u64,
    pub sha256: String,
    pub preview_hex: String,
    pub parser_error: String,
}

#[derive(Clone, Debug)]
pub struct FailureCase {
    pub title: String,
    pub detail: String,
    pub code: String,
    pub recovered: bool,
    pub actions: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct ChannelLane {
    pub label: String,
    pub name: String,
    pub rms_db: f32,
    pub points: Vec<f32>,
}

#[derive(Clone, Debug)]
pub struct InspectionReport {
    pub run_id: String,
    pub source: SourceSummary,
    pub steps: Vec<ProcessingStep>,
    pub findings: Vec<AutoFinding>,
    pub manual_checks: Vec<ManualCheck>,
    pub evidence: Vec<Evidence>,
    pub recovery: RecoveryEvent,
    pub extension: UnknownExtension,
    pub failure: FailureCase,
    pub lanes: Vec<ChannelLane>,
    pub report_dir: String,
    pub completed_at: String,
}

#[derive(Clone, Debug)]
pub struct FailureState {
    pub title: String,
    pub detail: String,
    pub code: String,
    pub source_path: String,
    pub preserved: Vec<String>,
}

#[derive(Serialize)]
pub struct ExportReport<'a> {
    pub schema: &'static str,
    pub run_id: &'a str,
    pub completed_at: &'a str,
    pub source: ExportSource<'a>,
    pub automatic: Vec<ExportFinding<'a>>,
    pub human: Vec<ExportManual<'a>>,
    pub evidence: Vec<ExportEvidence<'a>>,
}

#[derive(Serialize)]
pub struct ExportSource<'a> {
    pub path: &'a str,
    pub sha256: &'a str,
    pub channels: u32,
    pub sample_rate: u32,
    pub bits_per_sample: u32,
    pub duration_secs: f64,
}

#[derive(Serialize)]
pub struct ExportFinding<'a> {
    pub id: &'a str,
    pub title: &'a str,
    pub status: &'static str,
    pub summary: &'a str,
    pub rule: &'a str,
    pub confidence: Option<f32>,
    pub source_ref: &'a str,
    pub processing_ref: &'a str,
    pub evidence_ids: &'a [String],
}

#[derive(Serialize)]
pub struct ExportManual<'a> {
    pub id: &'a str,
    pub title: &'a str,
    pub status: &'static str,
    pub evidence_ids: &'a [String],
}

#[derive(Serialize)]
pub struct ExportEvidence<'a> {
    pub id: &'a str,
    pub title: &'a str,
    pub locator: &'a str,
    pub digest: &'a str,
}

impl InspectionReport {
    pub fn export_summary(&self) -> ExportReport<'_> {
        ExportReport {
            schema: "monitor-route-inspection/v1",
            run_id: &self.run_id,
            completed_at: &self.completed_at,
            source: ExportSource {
                path: &self.source.path,
                sha256: &self.source.sha256,
                channels: self.source.channels,
                sample_rate: self.source.sample_rate,
                bits_per_sample: self.source.bits_per_sample,
                duration_secs: self.source.duration_secs,
            },
            automatic: self
                .findings
                .iter()
                .map(|finding| ExportFinding {
                    id: &finding.id,
                    title: &finding.title,
                    status: match finding.status {
                        FindingStatus::Confirmed => "confirmed",
                        FindingStatus::Isolated => "isolated",
                        FindingStatus::Undecidable => "undecidable",
                    },
                    summary: &finding.summary,
                    rule: &finding.rule,
                    confidence: finding.confidence,
                    source_ref: &finding.source_ref,
                    processing_ref: &finding.processing_ref,
                    evidence_ids: &finding.evidence_ids,
                })
                .collect(),
            human: self
                .manual_checks
                .iter()
                .map(|check| ExportManual {
                    id: &check.id,
                    title: &check.title,
                    status: match check.status {
                        ManualStatus::Pending => "pending",
                        ManualStatus::Confirmed => "confirmed",
                        ManualStatus::Rejected => "rejected",
                    },
                    evidence_ids: &check.evidence_ids,
                })
                .collect(),
            evidence: self
                .evidence
                .iter()
                .map(|evidence| ExportEvidence {
                    id: &evidence.id,
                    title: &evidence.title,
                    locator: &evidence.locator,
                    digest: &evidence.digest,
                })
                .collect(),
        }
    }

    pub fn pending_manual(&self) -> usize {
        self.manual_checks
            .iter()
            .filter(|check| check.status == ManualStatus::Pending)
            .count()
    }

    pub fn digest_short(&self) -> &str {
        self.source.sha256.get(..12).unwrap_or(&self.source.sha256)
    }
}
