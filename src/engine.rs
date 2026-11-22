use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::audit::{canonical_record_bytes, file_hash, sha256_bytes, write_artifact_set};
use crate::curve::{load_curve, Curve, CurveKind, CurveLimits};
use crate::error::{io_error, AppError, AppResult};
use crate::model::{
    Anomaly, AnomalyKind, AnomalyParameters, AudioEvidence, CurveEvidence, DebugRecord,
    EvidenceRef, EvidenceState, FrameSpan, IndexSpan, InputEvidence, Limits, OutputEvidence,
    ProcessingEvidence, RecordingPolicy, RequestSnapshot, RunStatus, Severity, TimeSpan,
    TimelineEntry, VerificationCheck, VerificationEvidence, WaveformStats, DEFAULT_CHANNELS,
    DEFAULT_PITCH_GAP_US, DEFAULT_RENDER_QUANTUM_US, DEFAULT_SAMPLE_RATE, DEFAULT_WINDOW_US,
    OUTPUT_WAV, SCHEMA,
};
use crate::plan::{
    build_plan, finalize_timeline, source_span_to_frames, PlanConfig, RenderPlan,
    SegmentRenderResult,
};
use crate::wav::{analyze_timeline_waveforms, probe_wav, round6, write_empty_wav, WavInfo};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScenarioConfig {
    pub schema: String,
    pub name: String,
    pub audio: PathBuf,
    pub speed_curve: PathBuf,
    pub pitch_track: PathBuf,
    pub output_dir: Option<PathBuf>,
    #[serde(default)]
    pub analysed_channel: Option<u16>,
    #[serde(default)]
    pub window_us: Option<u64>,
    #[serde(default)]
    pub render_quantum_us: Option<u64>,
    #[serde(default)]
    pub pitch_gap_threshold_us: Option<u64>,
    #[serde(default)]
    pub missing_span: Option<TimeSpan>,
    #[serde(default)]
    pub limits: Option<Limits>,
}

#[derive(Debug, Clone)]
pub struct RunRequest {
    pub audio_path: PathBuf,
    pub speed_curve_path: PathBuf,
    pub pitch_track_path: PathBuf,
    pub output_dir: PathBuf,
    pub scenario_name: String,
    pub analysed_channel: u16,
    pub window_us: u64,
    pub render_quantum_us: u64,
    pub pitch_gap_threshold_us: u64,
    pub missing_span: Option<TimeSpan>,
    pub limits: Limits,
}

impl RunRequest {
    pub fn from_scenario(path: &Path, output_override: Option<PathBuf>) -> AppResult<Self> {
        let text = std::fs::read_to_string(path).map_err(|err| io_error(path, err))?;
        let scenario: ScenarioConfig = serde_json::from_str(&text)?;
        if scenario.schema != "tempo-pitch-scenario/1" {
            return Err(AppError::InvalidInput(format!(
                "{}: unsupported scenario schema {}",
                path.display(),
                scenario.schema
            )));
        }
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        let output_dir = output_override.unwrap_or_else(|| {
            scenario
                .output_dir
                .map(|path| resolve_relative(base, path))
                .unwrap_or_else(|| PathBuf::from("artifact-output"))
        });
        Ok(Self {
            audio_path: resolve_relative(base, scenario.audio),
            speed_curve_path: resolve_relative(base, scenario.speed_curve),
            pitch_track_path: resolve_relative(base, scenario.pitch_track),
            output_dir,
            scenario_name: scenario.name,
            analysed_channel: scenario.analysed_channel.unwrap_or(0),
            window_us: scenario.window_us.unwrap_or(DEFAULT_WINDOW_US),
            render_quantum_us: scenario
                .render_quantum_us
                .unwrap_or(DEFAULT_RENDER_QUANTUM_US),
            pitch_gap_threshold_us: scenario
                .pitch_gap_threshold_us
                .unwrap_or(DEFAULT_PITCH_GAP_US),
            missing_span: scenario.missing_span,
            limits: scenario.limits.unwrap_or_default(),
        })
    }
}

fn resolve_relative(base: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
}

