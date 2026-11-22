use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use sha2::{Digest, Sha256};

use crate::events;
use crate::evidence;
use crate::model::{
    ANCHOR_A_NS, ANCHOR_B_NS, ANCHOR_C_NS, AnchorEvidence, Diagnostic, EventInputEvidence,
    EvidenceCounts, EvidenceFile, FrameSample, InputEvidence, OutputEvidence, Params, ParsedLog,
    SCHEMA, VideoInputEvidence,
};
use crate::overlay;
use crate::waveform::{self, TrajectorySampler};

#[derive(Debug, Clone)]
pub struct AnalyzeOptions {
    pub video: PathBuf,
    pub events: PathBuf,
    pub out: PathBuf,
    pub max_events: usize,
    pub max_event_bytes: u64,
    pub start_ns: u64,
    pub end_ns: u64,
}

#[derive(Debug, Clone)]
pub struct RunResult {
    pub status: String,
    pub anchor_count: usize,
}

struct FileMeta {
    bytes: u64,
    sha256: String,
}

pub fn run(options: AnalyzeOptions) -> anyhow::Result<RunResult> {
    fs::create_dir_all(&options.out)
        .with_context(|| format!("cannot create output directory {}", options.out.display()))?;
    prepare_output(&options)?;
    let video_meta = file_meta(&options.video)?;
    let event_load = events::load(&options.events, options.max_event_bytes, options.max_events);
    if video_meta.bytes == 0 {
        let event_meta = file_meta(&options.events).ok();
        return write_input_error(
            &options,
            video_meta,
            event_meta,
            Diagnostic {
                code: "E_EMPTY_VIDEO".into(),
                message: "video input is empty (0 bytes)".into(),
                phase: "preflight".into(),
                raw_offset: Some(0),
            },
        );
    }

    let (event_source, log) = match event_load {
        Ok(loaded) => loaded,
        Err(error) => {
            let event_meta = file_meta(&options.events).ok();
            return write_input_error(
                &options,
                video_meta,
                event_meta,
                Diagnostic {
                    code: error.code.into(),
                    message: error.message,
                    phase: "event_load".into(),
                    raw_offset: error.raw_offset,
                },
            );
        }
    };

    let params = Params::default();
    let params_sha256 = waveform::params_sha256(&params);
    let trace_path = options.out.join("trace.jsonl");
    let csv_path = options.out.join("waveform.csv");
    let svg_path = options.out.join("waveform.svg");
    let overlay_path = options.out.join("overlay.mkv");
    let anchors_json = options.out.join("evidence").join("anchors.json");
    let raw_dir = options.out.join("evidence").join("raw");
    fs::create_dir_all(&raw_dir)?;

    let mut samples = Vec::new();
    let mut trail = Vec::new();
    let mut writer: Option<overlay::OutputWriter> = None;
    let mut anchor_frames = Vec::new();
    let mut dimensions = None;
    let mut frame_index = 0u64;
    let mut filter_start = options.start_ns;
    let mut filter_end = options.end_ns;

    let video_result = {
        let mut sampler = TrajectorySampler::new(&log, &params, 1, 1);
        overlay::read_video(&options.video, |mut frame| {
            dimensions = Some((frame.width, frame.height));
            sampler.set_dimensions(frame.width, frame.height);
            if filter_start == 0 && filter_end == 0 {
                filter_start = 0;
                filter_end = u64::MAX;
            }
            let in_range = frame.pts_ns >= filter_start && frame.pts_ns <= filter_end;
            let sample = sampler.sample(frame_index, frame.pts_ns, frame.duration_ns);
            frame_index += 1;
            if in_range {
                if sample.visible {
                    if let (Some(x), Some(y)) = (sample.x, sample.y) {
                        trail.push((x, y));
                    } else {
                        trail.clear();
                    }
                } else {
                    trail.clear();
                }
                overlay::draw_trail(
                    &mut frame.rgba,
                    frame.width,
                    frame.height,
                    &trail,
                    sample.anchor.as_deref(),
                );
                samples.push(sample.clone());

                if let Some(anchor) = &sample.anchor {
                    let filename = format!("anchor-{anchor}.png");
                    let path = options.out.join("frames").join(&filename);
                    fs::create_dir_all(path.parent().expect("frame parent"))?;
                    overlay::write_png(&frame.rgba, frame.width, frame.height, &path)?;
                    anchor_frames.push(format!("frames/{filename}"));
                }
            }
            let output_writer = match writer.as_mut() {
                Some(writer) => writer,
                None => {
                    writer = Some(overlay::OutputWriter::new(
                        &overlay_path,
                        frame.width,
                        frame.height,
                    )?);
                    writer.as_mut().expect("writer initialized")
                }
            };
            output_writer.push(&frame.rgba, frame.pts_ns, frame.duration_ns)
        })
    };

    if let Err(error) = video_result {
        return write_input_error(
            &options,
            video_meta,
            Some(FileMeta {
                bytes: event_source.bytes,
                sha256: event_source.sha256.clone(),
            }),
            Diagnostic {
                code: "E_VIDEO_PIPELINE".into(),
                message: format!("{error:#}"),
                phase: "video_decode".into(),
                raw_offset: None,
            },
        );
    }
    let (width, height) = dimensions.context("video produced no dimensions")?;
    if let Some(writer) = writer {
        writer.finish()?;
    } else {
        return write_input_error(
            &options,
            video_meta,
            Some(FileMeta {
                bytes: event_source.bytes,
                sha256: event_source.sha256.clone(),
            }),
            Diagnostic {
                code: "E_EMPTY_FRAMES".into(),
                message: "video track contained no frames".into(),
                phase: "video_decode".into(),
                raw_offset: Some(0),
            },
        );
    }

    waveform::write_trace(&trace_path, &samples)?;
    waveform::write_csv(&csv_path, &samples)?;
    waveform::write_svg(&svg_path, &samples, &params)?;
    let anchors = if log.events.is_empty() {
        Vec::new()
    } else {
        build_anchors(&options.events, &log, &samples, &raw_dir, &params_sha256)?
    };
    evidence::write_json(&anchors_json, &anchors)?;

    let video_after = file_meta(&options.video)?;
    let events_after = file_meta(&options.events)?;
    let source_unchanged =
        video_after.sha256 == video_meta.sha256 && events_after.sha256 == event_source.sha256;
    let status = if log.events.is_empty() {
        "ok_empty_events"
    } else {
        "ok"
    };
    let evidence_file = EvidenceFile {
        schema: SCHEMA.into(),
        status: status.into(),
        generated_at_utc: evidence::now_utc(),
        implementation: evidence::implementation(),
        inputs: InputEvidence {
            video: VideoInputEvidence {
                path: options.video.display().to_string(),
                bytes: video_meta.bytes,
                sha256: video_meta.sha256.clone(),
                width: Some(width),
                height: Some(height),
                frames: samples.len() as u64,
                first_pts_ns: samples.first().map(|sample| sample.pts_ns),
                last_pts_ns: samples.last().map(|sample| sample.pts_ns),
                duration_ns: samples
                    .last()
                    .map_or(0, |sample| sample.pts_ns + sample.duration_ns),
            },
            events: EventInputEvidence {
                path: options.events.display().to_string(),
                bytes: event_source.bytes,
                sha256: event_source.sha256.clone(),
                max_event_bytes: options.max_event_bytes,
                max_events: options.max_events,
                min_ts_ns: log.min_ts_ns,
                max_ts_ns: log.max_ts_ns,
            },
        },
        params,
        params_sha256: params_sha256.clone(),
        anchors: anchors.clone(),
        counts: EvidenceCounts {
            events_parsed: log.records_parsed,
            events_kept: log.records_kept,
            duplicates_removed: log.records_dropped_duplicate,
            bad_json_records: log.records_dropped_bad_json,
            raw_order_inversions: log
                .events
                .iter()
                .filter(|event| event.raw_order_inversion)
                .count(),
            clamped_frames: samples
                .iter()
                .filter(|sample| sample.clamp_x || sample.clamp_y)
                .count(),
        },
        outputs: OutputEvidence {
            trace_jsonl: trace_path.display().to_string(),
            waveform_csv: csv_path.display().to_string(),
            waveform_svg: svg_path.display().to_string(),
            overlay_video: overlay_path.display().to_string(),
            anchors_json: anchors_json.display().to_string(),
            raw_evidence_dir: raw_dir.display().to_string(),
            anchor_frames,
        },
        error: None,
        source_unchanged,
    };
    evidence::write_json(&options.out.join("evidence.json"), &evidence_file)?;
    if !source_unchanged {
        bail!("original input changed during analysis");
    }
    Ok(RunResult {
        status: status.into(),
        anchor_count: anchors.len(),
    })
}

