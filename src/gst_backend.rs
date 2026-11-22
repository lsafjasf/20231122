use std::fs;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc::Sender, Arc, Mutex};
use std::time::{Duration, Instant};

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use gstreamer_video as gst_video;
use gstreamer_video::VideoFrameExt;

use crate::analysis::{AnalysisOutput, AnalysisRequest, KeyFrame, ProgressStage, ProgressUpdate};
use crate::failure::AnalysisFailure;
use crate::luma::{conclude, gray8_stats, is_black, waveform_from_samples};
use crate::model::{
    DetectorParams, ExtensionStatus, ReplayEvent, ReplayStatus, ReviewReport, TrackKind,
    TrackRecord, WaveSample,
};
use crate::report::{new_replay, source_record};

#[derive(Debug, Clone)]
struct TrackSelection {
    caps: String,
    width: Option<u32>,
    height: Option<u32>,
    fps: Option<f64>,
}

#[derive(Debug, Clone)]
struct FrameRun {
    start_sample: u64,
    start_time_ms: u64,
    start_frame: u64,
    start_gray: Vec<u8>,
    last_time_ms: u64,
    darkest_sample: u64,
    darkest_time_ms: u64,
    darkest_frame: u64,
    darkest_y: f32,
    darkest_gray: Vec<u8>,
}

#[derive(Default)]
struct SamplerState {
    samples: Vec<WaveSample>,
    keyframes: Vec<KeyFrame>,
    current_run: Option<FrameRun>,
    completed_runs: usize,
    frame_count: u64,
}

