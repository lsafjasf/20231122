use std::path::PathBuf;

use crate::failure::AnalysisFailure;
use crate::model::{
    extension_record, AutoConclusion, BlackInterval, DetectorParams, ReplayEvent, ReplayStatus,
    ReviewReport, SourceRecord, TrackKind, TrackRecord, Verdict, WaveSample, Waveform,
};
use crate::report::new_replay;

#[derive(Debug, Clone)]
pub struct AnalysisRequest {
    pub source: PathBuf,
    pub params: DetectorParams,
    pub recovered: bool,
    pub recovery_note: String,
    pub replayed_from_report: Option<String>,
    pub replay_number: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressStage {
    Probing,
    Decoding,
    Sampling,
    WritingEvidence,
    Complete,
    Failed,
}

#[derive(Debug, Clone)]
pub struct ProgressUpdate {
    pub stage: ProgressStage,
    pub fraction: f32,
    pub message: String,
    pub time_ms: u64,
    pub frame: u64,
}

#[derive(Debug, Clone)]
pub struct KeyFrame {
    pub id: String,
    pub interval_id: Option<String>,
    pub time_ms: u64,
    pub frame: u64,
    pub track_id: String,
    pub reason: String,
    pub width: u32,
    pub height: u32,
    pub gray: Vec<u8>,
}

impl KeyFrame {
    pub fn to_png(&self) -> Result<Vec<u8>, String> {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, self.width, self.height);
            encoder.set_color(png::ColorType::Grayscale);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder
                .write_header()
                .map_err(|error| format!("PNG header: {error}"))?;
            writer
                .write_image_data(&self.gray)
                .map_err(|error| format!("PNG data: {error}"))?;
        }
        Ok(bytes)
    }
}

#[derive(Debug, Clone)]
pub struct AnalysisOutput {
    pub report: ReviewReport,
    pub keyframes: Vec<KeyFrame>,
}

pub fn interval_keys<'a>(
    waveform: &'a Waveform,
    interval: &BlackInterval,
) -> (
    Option<&'a crate::model::WaveSample>,
    Option<&'a crate::model::WaveSample>,
) {
    let first = waveform
        .samples
        .iter()
        .find(|sample| sample.track_id == interval.track_id && sample.time_ms >= interval.start_ms);
    let darkest = waveform
        .samples
        .iter()
        .filter(|sample| {
            sample.track_id == interval.track_id
                && sample.time_ms >= interval.start_ms
                && sample.time_ms < interval.end_ms
        })
        .min_by(|left, right| left.y_mean.total_cmp(&right.y_mean));
    (first, darkest)
}

pub fn coverage_ratio(report: &ReviewReport) -> f32 {
    if report.waveform.covered_ms == 0 || report.detector.sample_fps == 0 {
        return 0.0;
    }
    let expected = report.waveform.covered_ms * report.detector.sample_fps as u64 / 1000;
    (report.waveform.samples.len() as f32 / expected.max(1) as f32).min(1.0)
}

pub fn failure_report(request: &AnalysisRequest, failure: &AnalysisFailure) -> ReviewReport {
    let source = match crate::report::source_record(&request.source) {
        Ok(source) => source,
        Err(_) => SourceRecord {
            path: request.source.display().to_string(),
            uri: format!("file://{}", request.source.display()),
            file_name: request
                .source
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("unavailable")
                .into(),
            extension: extension_record(&request.source),
            size_bytes: std::fs::metadata(&request.source)
                .map(|metadata| metadata.len())
                .unwrap_or(0),
            modified_utc: "unavailable".into(),
            sha256: "unavailable".into(),
            probe_caps: "probe not completed".into(),
        },
    };
    let mut replay = new_replay(
        request.recovered,
        &request.recovery_note,
        request.replayed_from_report.clone(),
    );
    replay.replay_number = request.replay_number.max(1);
    replay.status = ReplayStatus::Failed;
    replay.events.push(ReplayEvent {
        sequence: replay.events.len() as u64 + 1,
        at_ms: 0,
        frame: None,
        track_id: None,
        kind: "replay_failed".into(),
        detail: format!("{}: {}", failure.code, failure.detail),
        params_fingerprint: request.params.fingerprint(),
        waveform_sample: None,
        evidence_refs: Vec::new(),
    });
    let mut report = ReviewReport::new(source, replay, request.params.clone());
    report.auto_conclusion = AutoConclusion {
        verdict: Verdict::Failed,
        headline: failure.title.clone(),
        confidence: 0.0,
        reason_codes: vec![failure.code.clone()],
        limitations: vec![
            "自动结论失败；未执行人工确认。".into(),
            "失败报告仍保留源文件指纹、参数与事件，便于复核排障。".into(),
        ],
        analyzed_samples: 0,
        analyzed_ms: 0,
        intervals: Vec::new(),
    };
    report.follow_up_actions = failure.actions.clone();
    report.failure = Some(failure.to_record(false));
    report
}

