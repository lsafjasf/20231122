//! 生成确定性测试媒体：深灰背景 + 从中心向左上角漂移的白色方块，
//! 方块在可计算的帧号越过字幕安全框，用于端到端复核。
use anyhow::{Context, Result, anyhow};
use gstreamer as gst;
use gstreamer::prelude::*;
use std::path::Path;

pub const GEN_W: u32 = 320;
pub const GEN_H: u32 = 180;
pub const GEN_FPS: u32 = 10;
pub const BOX: u32 = 40;

/// 第 `frame` 帧的 RGB 图像（与检测端共享，保证期望可计算）。
pub fn synthetic_frame(frame: u64) -> Vec<u8> {
    let (w, h) = (GEN_W as usize, GEN_H as usize);
    let mut rgb = vec![32u8; w * h * 3];
    // 方块从中心出发，每帧左移 2px、上移 1px
    let cx = 140i64 - 2 * frame as i64;
    let cy = 70i64 - frame as i64;
    for y in cy..cy + BOX as i64 {
        for x in cx..cx + BOX as i64 {
            if x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h {
                let i = (y as usize * w + x as usize) * 3;
                rgb[i] = 255;
                rgb[i + 1] = 255;
                rgb[i + 2] = 255;
            }
        }
    }
    rgb
}

/// 用 appsrc → matroskamux 写无损 raw 视频（解码确定，复核可逐比特对齐）。
pub fn generate(media: &Path, frames: u64) -> Result<()> {
    gst::init().context("gstreamer init")?;
    let pipe = gst::parse::launch(&format!(
        "appsrc name=src is-live=false format=time ! videoconvert ! \
         matroskamux ! filesink location=\"{}\"",
        media.display()
    ))
    .context("build generate pipeline")?
    .downcast::<gst::Pipeline>()
    .map_err(|_| anyhow!("not a pipeline"))?;
    let src = pipe
        .by_name("src")
        .ok_or_else(|| anyhow!("no appsrc"))?
        .downcast::<gstreamer_app::AppSrc>()
        .map_err(|_| anyhow!("src downcast"))?;
    src.set_caps(Some(
        &gst::Caps::builder("video/x-raw")
            .field("format", "RGB")
            .field("width", GEN_W as i32)
            .field("height", GEN_H as i32)
            .field("framerate", gst::Fraction::new(GEN_FPS as i32, 1))
            .build(),
    ));

    pipe.set_state(gst::State::Playing)?;
    let frame_dur = gst::ClockTime::from_mseconds(1000 / GEN_FPS as u64);
    for f in 0..frames {
        let rgb = synthetic_frame(f);
        let mut buf = gst::Buffer::with_size(rgb.len()).map_err(|_| anyhow!("alloc"))?;
        {
            let bref = buf.get_mut().ok_or_else(|| anyhow!("buf mut"))?;
            bref.set_pts(gst::ClockTime::from_mseconds(f * 1000 / GEN_FPS as u64));
            bref.set_dts(gst::ClockTime::from_mseconds(f * 1000 / GEN_FPS as u64));
            bref.set_duration(frame_dur);
            let mut map = bref.map_writable().map_err(|_| anyhow!("map"))?;
            map.copy_from_slice(&rgb);
        }
        src.push_buffer(buf).map_err(|e| anyhow!("push: {e:?}"))?;
    }
    src.end_of_stream().map_err(|e| anyhow!("eos: {e:?}"))?;

    let bus = pipe.bus().ok_or_else(|| anyhow!("no bus"))?;
    for msg in bus.iter_timed(gst::ClockTime::from_seconds(30)) {
        match msg.view() {
            gst::MessageView::Eos(_) => break,
            gst::MessageView::Error(e) => {
                pipe.set_state(gst::State::Null)?;
                return Err(anyhow!("pipeline error: {}", e.error()));
            }
            _ => {}
        }
    }
    pipe.set_state(gst::State::Null)?;
    Ok(())
}