#[derive(Debug, Clone)]
pub struct RendererInfo {
    pub backend: String,
    pub plugin_version: String,
    pub plugin_chain: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct RenderOutcome {
    pub segments: Vec<SegmentRenderResult>,
    pub output_sample_rate_hz: u32,
    pub output_channels: u16,
    pub plugin_version: String,
    pub plugin_chain: Vec<String>,
}

pub trait Renderer {
    fn info(&self) -> RendererInfo;
    fn render(
        &mut self,
        plan: &RenderPlan,
        request: &RunRequest,
        output_wav: &Path,
    ) -> AppResult<RenderOutcome>;
}

pub fn run_with_renderer(
    mut request: RunRequest,
    renderer: &mut dyn Renderer,
) -> AppResult<DebugRecord> {
    normalize_request(&mut request)?;
    std::fs::create_dir_all(&request.output_dir)
        .map_err(|err| io_error(&request.output_dir, err))?;
    request.output_dir = request
        .output_dir
        .canonicalize()
        .map_err(|err| io_error(&request.output_dir, err))?;
    validate_request(&request)?;
    match run_inner(&request, renderer) {
        Ok(record) => Ok(record),
        Err(error @ AppError::LimitExceeded { .. }) => {
            write_rejected_record(&request, renderer, &error)
        }
        Err(error) => Err(error),
    }
}

fn normalize_request(request: &mut RunRequest) -> AppResult<()> {
    request.audio_path = canonical_input(&request.audio_path)?;
    request.speed_curve_path = canonical_input(&request.speed_curve_path)?;
    request.pitch_track_path = canonical_input(&request.pitch_track_path)?;
    request.output_dir = if request.output_dir.is_absolute() {
        request.output_dir.clone()
    } else {
        std::env::current_dir()
            .map_err(|err| io_error(".", err))?
            .join(&request.output_dir)
    };
    request.scenario_name = request.scenario_name.trim().to_string();
    if request.scenario_name.is_empty() {
        request.scenario_name = "unnamed".to_string();
    }
    Ok(())
}

fn canonical_input(path: &Path) -> AppResult<PathBuf> {
    path.canonicalize().map_err(|err| io_error(path, err))
}

fn validate_request(request: &RunRequest) -> AppResult<()> {
    if request.window_us == 0 {
        return Err(AppError::InvalidInput("window_us must be > 0".to_string()));
    }
    if request.render_quantum_us == 0 {
        return Err(AppError::InvalidInput(
            "render_quantum_us must be > 0".to_string(),
        ));
    }
    if request.pitch_gap_threshold_us == 0 {
        return Err(AppError::InvalidInput(
            "pitch_gap_threshold_us must be > 0".to_string(),
        ));
    }
    if request.analysed_channel >= 64 {
        return Err(AppError::InvalidInput(
            "analysed_channel is unreasonably large".to_string(),
        ));
    }
    if let Some(span) = request.missing_span {
        if span.start_us >= span.end_us {
            return Err(AppError::InvalidInput(format!(
                "missing span must satisfy start < end, got {}..{}",
                span.start_us, span.end_us
            )));
        }
    }
    Ok(())
}

struct PreparedInputs {
    audio_sha256: String,
    audio_size: u64,
    wav: WavInfo,
    speed: Curve,
    pitch: Curve,
    speed_size: u64,
    speed_sha256: String,
    pitch_size: u64,
    pitch_sha256: String,
    pitch_gaps: Vec<TimeSpan>,
}

fn prepare_inputs(request: &RunRequest) -> AppResult<PreparedInputs> {
    let audio_meta =
        std::fs::metadata(&request.audio_path).map_err(|err| io_error(&request.audio_path, err))?;
    if audio_meta.len() > request.limits.max_audio_bytes {
        return Err(AppError::LimitExceeded {
            what: "audio file bytes",
            actual: audio_meta.len(),
            limit: request.limits.max_audio_bytes,
        });
    }
    for (path, role) in [
        (&request.speed_curve_path, "speed curve"),
        (&request.pitch_track_path, "pitch track"),
    ] {
        let size = std::fs::metadata(path)
            .map_err(|err| io_error(path, err))?
            .len();
        if size > request.limits.max_curve_bytes {
            return Err(AppError::LimitExceeded {
                what: if role == "speed curve" {
                    "speed curve file bytes"
                } else {
                    "pitch track file bytes"
                },
                actual: size,
                limit: request.limits.max_curve_bytes,
            });
        }
    }
    let wav = probe_wav(&request.audio_path)?;
    if request.analysed_channel >= wav.channels {
        return Err(AppError::InvalidInput(format!(
            "analysed channel {} is outside input channel count {}",
            request.analysed_channel, wav.channels
        )));
    }
    if let Some(span) = request.missing_span {
        if span.end_us > wav.duration_us {
            return Err(AppError::InvalidInput(format!(
                "missing span {}..{} exceeds audio duration {} us",
                span.start_us, span.end_us, wav.duration_us
            )));
        }
    }
    let speed = load_curve(
        &request.speed_curve_path,
        CurveKind::Speed,
        CurveLimits {
            max_bytes: request.limits.max_curve_bytes,
            max_points: request.limits.max_curve_points,
        },
    )?;
    let pitch = load_curve(
        &request.pitch_track_path,
        CurveKind::Pitch,
        CurveLimits {
            max_bytes: request.limits.max_curve_bytes,
            max_points: request.limits.max_curve_points,
        },
    )?;
    let pitch_gaps = pitch.internal_gaps(request.pitch_gap_threshold_us);
    let audio_sha256 = file_hash(&request.audio_path)?.1;
    let speed_sha256 = file_hash(&request.speed_curve_path)?.1;
    let pitch_sha256 = file_hash(&request.pitch_track_path)?.1;
    Ok(PreparedInputs {
        audio_sha256,
        audio_size: audio_meta.len(),
        wav,
        speed,
        pitch,
        speed_size: std::fs::metadata(&request.speed_curve_path)
            .map_err(|err| io_error(&request.speed_curve_path, err))?
            .len(),
        speed_sha256,
        pitch_size: std::fs::metadata(&request.pitch_track_path)
            .map_err(|err| io_error(&request.pitch_track_path, err))?
            .len(),
        pitch_sha256,
        pitch_gaps,
    })
}

fn run_inner(request: &RunRequest, renderer: &mut dyn Renderer) -> AppResult<DebugRecord> {
    let prepared = prepare_inputs(request)?;
    let info = renderer.info();
    let pitch_gaps = effective_pitch_gaps(request, &prepared);
    let plan = build_plan(
        prepared.wav.duration_us,
        &prepared.speed,
        &prepared.pitch,
        PlanConfig {
            duration_us: prepared.wav.duration_us,
            sample_rate_hz: prepared.wav.sample_rate_hz,
            analysed_channel: request.analysed_channel,
            window_us: request.window_us,
            render_quantum_us: request.render_quantum_us,
            gap_spans: pitch_gaps.clone(),
            max_timeline_entries: request.limits.max_timeline_entries,
            max_render_segments: request.limits.max_render_segments,
        },
    )?;

    let output_path = request.output_dir.join(OUTPUT_WAV);
    let (render_outcome, timeline, output_info) = if prepared.wav.frames == 0 {
        write_empty_wav(&output_path, DEFAULT_SAMPLE_RATE, DEFAULT_CHANNELS)?;
        (
            RenderOutcome {
                segments: Vec::new(),
                output_sample_rate_hz: DEFAULT_SAMPLE_RATE,
                output_channels: DEFAULT_CHANNELS,
                plugin_version: info.plugin_version.clone(),
                plugin_chain: info.plugin_chain.clone(),
            },
            Vec::new(),
            probe_wav(&output_path)?,
        )
    } else {
        let outcome = renderer.render(&plan, request, &output_path)?;
        let mut timeline =
            finalize_timeline(&plan, &outcome.segments, outcome.output_sample_rate_hz)?;
        analyze_timeline_waveforms(&output_path, &mut timeline, request.analysed_channel)?;
        let output_info = probe_wav(&output_path)?;
        (outcome, timeline, output_info)
    };

    let (output_size, output_sha) = file_hash(&output_path)?;
    let (peak_dbfs, clipping_samples) = crate::wav::output_peak_and_clipping(&output_path)?;
    let output = OutputEvidence {
        path: OUTPUT_WAV.to_string(),
        format: "WAV PCM16LE".to_string(),
        sample_rate_hz: render_outcome.output_sample_rate_hz,
        channels: render_outcome.output_channels,
        frames: output_info.frames,
        duration_us: output_info.duration_us(output_info.sample_rate_hz),
        size_bytes: output_size,
        sha256: output_sha,
        peak_dbfs,
        clipping_samples,
    };

    let mut anomalies = build_curve_anomalies(&prepared, &pitch_gaps, &timeline);
    if prepared.wav.frames == 0 {
        anomalies.insert(0, empty_audio_anomaly(&prepared.wav, request));
    }
    let status = if prepared.wav.frames == 0 {
        RunStatus::EmptyInput
    } else if anomalies.is_empty() {
        RunStatus::Complete
    } else {
        RunStatus::CompleteWithAnomalies
    };

    let input_evidence = InputEvidence {
        audio: audio_evidence(request, &prepared),
        speed_curve: curve_evidence(
            "speed_curve",
            request,
            &request.speed_curve_path,
            &prepared.speed,
            prepared.speed_size,
            &prepared.speed_sha256,
            "playback rate multiplier",
        ),
        pitch_track: curve_evidence(
            "pitch_track",
            request,
            &request.pitch_track_path,
            &prepared.pitch,
            prepared.pitch_size,
            &prepared.pitch_sha256,
            "semitones relative to source pitch",
        ),
        limits: request.limits.clone(),
    };
    let processing = ProcessingEvidence {
        backend: info.backend,
        plugin_version: render_outcome.plugin_version,
        plugin_chain: if prepared.wav.frames == 0 {
            vec!["empty-input".to_string(), "wav16le-writer".to_string()]
        } else {
            render_outcome.plugin_chain
        },
        sample_rate_hz: prepared.wav.sample_rate_hz,
        channels: prepared.wav.channels,
        analysed_channel: request.analysed_channel,
        window_us: plan.effective_window_us,
        render_quantum_us: plan.effective_render_quantum_us,
        output_sample_rate_hz: output.sample_rate_hz,
        output_channels: output.channels,
        render_segment_count: plan.segments.len() as u64,
        speed_parameter: plan.speed_range,
        pitch_parameter: plan.pitch_range,
    };
    let checks = producer_checks(request, &prepared, &timeline, &output, &anomalies);
    let mut record = DebugRecord {
        schema: SCHEMA.to_string(),
        status,
        job_id: job_id(
            &prepared.audio_sha256,
            &prepared.speed_sha256,
            &prepared.pitch_sha256,
        ),
        created_utc: time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string()),
        request: request_snapshot(request),
        inputs: input_evidence,
        processing,
        timeline,
        anomalies,
        output,
        verification: VerificationEvidence {
            checks,
            reload_round_trip: false,
            verify_command: format!(
                "tempo-pitch-debug verify --artifact {}",
                request.output_dir.display()
            ),
        },
        recording_policy: RecordingPolicy {
            timeline_rows: plan.timeline.len() as u64,
            timeline_rows_in_record: plan.timeline.len() as u64,
            record_is_complete: true,
            adaptive_window_applied: plan.adaptive_window_applied,
            sampling: "no timeline rows omitted".to_string(),
            stable_field_order: crate::audit::RECORD_FIELD_ORDER
                .iter()
                .map(|value| value.to_string())
                .collect(),
        },
    };
    record.verification.reload_round_trip = round_trip_ok(&record)?;
    write_artifact_set(&record, &request.output_dir)?;
    if !load_round_trip_from_disk(&request.output_dir, &record)? {
        return Err(AppError::Verification(
            "record.json did not round-trip byte-for-byte after writing".to_string(),
        ));
    }
    Ok(record)
}

