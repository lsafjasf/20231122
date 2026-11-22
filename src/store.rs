//! Atomic session persistence.

use crate::model::{SCHEMA_VERSION, Session};
use anyhow::{Context, Result, bail};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct SessionStore {
    path: PathBuf,
}

#[derive(Debug)]
pub struct LoadResult {
    pub session: Session,
    pub recovered: bool,
    pub used_backup: bool,
}

impl SessionStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> Result<Option<LoadResult>> {
        if !self.path.exists() {
            return Ok(None);
        }
        let primary = self.parse_file(&self.path);
        let (mut session, used_backup) = match primary {
            Ok(session) => (session, false),
            Err(primary_error) => {
                let backup_path = self.backup_path();
                if backup_path.exists() {
                    match self.parse_file(&backup_path) {
                        Ok(session) => (session, true),
                        Err(backup_error) => {
                            let preserved = self.preserve_corrupt();
                            return Err(backup_error).with_context(|| {
                                format!(
                                    "主会话与备份均损坏；主文件错误：{primary_error:#}；\
                                     损坏主文件已保留：{}",
                                    preserved
                                        .map(|path| path.display().to_string())
                                        .unwrap_or_else(|| "保留失败".to_owned())
                                )
                            });
                        }
                    }
                } else {
                    let preserved = self.preserve_corrupt();
                    return Err(primary_error).with_context(|| {
                        format!(
                            "会话损坏且没有备份；损坏文件已保留：{}",
                            preserved
                                .map(|path| path.display().to_string())
                                .unwrap_or_else(|| "保留失败".to_owned())
                        )
                    });
                }
            }
        };
        let had_running = session
            .segments
            .iter()
            .any(|segment| segment.status == crate::model::SegmentStatus::Running);
        session.recover_interrupted();
        let recovered = had_running;
        Ok(Some(LoadResult {
            session,
            recovered,
            used_backup,
        }))
    }

    pub fn save(&self, session: &Session) -> Result<()> {
        session.validate().map_err(anyhow::Error::msg)?;
        if session.schema_version != SCHEMA_VERSION {
            bail!("refusing to save unsupported schema");
        }
        let parent = self
            .path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)
            .with_context(|| format!("创建会话目录失败：{}", parent.display()))?;

        let data = serde_json::to_vec_pretty(session).context("序列化会话失败")?;
        let temp_path = self
            .path
            .with_extension(format!("json.tmp-{}", std::process::id()));
        {
            let mut temp = File::create(&temp_path)
                .with_context(|| format!("创建临时会话失败：{}", temp_path.display()))?;
            temp.write_all(&data).context("写入临时会话失败")?;
            temp.write_all(b"\n").context("写入会话结束符失败")?;
            temp.sync_all().context("同步临时会话失败")?;
        }

        if self.path.exists() && self.parse_file(&self.path).is_ok() {
            fs::copy(&self.path, self.backup_path())
                .with_context(|| format!("更新会话备份失败：{}", self.backup_path().display()))?;
        }
        fs::rename(&temp_path, &self.path)
            .with_context(|| format!("原子替换会话失败：{}", self.path.display()))?;
        sync_directory(parent);
        Ok(())
    }

    fn parse_file(&self, path: &Path) -> Result<Session> {
        let data = fs::read(path).with_context(|| format!("读取会话失败：{}", path.display()))?;
        let session: Session = serde_json::from_slice(&data)
            .with_context(|| format!("解析会话 JSON 失败：{}", path.display()))?;
        session.validate().map_err(anyhow::Error::msg)?;
        Ok(session)
    }

    fn backup_path(&self) -> PathBuf {
        self.path.with_extension("json.bak")
    }

    fn preserve_corrupt(&self) -> Option<PathBuf> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_secs();
        let path = self.path.with_extension(format!("json.corrupt-{stamp}"));
        fs::copy(&self.path, &path).ok().map(|_| path)
    }
}

fn sync_directory(path: &Path) {
    if let Ok(directory) = File::open(path) {
        let _ = directory.sync_all();
    }
}

pub fn default_session_path() -> PathBuf {
    if let Ok(path) = std::env::var("QP_SESSION") {
        return PathBuf::from(path);
    }
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("artifacts")
        .join("queue_session.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::seed_session;

    #[test]
    fn save_and_load_preserve_segment_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/session.json");
        let store = SessionStore::new(&path);
        let mut session = seed_session(dir.path());
        session.segments.reverse();
        let expected: Vec<String> = session
            .segments
            .iter()
            .map(|segment| segment.id.clone())
            .collect();
        store.save(&session).unwrap();
        let loaded = store.load().unwrap().unwrap();
        let actual: Vec<String> = loaded
            .session
            .segments
            .iter()
            .map(|segment| segment.id.clone())
            .collect();
        assert_eq!(expected, actual);
    }

    #[test]
    fn backup_is_used_when_primary_is_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        let store = SessionStore::new(&path);
        let session = seed_session(dir.path());
        store.save(&session).unwrap();
        store.save(&session).unwrap();
        std::fs::write(&path, b"{not json").unwrap();
        let loaded = store.load().unwrap().unwrap();
        assert!(loaded.used_backup);
        assert_eq!(loaded.session.segments.len(), session.segments.len());

        // Saving a session recovered from the backup must not copy the corrupt
        // primary over the last healthy backup before the atomic rename.
        store.save(&loaded.session).unwrap();
        std::fs::write(&path, b"{corrupt again").unwrap();
        let loaded_again = store.load().unwrap().unwrap();
        assert!(loaded_again.used_backup);
        assert_eq!(loaded_again.session.segments.len(), session.segments.len());
    }

    #[test]
    fn unrecoverable_session_is_preserved_before_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        let store = SessionStore::new(&path);
        let session = seed_session(dir.path());
        store.save(&session).unwrap();
        store.save(&session).unwrap();
        std::fs::write(&path, b"broken-primary").unwrap();
        std::fs::write(store.backup_path(), b"broken-backup").unwrap();
        assert!(store.load().is_err());
        let preserved: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains(".corrupt-"))
            .collect();
        assert_eq!(preserved.len(), 1);
    }
}
