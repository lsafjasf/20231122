//! Temporary-state lifecycle helpers.
//!
//! Scratch data lives under the system temp directory and is always removed
//! at the end of a run. Durable evidence (journal + report) stays in the
//! repository's `evidence/` directory and is never deleted here.

use std::path::PathBuf;

/// Scratch directory for one run, keyed by `run_id`.
pub fn temp_dir_for(run_id: &str) -> PathBuf {
    std::env::temp_dir().join(format!("hls-resume-lab-{run_id}"))
}

/// Recursively remove a temp directory, reporting what happened.
pub fn remove_temp_dir(path: &std::path::Path) -> String {
    if !path.exists() {
        format!("{} 不存在，跳过", path.display())
    } else {
        match std::fs::remove_dir_all(path) {
            Ok(()) => format!("已删除临时目录 {}", path.display()),
            Err(err) => format!("删除 {} 失败: {err}", path.display()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_created_temp_dir() {
        let dir = temp_dir_for("unit-test");
        std::fs::create_dir_all(dir.join("nested")).unwrap();
        std::fs::write(dir.join("nested/scratch.txt"), b"x").unwrap();
        let message = remove_temp_dir(&dir);
        assert!(message.contains("已删除"));
        assert!(!dir.exists());
    }
}
