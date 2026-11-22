use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: &str = "2.0";
pub const LEGACY_SCHEMA_VERSION: &str = "1.0";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum TrackKind {
    Video,
    Audio,
    Subtitle,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DetectorParams {
    pub luma_threshold: f32,
    pub black_pixel_ratio: f32,
    pub min_duration_ms: u64,
    pub sample_fps: u32,
    pub analysis_width: u32,
    pub analysis_height: u32,
    pub pixel_format: String,
    pub track_index: u32,
    pub include_audio: bool,
}

impl Default for DetectorParams {
    fn default() -> Self {
        Self {
            luma_threshold: 0.10,
            black_pixel_ratio: 0.95,
            min_duration_ms: 500,
            sample_fps: 5,
            analysis_width: 320,
            analysis_height: 180,
            pixel_format: "GRAY8".into(),
            track_index: 0,
            include_audio: true,
        }
    }
}

impl DetectorParams {
    pub fn validate(&self) -> Result<(), String> {
        if !(0.0..=1.0).contains(&self.luma_threshold) {
            return Err("luma_threshold must be between 0 and 1".into());
        }
        if !(0.0..=1.0).contains(&self.black_pixel_ratio) {
            return Err("black_pixel_ratio must be between 0 and 1".into());
        }
        if self.sample_fps == 0 || self.sample_fps > 60 {
            return Err("sample_fps must be between 1 and 60".into());
        }
        if self.analysis_width < 16 || self.analysis_height < 16 {
            return Err("analysis dimensions are too small".into());
        }
        if self.min_duration_ms == 0 {
            return Err("min_duration_ms must be greater than zero".into());
        }
        Ok(())
    }

    pub fn fingerprint(&self) -> String {
        let canonical = format!(
            "{:.6}|{:.6}|{}|{}|{}x{}|{}|{}",
            self.luma_threshold,
            self.black_pixel_ratio,
            self.min_duration_ms,
            self.sample_fps,
            self.analysis_width,
            self.analysis_height,
            self.pixel_format,
            self.track_index
        );
        crate::report::sha256_hex(canonical.as_bytes())[..16].to_string()
    }

    pub fn sample_period_ms(&self) -> u64 {
        (1000 / self.sample_fps.max(1) as u64).max(1)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum ExtensionStatus {
    Known,
    #[default]
    Unknown,
    NoExtension,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ExtensionRecord {
    pub raw_extension: Option<String>,
    pub status: ExtensionStatus,
    pub recorded: bool,
    pub content_probe: String,
    pub policy: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceRecord {
    pub path: String,
    pub uri: String,
    pub file_name: String,
    pub extension: ExtensionRecord,
    pub size_bytes: u64,
    pub modified_utc: String,
    pub sha256: String,
    pub probe_caps: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum ReplayStatus {
    #[default]
    Running,
    Completed,
    Partial,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReplayEvent {
    pub sequence: u64,
    pub at_ms: u64,
    pub frame: Option<u64>,
    pub track_id: Option<String>,
    pub kind: String,
    pub detail: String,
    pub params_fingerprint: String,
    pub waveform_sample: Option<u64>,
    pub evidence_refs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReplayRecord {
    pub session_id: String,
    pub started_at: String,
    pub recovered: bool,
    pub recovery_note: String,
    pub replay_number: u32,
    pub replayed_from_report: Option<String>,
    pub status: ReplayStatus,
    pub selected_track: Option<String>,
    pub duration_ms: u64,
    pub events: Vec<ReplayEvent>,
}

impl Default for ReplayRecord {
    fn default() -> Self {
        Self {
            session_id: String::new(),
            started_at: String::new(),
            recovered: false,
            recovery_note: String::new(),
            replay_number: 1,
            replayed_from_report: None,
            status: ReplayStatus::Running,
            selected_track: None,
            duration_ms: 0,
            events: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrackRecord {
    pub id: String,
    pub index: u32,
    pub kind: TrackKind,
    pub stream_id: Option<String>,
    pub caps: String,
    pub source_width: Option<u32>,
    pub source_height: Option<u32>,
    pub source_fps: Option<f64>,
    pub frames_sampled: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WaveSample {
    pub sample: u64,
    pub time_ms: u64,
    pub pts_ns: u64,
    pub frame: u64,
    pub track_id: String,
    pub y_mean: f32,
    pub y_min: u8,
    pub y_max: u8,
    pub black_pixel_ratio: f32,
    pub is_black: bool,
    pub params_fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Waveform {
    pub source: String,
    pub unit: String,
    pub sample_period_ms: u64,
    pub covered_ms: u64,
    pub samples: Vec<WaveSample>,
}

impl Default for Waveform {
    fn default() -> Self {
        Self {
            source: "decoded video luma (Y)".into(),
            unit: "Y8".into(),
            sample_period_ms: 200,
            covered_ms: 0,
            samples: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum IntervalState {
    AutoDetected,
    ManualConfirmed,
    ManualRejected,
    NeedsReview,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BlackInterval {
    pub id: String,
    pub track_id: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub start_frame: u64,
    pub end_frame: u64,
    pub sample_start: u64,
    pub sample_end: u64,
    pub mean_luma: f32,
    pub lowest_luma: f32,
    pub params_fingerprint: String,
    pub evidence_refs: Vec<String>,
    pub state: IntervalState,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum Verdict {
    Clean,
    BlackFramesFound,
    Inconclusive,
    Failed,
    #[default]
    Pending,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AutoConclusion {
    pub verdict: Verdict,
    pub headline: String,
    pub confidence: f32,
    pub reason_codes: Vec<String>,
    pub limitations: Vec<String>,
    pub analyzed_samples: u64,
    pub analyzed_ms: u64,
    pub intervals: Vec<BlackInterval>,
}

impl Default for AutoConclusion {
    fn default() -> Self {
        Self {
            verdict: Verdict::Pending,
            headline: "等待分析".into(),
            confidence: 0.0,
            reason_codes: Vec::new(),
            limitations: Vec::new(),
            analyzed_samples: 0,
            analyzed_ms: 0,
            intervals: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum ManualStatus {
    #[default]
    Pending,
    Confirmed,
    Rejected,
    NeedsReview,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManualCheck {
    pub interval_id: String,
    pub decision: IntervalState,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManualConfirmation {
    pub status: ManualStatus,
    pub reviewer: String,
    pub reviewed_at: Option<String>,
    pub note: String,
    pub checks: Vec<ManualCheck>,
}

impl Default for ManualConfirmation {
    fn default() -> Self {
        Self {
            status: ManualStatus::Pending,
            reviewer: String::new(),
            reviewed_at: None,
            note: String::new(),
            checks: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum EvidenceKind {
    #[default]
    Metadata,
    Report,
    Waveform,
    KeyFrame,
    EventLog,
    SourceFingerprint,
    Failure,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvidenceRecord {
    pub id: String,
    pub kind: EvidenceKind,
    pub relative_path: String,
    pub bytes: u64,
    pub sha256: String,
    pub captured_at: String,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum ActionKind {
    #[default]
    Guidance,
    OpenPath,
    CopyCommand,
    Retry,
    ChooseFile,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FollowUpAction {
    pub code: String,
    pub label: String,
    pub detail: String,
    pub kind: ActionKind,
    pub command: Option<String>,
    pub target_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FailureRecord {
    pub code: String,
    pub title: String,
    pub detail: String,
    pub retryable: bool,
    pub evidence_retained: bool,
    pub actions: Vec<FollowUpAction>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompatibilityRecord {
    pub input_schema: String,
    pub output_schema: String,
    pub migrated: bool,
    pub warnings: Vec<String>,
}

impl Default for CompatibilityRecord {
    fn default() -> Self {
        Self {
            input_schema: SCHEMA_VERSION.into(),
            output_schema: SCHEMA_VERSION.into(),
            migrated: false,
            warnings: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReviewReport {
    pub schema_version: String,
    pub report_id: String,
    pub generated_at: String,
    pub tool: String,
    pub source: SourceRecord,
    pub replay: ReplayRecord,
    pub detector: DetectorParams,
    pub tracks: Vec<TrackRecord>,
    pub waveform: Waveform,
    pub auto_conclusion: AutoConclusion,
    pub manual_confirmation: ManualConfirmation,
    pub evidence: Vec<EvidenceRecord>,
    pub follow_up_actions: Vec<FollowUpAction>,
    pub failure: Option<FailureRecord>,
    pub compatibility: CompatibilityRecord,
}

impl ReviewReport {
    pub fn new(source: SourceRecord, replay: ReplayRecord, detector: DetectorParams) -> Self {
        let now = crate::report::now_utc();
        Self {
            schema_version: SCHEMA_VERSION.into(),
            report_id: crate::report::new_id("report"),
            generated_at: now,
            tool: "blackframe-review/0.1.0 (Rust + GStreamer + GTK4)".into(),
            source,
            replay,
            detector,
            tracks: Vec::new(),
            waveform: Waveform::default(),
            auto_conclusion: AutoConclusion::default(),
            manual_confirmation: ManualConfirmation::default(),
            evidence: Vec::new(),
            follow_up_actions: Vec::new(),
            failure: None,
            compatibility: CompatibilityRecord::default(),
        }
    }

    pub fn is_failed(&self) -> bool {
        self.failure.is_some() || self.auto_conclusion.verdict == Verdict::Failed
    }
}

pub const KNOWN_EXTENSIONS: &[&str] = &[
    "3g2", "3gp", "avi", "flv", "m2ts", "m4v", "mkv", "mov", "mp4", "mpeg", "mpg", "mts", "mxf",
    "ogv", "ts", "vob", "webm", "wmv", "y4m",
];

pub fn extension_record(path: &std::path::Path) -> ExtensionRecord {
    let raw = path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.trim_start_matches('.').to_ascii_lowercase())
        .filter(|value| !value.is_empty());
    match raw {
        Some(ext) => {
            let known = KNOWN_EXTENSIONS.contains(&ext.as_str());
            ExtensionRecord {
                raw_extension: Some(ext.clone()),
                status: if known {
                    ExtensionStatus::Known
                } else {
                    ExtensionStatus::Unknown
                },
                recorded: true,
                content_probe: "pending GStreamer decodebin probe".into(),
                policy: if known {
                    "extension accepted; content still verified by decodebin".into()
                } else {
                    "unknown extension recorded; content probe is authoritative".into()
                },
            }
        }
        None => ExtensionRecord {
            raw_extension: None,
            status: ExtensionStatus::NoExtension,
            recorded: true,
            content_probe: "pending GStreamer decodebin probe".into(),
            policy: "missing extension recorded; content probe is authoritative".into(),
        },
    }
}

pub fn format_timecode(ms: u64) -> String {
    let millis = ms % 1000;
    let total_seconds = ms / 1000;
    let seconds = total_seconds % 60;
    let minutes = (total_seconds / 60) % 60;
    let hours = total_seconds / 3600;
    format!("{hours:02}:{minutes:02}:{seconds:02}.{millis:03}")
}
