//! The repeated object at the centre of the flow: a [`DubJob`].
//!
//! Serialisation is canonical: structs are written with serde_json in
//! declaration order and [`STATE_FIELD_ORDER`] pins the top-level key order
//! so that a reloaded artifact keeps both fields and their order.

use serde::{Deserialize, Serialize};

use crate::clocks::ClockSync;

pub const STATE_SCHEMA: u32 = 1;

/// Pinned top-level key order of a checkpoint/report document.
/// `state::save_state` writes exactly this order and `state::load_state`
/// rejects documents whose keys arrive in a different order.
pub const STATE_FIELD_ORDER: [&str; 9] = [
    "schema",
    "job_id",
    "input",
    "output_dir",
    "segments",
    "next_segment",
    "results",
    "clock_sync",
    "degradations",
];

/// One unit of work: switch the dubbing to `language` for a time window.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SegmentSpec {
    pub index: u32,
    pub language: String,
    pub start_ms: u64,
    pub duration_ms: u64,
}

/// Outcome of one processed segment.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SegmentResult {
    pub index: u32,
    pub language: String,
    pub file: String,
    pub sha256: String,
    pub bytes: u64,
    /// Processing mode actually used: "full" or a degraded variant.
    pub mode: String,
    /// Wall-clock stamps on both unsynchronised device clocks.
    pub a_ns: i64,
    pub b_ns: i64,
}

/// A recorded degradation decision (resource shortage path).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Degradation {
    pub segment: u32,
    pub reason: String,
    pub detail: String,
    pub a_ns: i64,
    pub b_ns: i64,
}

/// The repeated object: one dubbing job, processed across pause/resume
/// cycles. All intermediate state lives here and is checkpointed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DubJob {
    pub schema: u32,
    pub job_id: String,
    pub input: String,
    pub output_dir: String,
    /// Ordered segment plan; order is significant and preserved on reload.
    pub segments: Vec<SegmentSpec>,
    /// Index of the first segment not yet completed.
    pub next_segment: u32,
    /// Completed results, in segment order.
    pub results: Vec<SegmentResult>,
    pub clock_sync: ClockSync,
    pub degradations: Vec<Degradation>,
}

/// Alias kept for readability at call sites that emphasise "the state".
pub type JobState = DubJob;

impl DubJob {
    /// Build the ordered segment plan: the timeline is split equally and
    /// each segment gets its dubbing language from `languages` (cycled).
    pub fn new(
        job_id: &str,
        input: &str,
        output_dir: &str,
        total_ms: u64,
        languages: &[String],
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(!languages.is_empty(), "at least one language required");
        anyhow::ensure!(total_ms > 0, "total duration must be positive");
        let n = languages.len() as u64;
        let per = total_ms / n;
        let segments = (0..n)
            .map(|i| SegmentSpec {
                index: i as u32,
                language: languages[i as usize].clone(),
                start_ms: i * per,
                duration_ms: if i == n - 1 { total_ms - i * per } else { per },
            })
            .collect();
        Ok(Self {
            schema: STATE_SCHEMA,
            job_id: job_id.into(),
            input: input.into(),
            output_dir: output_dir.into(),
            segments,
            next_segment: 0,
            results: Vec::new(),
            clock_sync: ClockSync::default(),
            degradations: Vec::new(),
        })
    }

    pub fn is_complete(&self) -> bool {
        self.next_segment as usize >= self.segments.len()
    }

    pub fn pending(&self) -> &[SegmentSpec] {
        &self.segments[self.next_segment.min(self.segments.len() as u32) as usize..]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_covers_timeline_in_order() {
        let langs = vec!["eng".to_string(), "zho".to_string(), "eng".to_string()];
        let job = DubJob::new("j1", "in.mkv", "out", 10_000, &langs).unwrap();
        assert_eq!(job.segments.len(), 3);
        assert_eq!(job.segments[0].start_ms, 0);
        assert_eq!(
            job.segments[2].start_ms + job.segments[2].duration_ms,
            10_000
        );
        assert!(job.pending().iter().map(|s| s.index).eq(0..3));
    }
}
