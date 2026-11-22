use std::path::PathBuf;

use gstreamer as gst;

#[derive(Debug, thiserror::Error)]
pub enum DownmixError {
    #[error("io error on {path:?}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("unsupported or unreadable audio object: {0:?}")]
    Unsupported(PathBuf),
    #[error("gstreamer error: {0}")]
    Gst(String),
    #[error("pipeline state error: {0}")]
    State(String),
    #[error("manifest error: {0}")]
    Manifest(String),
    #[error("verification failed for {0:?}: {1}")]
    Verification(PathBuf, String),
    #[error("audio stream error: {0}")]
    Stream(String),
}

pub type Result<T> = std::result::Result<T, DownmixError>;

impl DownmixError {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

impl From<gst::glib::BoolError> for DownmixError {
    fn from(value: gst::glib::BoolError) -> Self {
        Self::Gst(value.to_string())
    }
}

impl From<gst::StateChangeError> for DownmixError {
    fn from(value: gst::StateChangeError) -> Self {
        Self::State(value.to_string())
    }
}
