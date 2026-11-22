use std::path::PathBuf;

/// Result alias used across the crate.
pub type Result<T> = std::result::Result<T, Error>;

/// All error variants produced by vkri-core. Messages are stable enough to be
/// shown in the UI, the CLI and persisted audit records.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("csv error: {0}")]
    Csv(#[from] csv::Error),

    #[error("project not found at {0}")]
    ProjectNotFound(PathBuf),

    #[error("project directory {0} already exists and is not empty")]
    ProjectExists(PathBuf),

    #[error("segment id {0} not found in project")]
    SegmentUnknown(String),

    #[error("device id {0} not found in project")]
    DeviceUnknown(String),

    #[error("comparison id {0} not found in project")]
    ComparisonUnknown(String),

    #[error("segment {segment} is not in a runnable state: {state}")]
    NotRunnable { segment: String, state: String },

    #[error("media file not found or unreadable: {0}")]
    MediaMissing(PathBuf),

    #[error("media probe failed for {path}: {message}")]
    Probe { path: PathBuf, message: String },

    #[error("gstreamer pipeline error: {0}")]
    Pipeline(String),

    #[error("operation cancelled")]
    Cancelled,

    #[error("validation failed: {0}")]
    Validation(String),

    #[error("export verification failed: {0}")]
    Verify(String),
}

impl Error {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Error::Io {
            path: path.into(),
            source,
        }
    }
}
