//! GStreamer 检测：逐帧计算安全框外活跃像素占比（输出波形采样）。
use crate::params::SafeFrameParams;
use anyhow::{Context, Result, anyhow};
use gstreamer as gst;
use gstreamer::prelude::*;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct FrameMetric {
    pub pts_ms: i64,
    pub frame: u64,
    pub metric: f64,
    pub violation: bool,
}

fn luma(r: u8, g: u8, b: u8) -> i32 {
    (299 * r as i32 + 587 * g as i32 + 114 * b as i32) / 1000
}

/// 单帧指标：以四角平均亮度为背景，统计安全框外活跃像素占比。
pub fn metric_for_frame(rgb: &[u8], w: u32, h: u32, p: &SafeFrameParams) -> f64 {
    let (w, h) = (w as usize, h as usize);
    let step = p.sample_step.max(1) as usize;
    // 背景亮度：四角 8x8 块平均
    let mut bg_sum = 0i64;
    let mut bg_n = 0i64;
    for by in [0usize, h.saturating_sub(8)] {
        for bx in [0usize, w.saturating_sub(8)] {
            for y in by..(by + 8).min(h) {
                for x in bx..(bx + 8).min(w) {
                    let i = (y * w + x) * 3;
                    bg_sum += luma(rgb[i], rgb[i + 1], rgb[i + 2]) as i64;
                    bg_n += 1;
                }
            }
        }
    }
    let bg = if bg_n > 0 { (bg_sum / bg_n) as i32 } else { 0 };

    let (sx, sy, sw, sh) = p.safe_rect(w as u32, h as u32);
    let (sx, sy) = (sx as usize, sy as usize);
    let (ex, ey) = (sx + sw as usize, sy + sh as usize);
    let mut outside = 0u64;
    let mut total = 0u64;
    let mut y = 0usize;
    while y < h {
        let mut x = 0usize;
        while x < w {
            let i = (y * w + x) * 3;
            let d = (luma(rgb[i], rgb[i + 1], rgb[i + 2]) - bg).abs();
            if d > p.luma_threshold as i32 {
                let in_safe = x >= sx && x < ex && y >= sy && y < ey;
                if !in_safe {
                    outside += 1;
                }
            }
            total += 1;
            x += step;
        }
        y += step;
    }
    if total == 0 {
        0.0
    } else {
        outside as f64 / total as f64
    }
}

/// 在帧副本上绘制安全框叠加（违规红色 / 安全绿色），用于证据 PNG。
pub fn overlay_safe_rect(
    rgb: &[u8],
    w: u32,
    h: u32,
    p: &SafeFrameParams,
    violation: bool,
) -> Vec<u8> {
    let mut out = rgb.to_vec();
    let (x, y, sw, sh) = p.safe_rect(w, h);
    let (r, g, b) = if violation {
        (255u8, 32u8, 32u8)
    } else {
        (32u8, 220u8, 32u8)
    };
    let (w_us, h_us) = (w as usize, h as usize);
    let mut put = |px: u32, py: u32| {
        let i = (py as usize * w_us + px as usize) * 3;
        out[i] = r;
        out[i + 1] = g;
        out[i + 2] = b;
    };
    for t in 0..2u32 {
        for px in x..x + sw {
            put(px, y + t);
            put(px, (y + sh - 1).saturating_sub(t).min(h as u32 - 1));
        }
        for py in y..y + sh {
            put(x + t, py);
            put((x + sw - 1).saturating_sub(t).min(w as u32 - 1), py);
        }
    }
    let _ = h_us;
    out
}

/// 分析整个媒体文件，逐帧回调（含原始 RGB，供证据截图）。
pub fn analyze(
    media: &Path,
    params: &SafeFrameParams,
    mut on_frame: impl FnMut(&FrameMetric, &[u8], u32, u32),
) -> Result<Vec<FrameMetric>> {
    gst::init().context("gstreamer init")?;
    let uri = format!("file://{}", media.canonicalize()?.display());
    let pipe = gst::parse::launch(&format!(
        "uridecodebin uri=\"{uri}\" caps=video/x-raw ! videoconvert ! \
         video/x-raw,format=RGB ! appsink name=sink emit-signals=false"
    ))
    .context("build pipeline")?
    .downcast::<gst::Pipeline>()
    .map_err(|_| anyhow!("not a pipeline"))?;
    let sink = pipe
        .by_name("sink")
        .ok_or_else(|| anyhow!("no appsink"))?
        .downcast::<gstreamer_app::AppSink>()
        .map_err(|_| anyhow!("sink downcast"))?;

    pipe.set_state(gst::State::Playing)?;
    let mut out = Vec::new();
    let mut frame_idx = 0u64;
    loop {
        match sink.pull_sample() {
            Ok(sample) => {
                let caps = sample.caps().ok_or_else(|| anyhow!("no caps"))?;
                let info = gstreamer_video::VideoInfo::from_caps(&caps)
                    .map_err(|_| anyhow!("bad video caps"))?;
                let (w, h) = (info.width(), info.height());
                let buf = sample.buffer().ok_or_else(|| anyhow!("no buffer"))?;
                let pts_ms = buf.pts().map(|t| t.mseconds() as i64).unwrap_or(-1);
                let map = buf.map_readable().map_err(|_| anyhow!("map buffer"))?;
                let rgb = map.as_slice();
                let stride = info.stride()[0] as usize;
                // 去掉行对齐 padding，得到紧凑 RGB
                let tight = if stride == (w * 3) as usize {
                    rgb[..(w * h * 3) as usize].to_vec()
                } else {
                    let mut v = Vec::with_capacity((w * h * 3) as usize);
                    for row in 0..h as usize {
                        v.extend_from_slice(&rgb[row * stride..row * stride + w as usize * 3]);
                    }
                    v
                };
                let metric = metric_for_frame(&tight, w, h, params);
                let fm = FrameMetric {
                    pts_ms,
                    frame: frame_idx,
                    metric,
                    violation: metric > params.max_ratio,
                };
                on_frame(&fm, &tight, w, h);
                out.push(fm);
                frame_idx += 1;
            }
            Err(_) => break, // EOS
        }
    }
    pipe.set_state(gst::State::Null)?;
    Ok(out)
}
