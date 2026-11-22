use std::path::Path;

use anyhow::{bail, Context, Result};

use crate::model::MediaInfo;

pub fn ensure_gstreamer() -> Result<()> {
    gst::init().context("GStreamer 初始化失败")?;
    Ok(())
}

pub fn probe_media(path: &Path) -> Result<MediaInfo> {
    ensure_gstreamer()?;
    let uri = gst::glib::filename_to_uri(path, None)
        .map_err(|_| anyhow::anyhow!("无法将素材路径转换为 URI：{}", path.display()))?;
    let discoverer = gst_pbutils::Discoverer::new(gst::ClockTime::from_seconds(30))
        .context("无法创建 GStreamer Discoverer")?;
    let info = discoverer
        .discover_uri(uri.as_str())
        .with_context(|| format!("GStreamer 无法探测素材：{}", path.display()))?;
    if info.result() != gst_pbutils::DiscovererResult::Ok {
        bail!(
            "GStreamer 探测失败（{:?}）；请安装 gstreamer1.0-plugins-base/good/libav",
            info.result()
        );
    }
    let video = info
        .video_streams()
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("素材中未找到视频轨道"))?;
    let duration = info
        .duration()
        .ok_or_else(|| anyhow::anyhow!("素材时长未知，无法建立三段审查"))?;
    if duration.nseconds() == 0 {
        bail!("素材时长为 0");
    }
    let framerate = video.framerate();
    let container = info
        .container_streams()
        .first()
        .and_then(|stream| {
            use gst_pbutils::prelude::*;
            stream.caps()
        })
        .and_then(|caps| {
            caps.structure(0)
                .map(|structure| structure.name().to_string())
        })
        .unwrap_or_else(|| {
            path.extension()
                .and_then(|extension| extension.to_str())
                .map(|extension| format!("file/{extension}"))
                .unwrap_or_else(|| "unknown".to_string())
        });
    let video_codec = {
        use gst_pbutils::prelude::*;
        video
            .caps()
            .and_then(|caps| {
                caps.structure(0)
                    .map(|structure| structure.name().to_string())
            })
            .unwrap_or_else(|| "unknown".to_string())
    };
    Ok(MediaInfo {
        duration_ns: duration.nseconds(),
        width: video.width(),
        height: video.height(),
        fps_num: framerate.numer().max(0) as u32,
        fps_den: framerate.denom().max(1) as u32,
        container,
        video_codec,
        probe_engine: gst::version_string().to_string(),
    })
}
