//! 临时工作区：处理期间落盘原始输入，结束时自动清理全部临时状态。

use std::fs;
use std::path::{Path, PathBuf};

pub struct TempWorkspace {
    root: PathBuf,
    cleaned: bool,
}

impl TempWorkspace {
    /// 在系统临时目录下创建独立工作区（前缀 `submerge-check-`）。
    pub fn create() -> std::io::Result<Self> {
        let root = std::env::temp_dir().join(format!(
            "submerge-check-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(&root)?;
        Ok(Self {
            root,
            cleaned: false,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 将一份原始输入原样写入工作区，返回落盘路径。
    pub fn stage(&self, name: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
        let path = self.root.join(name);
        fs::write(&path, bytes)?;
        Ok(path)
    }

    /// 显式清理；重复调用安全。
    pub fn cleanup(&mut self) -> std::io::Result<()> {
        if self.cleaned {
            return Ok(());
        }
        if self.root.exists() {
            fs::remove_dir_all(&self.root)?;
        }
        self.cleaned = true;
        Ok(())
    }

    /// 仅供测试与报告使用：当前工作区仍存在。
    pub fn exists(&self) -> bool {
        self.root.exists()
    }
}

impl Drop for TempWorkspace {
    fn drop(&mut self) {
        // RAII 兜底：即使发生 panic 或提前返回，临时状态也会被清理。
        if let Err(err) = self.cleanup() {
            eprintln!("[workspace] 清理临时目录 {:?} 失败: {err}", self.root);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_creates_stages_and_cleans_up() {
        let mut ws = TempWorkspace::create().unwrap();
        let path = ws
            .stage("a.srt", b"1\n00:00:01,000 --> 00:00:02,000\nhi\n")
            .unwrap();
        assert!(path.exists());
        assert!(ws.exists());
        ws.cleanup().unwrap();
        assert!(!ws.exists());
        // 重复清理不报错。
        ws.cleanup().unwrap();
    }

    #[test]
    fn drop_cleans_temp_state() {
        let root;
        {
            let ws = TempWorkspace::create().unwrap();
            root = ws.root().to_path_buf();
            assert!(root.exists());
        }
        assert!(!root.exists());
    }
}
