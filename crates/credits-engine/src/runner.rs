use std::collections::HashSet;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use credits_core::integrity::{evidence_file, sha256_file, EvidenceFile};
use credits_core::model::{
    is_known_media_extension, Checkpoint, MediaObject, Project, TypesetParameters,
};
use credits_core::project::load_project;
use credits_core::report::{
    now_unix_ms, CorrelationEvidence, FailureRecord, MediaEvidence, ObjectReview, ObjectStatus,
    ReviewReport, SnapshotEvidence, SnapshotVersion, TimePointEvidence,
};
use credits_core::timecode::{format_clock_time, FramePosition};
use credits_core::waveform::WaveformEvidence;
use serde_json::json;

use crate::audio_wave::extract_waveform;
use crate::capture::{capture_frame, write_png};
use crate::probe::probe_media;
use crate::temp::WorkArea;
use crate::typeset::{blend_overlay, pixel_diff, render_overlay};

#[derive(Debug, Clone)]
pub struct ReviewOptions {
    pub out_dir: PathBuf,
    pub checkpoints: Vec<f64>,
}

#[derive(Debug)]
struct ObjectFailure {
    stage: String,
    message: String,
}

impl ObjectFailure {
    fn at(stage: &str, error: impl std::fmt::Display) -> Self {
        Self {
            stage: stage.to_string(),
            message: error.to_string(),
        }
    }
}

pub fn run_review(input: &Path, options: &ReviewOptions) -> Result<ReviewReport> {
    let loaded = load_project(input)?;
    let mut project = loaded.project;
    if !options.checkpoints.is_empty() {
        for object in &mut project.objects {
            object.checkpoints = options
                .checkpoints
                .iter()
                .enumerate()
                .map(|(index, seconds)| Checkpoint {
                    seconds: *seconds,
                    requested: Some(format!("cli:{seconds}")),
                    label: Some(format!("cli checkpoint {}", index + 1)),
                    track: Some(object.track.clone()),
                    requested_frame: None,
                })
                .collect();
        }
    }
    std::fs::create_dir_all(&options.out_dir).with_context(|| {
        format!(
            "cannot create output directory {}",
            options.out_dir.display()
        )
    })?;
    let evidence_root = options.out_dir.join("evidence");
    std::fs::create_dir_all(&evidence_root)?;

    let mut work = WorkArea::create(&options.out_dir).with_context(|| {
        format!(
            "cannot create work area under {}",
            options.out_dir.display()
        )
    })?;
    let mut report = ReviewReport::new(
        loaded.source,
        project.title.clone(),
        project.format,
        project.fps,
    );
    report.tool.gstreamer_version = Some(crate::probe::gstreamer_version());
    report.warnings.extend(project.warnings.clone());

    let normalized_path = options.out_dir.join("normalized-project.json");
    write_json(&normalized_path, &project)?;
    report.normalized_project = Some(evidence_file(&options.out_dir, &normalized_path)?);

    if project.objects.is_empty() {
        report
            .warnings
            .push("project contains no processable media objects".to_string());
    }

    let mut used_dirs = HashSet::new();
    for (index, object) in project.objects.iter().enumerate() {
        let object_dir = unique_object_dir(&evidence_root, object, index, &mut used_dirs);
        std::fs::create_dir_all(&object_dir)?;
        let result = catch_unwind(AssertUnwindSafe(|| {
            process_object(&project, object, &options.out_dir, &object_dir)
        }));
        match result {
            Ok(Ok((review, files))) => {
                for path in files {
                    if let Err(error) = report.add_evidence(&options.out_dir, &path) {
                        report.warnings.push(format!(
                            "{}: evidence hash failed for {}: {error}",
                            object.id,
                            path.display()
                        ));
                    }
                }
                report.objects.push(review);
            }
            Ok(Err(failure)) => {
                record_failure(
                    &mut report,
                    &options.out_dir,
                    &object_dir,
                    &object.id,
                    failure.stage,
                    failure.message,
                );
            }
            Err(payload) => {
                let message = panic_message(payload);
                record_failure(
                    &mut report,
                    &options.out_dir,
                    &object_dir,
                    &object.id,
                    "panic".to_string(),
                    message,
                );
            }
        }
    }

    report.cleanup = work.finish();
    report.recompute_summary();
    report.write(&options.out_dir)?;
    Ok(report)
}

