use std::fs;
use std::path::Path;

use crate::model::{EvidenceFile, Implementation, SCHEMA};

pub fn now_utc() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

pub fn implementation() -> Implementation {
    let rust = std::process::Command::new("rustc")
        .arg("--version")
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map_or_else(|| "unknown".into(), |line| line.trim().to_string());
    Implementation {
        language: "Rust".into(),
        language_version: rust,
        media: "GStreamer".into(),
        media_version: gstreamer::version_string().to_string(),
        ui: "GTK4".into(),
        ui_version: format!(
            "{}.{}.{}",
            gtk4::major_version(),
            gtk4::minor_version(),
            gtk4::micro_version()
        ),
    }
}

pub fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(value)?;
    fs::write(path, bytes)?;
    Ok(())
}

pub fn read_evidence(path: &Path) -> anyhow::Result<EvidenceFile> {
    let bytes = fs::read(path)?;
    let evidence: EvidenceFile = serde_json::from_slice(&bytes)?;
    if evidence.schema != SCHEMA {
        anyhow::bail!(
            "unsupported evidence schema {}, expected {SCHEMA}",
            evidence.schema
        );
    }
    Ok(evidence)
}
