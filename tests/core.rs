use std::fs;
use std::path::{Path, PathBuf};

use tempo_pitch_debug::audit::{canonical_record_bytes, load_record, verify_artifact};
use tempo_pitch_debug::engine::{
    run_with_renderer, RenderOutcome, Renderer, RendererInfo, RunRequest,
};
use tempo_pitch_debug::model::{AnomalyKind, Limits, RunStatus, TimeSpan, DEFAULT_PITCH_GAP_US};
use tempo_pitch_debug::plan::{us_to_frames, RenderPlan, SegmentRenderResult};
use tempo_pitch_debug::wav::{write_empty_wav, Pcm16Writer};

struct FakeRenderer;

impl Renderer for FakeRenderer {
    fn info(&self) -> RendererInfo {
        RendererInfo {
            backend: "test-double".to_string(),
            plugin_version: "test/1".to_string(),
            plugin_chain: vec!["fake-tempo".to_string(), "fake-pitch".to_string()],
        }
    }

    fn render(
        &mut self,
        plan: &RenderPlan,
        _request: &RunRequest,
        output_wav: &Path,
    ) -> Result<RenderOutcome, tempo_pitch_debug::AppError> {
        let channels = 2_u16;
        let mut writer = Pcm16Writer::create(output_wav, plan.sample_rate_hz, channels)?;
        let mut segments = Vec::new();
        for segment in &plan.segments {
            let source_frames = us_to_frames(
                segment.source_end_us - segment.source_start_us,
                plan.sample_rate_hz,
            );
            let output_frames = (source_frames as f64 / segment.speed_rate).round() as u64;
            let mut samples = Vec::with_capacity(output_frames as usize * channels as usize);
            for frame in 0..output_frames {
                let phase = frame as f64 / plan.sample_rate_hz as f64;
                let value = (0.2 * (2.0 * std::f64::consts::PI * 220.0 * phase).sin()) as f32;
                samples.push(value);
                samples.push(-value);
            }
            writer.write_f32_interleaved(&samples)?;
            segments.push(SegmentRenderResult {
                index: segment.index,
                output_frames,
            });
        }
        writer.finish(output_wav)?;
        Ok(RenderOutcome {
            segments,
            output_sample_rate_hz: plan.sample_rate_hz,
            output_channels: channels,
            plugin_version: "test/1".to_string(),
            plugin_chain: vec!["fake-tempo".to_string(), "fake-pitch".to_string()],
        })
    }
}

#[test]
fn missing_segment_maps_source_frames_params_and_waveform() {
    let dir = tempfile::tempdir().unwrap();
    let paths = write_scene(dir.path(), 1_000_000, false);
    let request = request(paths, 800_000, 1_000_000);
    let record = run_with_renderer(request, &mut FakeRenderer).unwrap();

    assert_eq!(record.status, RunStatus::CompleteWithAnomalies);
    assert_eq!(record.anomalies.len(), 1);
    let anomaly = &record.anomalies[0];
    assert_eq!(anomaly.kind, AnomalyKind::MissingPitchSegment);
    assert_eq!(anomaly.source_span.start_us, 800_000);
    assert_eq!(anomaly.source_span.end_us, 1_000_000);
    assert_eq!(
        anomaly.source_frames.end - anomaly.source_frames.start,
        9_600
    );
    assert!(anomaly.output_span.end_us > anomaly.output_span.start_us);
    assert!(anomaly.waveform_at_gap.sample_frames > 0);
    assert!(anomaly.parameters.speed_rate_applied > 0.0);
    assert!(!anomaly.evidence_refs.is_empty());

    let gap_rows: Vec<_> = record
        .timeline
        .iter()
        .filter(|entry| {
            entry.source_start_us < anomaly.source_span.end_us
                && entry.source_end_us > anomaly.source_span.start_us
        })
        .collect();
    assert!(!gap_rows.is_empty());
    for entry in gap_rows {
        assert_eq!(
            entry.pitch_evidence,
            tempo_pitch_debug::model::EvidenceState::HeldLastKnown
        );
        assert!(entry.waveform.sample_frames > 0);
        assert_eq!(
            entry.output_frame_end - entry.output_frame_start,
            entry.waveform.sample_frames
        );
    }

    let artifact = PathBuf::from(&record.request.output_dir);
    let stored = fs::read(artifact.join("record.json")).unwrap();
    let loaded = load_record(&artifact.join("record.json")).unwrap();
    assert_eq!(loaded, record);
    assert_eq!(canonical_record_bytes(&loaded).unwrap(), stored);
    assert!(verify_artifact(&artifact).unwrap().passed());
}