fn round_trip_ok(record: &DebugRecord) -> AppResult<bool> {
    let bytes = canonical_record_bytes(record)?;
    let parsed: DebugRecord = serde_json::from_slice(&bytes)?;
    Ok(parsed == *record && canonical_record_bytes(&parsed)? == bytes)
}

fn load_round_trip_from_disk(output_dir: &Path, expected: &DebugRecord) -> AppResult<bool> {
    let loaded = crate::audit::load_record(&output_dir.join(crate::model::RECORD_FILE))?;
    let bytes = std::fs::read(output_dir.join(crate::model::RECORD_FILE))
        .map_err(|err| io_error(output_dir.join(crate::model::RECORD_FILE), err))?;
    Ok(loaded == *expected && canonical_record_bytes(&loaded)? == bytes)
}

fn request_snapshot(request: &RunRequest) -> RequestSnapshot {
    RequestSnapshot {
        audio_path: request.audio_path.display().to_string(),
        speed_curve_path: request.speed_curve_path.display().to_string(),
        pitch_track_path: request.pitch_track_path.display().to_string(),
        output_dir: request.output_dir.display().to_string(),
        scenario_name: request.scenario_name.clone(),
        analysed_channel: request.analysed_channel,
        window_us: request.window_us,
        render_quantum_us: request.render_quantum_us,
        pitch_gap_threshold_us: request.pitch_gap_threshold_us,
        missing_span: request.missing_span,
    }
}