pub fn analyze(
    request: &AnalysisRequest,
    cancel: Arc<AtomicBool>,
    progress: Sender<ProgressUpdate>,
) -> Result<AnalysisOutput, AnalysisFailure> {
    request
        .params
        .validate()
        .map_err(|detail| AnalysisFailure::new("invalid_params", "检测参数无效", detail))?;
    if !request.source.is_file() {
        return Err(AnalysisFailure::missing_source(
            &request.source.display().to_string(),
            "文件不存在或不是普通文件。",
        ));
    }
    fs::File::open(&request.source).map_err(|error| {
        if error.kind() == std::io::ErrorKind::PermissionDenied {
            AnalysisFailure::new("permission_denied", "没有读取权限", error.to_string())
        } else {
            AnalysisFailure::missing_source(
                &request.source.display().to_string(),
                error.to_string(),
            )
        }
    })?;

    send(
        &progress,
        ProgressStage::Probing,
        0.02,
        "校验源文件并计算 SHA-256",
        0,
        0,
    );
    let mut source = source_record(&request.source).map_err(|error| {
        AnalysisFailure::missing_source(&request.source.display().to_string(), error.to_string())
    })?;

    gst::init().map_err(|error| {
        AnalysisFailure::new(
            "gstreamer_init_failed",
            "GStreamer 初始化失败",
            error.to_string(),
        )
    })?;
    for element in [
        "filesrc",
        "decodebin",
        "queue",
        "videoconvert",
        "videoscale",
        "videorate",
        "capsfilter",
        "appsink",
    ] {
        if gst::ElementFactory::find(element).is_none() {
            return Err(AnalysisFailure::missing_decoder(format!(
                "缺少 GStreamer 元素 `{element}`；请安装 gstreamer1.0-plugins-base/good。"
            )));
        }
    }

    let pipeline = gst::Pipeline::new();
    let filesrc = make_element("filesrc")?;
    filesrc.set_property("location", request.source.display().to_string());
    let decoder = make_element("decodebin")?;
    let queue = make_element("queue")?;
    let convert = make_element("videoconvert")?;
    let scale = make_element("videoscale")?;
    let rate = make_element("videorate")?;
    let capsfilter = make_element("capsfilter")?;
    let caps = gst::Caps::builder("video/x-raw")
        .field("format", "GRAY8")
        .field("width", request.params.analysis_width as i32)
        .field("height", request.params.analysis_height as i32)
        .field(
            "framerate",
            gst::Fraction::new(request.params.sample_fps as i32, 1),
        )
        .build();
    capsfilter.set_property("caps", &caps);
    let appsink = gst_app::AppSink::builder()
        .max_buffers(2)
        .drop(true)
        .sync(false)
        .build();
    appsink.set_property("emit-signals", false);

    pipeline
        .add_many([
            &filesrc,
            &decoder,
            &queue,
            &convert,
            &scale,
            &rate,
            &capsfilter,
            appsink.upcast_ref(),
        ])
        .map_err(|error| pipeline_failure("无法组装 GStreamer 管线", error.to_string()))?;
    filesrc
        .link(&decoder)
        .map_err(|error| pipeline_failure("filesrc 无法连接 decodebin", error.to_string()))?;
    queue
        .link(&convert)
        .and_then(|_| convert.link(&scale))
        .and_then(|_| scale.link(&rate))
        .and_then(|_| rate.link(&capsfilter))
        .and_then(|_| capsfilter.link(&appsink))
        .map_err(|error| pipeline_failure("视频处理链无法连接", error.to_string()))?;

    let selection = Arc::new(Mutex::new(None::<TrackSelection>));
    let video_pads = Arc::new(AtomicU32::new(0));
    let queue_for_pad = queue.clone();
    let selection_for_pad = selection.clone();
    let pads_for_pad = video_pads.clone();
    let track_index = request.params.track_index;
    decoder.connect_pad_added(move |_decoder, pad| {
        let caps = pad.current_caps().or_else(|| Some(pad.query_caps(None)));
        let Some(caps) = caps else {
            return;
        };
        let structure = match caps.structure(0) {
            Some(structure) => structure,
            None => return,
        };
        if !structure.name().starts_with("video/") {
            return;
        }
        let index = pads_for_pad.fetch_add(1, Ordering::SeqCst);
        if index != track_index {
            return;
        }
        let Some(sink_pad) = queue_for_pad.static_pad("sink") else {
            return;
        };
        if pad.link(&sink_pad).is_err() {
            return;
        }
        let width = structure.get::<i32>("width").ok().map(|value| value as u32);
        let height = structure
            .get::<i32>("height")
            .ok()
            .map(|value| value as u32);
        let fps = structure
            .get::<gst::Fraction>("framerate")
            .ok()
            .map(|fraction| fraction.numer() as f64 / fraction.denom().max(1) as f64);
        if let Ok(mut slot) = selection_for_pad.lock() {
            *slot = Some(TrackSelection {
                caps: caps.to_string(),
                width,
                height,
                fps,
            });
        }
    });

    let params_for_sample = request.params.clone();
    let track_id = format!("video-{}", request.params.track_index);
    let track_id_for_sample = track_id.clone();
    let sampler = Arc::new(Mutex::new(SamplerState::default()));
    let sampler_for_sink = sampler.clone();
    let progress_for_sink = progress.clone();
    appsink.set_callbacks(
        gst_app::AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                let caps = sample.caps().ok_or(gst::FlowError::Error)?;
                let info =
                    gst_video::VideoInfo::from_caps(caps).map_err(|_| gst::FlowError::Error)?;
                let buffer = sample.buffer().ok_or(gst::FlowError::Error)?;
                let frame = gst_video::VideoFrameRef::from_buffer_ref_readable(buffer, &info)
                    .map_err(|_| gst::FlowError::Error)?;
                let data = frame.plane_data(0).map_err(|_| gst::FlowError::Error)?;
                let width = info.width();
                let height = info.height();
                let stride = frame.info().stride()[0] as usize;
                let threshold = (params_for_sample.luma_threshold * 255.0).round() as u8;
                let stats = gray8_stats(data, width, height, stride, threshold);
                let black = is_black(stats, &params_for_sample);
                let gray = compact_gray8(data, width, height, stride);

                let mut state = sampler_for_sink.lock().map_err(|_| gst::FlowError::Error)?;
                let frame_number = state.frame_count;
                state.frame_count += 1;
                let fallback_ms = frame_number * 1000 / params_for_sample.sample_fps.max(1) as u64;
                let time_ms = buffer
                    .pts()
                    .map(|pts| pts.mseconds())
                    .unwrap_or(fallback_ms);
                let contiguous = state
                    .samples
                    .last()
                    .map(|previous| {
                        time_ms.saturating_sub(previous.time_ms)
                            <= params_for_sample.sample_period_ms() * 2
                    })
                    .unwrap_or(true);
                if !contiguous {
                    finalize_run(&mut state, &params_for_sample, &track_id_for_sample);
                }
                let sample_index = state.samples.len() as u64;
                state.samples.push(WaveSample {
                    sample: sample_index,
                    time_ms,
                    pts_ns: buffer.pts().map(|pts| pts.nseconds()).unwrap_or(0),
                    frame: frame_number,
                    track_id: track_id_for_sample.clone(),
                    y_mean: stats.mean,
                    y_min: stats.min,
                    y_max: stats.max,
                    black_pixel_ratio: stats.black_pixel_ratio,
                    is_black: black,
                    params_fingerprint: params_for_sample.fingerprint(),
                });

                if black {
                    if state.current_run.is_none() {
                        state.current_run = Some(FrameRun {
                            start_sample: sample_index,
                            start_time_ms: time_ms,
                            start_frame: frame_number,
                            start_gray: gray.clone(),
                            last_time_ms: time_ms,
                            darkest_sample: sample_index,
                            darkest_time_ms: time_ms,
                            darkest_frame: frame_number,
                            darkest_y: stats.mean,
                            darkest_gray: gray,
                        });
                    } else if let Some(run) = state.current_run.as_mut() {
                        run.last_time_ms = time_ms;
                        if stats.mean < run.darkest_y {
                            run.darkest_y = stats.mean;
                            run.darkest_sample = sample_index;
                            run.darkest_time_ms = time_ms;
                            run.darkest_frame = frame_number;
                            run.darkest_gray = gray;
                        }
                    }
                } else {
                    finalize_run(&mut state, &params_for_sample, &track_id_for_sample);
                }

                if frame_number % 25 == 0 {
                    send(
                        &progress_for_sink,
                        ProgressStage::Sampling,
                        0.0,
                        format!(
                            "已采样 {} 帧；当前 {}，亮度 {:.3}",
                            frame_number + 1,
                            if black { "黑场" } else { "正常" },
                            stats.mean
                        ),
                        time_ms,
                        frame_number,
                    );
                }
                Ok(gst::FlowSuccess::Ok)
            })
            .build(),
    );

    send(
        &progress,
        ProgressStage::Decoding,
        0.05,
        "启动 decodebin；仅连接选定视频轨",
        0,
        0,
    );
    pipeline
        .set_state(gst::State::Playing)
        .map_err(|error| pipeline_failure("无法启动 GStreamer 管线", error.to_string()))?;
    let bus = pipeline
        .bus()
        .ok_or_else(|| pipeline_failure("GStreamer 管线没有 bus", "bus unavailable"))?;
    let started = Instant::now();
    let mut eos = false;
    while !eos {
        if cancel.load(Ordering::SeqCst) {
            let _ = pipeline.set_state(gst::State::Null);
            return Err(AnalysisFailure::new(
                "cancelled",
                "重放已取消",
                "用户停止了当前分析。",
            ));
        }
        if let Some(message) = bus.timed_pop_filtered(
            gst::ClockTime::from_mseconds(120),
            &[gst::MessageType::Error, gst::MessageType::Eos],
        ) {
            match message.view() {
                gst::MessageView::Error(error) => {
                    let detail = format!(
                        "{} ({})",
                        error.error(),
                        error.debug().unwrap_or_else(|| "no debug".into())
                    );
                    let _ = pipeline.set_state(gst::State::Null);
                    return Err(classify_gst_error(&detail));
                }
                gst::MessageView::Eos(_) => eos = true,
                _ => {}
            }
        }
        let elapsed = started.elapsed();
        let has_track = selection.lock().map(|slot| slot.is_some()).unwrap_or(false);
        let sample_count = sampler.lock().map(|state| state.samples.len()).unwrap_or(0);
        if !has_track && elapsed > Duration::from_secs(20) {
            let found = video_pads.load(Ordering::SeqCst);
            let _ = pipeline.set_state(gst::State::Null);
            return Err(AnalysisFailure::no_video_track(format!(
                "请求轨道索引 {track_index}，decodebin 找到 {found} 条视频轨。"
            )));
        }
        if has_track && sample_count == 0 && elapsed > Duration::from_secs(25) {
            let _ = pipeline.set_state(gst::State::Null);
            return Err(AnalysisFailure::no_samples(
                "已连接视频轨，但 25 秒内未收到 GRAY8 样本。",
            ));
        }
    }
    let queried_duration = pipeline
        .query_duration::<gst::ClockTime>()
        .map(|duration| duration.mseconds())
        .unwrap_or(0);
    let _ = pipeline.set_state(gst::State::Null);

    let selection = selection
        .lock()
        .map_err(|_| pipeline_failure("读取轨道信息失败", "selection lock poisoned"))?
        .clone();
    let Some(selection) = selection else {
        return Err(AnalysisFailure::no_video_track(format!(
            "请求轨道索引 {track_index}，但没有成功连接视频轨。"
        )));
    };
    let (samples, mut keyframes, completed_runs) = {
        let mut state = sampler
            .lock()
            .map_err(|_| pipeline_failure("读取采样结果失败", "sampler lock poisoned"))?;
        finalize_run(&mut state, &request.params, &track_id);
        (
            state.samples.clone(),
            state.keyframes.clone(),
            state.completed_runs,
        )
    };
    if samples.is_empty() {
        return Err(AnalysisFailure::no_samples(
            "decodebin 完成但波形为空；请检查轨道索引与像素格式。",
        ));
    }
    let waveform = waveform_from_samples(samples, request.params.sample_period_ms());
    let mut auto_conclusion = conclude(&waveform, &request.params, &track_id);
    if completed_runs != auto_conclusion.intervals.len() {
        auto_conclusion.limitations.push(format!(
            "实时关键帧分段 {} 段，离线复核 {} 段；请优先按离线波形和原始帧复核。",
            completed_runs,
            auto_conclusion.intervals.len()
        ));
        auto_conclusion
            .reason_codes
            .push("LIVE_OFFLINE_INTERVAL_DIFF".into());
    }
    keyframes.retain(|keyframe| {
        keyframe.interval_id.as_ref().is_none_or(|interval_id| {
            auto_conclusion
                .intervals
                .iter()
                .any(|interval| &interval.id == interval_id)
        })
    });

    source.extension.content_probe = selection.caps.clone();
    source.probe_caps = selection.caps.clone();
    let duration_ms = queried_duration.max(waveform.covered_ms);
    let mut replay = new_replay(
        request.recovered,
        &request.recovery_note,
        request.replayed_from_report.clone(),
    );
    replay.replay_number = request.replay_number.max(1);
    replay.selected_track = Some(track_id.clone());
    replay.duration_ms = duration_ms;
    replay.status = ReplayStatus::Completed;
    let mut report = ReviewReport::new(source.clone(), replay, request.params.clone());
    report.tracks = vec![TrackRecord {
        id: track_id.clone(),
        index: request.params.track_index,
        kind: TrackKind::Video,
        stream_id: None,
        caps: selection.caps.clone(),
        source_width: selection.width,
        source_height: selection.height,
        source_fps: selection.fps,
        frames_sampled: waveform.samples.len() as u64,
    }];
    report.waveform = waveform;
    report.auto_conclusion = auto_conclusion;
    append_event(
        &mut report,
        0,
        "source_hashed",
        format!(
            "SHA-256 {}；{} bytes；扩展名状态 {:?}",
            source.sha256, source.size_bytes, source.extension.status
        ),
        None,
    );
    if source.extension.status == ExtensionStatus::Unknown {
        append_event(
            &mut report,
            0,
            "unknown_extension_recorded",
            format!(
                "未知扩展 `.{}` 已写入报告；以 decodebin caps 为准：{}",
                source.extension.raw_extension.as_deref().unwrap_or("none"),
                selection.caps
            ),
            None,
        );
    }
    append_event(
        &mut report,
        0,
        "track_selected",
        format!("{} / {}", track_id, selection.caps),
        Some(&track_id),
    );
    append_event(
        &mut report,
        0,
        "parameters_locked",
        format!(
            "阈值 {:.3}，黑像素比例 {:.3}，最短 {} ms，{} fps，{}x{}",
            request.params.luma_threshold,
            request.params.black_pixel_ratio,
            request.params.min_duration_ms,
            request.params.sample_fps,
            request.params.analysis_width,
            request.params.analysis_height
        ),
        Some(&track_id),
    );
    let last_sample = report.waveform.samples.len().saturating_sub(1) as u64;
    let covered_ms = report.waveform.covered_ms;
    let waveform_samples = report.waveform.samples.len();
    let sample_period_ms = report.waveform.sample_period_ms;
    append_event(
        &mut report,
        covered_ms,
        "waveform_emitted",
        format!(
            "{} 个采样，周期 {} ms，覆盖 {} ms",
            waveform_samples, sample_period_ms, covered_ms
        ),
        Some(&track_id),
    );
    if let Some(event) = report.replay.events.last_mut() {
        event.waveform_sample = Some(last_sample);
    }
    let auto_headline = report.auto_conclusion.headline.clone();
    append_event(
        &mut report,
        covered_ms,
        "auto_conclusion_recorded",
        auto_headline,
        Some(&track_id),
    );
    send(
        &progress,
        ProgressStage::Complete,
        1.0,
        format!(
            "重放完成：{} 个采样，{} 段自动黑场",
            report.waveform.samples.len(),
            report.auto_conclusion.intervals.len()
        ),
        duration_ms,
        report.waveform.samples.len() as u64,
    );
    Ok(AnalysisOutput { report, keyframes })
}