fn process_object(
    project: &Project,
    object: &MediaObject,
    out_dir: &Path,
    object_dir: &Path,
) -> std::result::Result<(ObjectReview, Vec<PathBuf>), ObjectFailure> {
    let mut files = Vec::new();
    let media = media_evidence(&object.path).map_err(|error| ObjectFailure::at("media", error))?;
    let probe = probe_media(&object.path).map_err(|error| ObjectFailure::at("probe", error))?;
    let probe_path = object_dir.join("probe.json");
    write_json(&probe_path, &probe).map_err(|error| ObjectFailure::at("probe-evidence", error))?;
    files.push(probe_path);

    let fps = probe
        .fps
        .filter(|fps| fps.is_finite() && *fps >= 1.0 && *fps <= 240.0)
        .unwrap_or(project.fps);
    let duration = object.duration_s.or(probe.duration_s);
    let checkpoints = effective_checkpoints(object, duration, fps);
    let mut warnings = Vec::new();

    let mut waveform = match probe.audio_codec.as_ref() {
        Some(_) => match extract_waveform(&object.path, duration) {
            Ok(waveform) => waveform,
            Err(error) => {
                warnings.push(format!("waveform extraction failed: {error}"));
                WaveformEvidence::unavailable(format!("extraction failed: {error}"))
            }
        },
        None => WaveformEvidence::unavailable("no audio stream discovered"),
    };
    for checkpoint in &checkpoints {
        waveform.add_marker(
            checkpoint.seconds,
            checkpoint
                .label
                .clone()
                .or_else(|| Some(format_clock_time(checkpoint.seconds))),
        );
    }
    let waveform_json = object_dir.join("waveform.json");
    waveform
        .write_json(&waveform_json)
        .map_err(|error| ObjectFailure::at("waveform-evidence", error))?;
    files.push(waveform_json.clone());
    let waveform_svg = object_dir.join("waveform.svg");
    waveform
        .write_svg(&waveform_svg, &format!("{} / audio", object.id))
        .map_err(|error| ObjectFailure::at("waveform-evidence", error))?;
    files.push(waveform_svg.clone());
    let waveform_json_entry = evidence_file(out_dir, &waveform_json)
        .map_err(|error| ObjectFailure::at("waveform-evidence", error))?;
    let waveform_svg_entry = evidence_file(out_dir, &waveform_svg)
        .map_err(|error| ObjectFailure::at("waveform-evidence", error))?;

    let mut correlations = Vec::new();
    for (index, checkpoint) in checkpoints.iter().enumerate() {
        let elapsed = checkpoint.seconds - object.start_s;
        let frame = capture_frame(&object.path, checkpoint.seconds).map_err(|error| {
            ObjectFailure::at("capture", format!("checkpoint {index}: {error}"))
        })?;
        let overlay = render_overlay(
            &object.effective_text(),
            frame.width,
            frame.height,
            &object.parameters,
            elapsed,
        )
        .map_err(|error| ObjectFailure::at("typeset", format!("checkpoint {index}: {error}")))?;
        let after_rgba = blend_overlay(&frame.rgba, &overlay).map_err(|error| {
            ObjectFailure::at("typeset", format!("checkpoint {index}: {error}"))
        })?;
        let frame_fps = frame
            .fps
            .filter(|fps| fps.is_finite() && *fps >= 1.0)
            .unwrap_or(fps);
        let captured_seconds = frame
            .pts_ns
            .map(|pts| pts as f64 / 1e9)
            .unwrap_or(checkpoint.seconds);
        let position =
            FramePosition::from_seconds(captured_seconds, frame_fps).unwrap_or(FramePosition {
                frame: 0,
                fps_num: frame_fps.round().max(1.0) as u32,
                fps_den: 1,
            });
        let stem = format!(
            "checkpoint-{index:03}-frame-{:08}",
            position.frame.min(99_999_999)
        );
        let before_path = object_dir.join(format!("{stem}-before.png"));
        let after_path = object_dir.join(format!("{stem}-after.png"));
        write_png(&before_path, frame.width, frame.height, &frame.rgba).map_err(|error| {
            ObjectFailure::at("snapshot", format!("checkpoint {index}: {error}"))
        })?;
        write_png(&after_path, frame.width, frame.height, &after_rgba).map_err(|error| {
            ObjectFailure::at("snapshot", format!("checkpoint {index}: {error}"))
        })?;
        files.push(before_path.clone());
        files.push(after_path.clone());

        let before_entry = evidence_file(out_dir, &before_path)
            .map_err(|error| ObjectFailure::at("evidence", error))?;
        let after_entry = evidence_file(out_dir, &after_path)
            .map_err(|error| ObjectFailure::at("evidence", error))?;
        let track = checkpoint
            .track
            .clone()
            .unwrap_or_else(|| object.track.clone());
        let time = TimePointEvidence {
            seconds: checkpoint.seconds,
            clock: format_clock_time(checkpoint.seconds),
            timecode: position.timecode(),
            frame: position.frame,
            pts_ns: frame.pts_ns,
            fps: frame_fps,
            track: track.clone(),
            requested: checkpoint.requested.clone(),
            label: checkpoint.label.clone(),
        };
        let before = snapshot_evidence(SnapshotInput {
            version: SnapshotVersion::Before,
            label: "captured source frame",
            entry: &before_entry,
            width: frame.width,
            height: frame.height,
            pts_ns: frame.pts_ns,
            frame: position.frame,
            timecode: &time.timecode,
            track: &track,
            applied: false,
            parameters: &object.parameters,
            pipeline: "uridecodebin ! videoconvert ! capsfilter(video/x-raw,format=RGBA) ! appsink",
        });
        let after = snapshot_evidence(SnapshotInput {
            version: SnapshotVersion::After,
            label: "typeset rolling-credit frame",
            entry: &after_entry,
            width: frame.width,
            height: frame.height,
            pts_ns: frame.pts_ns,
            frame: position.frame,
            timecode: &time.timecode,
            track: &track,
            applied: true,
            parameters: &object.parameters,
            pipeline:
                "uridecodebin ! videoconvert ! appsink; pango/cairo RGBA overlay; alpha blend",
        });
        let diff = pixel_diff(&frame.rgba, &after_rgba)
            .map_err(|error| ObjectFailure::at("compare", error))?;
        let peak_index = waveform.peak_index(checkpoint.seconds);
        let window = waveform.window_around(checkpoint.seconds, 16).to_vec();
        correlations.push(CorrelationEvidence {
            index,
            time,
            versions: vec![before, after],
            parameters: object.parameters.clone(),
            waveform_peak_index: peak_index,
            waveform_window: window,
            waveform_file: waveform_json_entry.path.clone(),
            pixel_diff: Some(diff),
        });
    }

    let mut local_evidence = Vec::new();
    for path in &files {
        local_evidence.push(
            evidence_file(out_dir, path).map_err(|error| ObjectFailure::at("evidence", error))?,
        );
    }
    let partial = (!waveform.available && probe.audio_codec.is_some()) || !warnings.is_empty();
    let review = ObjectReview {
        id: object.id.clone(),
        status: if partial {
            ObjectStatus::Partial
        } else {
            ObjectStatus::Ok
        },
        media,
        probe,
        correlations,
        waveform,
        waveform_json: waveform_json_entry.path,
        waveform_svg: waveform_svg_entry.path,
        evidence_files: local_evidence,
        warnings,
    };
    Ok((review, files))
}