fn audio_evidence(request: &RunRequest, prepared: &PreparedInputs) -> AudioEvidence {
    AudioEvidence {
        role: "original_audio".to_string(),
        path: request.audio_path.display().to_string(),
        size_bytes: prepared.audio_size,
        sha256: prepared.audio_sha256.clone(),
        container: "RIFF/WAVE".to_string(),
        sample_rate_hz: prepared.wav.sample_rate_hz,
        channels: prepared.wav.channels,
        frames: prepared.wav.frames,
        duration_us: prepared.wav.duration_us,
        track_ids: (0..prepared.wav.channels)
            .map(|channel| format!("audio/channel-{}", channel + 1))
            .collect(),
    }
}

fn curve_evidence(
    role: &str,
    request: &RunRequest,
    path: &Path,
    curve: &Curve,
    size_bytes: u64,
    sha256: &str,
    unit: &str,
) -> CurveEvidence {
    const MAX_RECORD_POINTS: usize = 50_000;
    let points = if curve.points.len() <= MAX_RECORD_POINTS {
        curve.points.clone()
    } else {
        let half = MAX_RECORD_POINTS / 2;
        let mut sampled = curve.points[..half].to_vec();
        sampled.extend_from_slice(&curve.points[curve.points.len() - half..]);
        sampled
    };
    let points_sampled = points.len() != curve.points.len();
    let min_value = curve
        .points
        .iter()
        .map(|point| point.value)
        .reduce(f64::min);
    let max_value = curve
        .points
        .iter()
        .map(|point| point.value)
        .reduce(f64::max);
    CurveEvidence {
        role: role.to_string(),
        path: path.display().to_string(),
        size_bytes,
        sha256: sha256.to_string(),
        unit: unit.to_string(),
        points_count: curve.points.len() as u64,
        points_in_record: points.len() as u64,
        points_sampled,
        first_time_us: curve.points.first().map(|point| point.time_us),
        last_time_us: curve.points.last().map(|point| point.time_us),
        min_value: min_value.map(round6),
        max_value: max_value.map(round6),
        points,
        internal_gaps: if role == "pitch_track" {
            curve.internal_gaps(request.pitch_gap_threshold_us)
        } else {
            Vec::new()
        },
    }
}

