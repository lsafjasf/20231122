#![cfg_attr(not(feature = "media"), allow(dead_code, unused_imports))]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde::de::DeserializeOwned;

use crate::artifact::{
    artifact_entry, load_bundle, pretty_json_bytes, render_sha256sums, verify_bundle,
};
use crate::clock;
use crate::limits::{LimitKind, Limits, check_limit};
use crate::model::{
    ApplicationInfo, AppliedInterval, ArtifactEntry, ArtifactManifest, FileEvidence, InputEvidence,
    Integrity, MediaInfo, RepairInput, RepairReport, RunStatus, SCHEMA_VERSION, VersionManifest,
    VersionResult,
};
use crate::review;
use crate::util::{iso8601_utc, rounded, sha256_file, stable_run_id, unix_millis};
use crate::waveform;

pub const OUTPUT_SAMPLE_RATE: u32 = 48_000;
pub const DEFAULT_FRAME_MS: f64 = 20.0;

#[derive(Debug, Clone)]
pub struct RunOptions {
    pub recording: PathBuf,
    pub intervals: PathBuf,
    pub versions: PathBuf,
    pub output_dir: PathBuf,
    pub limits: Limits,
    pub generated_at_ms: Option<u128>,
}

impl RunOptions {
    pub fn new(
        recording: impl Into<PathBuf>,
        intervals: impl Into<PathBuf>,
        versions: impl Into<PathBuf>,
        output_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            recording: recording.into(),
            intervals: intervals.into(),
            versions: versions.into(),
            output_dir: output_dir.into(),
            limits: Limits::default(),
            generated_at_ms: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RunOutcome {
    pub report: RepairReport,
    pub report_path: PathBuf,
    pub warnings: Vec<String>,
}

impl RunOutcome {
    pub fn succeeded(&self) -> bool {
        matches!(
            self.report.status,
            RunStatus::Succeeded | RunStatus::SucceededWithWarnings
        )
    }
}

#[derive(Debug)]
struct Probe {
    evidence: FileEvidence,
    fatal: Option<String>,
}

struct EvidenceLedger {
    entries: Vec<crate::model::EvidenceEntry>,
    max: usize,
    dropped: usize,
}

impl EvidenceLedger {
    fn new(max: usize) -> Self {
        Self {
            entries: Vec::new(),
            max: max.max(1),
            dropped: 0,
        }
    }

    fn push(&mut self, kind: &str, severity: &str, subject: &str, message: impl Into<String>) {
        if self.entries.len() >= self.max {
            self.dropped += 1;
            return;
        }
        self.entries.push(crate::model::EvidenceEntry {
            sequence: self.entries.len() + 1,
            kind: kind.to_string(),
            severity: severity.to_string(),
            subject: subject.to_string(),
            message: message.into(),
        });
    }

    fn info(&mut self, kind: &str, subject: &str, message: impl Into<String>) {
        self.push(kind, "info", subject, message);
    }

    fn warning(&mut self, kind: &str, subject: &str, message: impl Into<String>) {
        self.push(kind, "warning", subject, message);
    }

    fn error(&mut self, kind: &str, subject: &str, message: impl Into<String>) {
        self.push(kind, "error", subject, message);
    }

    fn finish(&mut self) -> Vec<crate::model::EvidenceEntry> {
        if self.dropped > 0 {
            if self.entries.len() == self.max {
                self.entries.pop();
            }
            let sequence = self.entries.len() + 1;
            self.entries.push(crate::model::EvidenceEntry {
                sequence,
                kind: "evidence_truncated".to_string(),
                severity: "warning".to_string(),
                subject: "ledger".to_string(),
                message: format!(
                    "EVIDENCE_TRUNCATED dropped={} retained={}",
                    self.dropped,
                    self.entries.len()
                ),
            });
        }
        self.entries.clone()
    }
}

fn absolute_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    }
}

