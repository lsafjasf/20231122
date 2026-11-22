//! Headless entry points used for tests and scripted review.
//!
//! The GUI is the primary interface; these commands make the same engine
//! runnable over SSH/CI and give reviewers an option without a display.

use std::path::PathBuf;

use anyhow::{bail, Context, Result};

use crate::engine;
use crate::model::Project;
use crate::review;
use crate::storage;
use crate::util::now_rfc3339;

pub struct AddArgs {
    pub project_dir: PathBuf,
    pub device_label: String,
    pub source_path: String,
    pub source_rate: u32,
    pub gap_start_ns: u64,
    pub gap_duration_ns: u64,
    pub gap_reason: String,
}

pub fn create_project(project_dir: PathBuf, target_rate: u32) -> Result<()> {
    let path = storage::project_path(&project_dir);
    if path.exists() {
        bail!("project already exists: {}", path.display());
    }
    let project_id = project_dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "srcrepair-project".into());
    let project = Project::new(project_id, target_rate, now_rfc3339());
    storage::save_project(&project_dir, &project)?;
    println!("created {}", path.display());
    Ok(())
}

pub fn add_fragment(args: AddArgs) -> Result<String> {
    let mut project = storage::load_project(&args.project_dir)?;
    let id = project.next_fragment_id();
    let fragment = crate::model::Fragment::new(
        id.clone(),
        args.device_label,
        args.source_path,
        args.source_rate,
        project.target_rate,
        args.gap_start_ns,
        args.gap_duration_ns,
        args.gap_reason,
        now_rfc3339(),
    );
    project.fragments.push(fragment);
    project.updated_at = now_rfc3339();
    storage::save_project(&args.project_dir, &project)?;
    println!("added {id}");
    Ok(id)
}

pub fn run(project_dir: PathBuf, id: Option<String>) -> Result<()> {
    crate::media::init()?;
    let mut project = storage::load_project(&project_dir)?;
    if engine::recover_interrupted(&mut project) {
        storage::save_project(&project_dir, &project)?;
    }
    let ids: Vec<String> = match id {
        Some(id) => vec![id],
        None => project.fragments.iter().map(|f| f.id.clone()).collect(),
    };
    for fragment_id in ids {
        match engine::run_fragment(&mut project, &project_dir, &fragment_id) {
            Ok(evidence) => println!(
                "{} done: {} frames, sha256 {}",
                fragment_id, evidence.output_frames, evidence.output_sha256
            ),
            Err(err) => {
                eprintln!("{fragment_id} failed: {err:#}");
                return Err(err);
            }
        }
    }
    Ok(())
}

pub fn verify(project_dir: PathBuf) -> Result<()> {
    let project = storage::load_project(&project_dir)?;
    let mut failed = 0;
    for (id, result) in engine::verify_project(&project, &project_dir) {
        match result {
            Ok(()) => println!("{id}: OK"),
            Err(err) => {
                failed += 1;
                eprintln!("{id}: FAIL - {err:#}");
            }
        }
    }
    if failed > 0 {
        bail!("{failed} fragment(s) failed verification");
    }
    Ok(())
}

pub fn export(project_dir: PathBuf) -> Result<()> {
    let mut project = storage::load_project(&project_dir)?;
    let report = review::export_bundle(&mut project, &project_dir)
        .context("export refused")?;
    println!(
        "exported {} fragment(s) to {}",
        report.fragment_count,
        report.bundle_dir.display()
    );
    Ok(())
}
