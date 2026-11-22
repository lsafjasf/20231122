//! Domain model for the HDR luminance histogram source-check page.
//!
//! Every conclusion in the audit report must be traceable back to a raw
//! input sample and the ordered processing steps that transformed it. These
//! types are serialized to JSON so the GTK4 page can present the provenance
//! chain and so a replay can be verified byte-for-byte.

use serde::{Deserialize, Serialize};

/// State of a single processing step, ordered by severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepState {
    /// Step ran and produced a usable output.
    Ok,
    /// Value sat on a bin edge within the configured tolerance; no bin can be
    /// decided without additional evidence.
    Indeterminate,
    /// Value exceeded the headroom and was clipped at the truncation step.
    Truncated,
    /// Step failed (for example a non-finite luminance value).
    Error,
}

impl StepState {
    /// Severity rank used when locating the first non-ok state.
    pub fn rank(self) -> u8 {
        match self {
            StepState::Ok => 0,
            StepState::Indeterminate => 1,
            StepState::Truncated => 2,
            StepState::Error => 3,
        }
    }

    pub fn is_problem(self) -> bool {
        !matches!(self, StepState::Ok)
    }
}

/// Raw HDR luminance sample as captured from the GStreamer pipeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawSample {
    /// Stable identifier of the sample, used by every downstream reference.
    pub sample_id: String,
    /// GStreamer buffer timestamp in nanoseconds, when available.
    pub pts_ns: Option<u64>,
    /// Element that produced the sample, e.g. an `appsink`.
    pub source_element: String,
    /// Luminance in candela per square metre (nits), pre-normalization.
    pub nits: f64,
    /// Optional pixel coordinate inside the source frame.
    pub pixel: Option<(u32, u32)>,
}

/// Immutable evidence attached to a step so a conclusion can be re-checked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Evidence {
    /// Human-readable description of what was observed.
    pub observation: String,
    /// Numeric facts captured before the step mutated the value.
    pub facts: serde_json::Value,
    /// Identifier of the raw sample this evidence derives from.
    pub source_sample_id: String,
}

/// One ordered transformation applied to a sample.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProcessingStep {
    /// Zero-based position of the step in the pipeline.
    pub index: usize,
    /// Stable machine name, e.g. `clip_headroom`.
    pub name: String,
    /// Human label shown in the UI.
    pub label: String,
    /// Resulting state after running the step.
    pub state: StepState,
    /// Value fed into the step.
    pub input_value: f64,
    /// Value produced by the step.
    pub output_value: f64,
    /// Parameters the step was configured with at run time.
    pub params: serde_json::Value,
    /// Evidence captured for this step; always present for problem states.
    pub evidence: Option<Evidence>,
}

/// Verdict for one fully processed (or halted) sample record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Verdict {
    /// Sample reached a histogram bin normally.
    Binned { bin: usize },
    /// Sample hit an undecidable boundary and was not assigned a bin.
    Indeterminate,
    /// Sample was truncated at the headroom before binning.
    Truncated,
    /// Processing stopped because a step returned an error.
    Error,
}

/// A sample together with every step that touched it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SampleRecord {
    /// The original, unmodified input.
    pub raw: RawSample,
    /// Ordered journal of processing steps.
    pub steps: Vec<ProcessingStep>,
    /// Final verdict for the sample.
    pub verdict: Verdict,
}

impl SampleRecord {
    /// Index of the first step whose state is not `Ok`, if any.
    ///
    /// This is what the UI highlights instead of reducing the run to a score.
    pub fn first_problem_step(&self) -> Option<usize> {
        self.steps.iter().position(|step| step.state.is_problem())
    }
}

/// Aggregated histogram plus the records that justify each count.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Histogram {
    /// Lower edge (in normalized headroom units) of every bin.
    pub bin_edges: Vec<f64>,
    /// Count of samples assigned to each bin.
    pub counts: Vec<u64>,
}