fn file_meta(path: &Path) -> anyhow::Result<FileMeta> {
    let mut file =
        fs::File::open(path).with_context(|| format!("cannot open {}", path.display()))?;
    let bytes = file.metadata()?.len();
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(FileMeta {
        bytes,
        sha256: format!("{:x}", digest.finalize()),
    })
}

fn prepare_output(options: &AnalyzeOptions) -> anyhow::Result<()> {
    let out = fs::canonicalize(&options.out)?;
    for input in [&options.video, &options.events] {
        if let Ok(input) = fs::canonicalize(input)
            && input.starts_with(&out)
        {
            bail!(
                "output directory {} contains original input {}; refusing to overwrite",
                out.display(),
                input.display()
            );
        }
    }
    for file in [
        "trace.jsonl",
        "waveform.csv",
        "waveform.svg",
        "overlay.mkv",
        "evidence/anchors.json",
    ] {
        let path = options.out.join(file);
        if path.is_file() {
            fs::remove_file(&path)
                .with_context(|| format!("cannot replace output {}", path.display()))?;
        }
    }
    for directory in ["frames", "evidence/raw"] {
        let path = options.out.join(directory);
        if path.is_dir() {
            fs::remove_dir_all(&path)?;
        }
    }
    Ok(())
}

fn build_anchors(
    events_path: &Path,
    log: &ParsedLog,
    samples: &[FrameSample],
    raw_dir: &Path,
    params_sha256: &str,
) -> anyhow::Result<Vec<AnchorEvidence>> {
    let definitions = [
        ("A", ANCHOR_A_NS, "duplicate_timestamp"),
        ("B", ANCHOR_B_NS, "raw_order_inversion"),
        ("C", ANCHOR_C_NS, "out_of_bounds_clamped"),
    ];
    let mut anchors = Vec::with_capacity(definitions.len());
    for (id, pts_ns, kind) in definitions {
        let sample = samples
            .iter()
            .find(|sample| sample.pts_ns == pts_ns)
            .with_context(|| format!("missing frame at anchor {id} PTS {pts_ns}"))?;
        let event_index = log
            .events
            .iter()
            .position(|event| Some(event.raw_offset) == sample.raw_offset)
            .with_context(|| format!("anchor {id} did not resolve to a raw event"))?;
        let event = &log.events[event_index];
        let raw = events::raw_slice(events_path, event.raw_offset, event.raw_len)?;
        let raw_sha256 = events::sha256_hex(&raw);
        if raw_sha256 != event.raw_sha256 {
            bail!("anchor {id} raw bytes no longer match parsed event");
        }
        let raw_path = raw_dir.join(format!("anchor-{id}.jsonl"));
        fs::write(&raw_path, &raw)?;
        anchors.push(AnchorEvidence {
            id: id.into(),
            kind: kind.into(),
            pts_ns: sample.pts_ns,
            frame_index: sample.frame_index,
            track_id: sample.track_id,
            raw_offset: event.raw_offset,
            raw_length: event.raw_len,
            raw_sha256,
            raw_json: String::from_utf8_lossy(&raw).into_owned(),
            event_seq: event.seq,
            event_ts_ns: event.ts_ns,
            raw_x: sample.raw_x.context("anchor has no raw x")?,
            raw_y: sample.raw_y.context("anchor has no raw y")?,
            output_x: sample.x.context("anchor has no output x")?,
            output_y: sample.y.context("anchor has no output y")?,
            output_speed_px_s: sample.speed_px_s,
            flags: sample.flags.clone(),
            params_sha256: params_sha256.into(),
            duplicate_records: log.duplicates[event_index].clone(),
        });
    }
    Ok(anchors)
}