fn finalize_run(state: &mut SamplerState, params: &DetectorParams, track_id: &str) {
    let Some(run) = state.current_run.take() else {
        return;
    };
    let end_ms = run.last_time_ms.saturating_add(params.sample_period_ms());
    if end_ms.saturating_sub(run.start_time_ms) < params.min_duration_ms {
        return;
    }
    state.completed_runs += 1;
    let interval_id = format!("bf-{:04}", state.completed_runs);
    let (width, height) = (params.analysis_width, params.analysis_height);
    state.keyframes.push(KeyFrame {
        id: format!("{interval_id}-start"),
        interval_id: Some(interval_id.clone()),
        time_ms: run.start_time_ms,
        frame: run.start_frame,
        track_id: track_id.into(),
        reason: format!("黑场起点 / sample {}", run.start_sample),
        width,
        height,
        gray: run.start_gray,
    });
    if run.darkest_sample != run.start_sample {
        state.keyframes.push(KeyFrame {
            id: format!("{interval_id}-darkest"),
            interval_id: Some(interval_id),
            time_ms: run.darkest_time_ms,
            frame: run.darkest_frame,
            track_id: track_id.into(),
            reason: format!(
                "区间最暗帧 Y={:.3} / sample {}",
                run.darkest_y, run.darkest_sample
            ),
            width,
            height,
            gray: run.darkest_gray,
        });
    }
}