fn probe_file(
    path: &Path,
    byte_limit: u64,
    kind: LimitKind,
    label: &str,
    ledger: &mut EvidenceLedger,
) -> Probe {
    let absolute = absolute_path(path);
    let display = absolute.display().to_string();
    let metadata = match std::fs::metadata(&absolute) {
        Ok(metadata) => metadata,
        Err(error) => {
            let message = format!("INPUT_MISSING {label}: {error}");
            ledger.error("input_probe", label, message.clone());
            return Probe {
                evidence: FileEvidence {
                    path: display,
                    bytes: 0,
                    sha256: "unavailable:missing".to_string(),
                },
                fatal: Some(message),
            };
        }
    };
    if !metadata.is_file() {
        let message = format!("INPUT_NOT_FILE {label}: {}", absolute.display());
        ledger.error("input_probe", label, message.clone());
        return Probe {
            evidence: FileEvidence {
                path: display,
                bytes: metadata.len(),
                sha256: "unavailable:not_a_file".to_string(),
            },
            fatal: Some(message),
        };
    }
    if let Err(error) = check_limit(kind, metadata.len(), byte_limit) {
        let message = format!("INPUT_LIMIT_EXCEEDED {label}: {error}");
        ledger.error("input_limit", label, message.clone());
        return Probe {
            evidence: FileEvidence {
                path: display,
                bytes: metadata.len(),
                sha256: "unavailable:size_limit_exceeded".to_string(),
            },
            fatal: Some(message),
        };
    }
    match sha256_file(&absolute) {
        Ok((bytes, sha256)) => {
            ledger.info(
                "input_probe",
                label,
                format!(
                    "INPUT_OK path={} bytes={bytes} sha256={sha256}",
                    absolute.display()
                ),
            );
            Probe {
                evidence: FileEvidence {
                    path: display,
                    bytes,
                    sha256,
                },
                fatal: None,
            }
        }
        Err(error) => {
            let message = format!("INPUT_HASH_FAILED {label}: {error:#}");
            ledger.error("input_probe", label, message.clone());
            Probe {
                evidence: FileEvidence {
                    path: display,
                    bytes: metadata.len(),
                    sha256: "unavailable:hash_failed".to_string(),
                },
                fatal: Some(message),
            }
        }
    }
}

fn read_limited_json<T: DeserializeOwned>(
    evidence: &FileEvidence,
    limit: u64,
    label: &str,
    ledger: &mut EvidenceLedger,
) -> Result<T> {
    let path = Path::new(&evidence.path);
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("cannot stat {label} at {}", path.display()))?;
    check_limit(LimitKind::JsonBytes, metadata.len(), limit)
        .map_err(|error| anyhow!("{label}: {error}"))?;
    let bytes = std::fs::read(path)
        .with_context(|| format!("cannot read {label} at {}", path.display()))?;
    check_limit(LimitKind::JsonBytes, bytes.len() as u64, limit)
        .map_err(|error| anyhow!("{label}: {error}"))?;
    if bytes.is_empty() {
        ledger.error(
            "json_parse",
            label,
            format!("EMPTY_JSON path={}", path.display()),
        );
        bail!("{label} is empty");
    }
    let value = serde_json::from_slice(&bytes)
        .with_context(|| format!("invalid JSON in {label} at {}", path.display()))?;
    ledger.info(
        "json_parse",
        label,
        format!("JSON_OK bytes={} path={}", bytes.len(), path.display()),
    );
    Ok(value)
}

fn app_info() -> ApplicationInfo {
    ApplicationInfo {
        name: env!("CARGO_PKG_NAME").to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        rust: env!("AUDIO_REPAIR_RUSTC").to_string(),
        gstreamer: media_version(),
        gtk4: gtk4_version(),
        schema_version: SCHEMA_VERSION,
    }
}

#[cfg(feature = "media")]
fn media_version() -> String {
    crate::media::gstreamer_version()
}

#[cfg(not(feature = "media"))]
fn media_version() -> String {
    "unavailable (media feature disabled)".to_string()
}

#[cfg(feature = "gui")]
fn gtk4_version() -> String {
    format!(
        "{}.{}.{}",
        gtk4::major_version(),
        gtk4::minor_version(),
        gtk4::micro_version()
    )
}

#[cfg(not(feature = "gui"))]
fn gtk4_version() -> String {
    "unavailable (gui feature disabled)".to_string()
}

fn empty_media() -> MediaInfo {
    MediaInfo {
        requested_sample_rate: OUTPUT_SAMPLE_RATE,
        decoded_sample_rate: 0,
        channels: 0,
        format: "unknown".to_string(),
        frame_ms: DEFAULT_FRAME_MS,
        total_frames: 0,
        duration_ms: 0.0,
        gstreamer_pipeline: media_pipeline_name(),
    }
}

#[cfg(feature = "media")]
fn media_pipeline_name() -> String {
    crate::media::DECODE_PIPELINE.to_string()
}

#[cfg(not(feature = "media"))]
fn media_pipeline_name() -> String {
    "unavailable (media feature disabled)".to_string()
}

