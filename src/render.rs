use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use gst::prelude::*;
use gst_app::{AppSink, AppSrc};
use serde_json::json;

use crate::evidence::{append_event, relative_evidence_path, sha256_file};
use crate::mask::MaskConfig;
use crate::media::ensure_gstreamer;
use crate::model::{EvidenceFrame, ProjectSession};
use crate::player::connect_mask_draw;
use crate::project::{verify_original, EVIDENCE_DIR};
use crate::timecode::{format_signed_offset, format_timecode, frame_duration_ns};

#[derive(Debug, Clone)]
pub struct RenderOutcome {
    pub output_path: PathBuf,
    pub output_sha256: String,
    pub measured_start_ns: u64,
    pub measured_end_ns: u64,
    pub evidence: Vec<EvidenceFrame>,
}

#[derive(Debug, Default)]
struct TimelineStats {
    first_pts_ns: Option<u64>,
    last_pts_ns: Option<u64>,
    frames: u64,
}

pub fn render_segment(
    session: &ProjectSession,
    segment_index: usize,
    run_number: u32,
    output_path: &Path,
    capture_evidence: bool,
) -> Result<RenderOutcome> {
    ensure_gstreamer()?;
    let integrity = verify_original(session);
    if !integrity.verified {
        bail!("原始素材完整性检查失败：{}", integrity.message);
    }
    session.validate().map_err(anyhow::Error::msg)?;
    let segment = session.segment(segment_index).map_err(anyhow::Error::msg)?;
    let range = segment
        .range(session.media.duration_ns)
        .map_err(anyhow::Error::msg)?;
    let source = session
        .project_dir
        .join(&session.original.stored_relative_path);
    if output_path == source {
        bail!("导出路径不能覆盖只读原始素材");
    }
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("无法创建导出目录：{}", parent.display()))?;
    }
    let temp_output = output_path.with_extension(format!(
        "{}.part",
        output_path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("webm")
    ));
    if temp_output.exists() {
        let _ = fs::remove_file(&temp_output);
    }

    let mask = Arc::new(std::sync::RwLock::new(segment.mask.clone().sanitized()));
    let evidence = if capture_evidence {
        let evidence = capture_run_evidence(
            session,
            segment_index,
            run_number,
            &source,
            Arc::clone(&mask),
            &range,
        )?;
        persist_evidence_manifest(output_path, session, &range, &evidence)?;
        append_event(
            &session.project_dir,
            "run_evidence_captured",
            Some(segment_index),
            None,
            format!(
                "片段 {} 第 {run_number} 次重跑证据已先行落盘",
                segment_index + 1
            ),
            json!({
                "run_number": run_number,
                "anchor_ns": range.anchor_ns,
                "in_offset_ns": range.in_offset_ns,
                "out_offset_ns": range.out_offset_ns,
                "range_start_ns": range.start_ns,
                "range_end_ns": range.end_ns,
                "evidence": evidence,
            }),
        )?;
        evidence
    } else {
        Vec::new()
    };
    let (source_pipeline, appsink) = build_source_pipeline(&source, Arc::clone(&mask))?;
    let source_supervisor = PipelineSupervisor::new(&source_pipeline);
    source_supervisor.wait_for_ready(Duration::from_secs(20))?;
    source_supervisor.seek_pipeline(range.start_ns)?;
    source_supervisor.play()?;

    let (encoder_pipeline, appsrc) = build_encoder_pipeline(&temp_output)?;
    encoder_pipeline.set_state(gst::State::Playing)?;
    let frame_duration =
        frame_duration_ns(session.media.fps_num, session.media.fps_den).unwrap_or(40_000_000);
    let mut timeline = TimelineStats::default();
    let mut timeline_shift = 0_i64;
    let mut caps_set = false;
    let mut base_pts = None;
    loop {
        let sample = appsink
            .try_pull_sample(gst::ClockTime::from_seconds(15))
            .ok_or_else(|| anyhow::anyhow!("解码流水线在片段范围内未返回帧"))?;
        let buffer = sample.buffer().context("解码帧没有 buffer")?;
        let pts = buffer
            .pts()
            .map(|pts| pts.nseconds() as i64)
            .ok_or_else(|| anyhow::anyhow!("解码帧没有 PTS"))?;
        if !caps_set {
            timeline_shift = if (pts.max(0) as u64).saturating_add(frame_duration) < range.start_ns
            {
                range.start_ns as i64 - pts
            } else {
                0
            };
            if let Some(caps) = sample.caps() {
                appsrc.set_caps(Some(&caps.to_owned()));
            }
            caps_set = true;
        }
        let effective_pts = pts.saturating_add(timeline_shift).max(0) as u64;
        if effective_pts < range.start_ns {
            continue;
        }
        if effective_pts >= range.end_ns {
            break;
        }
        if base_pts.is_none() {
            base_pts = Some(pts);
            timeline.first_pts_ns = Some(effective_pts);
        }
        let base = base_pts.unwrap_or(pts);
        let output_pts = pts.saturating_sub(base).max(0) as u64;
        let mut output_buffer = buffer.copy();
        {
            let buffer = output_buffer.make_mut();
            buffer.set_pts(Some(gst::ClockTime::from_nseconds(output_pts)));
            buffer.set_dts(Some(gst::ClockTime::from_nseconds(output_pts)));
        }
        appsrc
            .push_buffer(output_buffer)
            .map_err(|_| anyhow::anyhow!("向编码流水线推送帧失败"))?;
        timeline.last_pts_ns = Some(effective_pts);
        timeline.frames += 1;
    }
    if timeline.frames == 0 {
        bail!("片段范围内没有可导出的视频帧");
    }
    drop(source_supervisor);
    let _ = source_pipeline.set_state(gst::State::Null);
    drop(source_pipeline);
    let _ = appsrc.end_of_stream();
    let encoder_supervisor = PipelineSupervisor::new(&encoder_pipeline);
    encoder_supervisor.wait_for_end(Duration::from_secs(
        (range.duration_ns() / 1_000_000_000).saturating_add(30),
    ))?;
    drop(encoder_supervisor);
    let _ = encoder_pipeline.set_state(gst::State::Null);
    drop(encoder_pipeline);

    if !temp_output.is_file() {
        bail!("编码器未生成输出文件");
    }
    fs::rename(&temp_output, output_path)
        .with_context(|| format!("无法完成导出文件：{}", output_path.display()))?;
    let output_sha256 = sha256_file(output_path)?;
    Ok(RenderOutcome {
        output_path: output_path.to_path_buf(),
        output_sha256,
        measured_start_ns: timeline.first_pts_ns.unwrap_or(range.start_ns),
        measured_end_ns: timeline.last_pts_ns.unwrap_or(range.end_ns),
        evidence,
    })
}

