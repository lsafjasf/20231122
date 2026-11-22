use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid CSV: {0}")]
    Csv(#[from] csv::Error),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("limit exceeded for {what}: actual={actual}, limit={limit}")]
    LimitExceeded {
        what: &'static str,
        actual: u64,
        limit: u64,
    },
    #[error("audio error: {0}")]
    Audio(String),
    #[error("GStreamer error: {0}")]
    GStreamer(String),
    #[error("artifact verification failed: {0}")]
    Verification(String),
}

pub type AppResult<T> = Result<T, AppError>;

pub fn io_error(path: impl Into<PathBuf>, source: std::io::Error) -> AppError {
    AppError::Io {
        path: path.into(),
        source,
    }
}