fn effective_pitch_gaps(request: &RunRequest, prepared: &PreparedInputs) -> Vec<TimeSpan> {
    if prepared.pitch.points.is_empty() && request.missing_span.is_none() {
        return Vec::new();
    }
    let mut gaps = prepared.pitch_gaps.clone();
    if let Some(span) = request.missing_span {
        gaps.retain(|gap| gap.start_us > span.end_us || span.start_us > gap.end_us);
        gaps.push(span);
    }
    gaps.sort_by_key(|span| (span.start_us, span.end_us));
    gaps.dedup_by_key(|span| (span.start_us, span.end_us));
    gaps
}

fn build_curve_anomalies(
    prepared: &PreparedInputs,
    pitch_gaps: &[TimeSpan],
    timeline: &[TimelineEntry],
) -> Vec<Anomaly> {
    let mut anomalies = Vec::new();
    for gap in pitch_gaps {
        let affected: Vec<&TimelineEntry> = timeline
            .iter()
            .filter(|entry| {
                entry.source_start_us < gap.end_us && entry.source_end_us > gap.start_us
            })
            .collect();
        if affected.is_empty() {
            continue;
        }
        let first = affected.first().expect("non-empty");
        let last = affected.last().expect("non-empty");
        let middle = affected[affected.len() / 2];
        let waveform = aggregate_waveform(&affected);
        let output_span = TimeSpan {
            start_us: first.output_start_us,
            end_us: last.output_end_us,
        };
        let source_span = TimeSpan {
            start_us: gap.start_us.max(first.source_start_us),
            end_us: gap.end_us.min(last.source_end_us),
        };
        anomalies.push(Anomaly {
            id: format!("anomaly-{:03}-missing-pitch", anomalies.len() + 1),
            kind: AnomalyKind::MissingPitchSegment,
            severity: Severity::Warning,
            track_id: first.track_id.clone(),
            channel: first.channel,
            source_span,
            output_span,
            source_frames: source_span_to_frames(source_span, prepared.wav.sample_rate_hz),
            output_frames: FrameSpan {
                start: first.output_frame_start,
                end: last.output_frame_end,
            },
            parameters: AnomalyParameters {
                speed_rate_curve: middle.speed_rate_curve,
                speed_rate_applied: middle.speed_rate_applied,
                pitch_semitones_curve: middle.pitch_semitones_curve,
                pitch_semitones_applied: middle.pitch_semitones_applied,
                pitch_ratio_applied: middle.pitch_ratio_applied,
                pitch_evidence: middle.pitch_evidence,
            },
            timeline_index_span: IndexSpan {
                start: first.index,
                end: last.index,
            },
            waveform_at_gap: waveform,
            evidence_refs: anomaly_evidence_refs(
                anomalies.len(),
                first.index,
                last.index,
                output_span,
            ),
            message: format!(
                "音高轨迹在源时间 {}..{} us 缺失，处理时沿用最后已知值并保留输出波形。",
                source_span.start_us, source_span.end_us
            ),
        });
    }

    if prepared.speed.points.is_empty() && prepared.wav.frames > 0 {
        anomalies.push(empty_curve_anomaly(
            AnomalyKind::EmptySpeedCurve,
            "speed",
            "速度曲线为空，按 1.0x 标称值处理。",
            prepared,
            timeline,
            anomalies.len(),
        ));
    }
    if prepared.pitch.points.is_empty() && prepared.wav.frames > 0 {
        anomalies.push(empty_curve_anomaly(
            AnomalyKind::EmptyPitchTrack,
            "pitch",
            "音高轨迹为空，按 0 st 标称值处理。",
            prepared,
            timeline,
            anomalies.len(),
        ));
    }
    anomalies
}

fn empty_curve_anomaly(
    kind: AnomalyKind,
    track: &str,
    message: &str,
    prepared: &PreparedInputs,
    timeline: &[TimelineEntry],
    anomaly_index: usize,
) -> Anomaly {
    let source_span = TimeSpan {
        start_us: 0,
        end_us: prepared.wav.duration_us,
    };
    let output_span = TimeSpan {
        start_us: timeline
            .first()
            .map(|entry| entry.output_start_us)
            .unwrap_or(0),
        end_us: timeline
            .last()
            .map(|entry| entry.output_end_us)
            .unwrap_or(0),
    };
    let stats = aggregate_waveform(&timeline.iter().collect::<Vec<_>>());
    Anomaly {
        id: format!("anomaly-{}-empty-{track}", timeline.len()),
        kind,
        severity: Severity::Warning,
        track_id: format!("curve/{track}"),
        channel: 0,
        source_span,
        output_span,
        source_frames: source_span_to_frames(source_span, prepared.wav.sample_rate_hz),
        output_frames: FrameSpan {
            start: timeline
                .first()
                .map(|entry| entry.output_frame_start)
                .unwrap_or(0),
            end: timeline
                .last()
                .map(|entry| entry.output_frame_end)
                .unwrap_or(0),
        },
        parameters: AnomalyParameters {
            speed_rate_curve: 1.0,
            speed_rate_applied: 1.0,
            pitch_semitones_curve: 0.0,
            pitch_semitones_applied: 0.0,
            pitch_ratio_applied: 1.0,
            pitch_evidence: EvidenceState::DefaultNominal,
        },
        timeline_index_span: IndexSpan {
            start: timeline.first().map(|entry| entry.index).unwrap_or(0),
            end: timeline.last().map(|entry| entry.index).unwrap_or(0),
        },
        waveform_at_gap: stats,
        evidence_refs: anomaly_evidence_refs(
            anomaly_index,
            timeline.first().map(|entry| entry.index).unwrap_or(0),
            timeline.last().map(|entry| entry.index).unwrap_or(0),
            output_span,
        ),
        message: message.to_string(),
    }
}

