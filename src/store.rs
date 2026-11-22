use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::model::{AttemptStatus, EvidenceEvent, Workspace, now_rfc3339};

pub const WORKSPACE_FILE: &str = "workspace.json";
pub const EVIDENCE_DIR: &str = "evidence";
pub const EVENTS_FILE: &str = "events.jsonl";
pub const ATTEMPTS_DIR: &str = "attempts";

pub struct WorkspaceStore {
    root: PathBuf,
    workspace: Workspace,
    events: Vec<EvidenceEvent>,
}

impl WorkspaceStore {
    pub fn create(root: impl AsRef<Path>, workspace: Workspace) -> Result<Self> {
        workspace.validate().context("工作区数据无效")?;
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(root.join(EVIDENCE_DIR).join(ATTEMPTS_DIR))
            .with_context(|| format!("创建证据目录失败: {}", root.display()))?;
        let mut store = Self {
            root,
            workspace,
            events: Vec::new(),
        };
        store.save()?;
        store.record_event(
            "workspace_created",
            None,
            None,
            "创建修正工作区",
            vec![
                format!("标题: {}", store.workspace.title),
                format!("设备数: {}", store.workspace.devices.len()),
            ],
        )?;
        Ok(store)
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let workspace_path = path.as_ref();
        let root = workspace_path
            .parent()
            .context("工作区文件缺少父目录")?
            .to_path_buf();
        let workspace: Workspace = read_json(workspace_path)?;
        workspace.validate().context("工作区数据无效")?;
        fs::create_dir_all(root.join(EVIDENCE_DIR).join(ATTEMPTS_DIR))?;
        let events = read_events(&root.join(EVIDENCE_DIR).join(EVENTS_FILE))?;
        let mut store = Self {
            root,
            workspace,
            events,
        };
        store.recover_interrupted_attempts()?;
        Ok(store)
    }

