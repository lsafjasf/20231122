/// Metadata extracted from a video file via GStreamer.
#[derive(Debug, Clone)]
pub struct VideoMeta {
    pub duration_ns: u64,
    pub framerate: Option<(u32, u32)>,
}

pub fn init() -> Result<(), String> {
    gstreamer::init().map_err(|e| format!("GStreamer 初始化失败: {e}"))
}

/// Probe a video file with GstDiscoverer: duration + first video framerate.
pub fn probe(uri: &str) -> Result<VideoMeta, String> {
    let discoverer = gstreamer_pbutils::Discoverer::new(gstreamer::ClockTime::from_seconds(15))
        .map_err(|e| format!("无法创建 Discoverer: {e}"))?;
    let info = discoverer
        .discover_uri(uri)
        .map_err(|e| format!("无法解析 {uri}: {e}"))?;

    let duration_ns = info
        .duration()
        .map(|d| d.nseconds())
        .ok_or_else(|| "无法读取视频时长".to_string())?;

    let framerate = info.video_streams().first().map(|s| {
        let f = s.framerate();
        (f.numer() as u32, f.denom() as u32)
    });

    Ok(VideoMeta {
        duration_ns,
        framerate,
    })
}

/// Attach a file to a gtk4 Video widget for preview.
///
/// GTK's media stack is GStreamer-backed; using MediaFile keeps the preview
/// path simple while Discoverer (above) provides authoritative metadata.
pub fn load_into_preview(video: &gtk4::Video, file: &gtk4::gio::File) {
    let media = gtk4::MediaFile::for_file(file);
    video.set_media_stream(Some(&media));
}