fn persist_evidence_manifest(
    output_path: &Path,
    session: &ProjectSession,
    range: &crate::model::SegmentRange,
    evidence: &[EvidenceFrame],
) -> Result<()> {
    let run_dir = output_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("运行输出没有父目录"))?;
    fs::create_dir_all(run_dir)?;
    let payload = json!({
        "original_sha256": session.original.sha256,
        "anchor_ns": range.anchor_ns,
        "in_offset_ns": range.in_offset_ns,
        "out_offset_ns": range.out_offset_ns,
        "range_start_ns": range.start_ns,
        "range_end_ns": range.end_ns,
        "evidence": evidence,
    });
    let temp = run_dir.join(".evidence-manifest.json.tmp");
    fs::write(&temp, serde_json::to_vec_pretty(&payload)?)?;
    fs::rename(temp, run_dir.join("evidence-manifest.json"))?;
    Ok(())
}

fn build_source_pipeline(
    source: &Path,
    mask: Arc<std::sync::RwLock<MaskConfig>>,
) -> Result<(gst::Pipeline, AppSink)> {
    let pipeline = gst::Pipeline::new();
    let filesrc = make("filesrc")?;
    filesrc.set_property("location", source.to_string_lossy().as_ref());
    let decode = make("decodebin")?;
    let convert_in = make("videoconvert")?;
    let caps_in = make("capsfilter")?;
    caps_in.set_property(
        "caps",
        gst::Caps::builder("video/x-raw")
            .field("format", "BGRA")
            .build(),
    );
    let overlay = make("cairooverlay")?;
    connect_mask_draw(&overlay, Arc::clone(&mask));
    let appsink = make("appsink")?
        .downcast::<AppSink>()
        .map_err(|_| anyhow::anyhow!("解码 appsink 类型转换失败"))?;
    appsink.set_max_buffers(8);
    appsink.set_drop(false);
    appsink.set_sync(false);
    pipeline.add_many([
        &filesrc,
        &decode,
        &convert_in,
        &caps_in,
        &overlay,
        appsink.upcast_ref(),
    ])?;
    filesrc.link(&decode)?;
    convert_in.link(&caps_in)?;
    caps_in.link(&overlay)?;
    overlay.link(&appsink)?;
    connect_decode_video_pad(&decode, &convert_in)?;
    Ok((pipeline, appsink))
}

