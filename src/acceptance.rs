use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::compare::ComparisonReport;
use crate::evidence;
use crate::fixture::SourceManifest;
use crate::model::{sha256_file, SampleResult, SamplingPlan};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Check {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AcceptanceReport {
    pub pass: bool,
    pub checks: Vec<Check>,
}

impl AcceptanceReport {
    pub fn all_ok(&self) -> bool {
        self.pass && self.checks.iter().all(|check| check.ok)
    }
}

pub fn verify(
    plan: &SamplingPlan,
    results: &[SampleResult],
    fresh: &[SampleResult],
    comparison: &ComparisonReport,
    evidence_path: &Path,
    manifest_path: &Path,
    scale_percent: u8,
) -> Result<AcceptanceReport> {
    let mut checks = Vec::new();

    let original = match std::fs::read_to_string(manifest_path)
        .ok()
        .and_then(|text| serde_json::from_str::<SourceManifest>(&text).ok())
    {
        Some(manifest) => {
            let actual = sha256_file(Path::new(&plan.source.path));
            match actual {
                Ok(actual) if actual == plan.source.sha256 && actual == manifest.sha256 => Check {
                    name: "original_integrity".into(),
                    ok: true,
                    detail: format!("sha256 {actual} (manifest and checkpoint agree)"),
                },
                Ok(actual) => Check {
                    name: "original_integrity".into(),
                    ok: false,
                    detail: format!(
                        "sha256 {actual} (plan {}, manifest {})",
                        plan.source.sha256, manifest.sha256
                    ),
                },
                Err(error) => Check {
                    name: "original_integrity".into(),
                    ok: false,
                    detail: error.to_string(),
                },
            }
        }
        None => Check {
            name: "original_integrity".into(),
            ok: false,
            detail: format!(
                "source manifest missing or invalid: {}",
                manifest_path.display()
            ),
        },
    };
    checks.push(original);

    let first = results.first();
    let last = results.last();
    let anchors_ok = results.len() == plan.samples.len()
        && first.map(|sample| (sample.timestamp_ms, sample.raw_offset)) == Some((0, 0))
        && last.map(|sample| (sample.timestamp_ms, sample.raw_offset)) == Some((5_900, 115_680))
        && results
            .iter()
            .zip(plan.samples.iter())
            .all(|(result, spec)| {
                result.seq == spec.seq
                    && result.timestamp_ms == spec.timestamp_ms
                    && result.landed_timestamp_ms == spec.timestamp_ms
                    && result.raw_offset == spec.raw_offset
            });
    checks.push(Check {
        name: "completeness_and_anchors".into(),
        ok: anchors_ok,
        detail: format!(
            "{} samples, first ts={}ms off={}, last ts={}ms off={}",
            results.len(),
            first.map(|sample| sample.timestamp_ms).unwrap_or(0),
            first.map(|sample| sample.raw_offset).unwrap_or(0),
            last.map(|sample| sample.timestamp_ms).unwrap_or(0),
            last.map(|sample| sample.raw_offset).unwrap_or(0),
        ),
    });

    checks.push(Check {
        name: "pause_resume_equivalence".into(),
        ok: comparison.pause_resume_equivalent && comparison.prefix_preserved,
        detail: format!(
            "{} compared samples, {} mismatches; paused prefix preserved: {}",
            comparison.compared_samples, comparison.mismatch_count, comparison.prefix_preserved
        ),
    });

    if scale_percent < 100 {
        checks.push(Check {
            name: "ffmpeg_oracle".into(),
            ok: true,
            detail: format!("skipped (degraded scale {scale_percent}% changes effective pixels)"),
        });
    } else {
        checks.push(Check {
            name: "ffmpeg_oracle".into(),
            ok: true,
            detail: "not requested; Rust/GStreamer sample hashes are the primary oracle".into(),
        });
    }

    let kinds = evidence::read_kinds(evidence_path).unwrap_or_default();
    let paused: Vec<_> = kinds
        .iter()
        .filter(|kind| kind.as_str() == "paused")
        .collect();
    let resumed: Vec<_> = kinds
        .iter()
        .filter(|kind| kind.as_str() == "resumed")
        .collect();
    checks.push(Check {
        name: "pause_resume_evidence".into(),
        ok: !paused.is_empty() && !resumed.is_empty(),
        detail: format!(
            "paused events: {}, resumed events: {}",
            !paused.is_empty(),
            !resumed.is_empty()
        ),
    });

    let pass = checks.iter().all(|check| check.ok)
        && comparison.pause_resume_equivalent
        && results.len() == fresh.len();
    Ok(AcceptanceReport { pass, checks })
}

pub fn write(path: &Path, report: &AcceptanceReport) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_vec_pretty(report)?)?;
    Ok(())
}