pub fn demo_output() -> AnalysisOutput {
    let params = DetectorParams::default();
    let source = SourceRecord {
        path: "/recovered/2026-11-21/camera-A.recx".into(),
        uri: "file:///recovered/2026-11-21/camera-A.recx".into(),
        file_name: "camera-A.recx".into(),
        extension: crate::model::ExtensionRecord {
            raw_extension: Some("recx".into()),
            status: crate::model::ExtensionStatus::Unknown,
            recorded: true,
            content_probe: "video/x-raw, format=(string)NV12, width=(int)1920, height=(int)1080, framerate=(fraction)25/1".into(),
            policy: "unknown extension recorded; content probe is authoritative".into(),
        },
        size_bytes: 734_003_200,
        modified_utc: "2026-11-21T16:31:02.000Z".into(),
        sha256: "6f1c5d0a9e6f9b08a7f746cf4fca8d2c152f1c3cb3c2a73cc53b88e0d0411c2a".into(),
        probe_caps: "video/x-raw, format=(string)NV12, width=(int)1920, height=(int)1080, framerate=(fraction)25/1".into(),
    };
    let mut replay = new_replay(
        true,
        "上次会话在证据封存前中断；从恢复索引重建第 2 次重放。",
        Some("report-demo-previous".into()),
    );
    replay.replay_number = 2;
    replay.status = ReplayStatus::Completed;
    replay.selected_track = Some("video-0".into());
    replay.duration_ms = 24_000;
    let mut samples = Vec::new();
    for index in 0..120u64 {
        let time_ms = index * 200;
        let black = (4_600..=6_800).contains(&time_ms) || (15_200..=16_200).contains(&time_ms);
        let variation = ((index as f32 * 0.37).sin().abs() * 0.08) + 0.12;
        samples.push(WaveSample {
            sample: index,
            time_ms,
            pts_ns: time_ms * 1_000_000,
            frame: index,
            track_id: "video-0".into(),
            y_mean: if black { 0.003 } else { variation.min(0.32) },
            y_min: if black { 0 } else { 12 },
            y_max: if black { 3 } else { 210 },
            black_pixel_ratio: if black { 1.0 } else { 0.0 },
            is_black: black,
            params_fingerprint: params.fingerprint(),
        });
    }
    let mut waveform = crate::luma::waveform_from_samples(samples, 200);
    waveform.covered_ms = 24_000;
    let auto_conclusion = crate::luma::conclude(&waveform, &params, "video-0");
    let mut report = ReviewReport::new(source, replay, params.clone());
    report.tracks = vec![TrackRecord {
        id: "video-0".into(),
        index: 0,
        kind: TrackKind::Video,
        stream_id: Some("stream-0".into()),
        caps: "video/x-raw, format=NV12, 1920x1080@25".into(),
        source_width: Some(1920),
        source_height: Some(1080),
        source_fps: Some(25.0),
        frames_sampled: waveform.samples.len() as u64,
    }];
    report.waveform = waveform;
    report.auto_conclusion = auto_conclusion;
    report.replay.events = vec![
        demo_event(
            1,
            0,
            "recovery_replay_started",
            "从恢复索引开始第 2 次重放；保留上一会话引用。",
        ),
        demo_event(
            2,
            0,
            "source_hashed",
            "SHA-256 6f1c5d0a… 已与恢复索引比对。",
        ),
        demo_event(
            3,
            0,
            "unknown_extension_recorded",
            "未知扩展 `.recx` 已记录；decodebin 按内容识别为 NV12 视频。",
        ),
        demo_event(4, 0, "track_selected", "video-0 / NV12 1920x1080@25"),
        demo_event(
            5,
            0,
            "parameters_locked",
            "阈值 0.100，最短 500 ms，5 fps，320x180 GRAY8。",
        ),
        demo_event(
            6,
            24_000,
            "waveform_emitted",
            "120 个亮度采样已输出；周期 200 ms。",
        ),
        demo_event(
            7,
            24_000,
            "auto_conclusion_recorded",
            "发现 2 段黑场；等待人工确认。",
        ),
    ];
    let mut keyframes = Vec::new();
    for (index, interval) in report.auto_conclusion.intervals.iter().enumerate() {
        let gray = demo_frame(interval.start_frame, interval.start_ms, false);
        keyframes.push(KeyFrame {
            id: format!("{}-start", interval.id),
            interval_id: Some(interval.id.clone()),
            time_ms: interval.start_ms,
            frame: interval.start_frame,
            track_id: interval.track_id.clone(),
            reason: "黑场起点".into(),
            width: params.analysis_width,
            height: params.analysis_height,
            gray,
        });
        if index == 1 {
            keyframes.push(KeyFrame {
                id: format!("{}-darkest", interval.id),
                interval_id: Some(interval.id.clone()),
                time_ms: interval.start_ms + 600,
                frame: interval.start_frame + 3,
                track_id: interval.track_id.clone(),
                reason: "区间最暗帧 Y=0.003".into(),
                width: params.analysis_width,
                height: params.analysis_height,
                gray: demo_frame(interval.start_frame + 3, interval.start_ms + 600, false),
            });
        }
    }
    let normal = KeyFrame {
        id: "normal-preview".into(),
        interval_id: None,
        time_ms: 2_400,
        frame: 12,
        track_id: "video-0".into(),
        reason: "恢复后正常画面".into(),
        width: params.analysis_width,
        height: params.analysis_height,
        gray: demo_frame(12, 2_400, true),
    };
    keyframes.insert(0, normal);
    AnalysisOutput { report, keyframes }
}