fn base_report(
    options: &RunOptions,
    files: [FileEvidence; 3],
    status: RunStatus,
    ledger: &EvidenceLedger,
) -> RepairReport {
    let run_id = stable_run_id(&[
        &files[0].sha256,
        &files[1].sha256,
        &files[2].sha256,
        &serde_json::to_string(&options.limits).unwrap_or_default(),
    ]);
    let created_ms = options.generated_at_ms.unwrap_or_else(unix_millis);
    let report_path = absolute_path(&options.output_dir).join("report.json");
    RepairReport {
        schema_version: SCHEMA_VERSION,
        application: app_info(),
        run_id,
        created_at: iso8601_utc(created_ms),
        status,
        inputs: InputEvidence {
            recording_path: files[0].path.clone(),
            intervals_path: files[1].path.clone(),
            versions_path: files[2].path.clone(),
            recording: files[0].clone(),
            intervals: files[1].clone(),
            versions: files[2].clone(),
            limits: options.limits.snapshot(),
        },
        media: empty_media(),
        clocks: Vec::new(),
        intervals: Vec::new(),
        versions: Vec::new(),
        evidence: ledger.entries.clone(),
        integrity: Integrity {
            report_path: report_path.display().to_string(),
            artifact_manifest_path: "artifact_manifest.json".to_string(),
            artifacts: Vec::new(),
        },
    }
}

fn validate_versions(
    manifest: &VersionManifest,
    intervals: &RepairInput,
    limits: &Limits,
    ledger: &mut EvidenceLedger,
) -> Result<()> {
    let mut errors = Vec::new();
    if manifest.schema_version != SCHEMA_VERSION {
        errors.push(format!(
            "versions schema_version={} expected={}",
            manifest.schema_version, SCHEMA_VERSION
        ));
    }
    if manifest.versions.is_empty() {
        errors.push("versions list must not be empty".to_string());
    }
    if let Err(error) = check_limit(
        LimitKind::Versions,
        manifest.versions.len() as u64,
        limits.max_versions as u64,
    ) {
        errors.push(error);
    }
    if intervals.intervals.is_empty() {
        ledger.warning(
            "input_empty",
            "intervals",
            "EMPTY_REPAIR_SET no intervals; pass-through versions will be produced",
        );
    }
    let known_intervals = intervals
        .intervals
        .iter()
        .map(|interval| interval.id.as_str())
        .collect::<BTreeSet<_>>();
    let mut version_ids = BTreeSet::new();
    for version in &manifest.versions {
        if version.id.trim().is_empty() {
            errors.push("version id must not be empty".to_string());
        }
        if !version_ids.insert(version.id.clone()) {
            errors.push(format!("duplicate version id={}", version.id));
        }
        if let Err(error) = check_limit(
            LimitKind::IntervalRefs,
            version.interval_ids.len() as u64,
            limits.max_interval_refs_per_version as u64,
        ) {
            errors.push(format!("version {}: {error}", version.id));
        }
        let mut refs = BTreeSet::new();
        for interval_id in &version.interval_ids {
            if !known_intervals.contains(interval_id.as_str()) {
                errors.push(format!(
                    "version {} references unknown interval {}",
                    version.id, interval_id
                ));
            }
            if !refs.insert(interval_id) {
                errors.push(format!(
                    "version {} references interval {} more than once",
                    version.id, interval_id
                ));
            }
        }
        ledger.info(
            "version_manifest",
            &version.id,
            format!(
                "VERSION_DECLARED label={} interval_ids={}",
                version.label,
                version.interval_ids.join(",")
            ),
        );
    }
    if errors.is_empty() {
        Ok(())
    } else {
        for error in &errors {
            ledger.error("version_validation", "versions", error.clone());
        }
        bail!("version validation failed: {}", errors.join("; "))
    }
}

fn preflight_intervals(
    intervals: &RepairInput,
    limits: &Limits,
    ledger: &mut EvidenceLedger,
) -> Result<()> {
    let mut errors = Vec::new();
    if intervals.schema_version != SCHEMA_VERSION {
        errors.push(format!(
            "intervals schema_version={} expected={}",
            intervals.schema_version, SCHEMA_VERSION
        ));
    }
    if let Err(error) = check_limit(
        LimitKind::Intervals,
        intervals.intervals.len() as u64,
        limits.max_intervals as u64,
    ) {
        errors.push(error);
    }
    let mut ids = BTreeSet::new();
    for interval in &intervals.intervals {
        if !ids.insert(interval.id.as_str()) {
            errors.push(format!("duplicate interval id={}", interval.id));
        }
    }
    if errors.is_empty() {
        ledger.info(
            "interval_preflight",
            "intervals",
            format!(
                "INTERVAL_PREFLIGHT_OK count={} devices={} tracks={}",
                intervals.intervals.len(),
                intervals.devices.len(),
                intervals.tracks.len()
            ),
        );
        Ok(())
    } else {
        for error in &errors {
            ledger.error("interval_preflight", "intervals", error.clone());
        }
        bail!("interval preflight failed: {}", errors.join("; "))
    }
}

