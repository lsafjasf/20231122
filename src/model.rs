use serde::{Deserialize, Serialize};

pub const SCHEMA: &str = "mouse-trail/evidence/v1";
pub const TRACK_ID: u32 = 0;
pub const DEFAULT_HOLD_NS: u64 = 250_000_000;
pub const DEFAULT_MAX_GAP_NS: u64 = 500_000_000;
pub const ANCHOR_A_NS: u64 = 1_000_000_000;
pub const ANCHOR_B_NS: u64 = 2_000_000_000;
pub const ANCHOR_C_NS: u64 = 3_000_000_000;

#[derive(Debug, Clone, Deserialize)]
pub struct RawEvent {
    pub seq: u64,
    pub ts_ns: u64,
    pub x: f64,
    pub y: f64,
    #[serde(default)]
    pub buttons: u32,
    #[serde(default)]
    pub source: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ParsedEvent {
    pub seq: u64,
    pub ts_ns: u64,
    pub x: f64,
    pub y: f64,
    pub buttons: u32,
    pub source: String,
    pub raw_offset: u64,
    pub raw_len: u64,
    pub raw_sha256: String,
    pub raw_order_inversion: bool,
    pub raw_prev_ts_ns: Option<u64>,
    pub raw_prev_offset: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DuplicateRecord {
    pub seq: u64,
    pub raw_offset: u64,
    pub raw_len: u64,
    pub selected: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Params {
    pub track_id: u32,
    pub frame_rate_num: u32,
    pub frame_rate_den: u32,
    pub hold_ns: u64,
    pub max_gap_ns: u64,
    pub clamp_to_frame: bool,
    pub dedupe_policy: String,
    pub interpolation: String,
    pub stale_policy: String,
    pub anchor_a_ns: u64,
    pub anchor_b_ns: u64,
    pub anchor_c_ns: u64,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            track_id: TRACK_ID,
            frame_rate_num: 10,
            frame_rate_den: 1,
            hold_ns: DEFAULT_HOLD_NS,
            max_gap_ns: DEFAULT_MAX_GAP_NS,
            clamp_to_frame: true,
            dedupe_policy: "same_ts_last_raw_offset_wins".into(),
            interpolation: "linear_between_events_when_gap_lte_max".into(),
            stale_policy: "hold_last_for_250ms_then_hide".into(),
            anchor_a_ns: ANCHOR_A_NS,
            anchor_b_ns: ANCHOR_B_NS,
            anchor_c_ns: ANCHOR_C_NS,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrameSample {
    pub frame_index: u64,
    pub pts_ns: u64,
    pub duration_ns: u64,
    pub track_id: u32,
    pub visible: bool,
    pub raw_x: Option<f64>,
    pub raw_y: Option<f64>,
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub clamp_x: bool,
    pub clamp_y: bool,
    pub speed_px_s: f64,
    pub event_seq: Option<u64>,
    pub event_ts_ns: Option<u64>,
    pub raw_offset: Option<u64>,
    pub event_gap_ns: Option<u64>,
    pub flags: Vec<String>,
    pub anchor: Option<String>,
    pub params_sha256: String,
}

#[derive(Debug, Clone)]
pub struct ParsedLog {
    pub events: Vec<ParsedEvent>,
    pub records_parsed: usize,
    pub records_kept: usize,
    pub records_dropped_duplicate: usize,
    pub records_dropped_bad_json: usize,
    pub duplicates: Vec<Vec<DuplicateRecord>>,
    pub min_ts_ns: Option<u64>,
    pub max_ts_ns: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnchorEvidence {
    pub id: String,
    pub kind: String,
    pub pts_ns: u64,
    pub frame_index: u64,
    pub track_id: u32,
    pub raw_offset: u64,
    pub raw_length: u64,
    pub raw_sha256: String,
    pub raw_json: String,
    pub event_seq: u64,
    pub event_ts_ns: u64,
    pub raw_x: f64,
    pub raw_y: f64,
    pub output_x: f64,
    pub output_y: f64,
    pub output_speed_px_s: f64,
    pub flags: Vec<String>,
    pub params_sha256: String,
    pub duplicate_records: Vec<DuplicateRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceCounts {
    pub events_parsed: usize,
    pub events_kept: usize,
    pub duplicates_removed: usize,
    pub bad_json_records: usize,
    pub raw_order_inversions: usize,
    pub clamped_frames: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceFile {
    pub schema: String,
    pub status: String,
    pub generated_at_utc: String,
    pub implementation: Implementation,
    pub inputs: InputEvidence,
    pub params: Params,
    pub params_sha256: String,
    pub anchors: Vec<AnchorEvidence>,
    pub counts: EvidenceCounts,
    pub outputs: OutputEvidence,
    pub error: Option<Diagnostic>,
    pub source_unchanged: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Implementation {
    pub language: String,
    pub language_version: String,
    pub media: String,
    pub media_version: String,
    pub ui: String,
    pub ui_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputEvidence {
    pub video: VideoInputEvidence,
    pub events: EventInputEvidence,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoInputEvidence {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub frames: u64,
    pub first_pts_ns: Option<u64>,
    pub last_pts_ns: Option<u64>,
    pub duration_ns: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventInputEvidence {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
    pub max_event_bytes: u64,
    pub max_events: usize,
    pub min_ts_ns: Option<u64>,
    pub max_ts_ns: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputEvidence {
    pub trace_jsonl: String,
    pub waveform_csv: String,
    pub waveform_svg: String,
    pub overlay_video: String,
    pub anchors_json: String,
    pub raw_evidence_dir: String,
    pub anchor_frames: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub code: String,
    pub message: String,
    pub phase: String,
    pub raw_offset: Option<u64>,
}
