use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub fn sha256_bytes(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

pub fn sha256_file(path: &Path) -> Result<String, String> {
    let data = std::fs::read(path).map_err(|e| format!("读取 {}: {e}", path.display()))?;
    Ok(sha256_bytes(&data))
}

pub fn canonical_json_sha256<T: Serialize>(value: &T) -> Result<String, String> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    Ok(sha256_bytes(&bytes))
}

pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    let tmp = sibling_temp(path, "json");
    std::fs::write(&tmp, bytes).map_err(|e| format!("写 {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("发布 {}: {e}", path.display()))
}

pub fn write_text_atomic(path: &Path, text: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tmp = sibling_temp(path, "txt");
    std::fs::write(&tmp, text).map_err(|e| format!("写 {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("发布 {}: {e}", path.display()))
}

pub fn copy_evidence(source: &Path, dest: &Path) -> Result<String, String> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::copy(source, dest)
        .map_err(|e| format!("复制证据 {} -> {}: {e}", source.display(), dest.display()))?;
    sha256_file(dest)
}

pub fn sibling_temp(path: &Path, suffix: &str) -> PathBuf {
    let file = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("artifact");
    path.with_file_name(format!(".{file}.{}.{}.tmp", std::process::id(), suffix))
}

pub fn relative_string(base: &Path, path: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

pub fn modified_ns(path: &Path) -> u128 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

pub fn now_unix_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}