fn finalize(options: &RunOptions, report: &mut RepairReport) -> Result<PathBuf> {
    let output_dir = absolute_path(&options.output_dir);
    let report_path = output_dir.join("report.json");
    report.integrity.report_path = report_path.display().to_string();
    report.integrity.artifact_manifest_path = "artifact_manifest.json".to_string();

    std::fs::write(&report_path, pretty_json_bytes(report)?)
        .with_context(|| format!("cannot write {}", report_path.display()))?;
    let review_md = review::markdown(report);
    let review_csv = review::csv(report);
    std::fs::write(output_dir.join("review.md"), review_md).context("cannot write review.md")?;
    std::fs::write(output_dir.join("review.csv"), review_csv).context("cannot write review.csv")?;

    let mut entries = Vec::<ArtifactEntry>::new();
    entries.push(artifact_entry(&output_dir, "report", "report.json")?);
    entries.push(artifact_entry(&output_dir, "review_markdown", "review.md")?);
    entries.push(artifact_entry(&output_dir, "review_csv", "review.csv")?);
    entries.extend(report.integrity.artifacts.clone());
    let manifest = ArtifactManifest {
        schema_version: SCHEMA_VERSION,
        run_id: report.run_id.clone(),
        entries,
    };
    std::fs::write(
        output_dir.join("artifact_manifest.json"),
        pretty_json_bytes(&manifest)?,
    )
    .context("cannot write artifact_manifest.json")?;
    std::fs::write(
        output_dir.join("SHA256SUMS"),
        render_sha256sums(&manifest.entries),
    )
    .context("cannot write SHA256SUMS")?;
    Ok(report_path)
}

fn mark_rejected(
    options: &RunOptions,
    report: &mut RepairReport,
    ledger: &mut EvidenceLedger,
    message: &str,
) -> Result<RunOutcome> {
    report.status = RunStatus::Rejected;
    ledger.error("run", "pipeline", message.to_string());
    report.evidence = ledger.finish();
    let report_path = finalize(options, report)?;
    Ok(RunOutcome {
        report: report.clone(),
        report_path,
        warnings: report
            .evidence
            .iter()
            .filter(|entry| entry.severity == "warning")
            .map(|entry| entry.message.clone())
            .collect(),
    })
}