#[test]
fn empty_audio_is_a_valid_auditable_artifact() {
    let dir = tempfile::tempdir().unwrap();
    let audio = dir.path().join("empty.wav");
    write_empty_wav(&audio, 48_000, 2).unwrap();
    let speed = dir.path().join("speed.csv");
    let pitch = dir.path().join("pitch.csv");
    fs::write(&speed, "time_us,rate\n").unwrap();
    fs::write(&pitch, "time_us,semitones\n").unwrap();
    let output = dir.path().join("artifact-empty");
    let request = RunRequest {
        audio_path: audio,
        speed_curve_path: speed,
        pitch_track_path: pitch,
        output_dir: output.clone(),
        scenario_name: "empty".to_string(),
        analysed_channel: 0,
        window_us: 20_000,
        render_quantum_us: 100_000,
        pitch_gap_threshold_us: DEFAULT_PITCH_GAP_US,
        missing_span: None,
        limits: Limits::default(),
    };
    let record = run_with_renderer(request, &mut FakeRenderer).unwrap();
    assert_eq!(record.status, RunStatus::EmptyInput);
    assert_eq!(record.output.frames, 0);
    assert!(record.timeline.is_empty());
    assert!(record
        .anomalies
        .iter()
        .any(|anomaly| anomaly.kind == AnomalyKind::EmptyAudio));
    assert!(verify_artifact(&output).unwrap().passed());
}

#[test]
fn oversized_curve_is_rejected_without_large_allocation() {
    let dir = tempfile::tempdir().unwrap();
    let paths = write_scene(dir.path(), 500_000, false);
    let mut request = request(paths, 0, 0);
    request.missing_span = None;
    request.limits.max_curve_points = 3;
    let record = run_with_renderer(request, &mut FakeRenderer).unwrap();
    assert_eq!(record.status, RunStatus::Rejected);
    assert!(record
        .anomalies
        .iter()
        .any(|anomaly| anomaly.kind == AnomalyKind::LimitExceeded));
    let artifact = PathBuf::from(&record.request.output_dir);
    assert!(verify_artifact(&artifact).unwrap().passed());
}

#[test]
fn empty_curves_use_nominal_values_and_keep_anomaly_locations() {
    let dir = tempfile::tempdir().unwrap();
    let paths = write_scene(dir.path(), 300_000, true);
    fs::write(&paths.speed, "time_us,rate\n").unwrap();
    fs::write(&paths.pitch, "time_us,semitones\n").unwrap();
    let record = run_with_renderer(request(paths, 0, 0), &mut FakeRenderer).unwrap();
    assert_eq!(record.status, RunStatus::CompleteWithAnomalies);
    assert_eq!(record.anomalies.len(), 2);
    assert!(record
        .anomalies
        .iter()
        .any(|anomaly| anomaly.kind == AnomalyKind::EmptySpeedCurve));
    assert!(record
        .anomalies
        .iter()
        .any(|anomaly| anomaly.kind == AnomalyKind::EmptyPitchTrack));
    for (index, anomaly) in record.anomalies.iter().enumerate() {
        assert!(anomaly
            .evidence_refs
            .iter()
            .any(|reference| reference.locator == format!("/anomalies/{index}")));
    }
}

