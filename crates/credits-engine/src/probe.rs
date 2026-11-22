use std::path::Path;

use anyhow::{anyhow, Context, Result};
use credits_core::report::{ProbeEvidence, StreamEvidence};
use gstreamer as gst;
use gstreamer_pbutils::prelude::*;

use crate::gst_support::ensure_gst;

pub fn probe_media(path: &Path) -> Result<ProbeEvidence> {
    ensure_gst()?;
    let uri = gst::glib::filename_to_uri(path, None)
        .map_err(|error| anyhow!("cannot build URI for {}: {error}", path.display()))?;
    let discoverer = gstreamer_pbutils::Discoverer::new(gst::ClockTime::from_seconds(15))
        .map_err(|error| anyhow!("cannot create GStreamer discoverer: {error}"))?;
    let info = discoverer
        .discover_uri(uri.as_str())
        .with_context(|| format!("GStreamer could not discover {}", path.display()))?;

    let mut evidence = ProbeEvidence {
        uri: uri.to_string(),
        duration_s: info
            .duration()
            .map(|duration| duration.nseconds() as f64 / 1e9),
        ..ProbeEvidence::default()
    };

    if let Some(container) = info.container_streams().first() {
        if let Some(caps) = container.caps() {
            evidence.container = caps_name(&caps);
        }
    }

    for (index, stream) in info.video_streams().iter().enumerate() {
        let caps = stream.caps();
        let fps = stream.framerate();
        let fps = if fps.denom() > 0 {
            Some(fps.numer() as f64 / fps.denom() as f64)
        } else {
            None
        };
        evidence.width.get_or_insert(stream.width());
        evidence.height.get_or_insert(stream.height());
        if evidence.fps.is_none() {
            evidence.fps = fps;
        }
        if evidence.video_codec.is_none() {
            evidence.video_codec = caps.as_ref().and_then(caps_name);
        }
        evidence.streams.push(StreamEvidence {
            index: index as u32,
            kind: "video".to_string(),
            caps: caps_summary(caps.as_ref()),
            codec: caps.as_ref().and_then(caps_name),
            width: Some(stream.width()),
            height: Some(stream.height()),
            fps,
            sample_rate: None,
            channels: None,
        });
    }

    for (index, stream) in info.audio_streams().iter().enumerate() {
        let caps = stream.caps();
        if evidence.audio_codec.is_none() {
            evidence.audio_codec = caps.as_ref().and_then(caps_name);
        }
        evidence.streams.push(StreamEvidence {
            index: index as u32,
            kind: "audio".to_string(),
            caps: caps_summary(caps.as_ref()),
            codec: caps.as_ref().and_then(caps_name),
            width: None,
            height: None,
            fps: None,
            sample_rate: Some(stream.sample_rate()),
            channels: Some(stream.channels()),
        });
    }

    if evidence.container.is_none() {
        evidence.container = info
            .stream_info()
            .and_then(|stream| stream.caps())
            .as_ref()
            .and_then(caps_name);
    }
    if evidence.container.is_none() {
        evidence.container = info
            .stream_info()
            .map(|stream| stream.stream_type_nick().to_string())
            .filter(|nick| !nick.is_empty());
    }

    Ok(evidence)
}

fn caps_name(caps: &gst::Caps) -> Option<String> {
    caps.structure(0)
        .map(|structure| structure.name().to_string())
}

fn caps_summary(caps: Option<&gst::Caps>) -> String {
    let Some(caps) = caps else {
        return String::new();
    };
    let full = caps.to_string();
    if full.chars().count() <= 512 {
        return full;
    }
    let truncated: String = full.chars().take(512).collect();
    format!("{truncated}... [truncated]")
}

pub fn gstreamer_version() -> String {
    gst::version_string().to_string()
}