fn build_encoder_pipeline(output: &Path) -> Result<(gst::Pipeline, AppSrc)> {
    let pipeline = gst::Pipeline::new();
    let appsrc = make("appsrc")?
        .downcast::<AppSrc>()
        .map_err(|_| anyhow::anyhow!("appsrc 类型转换失败"))?;
    appsrc.set_format(gst::Format::Time);
    appsrc.set_is_live(false);
    appsrc.set_do_timestamp(false);
    appsrc.set_max_bytes(64 * 1024 * 1024);
    appsrc.set_property("block", true);
    let convert = make("videoconvert")?;
    let encoder = make("vp8enc")?;
    encoder.set_property("target-bitrate", 8_000_000_i32);
    encoder.set_property("keyframe-max-dist", 30_i32);
    let mux = make("webmmux")?;
    let filesink = make("filesink")?;
    filesink.set_property("location", output.to_string_lossy().as_ref());
    pipeline.add_many([appsrc.upcast_ref(), &convert, &encoder, &mux, &filesink])?;
    appsrc.link(&convert)?;
    convert.link(&encoder)?;
    encoder.link(&mux)?;
    mux.link(&filesink)?;
    Ok((pipeline, appsrc))
}

fn capture_run_evidence(
    session: &ProjectSession,
    segment_index: usize,
    run_number: u32,
    source: &Path,
    mask: Arc<std::sync::RwLock<MaskConfig>>,
    range: &crate::model::SegmentRange,
) -> Result<Vec<EvidenceFrame>> {
    let frame =
        frame_duration_ns(session.media.fps_num, session.media.fps_den).unwrap_or(40_000_000);
    let end_frame = range.end_ns.saturating_sub(frame).max(range.start_ns);
    let mut captures = vec![
        ("start", range.start_ns),
        ("anchor", range.anchor_ns),
        ("end", end_frame),
    ];
    captures.sort_by_key(|(_, timestamp)| *timestamp);
    captures.dedup_by_key(|(_, timestamp)| *timestamp);
    let mut evidence = Vec::new();
    for (kind, requested_ns) in captures {
        let relative = relative_evidence_path(segment_index, run_number, kind, requested_ns);
        let absolute = session.project_dir.join(&relative);
        let frame = capture_frame(source, requested_ns, &absolute, Arc::clone(&mask), kind)?;
        evidence.push(EvidenceFrame {
            kind: kind.to_string(),
            timestamp_ns: frame.timestamp_ns,
            timestamp_timecode: format_timecode(frame.timestamp_ns),
            source_offset_ns: frame.timestamp_ns as i64 - range.anchor_ns as i64,
            source_offset: format_signed_offset(frame.timestamp_ns as i64 - range.anchor_ns as i64),
            relative_path: relative.to_string_lossy().to_string(),
            sha256: frame.sha256,
        });
    }
    Ok(evidence)
}

