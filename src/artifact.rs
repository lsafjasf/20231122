use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use crate::model::{ArtifactEntry, ArtifactManifest, RepairInput, RepairReport, SCHEMA_VERSION};
use crate::util::{sha256_bytes, sha256_file};

#[derive(Debug, Clone)]
pub struct ArtifactBundle {
    pub root: PathBuf,
    pub report: RepairReport,
    pub report_bytes: Vec<u8>,
    pub manifest: ArtifactManifest,
    pub sha256sums: Vec<ShaLine>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ShaLine {
    pub sha256: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VerificationSummary {
    pub run_id: String,
    pub status: String,
    pub artifacts_verified: usize,
    pub inputs_verified: usize,
    pub inputs_missing: usize,
    pub warnings: Vec<String>,
}

pub fn load_bundle(root: &Path) -> Result<ArtifactBundle> {
    let report_path = root.join("report.json");
    let report_bytes = std::fs::read(&report_path)
        .with_context(|| format!("cannot read {}", report_path.display()))?;
    let report: RepairReport = serde_json::from_slice(&report_bytes)
        .with_context(|| format!("invalid {}", report_path.display()))?;
    if report.schema_version != SCHEMA_VERSION {
        bail!(
            "unsupported report schema_version={} expected={}",
            report.schema_version,
            SCHEMA_VERSION
        );
    }
    let manifest_relative = safe_relative_path(&report.integrity.artifact_manifest_path)?;
    let manifest_path = root.join(manifest_relative);
    let manifest_bytes = std::fs::read(&manifest_path)
        .with_context(|| format!("cannot read {}", manifest_path.display()))?;
    let manifest: ArtifactManifest = serde_json::from_slice(&manifest_bytes)
        .with_context(|| format!("invalid {}", manifest_path.display()))?;
    if manifest.schema_version != SCHEMA_VERSION {
        bail!(
            "unsupported artifact manifest schema_version={}",
            manifest.schema_version
        );
    }
    if manifest.run_id != report.run_id {
        bail!(
            "run_id mismatch: report={} manifest={}",
            report.run_id,
            manifest.run_id
        );
    }
    let sha256sums = read_sha256sums(root)?;
    Ok(ArtifactBundle {
        root: root.to_path_buf(),
        report,
        report_bytes,
        manifest,
        sha256sums,
    })
}

pub fn verify_bundle(root: &Path) -> Result<VerificationSummary> {
    let bundle = load_bundle(root)?;
    verify_manifest(&bundle)?;
    verify_sha256sums(&bundle)?;
    verify_report_associations(&bundle)?;
    let mut warnings = Vec::new();
    let mut inputs_verified = 0_usize;
    let mut inputs_missing = 0_usize;
    for (label, expected) in [
        ("recording", &bundle.report.inputs.recording),
        ("intervals", &bundle.report.inputs.intervals),
        ("versions", &bundle.report.inputs.versions),
    ] {
        let path = Path::new(&expected.path);
        if !path.exists() {
            inputs_missing += 1;
            warnings.push(format!(
                "input {label} is not available at {}",
                path.display()
            ));
            continue;
        }
        if expected.sha256.starts_with("unavailable:") {
            inputs_missing += 1;
            if let Ok(metadata) = std::fs::metadata(path)
                && expected.bytes > 0
                && metadata.len() != expected.bytes
            {
                warnings.push(format!(
                    "input {label} size changed while its hash was unavailable: expected={} actual={}",
                    expected.bytes,
                    metadata.len()
                ));
            }
            warnings.push(format!(
                "input {label} hash unavailable at export: {}",
                expected.sha256
            ));
            continue;
        }
        let (bytes, sha256) = sha256_file(path)?;
        if bytes != expected.bytes || sha256 != expected.sha256 {
            bail!("input {label} changed since export: {}", path.display());
        }
        inputs_verified += 1;
    }
    verify_input_order(&bundle, &mut warnings)?;
    Ok(VerificationSummary {
        run_id: bundle.report.run_id,
        status: format!("{:?}", bundle.report.status),
        artifacts_verified: bundle.manifest.entries.len(),
        inputs_verified,
        inputs_missing,
        warnings,
    })
}

fn verify_manifest(bundle: &ArtifactBundle) -> Result<()> {
    let mut seen = BTreeSet::new();
    for entry in &bundle.manifest.entries {
        let relative = safe_relative_path(&entry.path)?;
        if !seen.insert(entry.path.clone()) {
            bail!("duplicate artifact path in manifest: {}", entry.path);
        }
        let path = bundle.root.join(relative);
        let (bytes, sha256) = sha256_file(&path)?;
        if bytes != entry.bytes {
            bail!(
                "artifact size mismatch: {} expected={} actual={}",
                entry.path,
                entry.bytes,
                bytes
            );
        }
        if sha256 != entry.sha256 {
            bail!(
                "artifact sha256 mismatch: {} expected={} actual={}",
                entry.path,
                entry.sha256,
                sha256
            );
        }
    }
    Ok(())
}

fn verify_sha256sums(bundle: &ArtifactBundle) -> Result<()> {
    let manifest_lines: Vec<ShaLine> = bundle
        .manifest
        .entries
        .iter()
        .map(|entry| ShaLine {
            sha256: entry.sha256.clone(),
            path: entry.path.clone(),
        })
        .collect();
    if bundle.sha256sums != manifest_lines {
        bail!("SHA256SUMS does not match artifact_manifest.json order or contents");
    }
    Ok(())
}

fn verify_report_associations(bundle: &ArtifactBundle) -> Result<()> {
    let by_path = bundle
        .manifest
        .entries
        .iter()
        .map(|entry| (entry.path.as_str(), entry))
        .collect::<std::collections::BTreeMap<_, _>>();
    for (index, version) in bundle.report.versions.iter().enumerate() {
        if version.manifest_index != index {
            bail!(
                "version order changed: manifest_index={} array_index={}",
                version.manifest_index,
                index
            );
        }
        let output = by_path
            .get(version.output_path.as_str())
            .ok_or_else(|| anyhow!("output missing from manifest: {}", version.output_path))?;
        if output.sha256 != version.output_sha256 || output.bytes != version.output_bytes {
            bail!(
                "output evidence mismatch for version {}",
                version.version_id
            );
        }
        let waveform = by_path
            .get(version.waveform_path.as_str())
            .ok_or_else(|| anyhow!("waveform missing from manifest: {}", version.waveform_path))?;
        if waveform.sha256 != version.waveform_sha256 {
            bail!(
                "waveform evidence mismatch for version {}",
                version.version_id
            );
        }
        verify_waveform_csv(bundle, version)?;
        let refs = version
            .interval_ids
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        let mut seen_applied = BTreeSet::new();
        for applied in &version.applied_intervals {
            if !refs.contains(applied.interval_id.as_str()) {
                bail!(
                    "version {} applied interval {} is not in its manifest order",
                    version.version_id,
                    applied.interval_id
                );
            }
            if !seen_applied.insert(applied.interval_id.as_str()) {
                bail!(
                    "version {} applies interval {} more than once",
                    version.version_id,
                    applied.interval_id
                );
            }
            let evidence = bundle
                .report
                .intervals
                .iter()
                .find(|item| item.interval_id == applied.interval_id)
                .ok_or_else(|| {
                    anyhow!(
                        "version {} applied interval {} has no mapping evidence",
                        version.version_id,
                        applied.interval_id
                    )
                })?;
            if !evidence.applied
                || evidence.applied_start_frame != applied.start_frame
                || evidence.applied_end_frame != applied.end_frame
            {
                bail!(
                    "version {} applied interval {} frame evidence mismatch",
                    version.version_id,
                    applied.interval_id
                );
            }
        }
    }
    if !bundle
        .report
        .versions
        .iter()
        .all(|version| by_path.contains_key(version.output_path.as_str()))
    {
        bail!("one or more report versions have no manifest output");
    }
    Ok(())
}

fn verify_waveform_csv(
    bundle: &ArtifactBundle,
    version: &crate::model::VersionResult,
) -> Result<()> {
    let path = bundle
        .root
        .join(safe_relative_path(&version.waveform_path)?);
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("cannot read waveform CSV {}", path.display()))?;
    let mut lines = text.lines();
    let header = lines
        .next()
        .ok_or_else(|| anyhow!("waveform CSV is empty: {}", version.waveform_path))?;
    const EXPECTED_HEADER: &str = "bucket_index,start_frame,end_frame,start_ms,end_ms,peak,rms";
    if header != EXPECTED_HEADER {
        bail!(
            "waveform CSV header changed for {}: {}",
            version.version_id,
            header
        );
    }
    let rows = lines.collect::<Vec<_>>();
    if rows.len() != version.waveform.bucket_count {
        bail!(
            "waveform bucket count mismatch for {}: report={} csv={}",
            version.version_id,
            version.waveform.bucket_count,
            rows.len()
        );
    }
    let mut expected_start = 0_u64;
    for (index, (row, bucket)) in rows.iter().zip(&version.waveform.buckets).enumerate() {
        let fields = row.split(',').collect::<Vec<_>>();
        if fields.len() != 7 {
            bail!(
                "waveform row {} has {} fields, expected 7",
                index + 1,
                fields.len()
            );
        }
        let csv_index = fields[0].parse::<usize>()?;
        let start_frame = fields[1].parse::<u64>()?;
        let end_frame = fields[2].parse::<u64>()?;
        let peak = fields[5].parse::<f32>()?;
        let rms = fields[6].parse::<f32>()?;
        if csv_index != bucket.index
            || start_frame != bucket.start_frame
            || end_frame != bucket.end_frame
            || start_frame != expected_start
            || (peak - bucket.peak).abs() > 0.000_001
            || (rms - bucket.rms).abs() > 0.000_001
        {
            bail!(
                "waveform CSV row {} does not match report for version {}",
                index,
                version.version_id
            );
        }
        expected_start = end_frame;
    }
    if version.waveform.bucket_count > 0 && expected_start != version.waveform.source_frame_count {
        bail!(
            "waveform does not cover all output frames for version {}",
            version.version_id
        );
    }
    Ok(())
}

fn verify_input_order(bundle: &ArtifactBundle, warnings: &mut Vec<String>) -> Result<()> {
    let terminated = matches!(
        bundle.report.status,
        crate::model::RunStatus::Rejected | crate::model::RunStatus::Failed
    );
    let intervals_path = Path::new(&bundle.report.inputs.intervals.path);
    if intervals_path.exists() {
        let metadata = std::fs::metadata(intervals_path)?;
        if metadata.len() > bundle.report.inputs.limits.max_json_bytes && terminated {
            warnings.push(
                "intervals input exceeds its recorded JSON limit; order comparison skipped"
                    .to_string(),
            );
            return verify_version_order(bundle, warnings);
        }
        let bytes = std::fs::read(intervals_path)
            .with_context(|| format!("cannot read {}", intervals_path.display()))?;
        let input: RepairInput = match serde_json::from_slice(&bytes) {
            Ok(input) => input,
            Err(error) if terminated => {
                warnings.push(format!(
                    "intervals input is not parseable for a rejected report: {error}"
                ));
                return verify_version_order(bundle, warnings);
            }
            Err(error) => return Err(error.into()),
        };
        let expected = input
            .intervals
            .iter()
            .map(|interval| interval.id.as_str())
            .collect::<Vec<_>>();
        let actual = bundle
            .report
            .intervals
            .iter()
            .map(|interval| interval.interval_id.as_str())
            .collect::<Vec<_>>();
        if actual.is_empty() && terminated && !expected.is_empty() {
            warnings.push(
                "rejected/failed report has no mapped interval rows; input order cannot be compared"
                    .to_string(),
            );
            return verify_version_order(bundle, warnings);
        }
        if expected != actual {
            bail!("report interval order does not match input interval order");
        }
    } else {
        warnings.push("cannot compare interval order without the original intervals file".into());
    }
    verify_version_order(bundle, warnings)
}

fn verify_version_order(bundle: &ArtifactBundle, warnings: &mut Vec<String>) -> Result<()> {
    let versions_path = Path::new(&bundle.report.inputs.versions.path);
    if !versions_path.exists() {
        warnings.push("cannot compare version order without the original versions file".into());
        return Ok(());
    }
    let metadata = std::fs::metadata(versions_path)?;
    if metadata.len() > bundle.report.inputs.limits.max_json_bytes {
        warnings.push(
            "versions input exceeds its recorded JSON limit; order comparison skipped".to_string(),
        );
        return Ok(());
    }
    let bytes = std::fs::read(versions_path)
        .with_context(|| format!("cannot read {}", versions_path.display()))?;
    let input: crate::model::VersionManifest = match serde_json::from_slice(&bytes) {
        Ok(input) => input,
        Err(error)
            if matches!(
                bundle.report.status,
                crate::model::RunStatus::Rejected | crate::model::RunStatus::Failed
            ) =>
        {
            warnings.push(format!(
                "versions input is not parseable for a rejected report: {error}"
            ));
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };
    if bundle.report.versions.is_empty()
        && matches!(
            bundle.report.status,
            crate::model::RunStatus::Rejected | crate::model::RunStatus::Failed
        )
    {
        warnings.push(
            "rejected/failed report has no rendered version rows; manifest order is retained as evidence"
                .to_string(),
        );
        return Ok(());
    }
    let expected = input
        .versions
        .iter()
        .map(|version| (&version.id, &version.interval_ids))
        .collect::<Vec<_>>();
    let actual = bundle
        .report
        .versions
        .iter()
        .map(|version| (&version.version_id, &version.interval_ids))
        .collect::<Vec<_>>();
    if expected != actual {
        bail!("report version order or interval reference order does not match input manifest");
    }
    Ok(())
}

pub fn read_sha256sums(root: &Path) -> Result<Vec<ShaLine>> {
    let path = root.join("SHA256SUMS");
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("cannot read {}", path.display()))?;
    let mut lines = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let (sha256, file_path) = line.split_once("  ").ok_or_else(|| {
            anyhow!(
                "invalid SHA256SUMS line {} (expected '<sha>  <path>')",
                index + 1
            )
        })?;
        if sha256.len() != 64 || !sha256.chars().all(|ch| ch.is_ascii_hexdigit()) {
            bail!("invalid sha256 at SHA256SUMS line {}", index + 1);
        }
        lines.push(ShaLine {
            sha256: sha256.to_string(),
            path: file_path.to_string(),
        });
    }
    Ok(lines)
}