fn effective_checkpoints(object: &MediaObject, duration: Option<f64>, fps: f64) -> Vec<Checkpoint> {
    let mut checkpoints = object.resolved_checkpoints(fps);
    if checkpoints.is_empty() {
        checkpoints.push(Checkpoint::new(0.0));
    }
    let max = duration
        .filter(|duration| duration.is_finite() && *duration > 0.0)
        .map(|duration| (duration - 1.0 / fps.max(1.0)).max(0.0));
    for checkpoint in &mut checkpoints {
        if let Some(max) = max {
            checkpoint.seconds = checkpoint.seconds.min(max);
        }
    }
    checkpoints
}

fn media_evidence(path: &Path) -> Result<MediaEvidence> {
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("cannot read metadata for {}", path.display()))?;
    let mut head = std::fs::File::open(path)
        .and_then(|mut file| {
            use std::io::Read;
            let mut buffer = vec![0_u8; 64];
            let read = file.read(&mut buffer)?;
            buffer.truncate(read);
            Ok(buffer)
        })
        .with_context(|| format!("cannot read {}", path.display()))?;
    if head.len() > 64 {
        head.truncate(64);
    }
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase());
    let unknown_extension = extension
        .as_deref()
        .map(|extension| !is_known_media_extension(extension))
        .unwrap_or(true);
    Ok(MediaEvidence {
        path: path.to_path_buf(),
        extension,
        unknown_extension,
        size_bytes: metadata.len(),
        sha256: sha256_file(path)?,
        magic_hex: hex::encode(head),
    })
}