#[derive(Debug)]
struct CapturedFrame {
    timestamp_ns: u64,
    sha256: String,
}

fn capture_frame(
    source: &Path,
    requested_ns: u64,
    destination: &Path,
    mask: Arc<std::sync::RwLock<MaskConfig>>,
    _kind: &str,
) -> Result<CapturedFrame> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("无法创建证据目录：{}", parent.display()))?;
    }
    let pipeline = gst::Pipeline::new();
    let filesrc = make("filesrc")?;
    filesrc.set_property("location", source.to_string_lossy().as_ref());
    let decode = make("decodebin")?;
    let convert_in = make("videoconvert")?;
    let caps_in = make("capsfilter")?;
    caps_in.set_property(
        "caps",
        gst::Caps::builder("video/x-raw")
            .field("format", "BGRA")
            .build(),
    );
    let overlay = make("cairooverlay")?;
    connect_mask_draw(&overlay, mask);
    let convert_out = make("videoconvert")?;
    let png = make("pngenc")?;
    let appsink = make("appsink")?
        .downcast::<AppSink>()
        .map_err(|_| anyhow::anyhow!("证据 appsink 类型转换失败"))?;
    appsink.set_max_buffers(1);
    appsink.set_drop(true);
    appsink.set_sync(false);
    pipeline.add_many([
        &filesrc,
        &decode,
        &convert_in,
        &caps_in,
        &overlay,
        &convert_out,
        &png,
        appsink.upcast_ref(),
    ])?;
    filesrc.link(&decode)?;
    convert_in.link(&caps_in)?;
    caps_in.link(&overlay)?;
    overlay.link(&convert_out)?;
    convert_out.link(&png)?;
    png.link(&appsink)?;
    connect_decode_video_pad(&decode, &convert_in)?;

    let supervisor = PipelineSupervisor::new(&pipeline);
    supervisor.wait_for_ready(Duration::from_secs(20))?;
    supervisor.seek_pipeline(requested_ns)?;
    let sample = appsink
        .try_pull_preroll(gst::ClockTime::from_seconds(5))
        .or_else(|| {
            let _ = supervisor.play();
            appsink.try_pull_sample(gst::ClockTime::from_seconds(5))
        })
        .ok_or_else(|| anyhow::anyhow!("无法在 {} 捕获证据帧", format_timecode(requested_ns)))?;
    let actual_ns = sample
        .buffer()
        .and_then(|buffer| buffer.pts())
        .map(|pts| pts.nseconds())
        .unwrap_or(requested_ns);
    let buffer = sample.buffer().context("证据帧没有 buffer")?;
    let map = buffer.map_readable().context("证据帧不可读")?;
    let temp = destination.with_extension("png.part");
    {
        let file = File::create(&temp).context("无法创建证据文件")?;
        let mut writer = BufWriter::new(file);
        writer.write_all(map.as_slice()).context("写入证据失败")?;
        writer.flush().context("刷新证据失败")?;
        writer.get_ref().sync_all().context("证据落盘失败")?;
    }
    drop(supervisor);
    let _ = pipeline.set_state(gst::State::Null);
    drop(pipeline);
    fs::rename(&temp, destination).context("证据文件原子替换失败")?;
    let sha256 = sha256_file(destination)?;
    Ok(CapturedFrame {
        timestamp_ns: actual_ns,
        sha256,
    })
}

struct PipelineSupervisor<'a> {
    pipeline: &'a gst::Pipeline,
    bus: gst::Bus,
}

impl<'a> PipelineSupervisor<'a> {
    fn new(pipeline: &'a gst::Pipeline) -> Self {
        let bus = pipeline.bus().expect("pipeline always has a bus");
        Self { pipeline, bus }
    }