#[allow(clippy::too_many_arguments)]
fn write_input_error(
    options: &AnalyzeOptions,
    video: FileMeta,
    event: Option<FileMeta>,
    diagnostic: Diagnostic,
) -> anyhow::Result<RunResult> {
    let params = Params::default();
    let params_sha256 = waveform::params_sha256(&params);
    let event = event.unwrap_or(FileMeta {
        bytes: 0,
        sha256: String::new(),
    });
    let evidence_file = EvidenceFile {
        schema: SCHEMA.into(),
        status: "error".into(),
        generated_at_utc: evidence::now_utc(),
        implementation: evidence::implementation(),
        inputs: InputEvidence {
            video: VideoInputEvidence {
                path: options.video.display().to_string(),
                bytes: video.bytes,
                sha256: video.sha256,
                width: None,
                height: None,
                frames: 0,
                first_pts_ns: None,
                last_pts_ns: None,
                duration_ns: 0,
            },
            events: EventInputEvidence {
                path: options.events.display().to_string(),
                bytes: event.bytes,
                sha256: event.sha256,
                max_event_bytes: options.max_event_bytes,
                max_events: options.max_events,
                min_ts_ns: None,
                max_ts_ns: None,
            },
        },
        params,
        params_sha256,
        anchors: Vec::new(),
        counts: EvidenceCounts {
            events_parsed: 0,
            events_kept: 0,
            duplicates_removed: 0,
            bad_json_records: 0,
            raw_order_inversions: 0,
            clamped_frames: 0,
        },
        outputs: OutputEvidence {
            trace_jsonl: String::new(),
            waveform_csv: String::new(),
            waveform_svg: String::new(),
            overlay_video: String::new(),
            anchors_json: String::new(),
            raw_evidence_dir: options.out.join("evidence/raw").display().to_string(),
            anchor_frames: Vec::new(),
        },
        error: Some(diagnostic),
        source_unchanged: true,
    };
    evidence::write_json(&options.out.join("evidence.json"), &evidence_file)?;
    Ok(RunResult {
        status: "error".into(),
        anchor_count: 0,
    })
}

