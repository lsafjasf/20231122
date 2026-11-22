//! UI 辅助：用 GStreamer 把 PNG 证据文件解码成 gdk::MemoryTexture。
use anyhow::{Context, Result, anyhow};
use gstreamer as gst;
use gstreamer::prelude::*;
use gtk4::gdk;
use gtk4::glib;
use std::path::Path;

pub fn load_png_texture(path: &Path) -> Result<gdk::Texture> {
    gst::init().context("gstreamer init")?;
    let pipe = gst::parse::launch(&format!(
        "filesrc location=\"{}\" ! pngdec ! videoconvert ! video/x-raw,format=RGB ! \
         appsink name=sink emit-signals=false",
        path.display()
    ))?
    .downcast::<gst::Pipeline>()
    .map_err(|_| anyhow!("not a pipeline"))?;
    let sink = pipe
        .by_name("sink")
        .ok_or_else(|| anyhow!("no appsink"))?
        .downcast::<gstreamer_app::AppSink>()
        .map_err(|_| anyhow!("sink downcast"))?;
    pipe.set_state(gst::State::Playing)?;
    let sample = sink.pull_sample().map_err(|e| anyhow!("pull: {e:?}"))?;
    let caps = sample.caps().ok_or_else(|| anyhow!("no caps"))?;
    let info = gstreamer_video::VideoInfo::from_caps(&caps).map_err(|_| anyhow!("bad caps"))?;
    let buf = sample.buffer().ok_or_else(|| anyhow!("no buffer"))?;
    let map = buf.map_readable().map_err(|_| anyhow!("map"))?;
    let (w, h) = (info.width() as i32, info.height() as i32);
    let stride = info.stride()[0] as usize;
    let tight = if stride == (w as usize) * 3 {
        map.as_slice()[..(w * h * 3) as usize].to_vec()
    } else {
        let mut v = Vec::with_capacity((w * h * 3) as usize);
        for row in 0..h as usize {
            v.extend_from_slice(&map[row * stride..row * stride + w as usize * 3]);
        }
        v
    };
    pipe.set_state(gst::State::Null)?;
    let bytes = glib::Bytes::from_owned(tight);
    Ok(gdk::MemoryTexture::new(w, h, gdk::MemoryFormat::R8g8b8, &bytes, (w as usize) * 3).upcast())
}
