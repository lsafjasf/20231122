//! GStreamer 分析引擎：单片段重跑、边界前后采样、证据快照。
use crate::metrics::{analyze_frame_rgba, InkStat};
use crate::model::{BoundaryPair, BoundarySample, RunResult, Segment};
use crate::session::Project;
use anyhow::{anyhow, Context, Result};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use gstreamer_video as gst_video;
use gtk4::prelude::TextureExt;
use std::path::PathBuf;
use std::sync::Once;
use std::time::{SystemTime, UNIX_EPOCH};

static GST_INIT: Once = Once::new();

pub fn ensure_gst_init() {
    GST_INIT.call_once(|| {
        gst::init().expect("GStreamer 初始化失败");
    });
}

struct Frame {
    stat: InkStat,
    rgba: Vec<u8>,
    width: usize,
    height: usize,
    stride: usize,
}

pub struct Analyzer {
    pipeline: gst::Pipeline,
    appsink: gst_app::AppSink,
    duration_ms: u64,
}

impl Analyzer {
    pub fn new(media_uri: &str) -> Result<Self> {
        ensure_gst_init();
        let pipeline = gst::Pipeline::new();
        let src = gst::ElementFactory::make("uridecodebin")
            .property("uri", media_uri)
            .build()?;
        let convert = gst::ElementFactory::make("videoconvert").build()?;
        let caps = gst::Caps::builder("video/x-raw")
            .field("format", "RGBA")
            .build();
        let sink = gst::ElementFactory::make("appsink")
            .property("caps", &caps)
            .property("sync", false)
            // 不丢帧：队列满则上游阻塞（背压），保证逐帧扫描完整
            .property("max-buffers", 8u32)
            .property("drop", false)
            .build()?;
        let appsink = sink.clone().dynamic_cast::<gst_app::AppSink>().unwrap();
        pipeline.add_many([&src, &convert, &sink])?;
        gst::Element::link_many([&convert, &sink])?;
        let convert_weak = convert.downgrade();
        src.connect_pad_added(move |_, pad| {
            let Some(convert) = convert_weak.upgrade() else {
                return;
            };
            let Some(sinkpad) = convert.static_pad("sink") else {
                return;
            };
            if sinkpad.is_linked() || pad.link(&sinkpad).is_err() {
                // 非视频流或已连接，忽略
            }
        });
        // PLAYING + sync=false：帧以最快速度流动，pull 即取，
        // 单帧抓取与区间扫描共用同一机制。
        pipeline.set_state(gst::State::Playing)?;
        let bus = pipeline.bus().context("无总线")?;
        for msg in bus.iter_timed(gst::ClockTime::from_seconds(10)) {
            use gst::MessageView;
            match msg.view() {
                MessageView::AsyncDone(_) => break,
                MessageView::Error(e) => {
                    return Err(anyhow!("管线错误: {}", e.error()));
                }
                _ => {}
            }
        }
        let duration_ms = pipeline
            .query_duration::<gst::ClockTime>()
            .map(|d| d.mseconds())
            .unwrap_or(0);
        Ok(Self {
            pipeline,
            appsink,
            duration_ms,
        })
    }

    pub fn duration_ms(&self) -> u64 {
        self.duration_ms
    }

    /// 调试/示例用：定位到指定毫秒
    pub fn debug_seek(&self, ms: u64) {
        let _ = self.seek_ms(ms);
    }

    /// 调试/示例用：拉一帧并返回墨迹占比
    pub fn debug_pull(&self, params: &crate::model::ExtremeParams) -> Result<Option<(u64, f64)>> {
        Ok(self
            .pull_frame(params)?
            .map(|(pts, f)| (pts, f.stat.ink_ratio)))
    }

    fn seek_ms(&self, ms: u64) -> Result<()> {
        self.pipeline.seek_simple(
            gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
            gst::ClockTime::from_mseconds(ms),
        )?;
        Ok(())
    }

