use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// 临时工作区：管线运行期间存放中间产物，Drop 时自动清理。
#[derive(Debug)]
pub struct TempWorkspace {
    root: PathBuf,
}

impl TempWorkspace {
    pub fn create() -> std::io::Result<Self> {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let root = std::env::temp_dir().join(format!(
            "rotfix-prov-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    pub fn path(&self) -> &PathBuf {
        &self.root
    }

    /// 写入一个中间产物，返回路径。
    pub fn stage(&self, name: &str, content: &str) -> std::io::Result<PathBuf> {
        let path = self.root.join(name);
        fs::write(&path, content)?;
        Ok(path)
    }
}

impl Drop for TempWorkspace {
    fn drop(&mut self) {
        // 结束时清理临时状态。
        let _ = fs::remove_dir_all(&self.root);
    }
}
