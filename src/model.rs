use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: &str = "source-ledger/v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectManifest {
    pub project_id: String,
    pub title: String,
    pub package_version: String,
    pub assembled_at: String,
    pub owner: String,
    pub sources: Vec<SourceSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceSpec {
    pub id: String,
    pub path: String,
    pub label: String,
    pub role: String,
    pub origin: String,
    pub declared_format: Option<String>,
    pub capture_note: String,
    #[serde(default)]
    pub edge_case: Option<EdgeCase>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeCase {
    pub kind: String,
    pub question: String,
    pub boundary_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceState {
    Verified,
    Replayed,
    Unknown,
    Ambiguous,
    Failed,
}

impl SourceState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Verified => "自动通过",
            Self::Replayed => "恢复重放",
            Self::Unknown => "待确认",
            Self::Ambiguous => "边界待定",
            Self::Failed => "检查失败",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeSnapshot {
    pub uri: String,
    pub byte_size: u64,
    pub sha256: String,
    pub modified_at: String,
    pub extension: String,
    pub detected_mime: Option<String>,
    pub container: Option<String>,
    pub duration_ms: Option<u64>,
    pub video_streams: usize,
    pub audio_streams: usize,
    pub gst_error: Option<String>,
    pub head_hex: String,
    pub tail_hex: String,
    pub probed_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepState {
    Ok,
    Warning,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessingStep {
    pub seq: u32,
    pub action: String,
    pub input: String,
    pub output: String,
    pub state: StepState,
    pub elapsed_ms: u128,
    pub evidence_ref: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceRecord {
    pub id: String,
    pub label: String,
    pub path: String,
    pub note: String,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceCheck {
    pub spec: SourceSpec,
    pub resolved_path: String,
    pub state: SourceState,
    pub probe: Option<ProbeSnapshot>,
    pub steps: Vec<ProcessingStep>,
    pub evidence: Vec<EvidenceRecord>,
    pub auto_finding_id: String,
    pub manual_confirmation_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayRecord {
    pub id: String,
    pub source_id: String,
    pub trigger: String,
    pub checkpoint_id: String,
    pub original_input: String,
    pub replay_input: String,
    pub original_sha256: String,
    pub replay_sha256: String,
    pub identical: bool,
    pub compared_fields: Vec<String>,
    pub steps: Vec<ProcessingStep>,
    pub preserved_at: String,
    pub evidence_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingKind {
    Pass,
    Unresolved,
    Ambiguous,
    Failure,
    ReplayConsistent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoFinding {
    pub id: String,
    pub source_id: String,
    pub kind: FindingKind,
    pub headline: String,
    pub reasoning: String,
    pub evidence_refs: Vec<String>,
    pub step_refs: Vec<u32>,
    pub policy: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfirmationState {
    Pending,
    Confirmed,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManualConfirmation {
    pub id: String,
    pub source_id: String,
    pub title: String,
    pub question: String,
    pub blocker: String,
    pub evidence_refs: Vec<String>,
    pub state: ConfirmationState,
    pub reviewer: Option<String>,
    pub note: Option<String>,
    pub decided_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionableStep {
    pub label: String,
    pub detail: String,
    pub command: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailureAction {
    pub source_id: String,
    pub code: String,
    pub summary: String,
    pub next_actions: Vec<ActionableStep>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SummaryCounts {
    pub total: usize,
    pub auto_passed: usize,
    pub needs_manual: usize,
    pub failures: usize,
    pub replays: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanReport {
    pub schema_version: String,
    pub run_id: String,
    pub project: ProjectSummary,
    pub generated_at: String,
    pub sources: Vec<SourceCheck>,
    pub replay: Option<ReplayRecord>,
    pub auto_conclusions: Vec<AutoFinding>,
    pub manual_confirmations: Vec<ManualConfirmation>,
    pub failures: Vec<FailureAction>,
    pub counts: SummaryCounts,
    pub log_path: String,
    pub evidence_dir: String,
    pub report_sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectSummary {
    pub request: String,
    pub audit_scope: String,
    pub package_identity: String,
    pub manifest_sha256: String,
}

impl SummaryCounts {
    pub fn from_sources(sources: &[SourceCheck], replay_count: usize) -> Self {
        let mut counts = Self {
            total: sources.len(),
            replays: replay_count,
            ..Self::default()
        };
        for source in sources {
            match source.state {
                SourceState::Verified | SourceState::Replayed => counts.auto_passed += 1,
                SourceState::Unknown | SourceState::Ambiguous => counts.needs_manual += 1,
                SourceState::Failed => counts.failures += 1,
            }
        }
        counts
    }
}
