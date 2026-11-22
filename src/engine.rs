//! Orchestration layer: per-fragment state machine with crash-safe saves.
//!
//! Every state transition (`running`, then `done`/`failed`) is immediately
//! flushed to disk. If the application exits mid-run, the project reloads
//! with that fragment marked `running`; the next launch offers a rerun and
//! no partial evidence is ever accepted.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::media::{self, RunOutcome};
use crate::model::{Fragment, FragmentStatus, Project, RunEvidence};
use crate::storage::{output_dir, save_project};
use crate::util::now_rfc3339;

/// Recover fragments left in `running` by an interrupted session.
pub fn recover_interrupted(project: &mut Project) -> bool {
    let mut changed = false;
    for fragment in &mut project.fragments {
        if fragment.status == FragmentStatus::Running {
            fragment.status = FragmentStatus::Failed;
            fragment.last_error = Some(
                "interrupted before completion; rerun the fragment to produce evidence".into(),
            );
            fragment.updated_at = now_rfc3339();
            changed = true;
        }
    }
    changed
}

fn fragment_work_dir(project_dir: &Path, id: &str) -> PathBuf {
    output_dir(project_dir).join(id)
}

fn fragment_output_rel(id: &str) -> String {
    format!("fragments/{id}/output.wav")
}

/// Run (or rerun) a single fragment. Reruns never overwrite prior evidence:
/// a new attempt is appended, keeping the full audit trail.
pub fn run_fragment(project: &mut Project, project_dir: &Path, id: &str) -> Result<RunEvidence> {
    let index = project
        .find_index(id)
        .with_context(|| format!("no fragment with id {id}"))?;

    project.fragments[index].status = FragmentStatus::Running;
    project.fragments[index].attempts += 1;
    project.fragments[index].updated_at = now_rfc3339();
    project.updated_at = now_rfc3339();
    save_project(project_dir, project).context("cannot persist running state")?;

    let outcome = execute(index, project, project_dir);
    let result = match outcome {
        Ok((outcome, attempt)) => {
            let evidence = build_evidence(&project.fragments[index], attempt, outcome);
            let fragment = &mut project.fragments[index];
            fragment.status = FragmentStatus::Done;
            fragment.last_error = None;
            fragment.output_rel_path = Some(fragment_output_rel(id));
            fragment.evidence.push(evidence.clone());
            fragment.updated_at = now_rfc3339();
            project.updated_at = now_rfc3339();
            save_project(project_dir, project).context("cannot persist completed state")?;
            Ok(evidence)
        }
        Err(err) => {
            let fragment = &mut project.fragments[index];
            fragment.status = FragmentStatus::Failed;
            fragment.last_error = Some(format!("{err:#}"));
            fragment.updated_at = now_rfc3339();
            project.updated_at = now_rfc3339();
            let _ = save_project(project_dir, project);
            Err(err)
        }
    };
    result
}

fn execute(
    _index: usize,
    project: &Project,
    project_dir: &Path,
) -> Result<(RunOutcome, u32)> {
    let fragment = project
        .fragments
        .iter()
        .find(|f| f.status == FragmentStatus::Running)
        .context("no running fragment found")?
        .clone();
    let attempt = fragment.attempts;
    let work_dir = fragment_work_dir(project_dir, &fragment.id);
    let final_output = work_dir.join("output.wav");
    let outcome = media::process_fragment(&fragment, &work_dir, &final_output)?;
    Ok((outcome, attempt))
}

fn build_evidence(fragment: &Fragment, attempt: u32, outcome: RunOutcome) -> RunEvidence {
    let pipeline_desc = match &outcome.gap_pipeline {
        Some(gap) => format!(
            "# 1. resample\n{}\n# 2. missing-segment silence ({} frames)\n{}\n\
             # 3. deterministic PCM splice at frame {} (RIFF/WAVE header rebuilt; \
             exact PCM frame ranges copied)",
            outcome.resample_pipeline,
            outcome.gap_frames_inserted,
            gap,
            (fragment.target_rate as u128 * fragment.gap_start_ns as u128)
                / 1_000_000_000u128,
        ),
        None => format!("# resample only\n{}", outcome.resample_pipeline),
    };
    RunEvidence {
        attempt,
        finished_at: now_rfc3339(),
        pipeline_desc,
        gst_version: media::gst_version_string(),
        input_path: fragment.source_path.clone(),
        input_sha256: outcome.input_sha256,
        input_sample_rate: fragment.source_rate,
        input_channels: outcome.output_channels,
        input_frames: outcome.input_frames,
        output_path: fragment_output_rel(&fragment.id),
        output_sha256: outcome.output_sha256,
        output_sample_rate: outcome.output_sample_rate,
        output_channels: outcome.output_channels,
        output_frames: outcome.output_frames,
        expected_frames: outcome.expected_frames,
        gap_frames_inserted: outcome.gap_frames_inserted,
        duration_ns: outcome.duration_ns,
        output_bytes: outcome.output_bytes,
    }
}

/// Validate that all on-disk outputs still match their recorded hashes.
pub fn verify_project(project: &Project, project_dir: &Path) -> Vec<(String, Result<()>)> {
    project
        .fragments
        .iter()
        .map(|fragment| {
            let result = verify_fragment(fragment, project_dir);
            (fragment.id.clone(), result)
        })
        .collect()
}

pub fn verify_fragment(fragment: &Fragment, project_dir: &Path) -> Result<()> {
    let evidence = fragment
        .evidence
        .last()
        .context("fragment has no recorded evidence")?;
    let output = project_dir.join(&evidence.output_path);
    if !output.is_file() {
        bail!("output missing: {}", evidence.output_path);
    }
    let actual = crate::util::sha256_file(&output)?;
    if actual != evidence.output_sha256 {
        bail!(
            "hash mismatch: recorded {} but file is {actual}",
            evidence.output_sha256
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::model::{Fragment, FragmentStatus, Project};

    #[test]
    fn running_fragments_become_failed_with_rerun_message() {
        let mut project = Project::new("recover".into(), 48000, "2026-11-22T00:00:00Z".into());
        let mut fragment = Fragment::new(
            "f0001".into(),
            "dev".into(),
            "/tmp/source.wav".into(),
            44100,
            48000,
            0,
            0,
            String::new(),
            "2026-11-22T00:00:00Z".into(),
        );
        fragment.status = FragmentStatus::Running;
        project.fragments.push(fragment);

        assert!(super::recover_interrupted(&mut project));
        assert_eq!(project.fragments[0].status, FragmentStatus::Failed);
        assert!(project.fragments[0].last_error.is_some());
        // Idempotent: a second recovery pass changes nothing.
        assert!(!super::recover_interrupted(&mut project));
    }
}
