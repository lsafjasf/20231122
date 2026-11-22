//! Fixed, deterministic experiment scenarios.
//!
//! Source ids and variant ids are stable strings; the same `JobSpec` always
//! produces the same idempotency key and the same output WAV hash.

use crate::model::{JobSpec, SourceSpec, VariantSpec};

pub fn sources() -> Vec<SourceSpec> {
    vec![
        SourceSpec {
            source_id: "src-sine-440".into(),
            wave: 0,
            freq_hz: 440,
            duration_ms: 300,
        },
        SourceSpec {
            source_id: "src-square-220".into(),
            wave: 1,
            freq_hz: 220,
            duration_ms: 300,
        },
        SourceSpec {
            source_id: "src-saw-880".into(),
            wave: 2,
            freq_hz: 880,
            duration_ms: 300,
        },
    ]
}

pub fn variants() -> Vec<VariantSpec> {
    vec![
        VariantSpec {
            variant_id: "var-gain-half".into(),
            gain: 0.5,
            speed: 1.0,
        },
        VariantSpec {
            variant_id: "var-speed-fast".into(),
            gain: 1.0,
            speed: 1.5,
        },
        VariantSpec {
            variant_id: "var-soft-slow".into(),
            gain: 0.7,
            speed: 0.8,
        },
    ]
}

/// Builds jobs in row-major source order (source outer, variant inner).
pub fn canonical_jobs(work_delay_ms: u64) -> Vec<JobSpec> {
    let mut jobs = Vec::new();
    for source in sources() {
        for variant in variants() {
            jobs.push(JobSpec {
                job_id: format!("{}.{}", source.source_id, variant.variant_id),
                source: source.clone(),
                variant: variant.clone(),
                work_delay_ms,
            });
        }
    }
    jobs
}

/// Reorders the canonical list (variant outer, source inner). Multiset of
/// jobs is identical; only processing order changes.
pub fn reordered_jobs(work_delay_ms: u64) -> Vec<JobSpec> {
    let mut jobs = canonical_jobs(work_delay_ms);
    jobs.sort_by_key(|j| {
        (
            j.variant.variant_id.clone(),
            j.source.source_id.clone(),
        )
    });
    jobs
}

/// The canonical list with one logical job appended a second time.
pub fn duplicate_submission_jobs(work_delay_ms: u64) -> Vec<JobSpec> {
    let mut jobs = canonical_jobs(work_delay_ms);
    let dup = jobs[0].clone();
    jobs.push(dup);
    jobs
}