fn demo_event(sequence: u64, at_ms: u64, kind: &str, detail: &str) -> ReplayEvent {
    ReplayEvent {
        sequence,
        at_ms,
        frame: None,
        track_id: Some("video-0".into()),
        kind: kind.into(),
        detail: detail.into(),
        params_fingerprint: DetectorParams::default().fingerprint(),
        waveform_sample: if at_ms > 0 { Some(119) } else { None },
        evidence_refs: Vec::new(),
    }
}

fn demo_frame(frame: u64, time_ms: u64, normal: bool) -> Vec<u8> {
    let width = 320usize;
    let height = 180usize;
    let mut gray = vec![0u8; width * height];
    if normal {
        for y in 0..height {
            for x in 0..width {
                let sky: u8 = if y < height / 2 { 72 } else { 36 };
                let horizon: u8 = if y.abs_diff(height / 2) < 3 { 160 } else { 0 };
                let object: u8 = if x > 85 && x < 230 && y > 65 && y < 140 {
                    110 + ((x + y) % 24) as u8
                } else {
                    0
                };
                gray[y * width + x] = sky.saturating_add(horizon).saturating_add(object);
            }
        }
    }
    let marker = (frame as usize * 7 + time_ms as usize / 100) % width;
    for y in 165..175 {
        gray[y * width + marker] = 255;
    }
    gray
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyframe_png_has_png_signature() {
        let frame = KeyFrame {
            id: "frame-test".into(),
            interval_id: Some("bf-0001".into()),
            time_ms: 0,
            frame: 0,
            track_id: "video-0".into(),
            reason: "test".into(),
            width: 2,
            height: 2,
            gray: vec![0, 0, 0, 255],
        };
        let png = frame.to_png().unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    }
}
