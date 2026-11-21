//! 会话持久化：支持中途退出与重新进入，证据文件独立保存。
use crate::model::Segment;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub const SESSION_FILE: &str = "session.json";
pub const EVIDENCE_DIR: &str = "evidence";
pub const RUN_LOG: &str = "runs.jsonl";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub version: u32,
    pub media_uri: Option<String>,
    #[serde(default)]
    pub segments: Vec<Segment>,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            version: 1,
            media_uri: None,
            segments: Vec::new(),
        }
    }
}

/// 一个审查项目目录：session.json + evidence/
#[derive(Debug, Clone)]
pub struct Project {
    pub root: PathBuf,
    pub session: Session,
}

impl Project {
    pub fn session_path(&self) -> PathBuf {
        self.root.join(SESSION_FILE)
    }

    pub fn evidence_dir(&self) -> PathBuf {
        self.root.join(EVIDENCE_DIR)
    }

    /// 打开已有项目或创建空项目（重新进入的入口）
    pub fn open_or_create(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        fs::create_dir_all(root.join(EVIDENCE_DIR))
            .with_context(|| format!("创建证据目录失败: {}", root.display()))?;
        let path = root.join(SESSION_FILE);
        let session = if path.exists() {
            let text = fs::read_to_string(&path)
                .with_context(|| format!("读取会话失败: {}", path.display()))?;
            serde_json::from_str(&text).with_context(|| "会话文件损坏")?
        } else {
            Session::default()
        };
        Ok(Self { root, session })
    }

    /// 原子写会话，避免中途退出时写坏文件
    pub fn save(&self) -> Result<()> {
        let path = self.session_path();
        let tmp = path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(&self.session)?;
        fs::write(&tmp, text)?;
        fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// 追加一条运行记录（关键证据日志，只增不删）
    pub fn append_run_log(&self, segment_id: &str, run: &crate::model::RunResult) -> Result<()> {
        use std::io::Write;
        let mut line = serde_json::to_value(run)?;
        line["segment_id"] = serde_json::Value::String(segment_id.to_string());
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.evidence_dir().join(RUN_LOG))?;
        writeln!(f, "{}", serde_json::to_string(&line)?)?;
        Ok(())
    }

    /// 证据快照路径：evidence/<segment_id>/<name>.png
    pub fn snapshot_path(&self, segment_id: &str, name: &str) -> Result<PathBuf> {
        let dir = self.evidence_dir().join(segment_id);
        fs::create_dir_all(&dir)?;
        Ok(dir.join(format!("{name}.png")))
    }

    /// 导出某个片段的报告与证据到目标目录
    pub fn export_segment(&self, segment_id: &str, dest: &Path) -> Result<PathBuf> {
        let seg = self
            .session
            .segments
            .iter()
            .find(|s| s.id == segment_id)
            .with_context(|| format!("片段不存在: {segment_id}"))?;
        let out = dest.join(&seg.id);
        fs::create_dir_all(&out)?;
        let report = serde_json::to_string_pretty(seg)?;
        fs::write(out.join("report.json"), report)?;
        let ev = self.evidence_dir().join(&seg.id);
        if ev.exists() {
            for entry in fs::read_dir(&ev)? {
                let entry = entry?;
                if entry.path().extension().is_some_and(|e| e == "png") {
                    fs::copy(entry.path(), out.join(entry.file_name()))?;
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ExtremeParams, RunResult};

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ssr-test-{}-{}",
            tag,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn exit_and_reenter_preserves_state() {
        let root = temp_root("reenter");
        {
            let mut p = Project::open_or_create(&root).unwrap();
            p.session.media_uri = Some("file:///tmp/a.mp4".into());
            p.session
                .segments
                .push(Segment::new("s1".into(), "缺失片段1".into(), 1000, 2000));
            p.save().unwrap();
        } // 模拟中途退出
        let p = Project::open_or_create(&root).unwrap();
        assert_eq!(p.session.media_uri.as_deref(), Some("file:///tmp/a.mp4"));
        assert_eq!(p.session.segments.len(), 1);
        assert_eq!(p.session.segments[0].start_ms, 1000);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn run_log_appends() {
        let root = temp_root("log");
        let p = Project::open_or_create(&root).unwrap();
        let run = RunResult {
            run_at_unix: 1,
            params: ExtremeParams::default(),
            frames_scanned: 10,
            violating_frames: 2,
            max_ink_ratio: 0.5,
            mean_ink_ratio: 0.1,
            entry_boundary: None,
            exit_boundary: None,
        };
        p.append_run_log("s1", &run).unwrap();
        p.append_run_log("s1", &run).unwrap();
        let text = fs::read_to_string(p.evidence_dir().join(RUN_LOG)).unwrap();
        assert_eq!(text.lines().count(), 2);
        let _ = fs::remove_dir_all(&root);
    }
}
