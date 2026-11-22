//! Contract tests for the parts that make the experiment replayable.

use std::path::PathBuf;
use svbatch::audio::pipeline_description;
use svbatch::batch::{SubmitOutcome, SubmissionRegistry};
use svbatch::{canonical_jobs, duplicate_submission_jobs, reordered_jobs};

#[test]
fn idempotency_key_is_position_independent() {
    let canonical = canonical_jobs(0);
    let reordered = reordered_jobs(0);
    let mut keys_a: Vec<String> = canonical.iter().map(|j| j.idempotency_key()).collect();
    let mut keys_b: Vec<String> = reordered.iter().map(|j| j.idempotency_key()).collect();
    keys_a.sort();
    keys_b.sort();
    assert_eq!(keys_a, keys_b);
}

#[test]
fn duplicate_submission_list_has_one_repeated_key() {
    let jobs = duplicate_submission_jobs(0);
    assert_eq!(jobs.len(), 10);
    let registry_jobs = canonical_jobs(0);
    assert_eq!(registry_jobs.len(), 9);
    let first_key = jobs[0].idempotency_key();
    let repeats = jobs
        .iter()
        .filter(|j| j.idempotency_key() == first_key)
        .count();
    assert_eq!(repeats, 2);
}

#[test]
fn registry_accepts_first_and_rejects_second_submission() {
    let mut registry = SubmissionRegistry::new();
    let jobs = duplicate_submission_jobs(0);
    assert_eq!(registry.submit(&jobs[0]), SubmitOutcome::Accepted);
    assert_eq!(registry.submit(&jobs[9]), SubmitOutcome::Duplicate);
    assert!(registry.contains(&jobs[0]));
}

#[test]
fn reordering_changes_order_but_not_job_multiset() {
    let canonical = canonical_jobs(0);
    let mut reordered = reordered_jobs(0);
    assert_ne!(
        canonical.iter().map(|j| &j.job_id).collect::<Vec<_>>(),
        reordered.iter().map(|j| &j.job_id).collect::<Vec<_>>()
    );
    let mut canonical_ids: Vec<&str> = canonical.iter().map(|j| j.job_id.as_str()).collect();
    let mut reordered_ids: Vec<&str> = reordered.iter().map(|j| j.job_id.as_str()).collect();
    canonical_ids.sort();
    reordered_ids.sort();
    assert_eq!(canonical_ids, reordered_ids);
    // Sanity: nine unique jobs in both lists.
    assert_eq!(canonical_ids.len(), 9);
}

#[test]
fn pipeline_description_is_deterministic_and_uses_absolute_output() {
    let jobs = canonical_jobs(0);
    let out = PathBuf::from("/tmp/evidence/outputs/x.wav");
    let first = pipeline_description(&jobs[0], &out);
    let second = pipeline_description(&jobs[0], &out);
    assert_eq!(first, second);
    assert!(first.contains("filesink location=\"/tmp/evidence/outputs/x.wav\""));
    assert!(first.contains("volume volume="));
    assert!(first.contains("speed speed="));
}