    fn pull_frame(&self, params: &crate::model::ExtremeParams) -> Result<Option<(u64, Frame)>> {
        let Some(sample) = self
            .appsink
            .try_pull_sample(gst::ClockTime::from_seconds(10))
        else {
            return Ok(None); // EOS 或超时
        };
        let caps = sample.caps().context("样本无 caps")?;
        let info = gst_video::VideoInfo::from_caps(caps).context("无法解析视频信息")?;
        let buffer = sample.buffer().context("样本无缓冲")?;
        let pts_ms = buffer
            .pts()
            .map(|p| p.mseconds())
            .ok_or_else(|| anyhow!("帧无时间戳"))?;
        let map = buffer.map_readable()?;
        let width = info.width() as usize;
        let height = info.height() as usize;
        let stride = info.stride()[0] as usize;
        let stat = analyze_frame_rgba(&map, width, height, stride, params);
        Ok(Some((
            pts_ms,
            Frame {
                stat,
                rgba: map.to_vec(),
                width,
                height,
                stride,
            },
        )))
    }

    /// 抓取指定时刻的单帧（用于边界采样）
    fn grab_at(&self, ms: u64, params: &crate::model::ExtremeParams) -> Result<(u64, Frame)> {
        self.seek_ms(ms)?;
        match self.pull_frame(params)? {
            Some(f) => Ok(f),
            None => Err(anyhow!("{ms}ms 处无帧")),
        }
    }

}

impl Drop for Analyzer {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

fn save_snapshot(project: &Project, segment_id: &str, name: &str, frame: &Frame) -> Result<String> {
    let path: PathBuf = project.snapshot_path(segment_id, name)?;
    let bytes = gtk4::glib::Bytes::from_owned(frame.rgba.clone());
    let tex = gtk4::gdk::MemoryTexture::new(
        frame.width as i32,
        frame.height as i32,
        gtk4::gdk::MemoryFormat::R8g8b8a8,
        &bytes,
        frame.stride,
    );
    let tex: gtk4::gdk::Texture = tex.upcast();
    tex.save_to_png(&path)
        .map_err(|e| anyhow!("保存快照失败 {}: {e}", path.display()))?;
    Ok(format!("{segment_id}/{name}.png"))
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 重跑单个片段：扫描区间帧 + 入/出边界前后各采样一次。
pub fn run_segment(project: &Project, analyzer: &Analyzer, segment: &Segment) -> Result<RunResult> {
    let params = &segment.params;
    let eps = params.boundary_epsilon_ms;

    // 1) 边界采样（先采，避免区间扫描的 seek 干扰）
    let boundary = |boundary_ms: u64, tag: &str| -> Result<BoundaryPair> {
        let before_ms = boundary_ms.saturating_sub(eps);
        let after_ms = (boundary_ms + eps).min(analyzer.duration_ms().saturating_sub(1));
        let (bt, bf) = analyzer.grab_at(before_ms, params)?;
        let (at, af) = analyzer.grab_at(after_ms, params)?;
        let before = BoundarySample {
            timestamp_ms: bt,
            ink_pixels: bf.stat.ink_pixels,
            ink_ratio: bf.stat.ink_ratio,
            snapshot: save_snapshot(project, &segment.id, &format!("{tag}_before"), &bf)?,
        };
        let after = BoundarySample {
            timestamp_ms: at,
            ink_pixels: af.stat.ink_pixels,
            ink_ratio: af.stat.ink_ratio,
            snapshot: save_snapshot(project, &segment.id, &format!("{tag}_after"), &af)?,
        };
        Ok(BoundaryPair {
            boundary_ms,
            ink_ratio_delta: after.ink_ratio - before.ink_ratio,
            before,
            after,
        })
    };
    let entry = boundary(segment.start_ms, "entry")?;
    let exit = boundary(segment.end_ms, "exit")?;

    // 2) 区间逐帧扫描
    analyzer.seek_ms(segment.start_ms)?;
    let mut frames = 0u64;
    let mut violating = 0u64;
    let mut max_ratio = 0.0f64;
    let mut sum_ratio = 0.0f64;
    while let Some((pts, frame)) = analyzer.pull_frame(params)? {
        if pts >= segment.end_ms {
            break;
        }
        frames += 1;
        sum_ratio += frame.stat.ink_ratio;
        if frame.stat.ink_ratio > max_ratio {
            max_ratio = frame.stat.ink_ratio;
        }
        if frame.stat.violated {
            violating += 1;
        }
    }

    Ok(RunResult {
        run_at_unix: unix_now(),
        params: params.clone(),
        frames_scanned: frames,
        violating_frames: violating,
        max_ink_ratio: max_ratio,
        mean_ink_ratio: if frames == 0 { 0.0 } else { sum_ratio / frames as f64 },
        entry_boundary: Some(entry),
        exit_boundary: Some(exit),
    })
}