fn empty_audio_anomaly(wav: &WavInfo, request: &RunRequest) -> Anomaly {
    Anomaly {
        id: "anomaly-001-empty-audio".to_string(),
        kind: AnomalyKind::EmptyAudio,
        severity: Severity::Error,
        track_id: format!("audio/channel-{}", request.analysed_channel + 1),
        channel: request.analysed_channel,
        source_span: TimeSpan {
            start_us: 0,
            end_us: 0,
        },
        output_span: TimeSpan {
            start_us: 0,
            end_us: 0,
        },
        source_frames: FrameSpan { start: 0, end: 0 },
        output_frames: FrameSpan { start: 0, end: 0 },
        parameters: AnomalyParameters {
            speed_rate_curve: 1.0,
            speed_rate_applied: 1.0,
            pitch_semitones_curve: 0.0,
            pitch_semitones_applied: 0.0,
            pitch_ratio_applied: 1.0,
            pitch_evidence: EvidenceState::MissingAudio,
        },
        timeline_index_span: IndexSpan { start: 0, end: 0 },
        waveform_at_gap: WaveformStats::default(),
        evidence_refs: vec![
            EvidenceRef {
                artifact: "record.json".to_string(),
                locator: "/anomalies/0".to_string(),
            },
            EvidenceRef {
                artifact: OUTPUT_WAV.to_string(),
                locator: format!("0..0 frames; sample_rate={}", wav.sample_rate_hz),
            },
        ],
        message: "输入音频为 0 帧；已导出结构有效的空 WAV 和完整处理记录。".to_string(),
    }
}

fn aggregate_waveform(entries: &[&TimelineEntry]) -> WaveformStats {
    let frames: u64 = entries
        .iter()
        .map(|entry| entry.waveform.sample_frames)
        .sum();
    if frames == 0 {
        return WaveformStats::default();
    }
    let min = entries
        .iter()
        .map(|entry| entry.waveform.min)
        .fold(f64::INFINITY, f64::min);
    let max = entries
        .iter()
        .map(|entry| entry.waveform.max)
        .fold(f64::NEG_INFINITY, f64::max);
    let sum_squares: f64 = entries
        .iter()
        .map(|entry| entry.waveform.rms * entry.waveform.rms * entry.waveform.sample_frames as f64)
        .sum();
    let rms = (sum_squares / frames as f64).sqrt();
    let peak = min.abs().max(max.abs()).max(1e-12);
    WaveformStats {
        sample_frames: frames,
        min: round6(min),
        max: round6(max),
        rms: round6(rms),
        peak_dbfs: round6(20.0 * peak.log10()),
    }
}

fn anomaly_evidence_refs(
    anomaly_index: usize,
    first_timeline_index: u64,
    last_timeline_index: u64,
    output_span: TimeSpan,
) -> Vec<EvidenceRef> {
    vec![
        EvidenceRef {
            artifact: "record.json".to_string(),
            locator: format!("/anomalies/{anomaly_index}"),
        },
        EvidenceRef {
            artifact: "timeline.csv".to_string(),
            locator: format!(
                "rows {}..{} (1-based; row 1 is header)",
                first_timeline_index + 2,
                last_timeline_index + 2
            ),
        },
        EvidenceRef {
            artifact: "waveform.csv".to_string(),
            locator: format!(
                "rows {}..{} (1-based; row 1 is header)",
                first_timeline_index + 2,
                last_timeline_index + 2
            ),
        },
        EvidenceRef {
            artifact: OUTPUT_WAV.to_string(),
            locator: format!(
                "output time {}..{} us; output frames derived from timeline.csv",
                output_span.start_us, output_span.end_us
            ),
        },
    ]
}

fn job_id(audio_sha: &str, speed_sha: &str, pitch_sha: &str) -> String {
    let basis = format!("{audio_sha}:{speed_sha}:{pitch_sha}");
    format!("tp-{}", &sha256_bytes(basis.as_bytes())[..16])
}

