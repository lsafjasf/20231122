pub mod evidence;
pub mod model;
pub mod pipeline;
pub mod scenarios;
pub mod ui;
pub mod waveform;

pub use evidence::{
    CaptureEvidence, EvidenceBundle, EvidencePaths, write_capture_evidence, write_evidence,
};
pub use model::{
    BoundaryEvent, EncodeSample, EncodingStats, Metric, MetricLevel, ProcessedPoint,
    ProcessingParams, StatisticsProcessor, StatsSnapshot, WaveformPoint,
};
pub use scenarios::{ScenarioKind, ScenarioRun, run_scenario};