fn compact_gray8(data: &[u8], width: u32, height: u32, stride: usize) -> Vec<u8> {
    let mut output = vec![0u8; width as usize * height as usize];
    let row_bytes = width as usize;
    for row in 0..height as usize {
        let source_start = row * stride;
        let source_end = source_start.saturating_add(row_bytes);
        if source_end <= data.len() {
            output[row * row_bytes..(row + 1) * row_bytes]
                .copy_from_slice(&data[source_start..source_end]);
        }
    }
    output
}

fn append_event(
    report: &mut ReviewReport,
    at_ms: u64,
    kind: &str,
    detail: String,
    track_id: Option<&str>,
) {
    report.replay.events.push(ReplayEvent {
        sequence: report.replay.events.len() as u64 + 1,
        at_ms,
        frame: None,
        track_id: track_id.map(str::to_string),
        kind: kind.into(),
        detail,
        params_fingerprint: report.detector.fingerprint(),
        waveform_sample: None,
        evidence_refs: Vec::new(),
    });
}

fn make_element(name: &str) -> Result<gst::Element, AnalysisFailure> {
    gst::ElementFactory::make(name)
        .build()
        .map_err(|error| AnalysisFailure::missing_decoder(format!("无法创建 `{name}`: {error}")))
}

fn pipeline_failure(title: &str, detail: impl Into<String>) -> AnalysisFailure {
    AnalysisFailure::new("gstreamer_pipeline_failed", title, detail)
}

fn classify_gst_error(detail: &str) -> AnalysisFailure {
    let lower = detail.to_ascii_lowercase();
    if lower.contains("missing") || lower.contains("not found") || lower.contains("no such element")
    {
        AnalysisFailure::missing_decoder(detail)
    } else if lower.contains("permission denied") {
        AnalysisFailure::new("permission_denied", "没有读取权限", detail)
    } else if lower.contains("not-negotiated")
        || lower.contains("not negotiated")
        || lower.contains("streaming stopped")
    {
        AnalysisFailure::unsupported_media(detail)
    } else {
        AnalysisFailure::new("gstreamer_runtime_error", "GStreamer 重放失败", detail)
    }
}

fn send(
    sender: &Sender<ProgressUpdate>,
    stage: ProgressStage,
    fraction: f32,
    message: impl Into<String>,
    time_ms: u64,
    frame: u64,
) {
    let _ = sender.send(ProgressUpdate {
        stage,
        fraction,
        message: message.into(),
        time_ms,
        frame,
    });
}
