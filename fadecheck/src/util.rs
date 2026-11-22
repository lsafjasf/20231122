//! 通用工具：内容指纹、UTC 时间戳，以及带自动清理的临时状态。

use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// 计算文件的 SHA-256，分块读取以支持较大的音频文件。
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::default();
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        let read = reader.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        hasher.update(&chunk[..read]);
    }
    Ok(hex_encode(&hasher.finalize()))
}

/// 计算内存字节流的 SHA-256，用于快照之间的指纹核对。
pub fn sha256_bytes(data: &[u8]) -> String {
    let mut hasher = Sha256::default();
    hasher.update(data);
    hex_encode(&hasher.finalize())
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// 返回 RFC3339 风格的 UTC 时间戳（秒级，带 `Z` 后缀）。
pub fn utc_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format_unix_utc(secs)
}

/// 把 Unix 秒转换为 `YYYY-MM-DDTHH:MM:SSZ`，不依赖外部时钟库。
fn format_unix_utc(secs: u64) -> String {
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let hour = rem / 3600;
    let minute = (rem % 3600) / 60;
    let second = rem % 60;
    let (year, month, day) = civil_from_days(days as i64);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Howard Hinnant 的公历换算算法。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (year + i64::from(month <= 2), month, day)
}

/// 临时状态句柄：创建工作目录，并在 [`TempState::cleanup`] 时彻底删除。
///
/// 即使处理中途发生错误，调用方也应在 finally 风格的收尾中执行清理，
/// 保证任务结束时磁盘上不残留临时解码产物。
pub struct TempState {
    root: PathBuf,
}

impl TempState {
    pub fn new(label: &str) -> std::io::Result<Self> {
        let unique = format!(
            "fadecheck-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let root = std::env::temp_dir().join(unique);
        std::fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    pub fn path(&self) -> &Path {
        &self.root
    }

    /// 删除整个临时目录。重复调用是安全的。
    pub fn cleanup(&self) {
        if self.root.exists() {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
}

impl Drop for TempState {
    fn drop(&mut self) {
        self.cleanup();
    }
}
