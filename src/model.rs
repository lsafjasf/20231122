use serde::{Deserialize, Serialize};

pub const SCHEMA: &str = "tempo-pitch-debug/1";
pub const RECORD_FILE: &str = "record.json";
pub const MANIFEST_FILE: &str = "manifest.json";
pub const TIMELINE_FILE: &str = "timeline.csv";
pub const WAVEFORM_FILE: &str = "waveform.csv";
pub const OUTPUT_WAV: &str = "output.wav";
pub const REVIEW_FILE: &str = "REVIEW.md";
pub const DEFAULT_SAMPLE_RATE: u32 = 48_000;
pub const DEFAULT_CHANNELS: u16 = 2;
pub const DEFAULT_WINDOW_US: u64 = 20_000;
pub const DEFAULT_RENDER_QUANTUM_US: u64 = 100_000;
pub const DEFAULT_PITCH_GAP_US: u64 = 200_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DebugRecord {
    pub schema: String,
    pub status: RunStatus,
    pub job_id: String,
    pub created_utc: String,
    pub request: RequestSnapshot,
    pub inputs: InputEvidence,
    pub processing: ProcessingEvidence,
    pub timeline: Vec<TimelineEntry>,
    pub anomalies: Vec<Anomaly>,
    pub output: OutputEvidence,
    pub verification: VerificationEvidence,
    pub recording_policy: RecordingPolicy,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Complete,
    CompleteWithAnomalies,
    EmptyInput,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RequestSnapshot {
    pub audio_path: String,
    pub speed_curve_path: String,
    pub pitch_track_path: String,
    pub output_dir: String,
    pub scenario_name: String,
    pub analysed_channel: u16,
    pub window_us: u64,
    pub render_quantum_us: u64,
    pub pitch_gap_threshold_us: u64,
    pub missing_span: Option<TimeSpan>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TimeSpan {
    pub start_us: u64,
    pub end_us: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InputEvidence {
    pub audio: AudioEvidence,
    pub speed_curve: CurveEvidence,
    pub pitch_track: CurveEvidence,
    pub limits: Limits,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AudioEvidence {
    pub role: String,
    pub path: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub container: String,
    pub sample_rate_hz: u32,
    pub channels: u16,
    pub frames: u64,
    pub duration_us: u64,
    pub track_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CurveEvidence {
    pub role: String,
    pub path: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub unit: String,
    pub points_count: u64,
    pub points_in_record: u64,
    pub points_sampled: bool,
    pub first_time_us: Option<u64>,
    pub last_time_us: Option<u64>,
    pub min_value: Option<f64>,
    pub max_value: Option<f64>,
    pub points: Vec<CurvePoint>,
    pub internal_gaps: Vec<TimeSpan>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CurvePoint {
    pub time_us: u64,
    pub value: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub max_audio_bytes: u64,
    pub max_curve_bytes: u64,
    pub max_curve_points: u64,
    pub max_timeline_entries: u64,
    pub max_render_segments: u64,
    pub max_decoded_pcm_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_audio_bytes: 512 * 1024 * 1024,
            max_curve_bytes: 64 * 1024 * 1024,
            max_curve_points: 2_000_000,
            max_timeline_entries: 100_000,
            max_render_segments: 20_000,
            max_decoded_pcm_bytes: 4 * 1024 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProcessingEvidence {
    pub backend: String,
    pub plugin_version: String,
    pub plugin_chain: Vec<String>,
    pub sample_rate_hz: u32,
    pub channels: u16,
    pub analysed_channel: u16,
    pub window_us: u64,
    pub render_quantum_us: u64,
    pub output_sample_rate_hz: u32,
    pub output_channels: u16,
    pub render_segment_count: u64,
    pub speed_parameter: ParameterRange,
    pub pitch_parameter: ParameterRange,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ParameterRange {
    pub min: f64,
    pub max: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TimelineEntry {
    pub index: u64,
    pub render_segment_index: u64,
    pub track_id: String,
    pub channel: u16,
    pub source_start_us: u64,
    pub source_end_us: u64,
    pub source_frame_start: u64,
    pub source_frame_end: u64,
    pub output_start_us: u64,
    pub output_end_us: u64,
    pub output_frame_start: u64,
    pub output_frame_end: u64,
    pub speed_rate_curve: f64,
    pub speed_rate_applied: f64,
    pub pitch_semitones_curve: f64,
    pub pitch_semitones_applied: f64,
    pub pitch_ratio_applied: f64,
    pub pitch_evidence: EvidenceState,
    pub waveform: WaveformStats,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceState {
    Observed,
    InterpolatedAcrossGap,
    HeldLastKnown,
    DefaultNominal,
    MissingAudio,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WaveformStats {
    pub sample_frames: u64,
    pub min: f64,
    pub max: f64,
    pub rms: f64,
    pub peak_dbfs: f64,
}

impl Default for WaveformStats {
    fn default() -> Self {
        Self {
            sample_frames: 0,
            min: 0.0,
            max: 0.0,
            rms: 0.0,
            peak_dbfs: -240.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Anomaly {
    pub id: String,
    pub kind: AnomalyKind,
    pub severity: Severity,
    pub track_id: String,
    pub channel: u16,
    pub source_span: TimeSpan,
    pub output_span: TimeSpan,
    pub source_frames: FrameSpan,
    pub output_frames: FrameSpan,
    pub parameters: AnomalyParameters,
    pub timeline_index_span: IndexSpan,
    pub waveform_at_gap: WaveformStats,
    pub evidence_refs: Vec<EvidenceRef>,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AnomalyKind {
    MissingPitchSegment,
    EmptyAudio,
    EmptySpeedCurve,
    EmptyPitchTrack,
    LimitExceeded,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FrameSpan {
    pub start: u64,
    pub end: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct IndexSpan {
    pub start: u64,
    pub end: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AnomalyParameters {
    pub speed_rate_curve: f64,
    pub speed_rate_applied: f64,
    pub pitch_semitones_curve: f64,
    pub pitch_semitones_applied: f64,
    pub pitch_ratio_applied: f64,
    pub pitch_evidence: EvidenceState,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EvidenceRef {
    pub artifact: String,
    pub locator: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OutputEvidence {
    pub path: String,
    pub format: String,
    pub sample_rate_hz: u32,
    pub channels: u16,
    pub frames: u64,
    pub duration_us: u64,
    pub size_bytes: u64,
    pub sha256: String,
    pub peak_dbfs: f64,
    pub clipping_samples: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct VerificationEvidence {
    pub checks: Vec<VerificationCheck>,
    pub reload_round_trip: bool,
    pub verify_command: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct VerificationCheck {
    pub name: String,
    pub passed: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RecordingPolicy {
    pub timeline_rows: u64,
    pub timeline_rows_in_record: u64,
    pub record_is_complete: bool,
    pub adaptive_window_applied: bool,
    pub sampling: String,
    pub stable_field_order: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema: String,
    pub record_file: String,
    pub record_sha256: String,
    pub files: Vec<ManifestFile>,
    pub reload_contract: ReloadContract,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ManifestFile {
    pub role: String,
    pub path: String,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReloadContract {
    pub field_order: Vec<String>,
    pub canonical_json_pretty: bool,
    pub timeline_order: String,
    pub verified: bool,
}
