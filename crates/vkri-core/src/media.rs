//! Media file registration and probing.

use std::path::{Path, PathBuf};

use crate::hash::sha256_file;
use crate::{Error, Result};

#[cfg(feature = "gstreamer")]
use gstreamer_pbutils::prelude::DiscovererStreamInfoExt;

/// Probed media metadata captured at registration time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaInfo {
    pub sha256: String,
    pub bytes: u64,
    pub duration_ns: Option<i64>,
    pub width: u32,
    pub height: u32,
    pub container_name: String,
    pub video_caps: String,
}

#[cfg(feature = "gstreamer")]
mod gst {
    use std::path::Path;

    use gstreamer_pbutils::{Discoverer, DiscovererInfo};

    use crate::{Error, Result};

    pub fn init() -> Result<()> {
        gstreamer::init().map_err(|e| Error::Pipeline(format!("gstreamer init: {e:?}")))
    }

    pub fn discover(path: &Path) -> Result<DiscovererInfo> {
        let discoverer =
            Discoverer::new(gstreamer::ClockTime::from_seconds(15)).map_err(|e| Error::Probe {
                path: path.to_path_buf(),
                message: format!("discoverer construction: {e:?}"),
            })?;
        let uri = glib::filename_to_uri(path, None).map_err(|e| Error::Probe {
            path: path.to_path_buf(),
            message: format!("uri conversion: {e:?}"),
        })?;
        discoverer.discover_uri(&uri).map_err(|e| Error::Probe {
            path: path.to_path_buf(),
            message: format!("discover: {e:?}"),
        })
    }
}

#[cfg(not(feature = "gstreamer"))]
mod gst {
    use std::path::Path;

    use crate::{Error, Result};

    pub fn init() -> Result<()> {
        Err(Error::Pipeline(
            "binary built without the gstreamer feature".into(),
        ))
    }

    pub fn discover(_path: &Path) -> Result<std::convert::Infallible> {
        Err(Error::Pipeline(
            "binary built without the gstreamer feature".into(),
        ))
    }
}

/// One-time GStreamer initialization; safe to call repeatedly.
pub fn init_gstreamer() -> Result<()> {
    gst::init()
}

/// Hash and probe a media file before registering it.
pub fn inspect_media(path: &Path) -> Result<MediaInfo> {
    if !path.is_file() {
        return Err(Error::MediaMissing(path.to_path_buf()));
    }
    let sha256 = sha256_file(path)?;
    let bytes = std::fs::metadata(path)
        .map_err(|e| Error::io(path, e))?
        .len();

    #[cfg(feature = "gstreamer")]
    {
        let info = gst::discover(path)?;
        let duration_ns = info.duration().map(|time| time.nseconds() as i64);
        let (width, height, video_caps) = info
            .video_streams()
            .first()
            .map(|stream| {
                let caps = stream.caps().map(|c| c.to_string()).unwrap_or_default();
                (stream.width(), stream.height(), caps)
            })
            .unwrap_or((0, 0, String::new()));
        let container_name = info
            .container_streams()
            .first()
            .and_then(|container| container.caps().map(|caps| caps.to_string()))
            .unwrap_or_else(|| "unknown".to_string());
        Ok(MediaInfo {
            sha256,
            bytes,
            duration_ns,
            width,
            height,
            container_name,
            video_caps,
        })
    }

    #[cfg(not(feature = "gstreamer"))]
    {
        Ok(MediaInfo {
            sha256,
            bytes,
            duration_ns: None,
            width: 0,
            height: 0,
            container_name: "unknown".to_string(),
            video_caps: String::new(),
        })
    }
}

/// Validate that a registered source still hashes to the recorded digest.
pub fn verify_source(path: &Path, expected_sha256: &str) -> Result<()> {
    if !path.is_file() {
        return Err(Error::MediaMissing(PathBuf::from(path)));
    }
    let actual = sha256_file(path)?;
    if !crate::hash::digest_equal(&actual, expected_sha256) {
        return Err(Error::Verify(format!(
            "source hash mismatch for {}: expected {expected_sha256}, got {actual}",
            path.display()
        )));
    }
    Ok(())
}