const EXPECTED_A_OFFSET: u64 = 942;
const EXPECTED_B_OFFSET: u64 = 1931;
const EXPECTED_C_OFFSET: u64 = 2741;

pub fn verify(out: &Path) -> anyhow::Result<String> {
    let evidence_path = out.join("evidence.json");
    let evidence = evidence::read_evidence(&evidence_path)?;
    if evidence.status != "ok" {
        bail!(
            "evidence status is {}, expected ok: {:?}",
            evidence.status,
            evidence.error
        );
    }
    if evidence.anchors.len() != 3 {
        bail!("expected 3 anchors, found {}", evidence.anchors.len());
    }
    if !evidence.source_unchanged {
        bail!("evidence reports that original input changed");
    }
    let expected = [
        (
            "A",
            ANCHOR_A_NS,
            10,
            EXPECTED_A_OFFSET,
            128.0,
            96.0,
            128.0,
            96.0,
        ),
        (
            "B",
            ANCHOR_B_NS,
            20,
            EXPECTED_B_OFFSET,
            200.0,
            105.0,
            200.0,
            105.0,
        ),
        (
            "C",
            ANCHOR_C_NS,
            30,
            EXPECTED_C_OFFSET,
            365.0,
            -12.0,
            319.0,
            0.0,
        ),
    ];
    let mut lines = Vec::new();
    for (anchor, (id, pts, frame, offset, raw_x, raw_y, x, y)) in
        evidence.anchors.iter().zip(expected)
    {
        if anchor.id != id {
            bail!("anchor order mismatch: found {}, expected {id}", anchor.id);
        }
        if anchor.pts_ns != pts || anchor.frame_index != frame || anchor.track_id != 0 {
            bail!(
                "anchor {id}: PTS/frame/track mismatch: {}/{}/{}",
                anchor.pts_ns,
                anchor.frame_index,
                anchor.track_id
            );
        }
        if offset != 0 && anchor.raw_offset != offset {
            bail!(
                "anchor {id}: raw offset {} != expected {offset}",
                anchor.raw_offset
            );
        }
        if offset == 0 {
            bail!("anchor {id} expected raw offset constant is not configured");
        }
        if (anchor.raw_x, anchor.raw_y) != (raw_x, raw_y)
            || (anchor.output_x, anchor.output_y) != (x, y)
        {
            bail!(
                "anchor {id}: coordinate mismatch raw=({}, {}) output=({}, {})",
                anchor.raw_x,
                anchor.raw_y,
                anchor.output_x,
                anchor.output_y
            );
        }
        if anchor.params_sha256 != evidence.params_sha256 {
            bail!("anchor {id}: parameter hash mismatch");
        }
        verify_raw_anchor(&evidence, anchor)?;
        lines.push(format!(
            "{id}@{:.3}s frame={frame} raw_offset={} raw_len={} out=({:.0},{:.0})",
            pts as f64 / 1e9,
            anchor.raw_offset,
            anchor.raw_length,
            anchor.output_x,
            anchor.output_y
        ));
    }
    for (label, expected_sha, input_path) in [
        (
            "video",
            &evidence.inputs.video.sha256,
            &evidence.inputs.video.path,
        ),
        (
            "events",
            &evidence.inputs.events.sha256,
            &evidence.inputs.events.path,
        ),
    ] {
        let meta = file_meta(Path::new(input_path))?;
        if &meta.sha256 != expected_sha {
            bail!("{label} original checksum changed");
        }
    }
    for path in [
        &evidence.outputs.trace_jsonl,
        &evidence.outputs.waveform_csv,
        &evidence.outputs.waveform_svg,
        &evidence.outputs.overlay_video,
        &evidence.outputs.anchors_json,
    ] {
        let metadata = fs::metadata(path).with_context(|| format!("missing output {path}"))?;
        if metadata.len() == 0 {
            bail!("output {path} is empty");
        }
    }
    Ok(format!("PASS {}", lines.join(" | ")))
}

fn verify_raw_anchor(evidence: &EvidenceFile, anchor: &AnchorEvidence) -> anyhow::Result<()> {
    let bytes = fs::read(&evidence.inputs.events.path)?;
    let start = anchor.raw_offset as usize;
    let end = start + anchor.raw_length as usize;
    let raw = bytes
        .get(start..end)
        .context("anchor raw range is outside original event file")?;
    let hash = events::sha256_hex(raw);
    if hash != anchor.raw_sha256 {
        bail!("anchor {} raw hash mismatch", anchor.id);
    }
    let raw_evidence =
        Path::new(&evidence.outputs.raw_evidence_dir).join(format!("anchor-{}.jsonl", anchor.id));
    if fs::read(&raw_evidence)? != raw {
        bail!(
            "anchor {} preserved raw bytes differ from source",
            anchor.id
        );
    }
    Ok(())
}
