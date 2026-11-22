use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("cannot read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot write {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid project JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported project schema: {0}")]
    UnsupportedSchema(String),
    #[error("invalid time value: {0}")]
    InvalidTime(String),
    #[error("invalid project: {0}")]
    InvalidProject(String),
    #[error("integrity check failed: {0}")]
    Integrity(String),
}

pub type Result<T> = std::result::Result<T, CoreError>;
