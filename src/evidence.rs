use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::model::{BoundaryEvent, EncodeSample, ProcessedPoint, StatsSnapshot};
use crate::pipeline::CaptureReport;
use crate::scenarios::{ScenarioKind, ScenarioRun};
use crate::waveform;

pub const SCHEMA_VERSION: &str = "encode-panel.evidence/v1";

#[derive(Debug, Clone, Serialize)]
pub struct ToolchainEvidence {
    pub rust: &'static str,
    pub gstreamer: String,
    pub gtk4: String,
    pub target_os: &'static str,
    pub target_arch: &'static str,
}

pub fn toolchain_evidence() -> ToolchainEvidence {
    ToolchainEvidence {
        rust: env!("RUSTC_VERSION"),
        gstreamer: gstreamer::version_string().to_string(),
        gtk4: format!(
            "{}.{}.{}",
            gtk4::major_version(),
            gtk4::minor_version(),
            gtk4::micro_version()
        ),
        target_os: std::env::consts::OS,
        target_arch: std::env::consts::ARCH,
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct InputEvidence {
    pub source: String,
    pub scenario: ScenarioKind,
    pub generated_samples: u64,
    pub requested_huge_samples: Option<u64>,
    pub rejected_samples: u64,
    pub retained_sample_count: usize,
    pub first: Option<EncodeSample>,
    pub last: Option<EncodeSample>,
    pub samples: Vec<EncodeSample>,
}

#[derive(Debug, Clone, Serialize)]
pub struct KeySamples {
    pub first_processed: Option<ProcessedPoint>,
    pub last_processed: Option<ProcessedPoint>,
    pub first_boundary: Option<BoundaryEvent>,
    pub last_boundary: Option<BoundaryEvent>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProcessingEvidence {
    pub params: crate::model::ProcessingParams,
    pub parameter_changes: Vec<String>,
    pub waveform_points: Vec<crate::model::WaveformPoint>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BoundaryEvidence {
    pub events_total: u64,
    pub events_dropped: u64,
    pub events: Vec<BoundaryEvent>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InputCoverage {
    pub empty_input_covered: bool,
    pub huge_input_covered: bool,
    pub boundary_before_after_captured: bool,
    pub bounded_waveform: bool,
    pub duration_us: u128,
}

#[derive(Debug, Clone, Serialize)]
pub struct EvidenceBundle {
    pub schema_version: String,
    pub scenario: ScenarioKind,
    pub created_at_unix_ms: u128,
    pub toolchain: ToolchainEvidence,
    pub input: InputEvidence,
    pub processing: ProcessingEvidence,
    pub summary: StatsSnapshot,
    pub boundaries: BoundaryEvidence,
    pub key_samples: KeySamples,
    pub coverage: InputCoverage,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EvidencePaths {
    pub directory: PathBuf,
    pub evidence: PathBuf,
    pub boundaries: PathBuf,
    pub trace: PathBuf,
    pub waveform: PathBuf,
    pub manifest: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
pub struct CaptureEvidence {
    pub schema_version: String,
    pub created_at_unix_ms: u128,
    pub toolchain: ToolchainEvidence,
    pub report: CaptureReport,
    pub parameters: crate::model::ProcessingParams,
    pub summary: StatsSnapshot,
    pub boundaries: Vec<BoundaryEvent>,
    pub waveform_points: Vec<crate::model::WaveformPoint>,
    pub first_sample: Option<EncodeSample>,
    pub last_sample: Option<EncodeSample>,
    pub ingest_errors: u64,
}

#[derive(Debug, Serialize)]
struct Manifest {
    schema_version: &'static str,
    files: Vec<ManifestFile>,
}

#[derive(Debug, Serialize)]
struct ManifestFile {
    path: String,
    bytes: u64,
    sha256: String,
}

impl EvidenceBundle {
    pub fn from_scenario(run: &ScenarioRun) -> Self {
        let points = run.snapshot.waveform.clone();
        let samples = if run.scenario == ScenarioKind::Boundary {
            run.retained_inputs.clone()
        } else {
            Vec::new()
        };
        let notes = vec![
            "waveform points are normalized against reference/threshold scales".to_string(),
            "boundary events retain the full sample immediately before and after each level change"
                .to_string(),
            "the processing window is bounded; huge input is summarized instead of retained in full"
                .to_string(),
        ];

        Self {
            schema_version: SCHEMA_VERSION.to_string(),
            scenario: run.scenario,
            created_at_unix_ms: unix_ms(),
            toolchain: toolchain_evidence(),
            input: InputEvidence {
                source: format!("synthetic:{}", run.scenario),
                scenario: run.scenario,
                generated_samples: run.generated_samples,
                requested_huge_samples: run.requested_huge_samples,
                rejected_samples: run.rejected_samples,
                retained_sample_count: samples.len(),
                first: run.first_input.clone(),
                last: run.last_input.clone(),
                samples,
            },
            processing: ProcessingEvidence {
                params: run.parameters.clone(),
                parameter_changes: run.parameter_changes.clone(),
                waveform_points: points.clone(),
            },
            summary: run.snapshot.clone(),
            boundaries: BoundaryEvidence {
                events_total: run.snapshot.boundary_events_total,
                events_dropped: run.snapshot.boundary_events_dropped,
                events: run.boundaries.clone(),
            },
            key_samples: KeySamples {
                first_processed: run.processed_points.first().cloned(),
                last_processed: run.processed_points.last().cloned(),
                first_boundary: run.boundaries.first().cloned(),
                last_boundary: run.boundaries.last().cloned(),
            },
            coverage: InputCoverage {
                empty_input_covered: run.scenario == ScenarioKind::Empty,
                huge_input_covered: run.scenario == ScenarioKind::Huge,
                boundary_before_after_captured: run
                    .boundaries
                    .iter()
                    .any(|event| event.before.is_some() && event.changes.is_some()),
                bounded_waveform: run.snapshot.waveform.len() <= run.parameters.window_capacity,
                duration_us: run.elapsed.as_micros(),
            },
            notes,
        }
    }
}

pub fn write_evidence(output_dir: &Path, bundle: &EvidenceBundle) -> Result<EvidencePaths> {
    let directory = output_dir.join(bundle.scenario.to_string());
    fs::create_dir_all(&directory)
        .with_context(|| format!("create evidence directory {}", directory.display()))?;

    let evidence_path = directory.join("evidence.json");
    let boundaries_path = directory.join("boundaries.json");
    let trace_path = directory.join("trace.jsonl");
    let waveform_path = directory.join("waveform.svg");
    let manifest_path = directory.join("manifest.json");

    write_json(&evidence_path, bundle)?;
    write_json(&boundaries_path, &bundle.boundaries)?;
    write_trace(&trace_path, &bundle.processing.waveform_points)?;
    waveform::write_svg(
        &waveform_path,
        &bundle.processing.waveform_points,
        &bundle.processing.params,
        &format!("encoding statistics / {}", bundle.scenario),
    )?;

    let files = [
        &evidence_path,
        &boundaries_path,
        &trace_path,
        &waveform_path,
    ];
    write_manifest(&manifest_path, &files)?;

    Ok(EvidencePaths {
        directory,
        evidence: evidence_path,
        boundaries: boundaries_path,
        trace: trace_path,
        waveform: waveform_path,
        manifest: manifest_path,
    })
}

pub fn write_capture_evidence(
    output_dir: &Path,
    evidence: &CaptureEvidence,
) -> Result<EvidencePaths> {
    let directory = output_dir.join("gstreamer");
    fs::create_dir_all(&directory)
        .with_context(|| format!("create evidence directory {}", directory.display()))?;
    let evidence_path = directory.join("evidence.json");
    let boundaries_path = directory.join("boundaries.json");
    let trace_path = directory.join("trace.jsonl");
    let waveform_path = directory.join("waveform.svg");
    let manifest_path = directory.join("manifest.json");

    write_json(&evidence_path, evidence)?;
    write_json(&boundaries_path, &evidence.boundaries)?;
    write_trace(&trace_path, &evidence.waveform_points)?;
    waveform::write_svg(
        &waveform_path,
        &evidence.waveform_points,
        &evidence.parameters,
        "encoding statistics / live GStreamer",
    )?;
    write_manifest(
        &manifest_path,
        &[
            &evidence_path,
            &boundaries_path,
            &trace_path,
            &waveform_path,
        ],
    )?;

    Ok(EvidencePaths {
        directory,
        evidence: evidence_path,
        boundaries: boundaries_path,
        trace: trace_path,
        waveform: waveform_path,
        manifest: manifest_path,
    })
}

fn write_manifest(manifest_path: &Path, files: &[&PathBuf]) -> Result<()> {
    let mut manifest_files = Vec::with_capacity(files.len());
    for path in files {
        let bytes =
            fs::read(path).with_context(|| format!("read evidence file {}", path.display()))?;
        manifest_files.push(ManifestFile {
            path: path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("unknown")
                .to_string(),
            bytes: bytes.len() as u64,
            sha256: format!("{:x}", Sha256::digest(&bytes)),
        });
    }
    write_json(
        manifest_path,
        &Manifest {
            schema_version: SCHEMA_VERSION,
            files: manifest_files,
        },
    )
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let file = fs::File::create(path)
        .with_context(|| format!("create JSON evidence {}", path.display()))?;
    serde_json::to_writer_pretty(file, value)
        .with_context(|| format!("serialize JSON evidence {}", path.display()))?;
    Ok(())
}

fn write_trace(path: &Path, points: &[crate::model::WaveformPoint]) -> Result<()> {
    let mut file =
        fs::File::create(path).with_context(|| format!("create trace {}", path.display()))?;
    for point in points {
        serde_json::to_writer(&mut file, point)
            .with_context(|| format!("write trace point {}", point.sequence))?;
        file.write_all(b"\n")?;
    }
    Ok(())
}

fn unix_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

pub fn collect_artifacts(output_dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    if !output_dir.exists() {
        return Ok(paths);
    }
    for entry in fs::read_dir(output_dir)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            paths.push(entry.path());
        } else if entry.file_type()?.is_dir() {
            for child in fs::read_dir(entry.path())? {
                let child = child?;
                if child.file_type()?.is_file() {
                    paths.push(child.path());
                }
            }
        }
    }
    paths.sort();
    Ok(paths)
}