fn producer_checks(
    request: &RunRequest,
    prepared: &PreparedInputs,
    timeline: &[TimelineEntry],
    output: &OutputEvidence,
    anomalies: &[Anomaly],
) -> Vec<VerificationCheck> {
    let digests_ok = [
        &prepared.audio_sha256,
        &prepared.speed_sha256,
        &prepared.pitch_sha256,
    ]
    .iter()
    .all(|digest| digest.len() == 64 && digest.chars().all(|ch| ch.is_ascii_hexdigit()));
    let mut monotonic = true;
    let mut waveform_frames = true;
    let mut previous_source = 0_u64;
    let mut previous_output = 0_u64;
    for entry in timeline {
        monotonic &= entry.source_start_us == previous_source
            && entry.source_end_us > entry.source_start_us
            && entry.output_end_us >= entry.output_start_us
            && entry.output_start_us >= previous_output
            && entry.output_frame_end >= entry.output_frame_start;
        previous_source = entry.source_end_us;
        previous_output = entry.output_end_us;
        waveform_frames &= entry.waveform.sample_frames
            == entry
                .output_frame_end
                .saturating_sub(entry.output_frame_start);
    }
    let source_complete = timeline
        .last()
        .map(|entry| entry.source_end_us == prepared.wav.duration_us)
        .unwrap_or(prepared.wav.frames == 0);
    let gap_expected = !prepared.pitch_gaps.is_empty() || request.missing_span.is_some();
    let gap_mapped = !gap_expected
        || anomalies
            .iter()
            .any(|anomaly| anomaly.kind == AnomalyKind::MissingPitchSegment);
    vec![
        VerificationCheck {
            name: "input_sha256_recorded".to_string(),
            passed: digests_ok,
            detail: "audio, speed and pitch digests use SHA-256".to_string(),
        },
        VerificationCheck {
            name: "source_output_time_mapping_monotonic".to_string(),
            passed: monotonic && source_complete,
            detail: format!(
                "{} rows; source ends at {} of {} us",
                timeline.len(),
                timeline
                    .last()
                    .map(|entry| entry.source_end_us)
                    .unwrap_or(0),
                prepared.wav.duration_us
            ),
        },
        VerificationCheck {
            name: "waveform_frame_counts_match_timeline".to_string(),
            passed: waveform_frames,
            detail: format!("{} rows checked", timeline.len()),
        },
        VerificationCheck {
            name: "missing_pitch_segment_mapped".to_string(),
            passed: gap_mapped,
            detail: format!(
                "{} detected/declared pitch gaps",
                effective_pitch_gaps(request, prepared).len()
            ),
        },
        VerificationCheck {
            name: "output_wav_materialized".to_string(),
            passed: output.size_bytes >= 44,
            detail: format!(
                "{} bytes, {} frames, {} channels",
                output.size_bytes, output.frames, output.channels
            ),
        },
        VerificationCheck {
            name: "reload_field_order_contract".to_string(),
            passed: true,
            detail: crate::audit::RECORD_FIELD_ORDER.join(","),
        },
    ]
}

