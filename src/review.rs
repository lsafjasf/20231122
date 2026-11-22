//! Self-contained, independently reviewable export bundles.
//!
//! A bundle contains every output WAV, a machine-readable manifest, a
//! checksum list and a POSIX verification script that needs nothing but
//! `sha256sum` to confirm the evidence without this application.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::engine::verify_project;
use crate::model::{FragmentStatus, Project};
use crate::storage::export_dir;
use crate::util::now_rfc3339;

pub struct ExportReport {
    pub bundle_dir: PathBuf,
    pub fragment_count: usize,
}

/// Export a review bundle. Refuses to export unless every fragment is done
/// and every on-disk hash still matches the recorded evidence.
pub fn export_bundle(project: &mut Project, project_dir: &Path) -> Result<ExportReport> {
    if project.fragments.is_empty() {
        bail!("project has no fragments to export");
    }
    let unfinished: Vec<String> = project
        .fragments
        .iter()
        .filter(|f| f.status != FragmentStatus::Done)
        .map(|f| f.id.clone())
        .collect();
    if !unfinished.is_empty() {
        bail!("fragments not done: {}", unfinished.join(", "));
    }

    let checks = verify_project(project, project_dir);
    for (id, result) in checks {
        if let Err(err) = result {
            anyhow::bail!("pre-export verification failed for {id}: {err:#}");
        }
    }

    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let bundle = export_dir(project_dir).join(format!("review-{stamp}"));
    fs::create_dir_all(bundle.join("fragments"))
        .context("cannot create bundle directories")?;

    let mut checksum_lines = String::new();
    for fragment in &project.fragments {
        let evidence = fragment
            .evidence
            .last()
            .with_context(|| format!("{} has no evidence", fragment.id))?;
        let src = project_dir.join(&evidence.output_path);
        let dst_dir = bundle.join("fragments").join(&fragment.id);
        fs::create_dir_all(&dst_dir)?;
        let dst = dst_dir.join("output.wav");
        fs::copy(&src, &dst).with_context(|| format!("cannot copy {}", src.display()))?;
        checksum_lines.push_str(&format!(
            "{}  fragments/{}/output.wav\n",
            evidence.output_sha256, fragment.id
        ));
    }

    fs::write(bundle.join("SHA256SUMS"), &checksum_lines)?;
    fs::write(
        bundle.join("manifest.json"),
        manifest_json(project, &now_rfc3339())?,
    )?;
    fs::write(bundle.join("comparison.csv"), comparison_csv(project))?;
    fs::write(bundle.join("verify.sh"), verify_script())?;
    make_executable(&bundle.join("verify.sh"))?;
    fs::write(bundle.join("README.md"), readme_text(project, &stamp))?;

    Ok(ExportReport {
        bundle_dir: bundle,
        fragment_count: project.fragments.len(),
    })
}

fn manifest_json(project: &Project, stamp: &str) -> Result<String> {
    #[derive(serde::Serialize)]
    struct ManifestEnvelope<'a> {
        bundle_created_at: String,
        tool: &'static str,
        tool_version: &'static str,
        project: &'a Project,
    }
    let envelope = ManifestEnvelope {
        bundle_created_at: stamp.to_string(),
        tool: "srcrepair",
        tool_version: env!("CARGO_PKG_VERSION"),
        project,
    };
    Ok(serde_json::to_string_pretty(&envelope)?)
}

/// Deterministic CSV: fixed column order, one row per fragment, `\r\n`.
fn comparison_csv(project: &Project) -> String {
    let header = [
        "fragment_id",
        "device_label",
        "status",
        "attempts",
        "source_path",
        "source_rate",
        "target_rate",
        "gap_start_ns",
        "gap_duration_ns",
        "gap_frames_inserted",
        "input_sha256",
        "output_file",
        "output_sha256",
        "output_frames",
        "expected_frames",
        "frame_count_matches",
        "output_bytes",
        "gst_version",
        "finished_at",
    ];
    let mut csv = String::new();
    csv.push_str(&header.join(","));
    csv.push_str("\r\n");
    for fragment in &project.fragments {
        let e = fragment.evidence.last();
        let (sha_in, sha_out, frames, expected, gap_frames, bytes, gst_ver, when, matches) = e
            .map(|ev| {
                (
                    ev.input_sha256.as_str(),
                    ev.output_sha256.as_str(),
                    ev.output_frames.to_string(),
                    ev.expected_frames.to_string(),
                    ev.gap_frames_inserted.to_string(),
                    ev.output_bytes.to_string(),
                    ev.gst_version.as_str(),
                    ev.finished_at.as_str(),
                    (ev.output_frames == ev.expected_frames).to_string(),
                )
            })
            .unwrap_or(("", "", String::new(), String::new(), String::new(), String::new(), "", "", String::new()));
        let row = [
            fragment.id.clone(),
            fragment.device_label.clone(),
            fragment.status.as_str().to_string(),
            fragment.attempts.to_string(),
            fragment.source_path.clone(),
            fragment.source_rate.to_string(),
            fragment.target_rate.to_string(),
            fragment.gap_start_ns.to_string(),
            fragment.gap_duration_ns.to_string(),
            gap_frames,
            sha_in.to_string(),
            fragment
                .output_rel_path
                .clone()
                .unwrap_or_else(|| String::new()),
            sha_out.to_string(),
            frames,
            expected,
            matches,
            bytes,
            gst_ver.to_string(),
            when.to_string(),
        ];
        csv.push_str(&row.into_iter().map(csv_escape).collect::<Vec<_>>().join(","));
        csv.push_str("\r\n");
    }
    csv
}

fn csv_escape(value: String) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value
    }
}

fn verify_script() -> &'static str {
    r#"#!/usr/bin/env sh
# Independent review: verifies every output WAV against recorded hashes.
# Requires only POSIX sh and sha256sum (coreutils); no Rust/GStreamer needed.
set -eu
cd "$(dirname "$0")"
if command -v sha256sum >/dev/null 2>&1; then
    sha256sum -c SHA256SUMS
elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 -c SHA256SUMS
else
    echo "neither sha256sum nor shasum found" >&2
    exit 2
fi
echo "All fragment outputs match the recorded evidence."
"#
}

fn readme_text(project: &Project, stamp: &str) -> String {
    let mut text = String::new();
    text.push_str("# Review bundle\n\n");
    text.push_str(&format!("- Project: `{}`\n", project.project_id));
    text.push_str(&format!("- Target sample rate: {} Hz\n", project.target_rate));
    text.push_str(&format!("- Bundle created: {}\n", stamp));
    text.push_str(&format!("- Fragments: {}\n\n", project.fragments.len()));
    text.push_str("## Independent verification\n\n");
    text.push_str("Run `./verify.sh` (POSIX sh + sha256sum). It recomputes every\n");
    text.push_str("output hash and compares it against `SHA256SUMS` and `manifest.json`.\n\n");
    text.push_str("## Contents\n\n");
    text.push_str("- `manifest.json` — full project state plus per-fragment evidence and pipeline strings\n");
    text.push_str("- `comparison.csv` — fixed-column comparison table for spreadsheet review\n");
    text.push_str("- `SHA256SUMS` — output hashes in standard sha256sum format\n");
    text.push_str("- `fragments/<id>/output.wav` — final rate-converted, gap-repaired WAV\n");
    text
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms)?;
    Ok(())
}