pub fn render_sha256sums(entries: &[ArtifactEntry]) -> String {
    let mut output = String::new();
    for entry in entries {
        output.push_str(&format!("{}  {}\n", entry.sha256, entry.path));
    }
    output
}

pub fn artifact_entry(root: &Path, kind: &str, relative: &str) -> Result<ArtifactEntry> {
    let relative = safe_relative_path(relative)?;
    let (bytes, sha256) = sha256_file(&root.join(&relative))?;
    Ok(ArtifactEntry {
        kind: kind.to_string(),
        path: relative.to_string_lossy().replace('\\', "/"),
        bytes,
        sha256,
    })
}

pub fn safe_relative_path(value: &str) -> Result<PathBuf> {
    let path = Path::new(value);
    if path.is_absolute() {
        bail!("artifact path must be relative: {value}");
    }
    for component in path.components() {
        if !matches!(component, Component::Normal(_)) {
            bail!("artifact path contains unsafe component: {value}");
        }
    }
    Ok(path.to_path_buf())
}

pub fn pretty_json_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    Ok(bytes)
}

pub fn roundtrip_report(bundle: &ArtifactBundle) -> Result<Vec<u8>> {
    let rendered = pretty_json_bytes(&bundle.report)?;
    if rendered != bundle.report_bytes {
        bail!("report field order or values changed after reload");
    }
    let digest = sha256_bytes(&rendered);
    if digest.is_empty() {
        bail!("unreachable digest");
    }
    Ok(rendered)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsafe_artifact_paths_are_rejected() {
        assert!(safe_relative_path("../secret").is_err());
        assert!(safe_relative_path("/etc/passwd").is_err());
        assert!(safe_relative_path("media/a.wav").is_ok());
    }

    #[test]
    fn sha_render_and_parse_are_stable() {
        let entries = vec![ArtifactEntry {
            kind: "media".to_string(),
            path: "media/a.wav".to_string(),
            bytes: 3,
            sha256: "a".repeat(64),
        }];
        let text = render_sha256sums(&entries);
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("SHA256SUMS"), text).unwrap();
        let parsed = read_sha256sums(dir.path()).unwrap();
        assert_eq!(parsed[0].path, "media/a.wav");
        assert_eq!(parsed[0].sha256, "a".repeat(64));
    }
}