#[cfg(feature = "media")]
fn process_versions(
    options: &RunOptions,
    report: &RepairReport,
    decoded: &crate::media::DecodedAudio,
    manifest: &VersionManifest,
    interval_index: &BTreeMap<String, usize>,
    ledger: &mut EvidenceLedger,
) -> Result<(Vec<VersionResult>, Vec<ArtifactEntry>)> {
    let output_dir = absolute_path(&options.output_dir);
    std::fs::create_dir_all(output_dir.join("media"))?;
    std::fs::create_dir_all(output_dir.join("waveforms"))?;
    let mut results = Vec::with_capacity(manifest.versions.len());
    let mut artifacts = Vec::with_capacity(manifest.versions.len() * 2);

    for (version_index, version) in manifest.versions.iter().enumerate() {
        let mut samples = decoded.samples.clone();
        let mut applied_intervals = Vec::new();
        let mut warnings = Vec::new();
        for (manifest_order, interval_id) in version.interval_ids.iter().enumerate() {
            let index = interval_index.get(interval_id).ok_or_else(|| {
                anyhow!(
                    "version {} interval {} disappeared after validation",
                    version.id,
                    interval_id
                )
            })?;
            let interval = &report.intervals[*index];
            if !interval.applied {
                warnings.push(format!(
                    "INTERVAL_SKIPPED interval={} disposition={}",
                    interval.interval_id, interval.disposition
                ));
                continue;
            }
            let start = interval.applied_start_frame as usize;
            let end = interval.applied_end_frame as usize;
            let slice = samples
                .get_mut(start..end)
                .ok_or_else(|| anyhow!("interval frame range is outside decoded audio"))?;
            let processor_warnings =
                waveform::apply_processor(slice, decoded.sample_rate, &interval.processor);
            warnings.extend(
                processor_warnings
                    .into_iter()
                    .map(|warning| format!("{}: {warning}", interval.interval_id)),
            );
            applied_intervals.push(AppliedInterval {
                manifest_order,
                interval_id: interval.interval_id.clone(),
                start_frame: interval.applied_start_frame,
                end_frame: interval.applied_end_frame,
                processor: interval.processor.clone(),
            });
        }
        let summary = waveform::compute(
            &samples,
            decoded.sample_rate,
            options.limits.max_waveform_buckets,
        );
        let stem = format!(
            "{:04}-{}",
            version_index + 1,
            crate::util::safe_file_stem(&version.id)
        );
        let relative_output = format!("media/{stem}.wav");
        let relative_waveform = format!("waveforms/{stem}.csv");
        let output_path = output_dir.join(&relative_output);
        let waveform_path = output_dir.join(&relative_waveform);
        crate::media::encode_wav(&output_path, &samples, decoded.sample_rate)?;
        std::fs::write(
            &waveform_path,
            waveform::to_csv(&summary, decoded.sample_rate),
        )
        .with_context(|| format!("cannot write {}", waveform_path.display()))?;
        let (output_bytes, output_sha256) = sha256_file(&output_path)?;
        let (_, waveform_sha256) = sha256_file(&waveform_path)?;
        artifacts.push(artifact_entry(
            &output_dir,
            "audio_output",
            &relative_output,
        )?);
        artifacts.push(artifact_entry(
            &output_dir,
            "waveform_csv",
            &relative_waveform,
        )?);
        for warning in &warnings {
            ledger.warning("version_render", &version.id, warning.clone());
        }
        ledger.info(
            "version_render",
            &version.id,
            format!(
                "VERSION_RENDERED index={} output={} sha256={} intervals_applied={}",
                version_index,
                relative_output,
                output_sha256,
                applied_intervals.len()
            ),
        );
        results.push(VersionResult {
            manifest_index: version_index,
            version_id: version.id.clone(),
            label: version.label.clone(),
            note: version.note.clone(),
            interval_ids: version.interval_ids.clone(),
            output_path: relative_output,
            output_bytes,
            output_sha256,
            waveform_path: relative_waveform,
            waveform_sha256,
            waveform: summary,
            applied_intervals,
            warnings,
        });
    }
    Ok((results, artifacts))
}