    fn wait_for_ready(&self, timeout: Duration) -> Result<()> {
        self.pipeline.set_state(gst::State::Paused)?;
        self.wait_for_async_done(timeout, true)
    }

    fn seek_pipeline(&self, position_ns: u64) -> Result<()> {
        let duration = self.pipeline.query_duration::<gst::ClockTime>();
        let mut query = gst::query::Seeking::new(gst::Format::Time);
        let seekable = self.pipeline.query(&mut query) && query.result().0;
        self.pipeline
            .seek_simple(
                gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
                gst::ClockTime::from_nseconds(position_ns),
            )
            .with_context(|| {
                format!(
                    "无法 seek 到 {} ns（duration={:?}, seekable={seekable}）",
                    position_ns, duration
                )
            })?;
        self.wait_for_async_done(Duration::from_secs(8), true)?;
        Ok(())
    }

    fn play(&self) -> Result<()> {
        self.pipeline.set_state(gst::State::Playing)?;
        Ok(())
    }

    fn wait_for_end(&self, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            let Some(message) = self.bus.timed_pop_filtered(
                gst::ClockTime::from_mseconds(200),
                &[gst::MessageType::Error, gst::MessageType::Eos],
            ) else {
                continue;
            };
            match message.view() {
                gst::MessageView::Eos(_) => return Ok(()),
                gst::MessageView::Error(error) => {
                    bail!(
                        "GStreamer 渲染失败：{} ({})",
                        error.error(),
                        error.debug().unwrap_or_default()
                    );
                }
                _ => {}
            }
        }
        bail!("GStreamer 渲染超时")
    }

    fn wait_for_async_done(&self, timeout: Duration, required: bool) -> Result<()> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            let Some(message) = self.bus.timed_pop_filtered(
                gst::ClockTime::from_mseconds(200),
                &[
                    gst::MessageType::AsyncDone,
                    gst::MessageType::Error,
                    gst::MessageType::Eos,
                ],
            ) else {
                continue;
            };
            match message.view() {
                gst::MessageView::AsyncDone(_) => return Ok(()),
                gst::MessageView::Eos(_) => {
                    bail!("GStreamer 流水线在就绪前到达 EOS");
                }
                gst::MessageView::Error(error) => {
                    bail!(
                        "GStreamer 就绪失败：{} ({})",
                        error.error(),
                        error.debug().unwrap_or_default()
                    );
                }
                _ => {}
            }
        }
        if required {
            bail!("GStreamer 流水线在 {} 秒内未就绪", timeout.as_secs());
        }
        Ok(())
    }
}

fn connect_decode_video_pad(decode: &gst::Element, convert: &gst::Element) -> Result<()> {
    let convert = convert.clone();
    decode.connect_pad_added(move |_, pad| {
        let is_video = pad
            .current_caps()
            .or_else(|| Some(pad.query_caps(None)))
            .and_then(|caps| {
                caps.structure(0)
                    .map(|structure| structure.name().to_string())
            })
            .map(|name| name.starts_with("video/"))
            .unwrap_or(false);
        if std::env::var_os("MASK_EDGE_DEBUG").is_some() {
            eprintln!(
                "decodebin pad={} caps={:?} video={is_video}",
                pad.name(),
                pad.current_caps()
                    .or_else(|| Some(pad.query_caps(None)))
                    .map(|caps| caps.to_string())
            );
        }
        if !is_video {
            return;
        }
        if let Some(sink_pad) = convert.static_pad("sink") {
            if !sink_pad.is_linked() {
                let _ = pad.link(&sink_pad);
            }
        }
    });
    Ok(())
}

impl Drop for PipelineSupervisor<'_> {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

fn make(factory: &str) -> Result<gst::Element> {
    gst::ElementFactory::make(factory).build().with_context(|| {
        format!("缺少 GStreamer 元素 {factory}；请安装 gstreamer1.0-plugins-base/good")
    })
}

pub fn evidence_dir(session: &ProjectSession) -> PathBuf {
    session.project_dir.join(EVIDENCE_DIR)
}