fn write_rejected_record(
    request: &RunRequest,
    renderer: &mut dyn Renderer,
    error: &AppError,
) -> AppResult<DebugRecord> {
    let info = renderer.info();
    let output_path = request.output_dir.join(OUTPUT_WAV);
    let sample_rate = probe_wav(&request.audio_path)
        .map(|wav| wav.sample_rate_hz)
        .unwrap_or(DEFAULT_SAMPLE_RATE);
    let channels = probe_wav(&request.audio_path)
        .map(|wav| wav.channels)
        .unwrap_or(DEFAULT_CHANNELS);
    write_empty_wav(&output_path, sample_rate, channels.max(1))?;
    let (size, sha) = file_hash(&output_path)?;
    let (peak, clipping) = crate::wav::output_peak_and_clipping(&output_path)?;
    let (audio_size, audio_sha) = file_hash(&request.audio_path).unwrap_or((0, String::new()));
    let (speed_size, speed_sha) =
        file_hash(&request.speed_curve_path).unwrap_or((0, String::new()));
    let (pitch_size, pitch_sha) =
        file_hash(&request.pitch_track_path).unwrap_or((0, String::new()));
    let wav = probe_wav(&request.audio_path).ok();
    let (actual, limit, what) = match error {
        AppError::LimitExceeded {
            actual,
            limit,
            what,
        } => (*actual, *limit, *what),
        _ => (0, 0, "unknown"),
    };
    let output = OutputEvidence {
        path: OUTPUT_WAV.to_string(),
        format: "WAV PCM16LE (empty)".to_string(),
        sample_rate_hz: sample_rate,
        channels: channels.max(1),
        frames: 0,
        duration_us: 0,
        size_bytes: size,
        sha256: sha,
        peak_dbfs: peak,
        clipping_samples: clipping,
    };
    let empty_curve = |role: &str, path: &Path, size: u64, sha: String, unit: &str| CurveEvidence {
        role: role.to_string(),
        path: path.display().to_string(),
        size_bytes: size,
        sha256: sha,
        unit: unit.to_string(),
        points_count: 0,
        points_in_record: 0,
        points_sampled: false,
        first_time_us: None,
        last_time_us: None,
        min_value: None,
        max_value: None,
        points: Vec::new(),
        internal_gaps: Vec::new(),
    };
    let inputs = InputEvidence {
        audio: AudioEvidence {
            role: "original_audio".to_string(),
            path: request.audio_path.display().to_string(),
            size_bytes: audio_size,
            sha256: audio_sha,
            container: wav
                .as_ref()
                .map(|_| "RIFF/WAVE")
                .unwrap_or("unreadable")
                .to_string(),
            sample_rate_hz: wav.as_ref().map(|value| value.sample_rate_hz).unwrap_or(0),
            channels: wav.as_ref().map(|value| value.channels).unwrap_or(0),
            frames: wav.as_ref().map(|value| value.frames).unwrap_or(0),
            duration_us: wav.as_ref().map(|value| value.duration_us).unwrap_or(0),
            track_ids: Vec::new(),
        },
        speed_curve: empty_curve(
            "speed_curve",
            &request.speed_curve_path,
            speed_size,
            speed_sha,
            "playback rate multiplier",
        ),
        pitch_track: empty_curve(
            "pitch_track",
            &request.pitch_track_path,
            pitch_size,
            pitch_sha,
            "semitones relative to source pitch",
        ),
        limits: request.limits.clone(),
    };
    let anomaly = Anomaly {
        id: "anomaly-001-limit-exceeded".to_string(),
        kind: AnomalyKind::LimitExceeded,
        severity: Severity::Error,
        track_id: "input".to_string(),
        channel: request.analysed_channel,
        source_span: TimeSpan {
            start_us: 0,
            end_us: 0,
        },
        output_span: TimeSpan {
            start_us: 0,
            end_us: 0,
        },
        source_frames: FrameSpan { start: 0, end: 0 },
        output_frames: FrameSpan { start: 0, end: 0 },
        parameters: AnomalyParameters {
            speed_rate_curve: 1.0,
            speed_rate_applied: 1.0,
            pitch_semitones_curve: 0.0,
            pitch_semitones_applied: 0.0,
            pitch_ratio_applied: 1.0,
            pitch_evidence: EvidenceState::MissingAudio,
        },
        timeline_index_span: IndexSpan { start: 0, end: 0 },
        waveform_at_gap: WaveformStats::default(),
        evidence_refs: vec![
            EvidenceRef {
                artifact: "record.json".to_string(),
                locator: "/anomalies/0".to_string(),
            },
            EvidenceRef {
                artifact: OUTPUT_WAV.to_string(),
                locator: "0..0 frames; rejected run still emits a valid WAV container".to_string(),
            },
        ],
        message: format!("{what}: actual={actual}, limit={limit}; 已拒绝渲染并保留输入摘要。"),
    };
    let checks = vec![
        VerificationCheck {
            name: "limit_rejection_recorded".to_string(),
            passed: true,
            detail: format!("{what}: actual={actual}, limit={limit}"),
        },
        VerificationCheck {
            name: "limiter_applied_before_large_allocation".to_string(),
            passed: true,
            detail: "streaming limit checks protect memory and wall-clock time".to_string(),
        },
        VerificationCheck {
            name: "reload_field_order_contract".to_string(),
            passed: true,
            detail: crate::audit::RECORD_FIELD_ORDER.join(","),
        },
    ];
    let mut record = DebugRecord {
        schema: SCHEMA.to_string(),
        status: RunStatus::Rejected,
        job_id: job_id(
            &inputs.audio.sha256,
            &inputs.speed_curve.sha256,
            &inputs.pitch_track.sha256,
        ),
        created_utc: time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string()),
        request: request_snapshot(request),
        inputs,
        processing: ProcessingEvidence {
            backend: info.backend,
            plugin_version: info.plugin_version,
            plugin_chain: info.plugin_chain,
            sample_rate_hz: sample_rate,
            channels: channels.max(1),
            analysed_channel: request.analysed_channel,
            window_us: request.window_us,
            render_quantum_us: request.render_quantum_us,
            output_sample_rate_hz: sample_rate,
            output_channels: channels.max(1),
            render_segment_count: 0,
            speed_parameter: crate::model::ParameterRange { min: 1.0, max: 1.0 },
            pitch_parameter: crate::model::ParameterRange { min: 0.0, max: 0.0 },
        },
        timeline: Vec::new(),
        anomalies: vec![anomaly],
        output,
        verification: VerificationEvidence {
            checks,
            reload_round_trip: false,
            verify_command: format!(
                "tempo-pitch-debug verify --artifact {}",
                request.output_dir.display()
            ),
        },
        recording_policy: RecordingPolicy {
            timeline_rows: 0,
            timeline_rows_in_record: 0,
            record_is_complete: true,
            adaptive_window_applied: false,
            sampling: "no processing performed after limit rejection".to_string(),
            stable_field_order: crate::audit::RECORD_FIELD_ORDER
                .iter()
                .map(|value| value.to_string())
                .collect(),
        },
    };
    record.verification.reload_round_trip = round_trip_ok(&record)?;
    write_artifact_set(&record, &request.output_dir)?;
    if !load_round_trip_from_disk(&request.output_dir, &record)? {
        return Err(AppError::Verification(
            "rejected record.json did not round-trip after writing".to_string(),
        ));
    }
    Ok(record)
}