#[cfg(feature = "media")]
pub fn run(options: RunOptions) -> Result<RunOutcome> {
    options
        .limits
        .validate()
        .map_err(|error| anyhow!("invalid limits: {error}"))?;
    let output_dir = absolute_path(&options.output_dir);
    std::fs::create_dir_all(&output_dir)
        .with_context(|| format!("cannot create output directory {}", output_dir.display()))?;
    if output_dir.join("report.json").exists() {
        bail!(
            "output directory already contains report.json; choose a fresh directory: {}",
            output_dir.display()
        );
    }
    let mut ledger = EvidenceLedger::new(options.limits.max_evidence_entries);
    let recording = probe_file(
        &options.recording,
        options.limits.max_audio_bytes,
        LimitKind::AudioBytes,
        "recording",
        &mut ledger,
    );
    let intervals_probe = probe_file(
        &options.intervals,
        options.limits.max_json_bytes,
        LimitKind::JsonBytes,
        "intervals",
        &mut ledger,
    );
    let versions_probe = probe_file(
        &options.versions,
        options.limits.max_json_bytes,
        LimitKind::JsonBytes,
        "versions",
        &mut ledger,
    );
    let fatal = [
        recording.fatal.clone(),
        intervals_probe.fatal.clone(),
        versions_probe.fatal.clone(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    let files = [
        recording.evidence.clone(),
        intervals_probe.evidence.clone(),
        versions_probe.evidence.clone(),
    ];
    let mut report = base_report(&options, files, RunStatus::Succeeded, &ledger);
    if !fatal.is_empty() {
        return mark_rejected(
            &options,
            &mut report,
            &mut ledger,
            &format!("input preflight failed: {}", fatal.join("; ")),
        );
    }

    let intervals: RepairInput = match read_limited_json(
        &report.inputs.intervals,
        options.limits.max_json_bytes,
        "intervals",
        &mut ledger,
    ) {
        Ok(value) => value,
        Err(error) => {
            return mark_rejected(
                &options,
                &mut report,
                &mut ledger,
                &format!("cannot load repair intervals: {error:#}"),
            );
        }
    };
    let versions: VersionManifest = match read_limited_json(
        &report.inputs.versions,
        options.limits.max_json_bytes,
        "versions",
        &mut ledger,
    ) {
        Ok(value) => value,
        Err(error) => {
            return mark_rejected(
                &options,
                &mut report,
                &mut ledger,
                &format!("cannot load version manifest: {error:#}"),
            );
        }
    };
    if let Err(error) = preflight_intervals(&intervals, &options.limits, &mut ledger) {
        return mark_rejected(&options, &mut report, &mut ledger, &format!("{error:#}"));
    }
    if let Err(error) = validate_versions(&versions, &intervals, &options.limits, &mut ledger) {
        return mark_rejected(&options, &mut report, &mut ledger, &format!("{error:#}"));
    }

    let decoded = match crate::media::decode(
        Path::new(&report.inputs.recording.path),
        OUTPUT_SAMPLE_RATE,
        options.limits.max_audio_seconds,
    ) {
        Ok(decoded) => decoded,
        Err(error) => {
            let message = format!("{error:#}");
            let limit_exceeded = message.contains("max_audio_seconds exceeded");
            report.status = if limit_exceeded {
                RunStatus::Rejected
            } else {
                RunStatus::Failed
            };
            ledger.error("media_decode", "recording", message.clone());
            report.evidence = ledger.finish();
            let report_path = finalize(&options, &mut report)?;
            return Ok(RunOutcome {
                report,
                report_path,
                warnings: vec![message],
            });
        }
    };
    let total_frames = decoded.samples.len() as u64;
    let duration_ms = crate::util::frame_to_ms(total_frames, decoded.sample_rate);
    report.media = MediaInfo {
        requested_sample_rate: OUTPUT_SAMPLE_RATE,
        decoded_sample_rate: decoded.sample_rate,
        channels: decoded.channels,
        format: decoded.format.clone(),
        frame_ms: DEFAULT_FRAME_MS,
        total_frames,
        duration_ms: rounded(duration_ms, 6),
        gstreamer_pipeline: decoded.pipeline.clone(),
    };
    ledger.info(
        "media_decode",
        "recording",
        format!(
            "DECODE_OK sample_rate={} channels={} frames={} duration_ms={:.3}",
            decoded.sample_rate, decoded.channels, total_frames, duration_ms
        ),
    );
    let projected_output_seconds = (duration_ms / 1000.0) * versions.versions.len() as f64;
    if projected_output_seconds > options.limits.max_total_output_seconds {
        let message = format!(
            "OUTPUT_BUDGET_EXCEEDED projected_seconds={:.3} allowed_seconds={:.3} versions={} duration_seconds={:.3}",
            projected_output_seconds,
            options.limits.max_total_output_seconds,
            versions.versions.len(),
            duration_ms / 1000.0
        );
        ledger.error("output_budget", "versions", message.clone());
        return mark_rejected(&options, &mut report, &mut ledger, &message);
    }
    let analysis = clock::analyze(
        &intervals,
        decoded.sample_rate,
        total_frames,
        duration_ms,
        &options.limits,
    );
    report.clocks = analysis.clocks;
    report.intervals = analysis.intervals;
    for warning in &analysis.warnings {
        ledger.warning("clock_mapping", "timeline", warning.clone());
    }
    for clock in &report.clocks {
        if let Some(warning) = &clock.warning {
            ledger.warning("clock_mapping", &clock.device_id, warning.clone());
        }
    }
    for interval in &report.intervals {
        for warning in &interval.warnings {
            if !analysis.warnings.contains(warning) {
                ledger.warning("interval_mapping", &interval.interval_id, warning.clone());
            }
        }
    }
    if !analysis.errors.is_empty() {
        for error in &analysis.errors {
            ledger.error("clock_mapping", "timeline", error.clone());
        }
        return mark_rejected(
            &options,
            &mut report,
            &mut ledger,
            &format!("timeline validation failed: {}", analysis.errors.join("; ")),
        );
    }

    let interval_index = report
        .intervals
        .iter()
        .enumerate()
        .map(|(index, interval)| (interval.interval_id.clone(), index))
        .collect::<BTreeMap<_, _>>();
    match process_versions(
        &options,
        &report,
        &decoded,
        &versions,
        &interval_index,
        &mut ledger,
    ) {
        Ok((versions, artifacts)) => {
            report.versions = versions;
            report.integrity.artifacts = artifacts;
        }
        Err(error) => {
            report.status = RunStatus::Failed;
            ledger.error("version_render", "pipeline", format!("{error:#}"));
            report.evidence = ledger.finish();
            let report_path = finalize(&options, &mut report)?;
            return Ok(RunOutcome {
                report,
                report_path,
                warnings: vec![format!("{error:#}")],
            });
        }
    }
    let has_warnings = ledger
        .entries
        .iter()
        .any(|entry| entry.severity == "warning")
        || report
            .versions
            .iter()
            .any(|version| !version.warnings.is_empty());
    report.status = if has_warnings {
        RunStatus::SucceededWithWarnings
    } else {
        RunStatus::Succeeded
    };
    ledger.info(
        "artifact_manifest",
        "output",
        format!(
            "ARTIFACT_PLAN versions={} artifacts={} output_dir={}",
            report.versions.len(),
            report.integrity.artifacts.len(),
            output_dir.display()
        ),
    );
    report.evidence = ledger.finish();
    let warnings = report
        .evidence
        .iter()
        .filter(|entry| entry.severity == "warning")
        .map(|entry| entry.message.clone())
        .collect::<Vec<_>>();
    let report_path = finalize(&options, &mut report)?;
    let bundle = load_bundle(&output_dir)?;
    let _ = verify_bundle(&output_dir)
        .with_context(|| format!("internal verification failed for {}", bundle.root.display()))?;
    Ok(RunOutcome {
        report,
        report_path,
        warnings,
    })
}

#[cfg(not(feature = "media"))]
pub fn run(_options: RunOptions) -> Result<RunOutcome> {
    bail!("audio processing requires the `media` feature (GStreamer)")
}

#[cfg(all(test, feature = "media"))]
mod tests {
    use super::*;
    use crate::artifact::roundtrip_report;
    use crate::demo;
    use std::path::Path;

    fn artifact_dir(root: &Path, name: &str) -> PathBuf {
        root.join(name)
    }

    #[test]
    fn clock_skew_demo_is_reproducible_reloadable_and_verifiable() {
        let temp = tempfile::tempdir().unwrap();
        let inputs = demo::write_demo_inputs(&temp.path().join("inputs")).unwrap();
        let out = artifact_dir(temp.path(), "out");
        let options = RunOptions::new(inputs.recording, inputs.intervals, inputs.versions, &out);
        let outcome = run(options).unwrap();
        assert!(outcome.succeeded());
        assert_eq!(outcome.report.status, RunStatus::SucceededWithWarnings);
        assert_eq!(outcome.report.versions.len(), 3);
        assert_eq!(
            outcome
                .report
                .versions
                .iter()
                .map(|version| version.version_id.as_str())
                .collect::<Vec<_>>(),
            ["v1-reference", "v2-draft", "v3-final"]
        );
        assert!(outcome.report.clocks[1].max_observed_skew_ms > 7_000.0);
        assert!(outcome.report.intervals.iter().any(|interval| {
            interval
                .warnings
                .iter()
                .any(|warning| warning.contains("OVERLAPPING_REPAIR"))
        }));
        let bundle = load_bundle(&out).unwrap();
        assert_eq!(bundle.report, outcome.report);
        assert_eq!(roundtrip_report(&bundle).unwrap(), bundle.report_bytes);
        assert_eq!(bundle.sha256sums.len(), bundle.manifest.entries.len());
        let verification = verify_bundle(&out).unwrap();
        assert_eq!(
            verification.artifacts_verified,
            bundle.manifest.entries.len()
        );
        assert_eq!(verification.inputs_verified, 3);
        let csv = std::fs::read_to_string(out.join("review.csv")).unwrap();
        assert_eq!(csv.lines().count(), 6);
        assert!(csv.contains("source_start_frame_delta"));
        let review = std::fs::read_to_string(out.join("review.md")).unwrap();
        assert!(review.contains("audio-repair-cli verify '"));
        assert!(!review.contains("verify '/tmp/demo-output3/report.json'"));
        assert_eq!(
            outcome
                .report
                .evidence
                .iter()
                .filter(|entry| entry.message.contains("CLOCK_NOT_SYNCHRONIZED"))
                .count(),
            1
        );
    }

    #[test]
    fn empty_repair_set_produces_pass_through_version() {
        let temp = tempfile::tempdir().unwrap();
        let inputs = demo::write_empty_inputs(&temp.path().join("inputs")).unwrap();
        let out = artifact_dir(temp.path(), "out");
        let outcome = run(RunOptions::new(
            inputs.recording,
            inputs.intervals,
            inputs.versions,
            &out,
        ))
        .unwrap();
        assert!(outcome.succeeded());
        assert_eq!(outcome.report.versions.len(), 1);
        assert!(outcome.report.versions[0].applied_intervals.is_empty());
        assert!(
            outcome
                .report
                .evidence
                .iter()
                .any(|entry| entry.message.contains("EMPTY_REPAIR_SET"))
        );
        verify_bundle(&out).unwrap();
    }

    #[test]
    fn oversized_interval_set_is_rejected_before_media_decode() {
        let temp = tempfile::tempdir().unwrap();
        let limits = Limits::default();
        let inputs =
            demo::write_oversized_inputs(&temp.path().join("inputs"), limits.max_intervals)
                .unwrap();
        let out = artifact_dir(temp.path(), "out");
        let outcome = run(RunOptions::new(
            inputs.recording,
            inputs.intervals,
            inputs.versions,
            &out,
        ))
        .unwrap();
        assert_eq!(outcome.report.status, RunStatus::Rejected);
        assert!(outcome.report.versions.is_empty());
        assert!(outcome.report.evidence.iter().any(|entry| {
            entry.message.contains("max_intervals")
                && entry.message.contains(&limits.max_intervals.to_string())
        }));
        assert!(!out.join("media").exists());
        verify_bundle(&out).unwrap();
    }

    #[test]
    fn empty_version_manifest_is_rejected_with_report() {
        let temp = tempfile::tempdir().unwrap();
        let inputs = demo::write_demo_inputs(&temp.path().join("inputs")).unwrap();
        let empty_versions = temp.path().join("empty-versions.json");
        std::fs::write(
            &empty_versions,
            serde_json::to_vec_pretty(&VersionManifest {
                schema_version: SCHEMA_VERSION,
                versions: Vec::new(),
            })
            .unwrap(),
        )
        .unwrap();
        let out = artifact_dir(temp.path(), "out");
        let outcome = run(RunOptions::new(
            inputs.recording,
            inputs.intervals,
            empty_versions,
            &out,
        ))
        .unwrap();
        assert_eq!(outcome.report.status, RunStatus::Rejected);
        assert!(
            outcome
                .report
                .evidence
                .iter()
                .any(|entry| entry.message.contains("must not be empty"))
        );
        verify_bundle(&out).unwrap();
    }

    #[test]
    fn oversized_json_is_rejected_and_remains_reviewable() {
        let temp = tempfile::tempdir().unwrap();
        let inputs = demo::write_demo_inputs(&temp.path().join("inputs")).unwrap();
        let oversized = temp.path().join("oversized-intervals.json");
        let limits = Limits::default();
        let bytes = vec![b' '; limits.max_json_bytes as usize + 1];
        std::fs::write(&oversized, bytes).unwrap();
        let out = artifact_dir(temp.path(), "out");
        let outcome = run(RunOptions::new(
            inputs.recording,
            oversized,
            inputs.versions,
            &out,
        ))
        .unwrap();
        assert_eq!(outcome.report.status, RunStatus::Rejected);
        assert_eq!(
            outcome.report.inputs.intervals.sha256,
            "unavailable:size_limit_exceeded"
        );
        let verification = verify_bundle(&out).unwrap();
        assert_eq!(verification.inputs_missing, 1);
        assert!(
            verification
                .warnings
                .iter()
                .any(|warning| warning.contains("hash unavailable"))
        );
    }

    #[test]
    fn projected_output_budget_is_rejected_before_rendering() {
        let temp = tempfile::tempdir().unwrap();
        let inputs = demo::write_demo_inputs(&temp.path().join("inputs")).unwrap();
        let out = artifact_dir(temp.path(), "out");
        let mut options =
            RunOptions::new(inputs.recording, inputs.intervals, inputs.versions, &out);
        options.limits.max_total_output_seconds = 1.0;
        let outcome = run(options).unwrap();
        assert_eq!(outcome.report.status, RunStatus::Rejected);
        assert!(
            outcome
                .report
                .evidence
                .iter()
                .any(|entry| entry.message.contains("OUTPUT_BUDGET_EXCEEDED"))
        );
        assert!(!out.join("media").exists());
        verify_bundle(&out).unwrap();
    }
}