    pub fn open_or_create_demo(root: impl AsRef<Path>) -> Result<Self> {
        let workspace_path = root.as_ref().join(WORKSPACE_FILE);
        if workspace_path.exists() {
            Self::open(workspace_path)
        } else {
            let workspace = Workspace::demo("", "");
            Self::create(root, workspace)
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn workspace_path(&self) -> PathBuf {
        self.root.join(WORKSPACE_FILE)
    }

    pub fn evidence_dir(&self) -> PathBuf {
        self.root.join(EVIDENCE_DIR)
    }

    pub fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    pub fn workspace_mut(&mut self) -> &mut Workspace {
        &mut self.workspace
    }

    pub fn events(&self) -> &[EvidenceEvent] {
        &self.events
    }

    pub fn save(&mut self) -> Result<()> {
        self.workspace.validate().context("拒绝保存无效工作区")?;
        let bytes = serde_json::to_vec_pretty(&self.workspace)?;
        atomic_write(&self.workspace_path(), &bytes)?;
        Ok(())
    }

    pub fn record_event(
        &mut self,
        kind: &str,
        segment_id: Option<String>,
        attempt_id: Option<String>,
        summary: &str,
        details: Vec<String>,
    ) -> Result<EvidenceEvent> {
        let seq = self
            .events
            .last()
            .map(|event| event.seq)
            .unwrap_or(0)
            .saturating_add(1);
        let event = EvidenceEvent::new(seq, kind, segment_id, attempt_id, summary, details);
        append_json_line(&self.root.join(EVIDENCE_DIR).join(EVENTS_FILE), &event)?;
        self.events.push(event.clone());
        Ok(event)
    }

    pub fn write_attempt_evidence<T: Serialize>(
        &self,
        attempt_id: &str,
        value: &T,
    ) -> Result<PathBuf> {
        let path = self
            .root
            .join(EVIDENCE_DIR)
            .join(ATTEMPTS_DIR)
            .join(format!("{attempt_id}.json"));
        let bytes = serde_json::to_vec_pretty(value)?;
        atomic_write(&path, &bytes)?;
        Ok(path)
    }

    fn recover_interrupted_attempts(&mut self) -> Result<()> {
        let mut recovered = Vec::new();
        for segment in &mut self.workspace.segments {
            for attempt in &mut segment.attempts {
                if attempt.status == AttemptStatus::Running {
                    let result_path = self
                        .root
                        .join(EVIDENCE_DIR)
                        .join(ATTEMPTS_DIR)
                        .join(format!("{}.result.json", attempt.id));
                    let cached = read_cached_run_result(&result_path).filter(|cached| {
                        cached.attempt_id == attempt.id
                            && cached.segment_id == segment.id
                            && cached.result.status.is_terminal()
                    });
                    let from_worker_evidence = cached.is_some();
                    if let Some(cached) = cached {
                        let result = cached.result;
                        attempt.status = result.status;
                        attempt.pipeline = result.pipeline;
                        attempt.wall_time_ms = result.wall_time_ms;
                        attempt.output_size_bytes = result.output_size_bytes;
                        attempt.output_sha256 = result.output_sha256;
                        attempt.source_size_bytes = result.source_size_bytes;
                        attempt.source_sha256 = result.source_sha256;
                        attempt.error = result.error;
                    } else {
                        attempt.status = AttemptStatus::Interrupted;
                        attempt.error =
                            Some("上次进程退出时任务尚未完成；重新载入后标记为中断".to_string());
                    }
                    attempt.finished_at = now_rfc3339();
                    if attempt.status == AttemptStatus::Succeeded
                        && segment.selected_attempt_id.is_none()
                    {
                        segment.selected_attempt_id = Some(attempt.id.clone());
                    }
                    recovered.push((
                        segment.id.clone(),
                        attempt.id.clone(),
                        attempt.attempt_no,
                        attempt.status,
                        from_worker_evidence,
                    ));
                }
            }
        }
        if recovered.is_empty() {
            return Ok(());
        }
        self.workspace.touch();
        self.save()?;
        for (segment_id, attempt_id, attempt_no, status, from_worker_evidence) in recovered {
            self.record_event(
                if from_worker_evidence {
                    "attempt_recovered_from_worker_evidence"
                } else {
                    "attempt_interrupted_on_reload"
                },
                Some(segment_id),
                Some(attempt_id),
                if from_worker_evidence {
                    "重新进入工作区：从后台结果证据恢复尝试状态"
                } else {
                    "重新进入工作区：发现未完成任务并保留为中断证据"
                },
                vec![
                    format!("尝试序号: {attempt_no}"),
                    format!("恢复状态: {}", status.label()),
                ],
            )?;
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct CachedRunEvidence {
    attempt_id: String,
    segment_id: String,
    result: CachedRunResult,
}

#[derive(Deserialize)]
struct CachedRunResult {
    status: AttemptStatus,
    pipeline: String,
    wall_time_ms: u64,
    output_size_bytes: Option<u64>,
    output_sha256: Option<String>,
    source_size_bytes: Option<u64>,
    source_sha256: Option<String>,
    error: Option<String>,
}

fn read_cached_run_result(path: &Path) -> Option<CachedRunEvidence> {
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("目标文件缺少父目录")?;
    fs::create_dir_all(parent)?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("目标文件名为空")?;
    let tmp_path = parent.join(format!(
        ".{file_name}.tmp-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&tmp_path)
            .with_context(|| format!("创建临时文件失败: {}", tmp_path.display()))?;
        file.write_all(bytes)?;
        file.flush()?;
        file.sync_all()?;
    }
    fs::rename(&tmp_path, path).with_context(|| format!("原子替换失败: {}", path.display()))?;
    if let Ok(dir) = File::open(parent) {
        let _ = dir.sync_all();
    }
    Ok(())
}

fn unique_suffix() -> u128 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default()
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = fs::read(path).with_context(|| format!("读取失败: {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("解析失败: {}", path.display()))
}

fn read_events(path: &Path) -> Result<Vec<EvidenceEvent>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let file = File::open(path).with_context(|| format!("读取证据日志失败: {}", path.display()))?;
    let mut events = Vec::new();
    for (index, line) in BufReader::new(file).lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let event: EvidenceEvent = serde_json::from_str(&line).with_context(|| {
            format!("证据日志第 {} 行损坏；为保留现场已拒绝静默跳过", index + 1)
        })?;
        if let Some(previous) = events.last() {
            let previous: &EvidenceEvent = previous;
            if event.seq <= previous.seq {
                bail!("证据日志序号未严格递增（第 {} 行）", index + 1);
            }
        }
        events.push(event);
    }
    Ok(events)
}

fn append_json_line<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let parent = path.parent().context("证据日志缺少父目录")?;
    fs::create_dir_all(parent)?;
    let mut line = serde_json::to_vec(value)?;
    line.push(b'\n');
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("打开证据日志失败: {}", path.display()))?;
    file.write_all(&line)?;
    file.sync_data()?;
    Ok(())
}