struct SnapshotInput<'a> {
    version: SnapshotVersion,
    label: &'a str,
    entry: &'a EvidenceFile,
    width: u32,
    height: u32,
    pts_ns: Option<u64>,
    frame: u64,
    timecode: &'a str,
    track: &'a str,
    applied: bool,
    parameters: &'a TypesetParameters,
    pipeline: &'a str,
}

fn snapshot_evidence(input: SnapshotInput<'_>) -> SnapshotEvidence {
    SnapshotEvidence {
        version: input.version,
        label: input.label.to_string(),
        file: input.entry.path.clone(),
        sha256: input.entry.sha256.clone(),
        width: input.width,
        height: input.height,
        pts_ns: input.pts_ns,
        frame: input.frame,
        timecode: input.timecode.to_string(),
        track: input.track.to_string(),
        parameters_applied: input.applied,
        parameters: input.parameters.clone(),
        pipeline: input.pipeline.to_string(),
    }
}

fn record_failure(
    report: &mut ReviewReport,
    out_dir: &Path,
    object_dir: &Path,
    object_id: &str,
    stage: String,
    message: String,
) {
    let relative_dir = object_dir
        .strip_prefix(out_dir)
        .ok()
        .map(|path| path.to_string_lossy().replace('\\', "/"));
    let failure_path = object_dir.join("failure.json");
    let payload = json!({
        "object_id": object_id,
        "stage": stage,
        "message": message,
        "evidence_dir": relative_dir,
        "kept_at_unix_ms": now_unix_ms(),
        "note": "evidence already written for this object is retained so the failure can be reviewed"
    });
    if let Ok(()) = write_json(&failure_path, &payload) {
        if let Err(error) = report.add_evidence(out_dir, &failure_path) {
            report.warnings.push(format!(
                "{object_id}: cannot hash failure evidence {}: {error}",
                failure_path.display()
            ));
        }
    }
    let mut retained = Vec::new();
    collect_files(object_dir, &mut retained);
    retained.sort();
    for path in retained {
        if let Err(error) = report.add_evidence(out_dir, &path) {
            report.warnings.push(format!(
                "{object_id}: cannot hash retained evidence {}: {error}",
                path.display()
            ));
        }
    }
    report.failures.push(FailureRecord {
        object_id: object_id.to_string(),
        stage,
        message,
        evidence_dir: relative_dir,
    });
}

fn unique_object_dir(
    root: &Path,
    object: &MediaObject,
    index: usize,
    used: &mut HashSet<String>,
) -> PathBuf {
    let base = sanitize_component(&object.id);
    let mut name = format!("{index:04}-{base}");
    let mut suffix = 2;
    while !used.insert(name.clone()) {
        name = format!("{index:04}-{base}-{suffix}");
        suffix += 1;
    }
    root.join(name)
}

fn sanitize_component(raw: &str) -> String {
    let mut value = String::new();
    for character in raw.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
            value.push(character.to_ascii_lowercase());
        } else if character.is_whitespace() {
            value.push('-');
        }
    }
    let value = value.trim_matches('-').to_string();
    if value.is_empty() {
        "object".to_string()
    } else {
        value
    }
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else if let Some(message) = payload.downcast_ref::<&str>() {
        message.to_string()
    } else {
        "object processing panicked".to_string()
    }
}

fn collect_files(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, files);
        } else if path.is_file() {
            files.push(path);
        }
    }
}

fn write_json(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    std::fs::write(path, bytes).with_context(|| format!("cannot write {}", path.display()))
}
