use crate::history::{ApplyResult, History, StepStatus, state_sha256};
use crate::model::{
    ClipState, EXPERIMENT_FORMAT, ExperimentObject, ExperimentSpec, Snapshot, StepSpec,
};
use crate::render::{self, RenderInfo};
use anyhow::{Context, Result, bail};
use gstreamer as gst;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const EVIDENCE_MARKER: &str = ".audio-undo-evidence-v1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryView {
    pub revision: u64,
    pub state_sha256: String,
    pub undo_len: usize,
    pub redo_len: usize,
    pub clip: ClipState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepEvent {
    pub event: String,
    pub experiment_id: String,
    pub object_id: String,
    pub step_index: usize,
    pub input: StepSpec,
    pub state_before: HistoryView,
    pub result: ApplyResult,
    pub state_after: HistoryView,
    pub media: RenderInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectEvent {
    pub event: String,
    pub experiment_id: String,
    pub object_id: String,
    pub passed: bool,
    pub expected_final_control: String,
    pub expected_final_sha256: String,
    pub actual_final_sha256: String,
    pub rejected_step_indices: Vec<usize>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunEvent {
    pub event: String,
    pub experiment_id: String,
    pub scenario_path: String,
    pub evidence_dir: String,
    pub extension: String,
    pub started_unix_ms: u128,
    pub rustc: String,
    pub gstreamer: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorEvent {
    pub event: String,
    pub experiment_id: String,
    pub object_id: String,
    pub error: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleanupReport {
    pub temporary_root: String,
    pub removed: bool,
    pub exists_after: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectReport {
    pub object_id: String,
    pub passed: bool,
    pub expected_final_control: String,
    pub expected_final_sha256: String,
    pub expected_final_media_sha256: String,
    pub actual_final_sha256: String,
    pub actual_final_media_sha256: String,
    pub steps: usize,
    pub rejected_step_indices: Vec<usize>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayStep {
    pub object_id: String,
    pub step_index: usize,
    pub request_id: String,
    pub expected_status: StepStatus,
    pub actual_status: StepStatus,
    pub expected_state_sha256: String,
    pub actual_state_sha256: String,
    pub matched: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayObjectReport {
    pub object_id: String,
    pub passed: bool,
    pub final_sha256_matched: bool,
    pub steps: Vec<ReplayStep>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayReport {
    pub ok: bool,
    pub source: String,
    pub objects: Vec<ReplayObjectReport>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExperimentReport {
    pub ok: bool,
    pub experiment_id: String,
    pub extension: String,
    pub snapshot_v1_sha256: String,
    pub snapshot_v2_sha256: String,
    pub snapshot_v1_media_sha256: String,
    pub snapshot_v2_media_sha256: String,
    pub objects: Vec<ObjectReport>,
    pub replay: ReplayReport,
    pub cleanup: CleanupReport,
    pub evidence_dir: String,
    pub events_path: String,
    pub summary_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceControls {
    pub experiment_id: String,
    pub snapshot_v1: Snapshot,
    pub snapshot_v2: Snapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotEvidence {
    pub snapshot: Snapshot,
    pub render: RenderInfo,
}

pub fn load_scenario(path: &Path) -> Result<ExperimentSpec> {
    let bytes = fs::read(path)
        .with_context(|| format!("failed to read experiment file {}", path.display()))?;
    let spec: ExperimentSpec = serde_json::from_slice(&bytes)
        .with_context(|| format!("failed to parse experiment file {}", path.display()))?;
    validate_spec(&spec)?;
    Ok(spec)
}

pub fn validate_spec(spec: &ExperimentSpec) -> Result<()> {
    if spec.format != EXPERIMENT_FORMAT {
        bail!(
            "unsupported experiment format '{}'; expected '{}'",
            spec.format,
            EXPERIMENT_FORMAT
        );
    }
    if spec.extension.value != crate::model::UNKNOWN_EXTENSION || spec.extension.registered {
        bail!("experiment must declare the intentionally unregistered .undoexp extension");
    }
    validate_snapshot(&spec.controls.snapshot_v1)?;
    validate_snapshot(&spec.controls.snapshot_v2)?;
    if spec.objects.is_empty() {
        bail!("experiment has no objects to process");
    }
    let mut object_ids = BTreeSet::new();
    for object in &spec.objects {
        if object.id.is_empty()
            || object.id.starts_with('.')
            || object.id.contains("..")
            || !object
                .id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        {
            bail!("object id '{}' is not a safe path component", object.id);
        }
        if !object_ids.insert(&object.id) {
            bail!("duplicate object id '{}'", object.id);
        }
        if object.steps.is_empty() {
            bail!("object '{}' has no steps", object.id);
        }
        if !matches!(object.expect.final_control.as_str(), "v1" | "v2") {
            bail!("object '{}' has unknown final_control", object.id);
        }
    }
    Ok(())
}

fn validate_snapshot(snapshot: &Snapshot) -> Result<()> {
    let actual = state_sha256(&snapshot.clip);
    if snapshot.state_sha256 != actual {
        bail!(
            "snapshot '{}' hash mismatch: declared {}, computed {}",
            snapshot.name,
            snapshot.state_sha256,
            actual
        );
    }
    Ok(())
}

fn history_view(history: &History) -> HistoryView {
    HistoryView {
        revision: history.revision(),
        state_sha256: history.state_hash(),
        undo_len: history.undo_len(),
        redo_len: history.redo_len(),
        clip: history.state().clone(),
    }
}

fn control_snapshot<'a>(spec: &'a ExperimentSpec, name: &str) -> Option<&'a Snapshot> {
    match name {
        "v1" => Some(&spec.controls.snapshot_v1),
        "v2" => Some(&spec.controls.snapshot_v2),
        _ => None,
    }
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(value)?;
    fs::write(path, bytes).with_context(|| format!("failed to write {}", path.display()))
}

fn event_line(value: &impl Serialize) -> Result<String> {
    Ok(serde_json::to_string(value)?)
}

pub fn run_experiment(scenario_path: &Path, evidence_dir: &Path) -> Result<ExperimentReport> {
    let spec = load_scenario(scenario_path)?;
    if evidence_dir.exists() {
        let has_marker = evidence_dir.join(EVIDENCE_MARKER).exists();
        let is_empty = fs::read_dir(evidence_dir)?.next().is_none();
        if !has_marker && !is_empty {
            bail!(
                "refusing to replace unmarked non-empty evidence directory {}",
                evidence_dir.display()
            );
        }
        fs::remove_dir_all(evidence_dir).with_context(|| {
            format!(
                "failed to reset evidence directory {}",
                evidence_dir.display()
            )
        })?;
    }
    fs::create_dir_all(evidence_dir)?;
    fs::write(
        evidence_dir.join(EVIDENCE_MARKER),
        b"audio-undo-lab evidence directory\n",
    )?;

    let temp_parent = evidence_dir
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("tmp");
    fs::create_dir_all(&temp_parent)?;
    let temporary = tempfile::Builder::new()
        .prefix("audio-undo-")
        .tempdir_in(&temp_parent)
        .context("failed to create temporary work directory")?;
    let temporary_root = temporary.path().to_path_buf();

    render::init()?;
    let control_v1 = render::render_into_dir(
        &spec.controls.snapshot_v1.clip,
        &evidence_dir.join("controls"),
        "snapshot_v1",
    )?;
    let control_v2 = render::render_into_dir(
        &spec.controls.snapshot_v2.clip,
        &evidence_dir.join("controls"),
        "snapshot_v2",
    )?;
    write_json(
        &evidence_dir.join("controls").join("controls.json"),
        &EvidenceControls {
            experiment_id: spec.experiment_id.clone(),
            snapshot_v1: spec.controls.snapshot_v1.clone(),
            snapshot_v2: spec.controls.snapshot_v2.clone(),
        },
    )?;
    write_json(
        &evidence_dir.join("controls").join("snapshot_v1.json"),
        &SnapshotEvidence {
            snapshot: spec.controls.snapshot_v1.clone(),
            render: control_v1.clone(),
        },
    )?;
    write_json(
        &evidence_dir.join("controls").join("snapshot_v2.json"),
        &SnapshotEvidence {
            snapshot: spec.controls.snapshot_v2.clone(),
            render: control_v2.clone(),
        },
    )?;
    fs::copy(scenario_path, evidence_dir.join("scenario.undoexp"))
        .context("failed to preserve the scenario file")?;

    let events_path = evidence_dir.join("events.jsonl");
    let mut event_file = BufWriter::new(File::create(&events_path)?);
    let started_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let run_event = RunEvent {
        event: "run_start".into(),
        experiment_id: spec.experiment_id.clone(),
        scenario_path: scenario_path.display().to_string(),
        evidence_dir: evidence_dir.display().to_string(),
        extension: spec.extension.value.clone(),
        started_unix_ms,
        rustc: rust_version(),
        gstreamer: gst::version_string().to_string(),
    };
    writeln!(event_file, "{}", event_line(&run_event)?)?;

    let mut object_reports = Vec::new();
    let mut step_events = Vec::new();
    let mut object_events = Vec::new();

    for object in &spec.objects {
        let result = process_object(
            &spec,
            object,
            evidence_dir,
            temporary.path(),
            &control_v1,
            &control_v2,
        );
        match result {
            Ok((report, mut steps, object_event)) => {
                for step in &steps {
                    writeln!(event_file, "{}", event_line(step)?)?;
                }
                writeln!(event_file, "{}", event_line(&object_event)?)?;
                if let Some(error) = &report.error {
                    writeln!(
                        event_file,
                        "{}",
                        event_line(&ErrorEvent {
                            event: "object_error".into(),
                            experiment_id: spec.experiment_id.clone(),
                            object_id: object.id.clone(),
                            error: error.clone(),
                        })?
                    )?;
                }
                object_reports.push(report);
                step_events.append(&mut steps);
                object_events.push(object_event);
            }
            Err(error) => {
                let message = format!("{error:#}");
                writeln!(
                    event_file,
                    "{}",
                    event_line(&ErrorEvent {
                        event: "object_error".into(),
                        experiment_id: spec.experiment_id.clone(),
                        object_id: object.id.clone(),
                        error: message.clone(),
                    })?
                )?;
                object_reports.push(ObjectReport {
                    object_id: object.id.clone(),
                    passed: false,
                    expected_final_control: object.expect.final_control.clone(),
                    expected_final_sha256: control_snapshot(&spec, &object.expect.final_control)
                        .map(|snapshot| snapshot.state_sha256.clone())
                        .unwrap_or_default(),
                    expected_final_media_sha256: expected_control_media_hash(
                        &object.expect.final_control,
                        &control_v1,
                        &control_v2,
                    ),
                    actual_final_sha256: String::new(),
                    actual_final_media_sha256: String::new(),
                    steps: 0,
                    rejected_step_indices: Vec::new(),
                    error: Some(message),
                });
            }
        }
    }

    event_file.flush()?;
    drop(event_file);

    let replay = replay_records(
        &spec.controls.snapshot_v1.clip,
        &step_events,
        &object_events,
        events_path.display().to_string(),
    );
    write_json(&evidence_dir.join("replay.json"), &replay)?;

    let temp_exists_after_close = {
        temporary.close().ok();
        temporary_root.exists()
    };
    if !temp_exists_after_close {
        let _ = fs::remove_dir(&temp_parent);
    }
    let cleanup = CleanupReport {
        temporary_root: temporary_root.display().to_string(),
        removed: !temp_exists_after_close,
        exists_after: temp_exists_after_close,
    };
    let ok = cleanup.removed && replay.ok && object_reports.iter().all(|report| report.passed);
    let summary_path = evidence_dir.join("summary.json");
    let report = ExperimentReport {
        ok,
        experiment_id: spec.experiment_id.clone(),
        extension: spec.extension.value.clone(),
        snapshot_v1_sha256: spec.controls.snapshot_v1.state_sha256.clone(),
        snapshot_v2_sha256: spec.controls.snapshot_v2.state_sha256.clone(),
        snapshot_v1_media_sha256: control_v1.media_sha256,
        snapshot_v2_media_sha256: control_v2.media_sha256,
        objects: object_reports,
        replay,
        cleanup,
        evidence_dir: evidence_dir.display().to_string(),
        events_path: events_path.display().to_string(),
        summary_path: summary_path.display().to_string(),
    };
    write_json(&summary_path, &report)?;
    Ok(report)
}

fn expected_control_media_hash(control: &str, v1: &RenderInfo, v2: &RenderInfo) -> String {
    match control {
        "v1" => v1.media_sha256.clone(),
        "v2" => v2.media_sha256.clone(),
        _ => String::new(),
    }
}

fn rust_version() -> String {
    std::process::Command::new("rustc")
        .arg("--version")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

fn process_object(
    spec: &ExperimentSpec,
    object: &ExperimentObject,
    evidence_dir: &Path,
    temporary_dir: &Path,
    control_v1: &RenderInfo,
    control_v2: &RenderInfo,
) -> Result<(ObjectReport, Vec<StepEvent>, ObjectEvent)> {
    let expected = control_snapshot(spec, &object.expect.final_control)
        .with_context(|| {
            format!(
                "object '{}' refers to unknown control '{}'",
                object.id, object.expect.final_control
            )
        })?
        .clone();
    let expected_media_hash =
        expected_control_media_hash(&object.expect.final_control, control_v1, control_v2);
    let mut history = History::new(spec.controls.snapshot_v1.clip.clone());
    if history.state_hash() != spec.controls.snapshot_v1.state_sha256 {
        bail!("object '{}' did not start from snapshot v1", object.id);
    }

    let mut step_events = Vec::with_capacity(object.steps.len());
    let mut rejected = Vec::new();
    let mut final_media = None;
    for (step_index, input) in object.steps.iter().enumerate() {
        let before = history_view(&history);
        let result = history.apply_step(input);
        if result.status == StepStatus::Rejected {
            rejected.push(step_index);
        }
        let after = history_view(&history);

        let relative_media = PathBuf::from("media")
            .join(&object.id)
            .join(format!("step-{step_index:02}.wav"));
        let scratch_path = temporary_dir.join(format!("{}-{step_index:02}.wav", object.id));
        let mut media = render::render_clip(&after.clip, &scratch_path)?;
        let destination = evidence_dir.join(&relative_media);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        move_file(&scratch_path, &destination)?;
        media.path = relative_media.display().to_string();
        final_media = Some(media.clone());

        step_events.push(StepEvent {
            event: "step".into(),
            experiment_id: spec.experiment_id.clone(),
            object_id: object.id.clone(),
            step_index,
            input: input.clone(),
            state_before: before,
            result,
            state_after: after,
            media,
        });
    }

    let actual_final_sha256 = history.state_hash();
    let actual_final_media_sha256 = final_media
        .as_ref()
        .map(|media| media.media_sha256.clone())
        .unwrap_or_default();
    let mut errors = Vec::new();
    if rejected != object.expect.rejected_step_indices {
        errors.push(format!(
            "rejected steps {:?} did not match expected {:?}",
            rejected, object.expect.rejected_step_indices
        ));
    }
    if actual_final_sha256 != expected.state_sha256 {
        errors.push(format!(
            "final state {} did not match control {} ({})",
            actual_final_sha256, object.expect.final_control, expected.state_sha256
        ));
    }
    if actual_final_media_sha256 != expected_media_hash {
        errors.push(format!(
            "final media {} did not match control {} ({})",
            actual_final_media_sha256, object.expect.final_control, expected_media_hash
        ));
    }
    let error = (!errors.is_empty()).then(|| errors.join("; "));
    let passed = error.is_none();
    let object_event = ObjectEvent {
        event: "object_complete".into(),
        experiment_id: spec.experiment_id.clone(),
        object_id: object.id.clone(),
        passed,
        expected_final_control: object.expect.final_control.clone(),
        expected_final_sha256: expected.state_sha256.clone(),
        actual_final_sha256: actual_final_sha256.clone(),
        rejected_step_indices: rejected.clone(),
        error: error.clone(),
    };
    let report = ObjectReport {
        object_id: object.id.clone(),
        passed,
        expected_final_control: object.expect.final_control.clone(),
        expected_final_sha256: expected.state_sha256,
        expected_final_media_sha256: expected_media_hash,
        actual_final_sha256,
        actual_final_media_sha256,
        steps: object.steps.len(),
        rejected_step_indices: rejected,
        error,
    };
    Ok((report, step_events, object_event))
}

fn move_file(source: &Path, destination: &Path) -> Result<()> {
    match fs::rename(source, destination) {
        Ok(()) => Ok(()),
        Err(_) => {
            fs::copy(source, destination)?;
            fs::remove_file(source)?;
            Ok(())
        }
    }
}

fn replay_records(
    initial: &ClipState,
    step_events: &[StepEvent],
    object_events: &[ObjectEvent],
    source: String,
) -> ReplayReport {
    let mut grouped: BTreeMap<String, Vec<&StepEvent>> = BTreeMap::new();
    for step in step_events {
        grouped
            .entry(step.object_id.clone())
            .or_default()
            .push(step);
    }
    for steps in grouped.values_mut() {
        steps.sort_by_key(|step| step.step_index);
    }

    let mut objects = Vec::with_capacity(object_events.len());
    for object_event in object_events {
        let object_steps = grouped
            .get(&object_event.object_id)
            .cloned()
            .unwrap_or_default();
        if object_steps.is_empty() {
            objects.push(ReplayObjectReport {
                object_id: object_event.object_id.clone(),
                passed: false,
                final_sha256_matched: false,
                steps: Vec::new(),
                error: Some("no recorded steps available for replay".into()),
            });
            continue;
        }

        let mut history = History::new(initial.clone());
        let mut replayed_steps = Vec::with_capacity(object_steps.len());
        for step in object_steps {
            let result = history.apply_step(&step.input);
            let actual_sha = history.state_hash();
            let matched =
                result.status == step.result.status && actual_sha == step.state_after.state_sha256;
            replayed_steps.push(ReplayStep {
                object_id: step.object_id.clone(),
                step_index: step.step_index,
                request_id: step.input.request_id.clone(),
                expected_status: step.result.status.clone(),
                actual_status: result.status,
                expected_state_sha256: step.state_after.state_sha256.clone(),
                actual_state_sha256: actual_sha,
                matched,
            });
        }
        let final_sha = history.state_hash();
        let final_sha256_matched = final_sha == object_event.expected_final_sha256;
        let all_steps_matched = replayed_steps.iter().all(|step| step.matched);
        objects.push(ReplayObjectReport {
            object_id: object_event.object_id.clone(),
            passed: final_sha256_matched && all_steps_matched && object_event.passed,
            final_sha256_matched,
            steps: replayed_steps,
            error: None,
        });
    }
    let ok = objects.iter().all(|object| object.passed);
    ReplayReport {
        ok,
        source,
        objects,
    }
}

pub fn replay_evidence(evidence_dir: &Path) -> Result<ReplayReport> {
    let controls_path = evidence_dir.join("controls").join("controls.json");
    let controls: EvidenceControls = serde_json::from_slice(
        &fs::read(&controls_path)
            .with_context(|| format!("failed to read {}", controls_path.display()))?,
    )?;
    validate_snapshot(&controls.snapshot_v1)?;
    validate_snapshot(&controls.snapshot_v2)?;

    let events_path = evidence_dir.join("events.jsonl");
    let reader = BufReader::new(
        File::open(&events_path)
            .with_context(|| format!("failed to open {}", events_path.display()))?,
    );
    let mut step_events = Vec::new();
    let mut object_events = Vec::new();
    for (line_number, line) in reader.lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let value: serde_json::Value = serde_json::from_str(&line).with_context(|| {
            format!(
                "invalid JSON in {} at line {}",
                events_path.display(),
                line_number + 1
            )
        })?;
        match value.get("event").and_then(|value| value.as_str()) {
            Some("step") => step_events.push(serde_json::from_value(value)?),
            Some("object_complete") => object_events.push(serde_json::from_value(value)?),
            _ => {}
        }
    }

    let report = replay_records(
        &controls.snapshot_v1.clip,
        &step_events,
        &object_events,
        events_path.display().to_string(),
    );
    write_json(&evidence_dir.join("replay.json"), &report)?;
    Ok(report)
}