#[test]
fn large_timeline_adapts_window_before_hitting_limit() {
    let dir = tempfile::tempdir().unwrap();
    let audio = dir.path().join("long.wav");
    let mut writer = Pcm16Writer::create(&audio, 48_000, 2).unwrap();
    writer
        .write_f32_interleaved(&vec![0.0_f32; 4 * 48_000 * 2])
        .unwrap();
    writer.finish(&audio).unwrap();
    fs::write(
        dir.path().join("speed.csv"),
        "time_us,rate\n0,1.2\n2000000,0.9\n4000000,1.0\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("pitch.csv"),
        "time_us,semitones\n0,0\n2000000,0\n4000000,0\n",
    )
    .unwrap();
    let request = RunRequest {
        audio_path: audio,
        speed_curve_path: dir.path().join("speed.csv"),
        pitch_track_path: dir.path().join("pitch.csv"),
        output_dir: dir.path().join("artifact-long"),
        scenario_name: "large-timeline".to_string(),
        analysed_channel: 0,
        window_us: 20_000,
        render_quantum_us: 100_000,
        pitch_gap_threshold_us: 3_000_000,
        missing_span: None,
        limits: Limits {
            max_timeline_entries: 100,
            max_render_segments: 20,
            ..Limits::default()
        },
    };
    let record = run_with_renderer(request, &mut FakeRenderer).unwrap();
    assert!(record.timeline.len() <= 80);
    assert!(record.processing.window_us > 20_000);
    assert!(record.recording_policy.adaptive_window_applied);
}

#[test]
fn canonical_record_keeps_top_level_field_order() {
    let dir = tempfile::tempdir().unwrap();
    let paths = write_scene(dir.path(), 300_000, true);
    let record = run_with_renderer(request(paths, 0, 0), &mut FakeRenderer).unwrap();
    let bytes = canonical_record_bytes(&record).unwrap();
    let text = String::from_utf8(bytes).unwrap();
    let expected = [
        "\"schema\"",
        "\"status\"",
        "\"job_id\"",
        "\"created_utc\"",
        "\"request\"",
        "\"inputs\"",
        "\"processing\"",
        "\"timeline\"",
        "\"anomalies\"",
        "\"output\"",
        "\"verification\"",
        "\"recording_policy\"",
    ];
    let mut previous = 0;
    for field in expected {
        let position = text.find(field).unwrap();
        assert!(position >= previous, "{field} moved out of order");
        previous = position;
    }
}

struct ScenePaths {
    audio: PathBuf,
    speed: PathBuf,
    pitch: PathBuf,
    output: PathBuf,
}

fn write_scene(dir: &Path, duration_us: u64, no_gap: bool) -> ScenePaths {
    let audio = dir.join("original.wav");
    let frames = us_to_frames(duration_us, 48_000);
    let mut writer = Pcm16Writer::create(&audio, 48_000, 2).unwrap();
    let mut left = Vec::with_capacity((frames * 2) as usize);
    for frame in 0..frames {
        let value =
            (0.18 * (2.0 * std::f64::consts::PI * 220.0 * frame as f64 / 48_000.0).sin()) as f32;
        left.push(value);
        left.push(-value);
    }
    writer.write_f32_interleaved(&left).unwrap();
    writer.finish(&audio).unwrap();

    let speed = dir.join("speed.csv");
    fs::write(
        &speed,
        "time_us,rate\n0,1.0\n400000,1.25\n800000,0.9\n1000000,1.1\n",
    )
    .unwrap();
    let pitch = dir.join("pitch.csv");
    let mut rows = String::from("time_us,semitones\n");
    let mut time_us = 0;
    while time_us <= duration_us {
        let in_gap = !no_gap && time_us > 800_000 && time_us < 1_000_000;
        if !in_gap {
            rows.push_str(&format!("{time_us},0.0\n"));
        }
        time_us += 10_000;
    }
    fs::write(&pitch, rows).unwrap();
    ScenePaths {
        audio,
        speed,
        pitch,
        output: dir.join("artifact"),
    }
}

fn request(paths: ScenePaths, missing_start: u64, missing_end: u64) -> RunRequest {
    RunRequest {
        audio_path: paths.audio,
        speed_curve_path: paths.speed,
        pitch_track_path: paths.pitch,
        output_dir: paths.output,
        scenario_name: "test-scene".to_string(),
        analysed_channel: 0,
        window_us: 20_000,
        render_quantum_us: 100_000,
        pitch_gap_threshold_us: 50_000,
        missing_span: (missing_start < missing_end).then_some(TimeSpan {
            start_us: missing_start,
            end_us: missing_end,
        }),
        limits: Limits::default(),
    }
}
