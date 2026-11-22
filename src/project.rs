use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::evidence::{append_event, now_rfc3339, sha256_file};
use crate::model::{MediaInfo, OriginalMaterial, ProjectSession, RunStatus, SCHEMA_VERSION};

pub const SESSION_FILE: &str = "session.json";
pub const ORIGINAL_DIR: &str = "original";
pub const RUNS_DIR: &str = "runs";
pub const EVIDENCE_DIR: &str = "evidence";
pub const EXPORTS_DIR: &str = "exports";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrityReport {
    pub verified: bool,
    pub expected_sha256: String,
    pub actual_sha256: Option<String>,
    pub read_only: bool,
    pub stored_path: String,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct OpenedProject {
    pub session: ProjectSession,
    pub integrity: IntegrityReport,
}

pub fn project_dirs(root: &Path) -> [PathBuf; 4] {
    [
        root.join(ORIGINAL_DIR),
        root.join(RUNS_DIR),
        root.join(EVIDENCE_DIR),
        root.join(EXPORTS_DIR),
    ]
}

pub fn default_projects_root() -> PathBuf {
    data_home().join("mask-edge-review").join("projects")
}

pub fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".config"))
        .join("mask-edge-review")
}

pub fn last_project_file() -> PathBuf {
    config_dir().join("last-project")
}

pub fn remember_last_project(root: &Path) -> Result<()> {
    fs::create_dir_all(config_dir()).context("无法创建配置目录")?;
    let path = last_project_file();
    fs::write(&path, root.to_string_lossy().as_bytes())
        .with_context(|| format!("无法记录最近项目：{}", path.display()))
}

pub fn load_last_project() -> Option<PathBuf> {
    let path = last_project_file();
    let value = fs::read_to_string(path).ok()?;
    let root = PathBuf::from(value.trim());
    if root.join(SESSION_FILE).is_file() {
        Some(root)
    } else {
        None
    }
}

pub fn create_project(
    root: &Path,
    source: &Path,
    title: String,
    media: MediaInfo,
) -> Result<OpenedProject> {
    if root.exists() {
        let mut entries = fs::read_dir(root).context("无法读取目标项目目录")?;
        if entries.next().is_some() {
            bail!("项目目录非空：{}", root.display());
        }
    }
    fs::create_dir_all(root).context("无法创建项目目录")?;
    for dir in project_dirs(root) {
        fs::create_dir_all(&dir).with_context(|| format!("无法创建目录：{}", dir.display()))?;
    }
    let original = import_original(source, root)?;
    let session = ProjectSession::new(title, original, media, root.to_path_buf());
    save_session(&session)?;
    append_event(
        root,
        "project_created",
        None,
        None,
        "项目已创建，原始素材以只读副本保存",
        json!({
            "source_sha256": session.original.sha256,
            "source_size": session.original.size_bytes,
            "stored_relative_path": session.original.stored_relative_path,
        }),
    )?;
    remember_last_project(root)?;
    let integrity = verify_original(&session);
    Ok(OpenedProject { session, integrity })
}

pub fn open_project(root: &Path) -> Result<OpenedProject> {
    let root = root
        .canonicalize()
        .with_context(|| format!("项目目录不存在：{}", root.display()))?;
    let session_path = root.join(SESSION_FILE);
    let text = fs::read_to_string(&session_path)
        .with_context(|| format!("无法读取项目会话：{}", session_path.display()))?;
    let mut session: ProjectSession = serde_json::from_str(&text).context("项目会话 JSON 损坏")?;
    if session.schema_version > SCHEMA_VERSION {
        bail!(
            "项目版本 {} 高于当前支持版本 {}",
            session.schema_version,
            SCHEMA_VERSION
        );
    }
    session.project_dir = root.clone();
    session.validate().map_err(anyhow::Error::msg)?;
    let interrupted = session.mark_abandoned_runs_interrupted();
    if interrupted > 0 {
        save_session(&session)?;
        append_event(
            &root,
            "runs_interrupted",
            None,
            None,
            format!("恢复会话时发现 {interrupted} 个未完成运行"),
            json!({ "count": interrupted }),
        )?;
    }
    let integrity = verify_original(&session);
    append_event(
        &root,
        "session_opened",
        None,
        None,
        "重新进入审查会话",
        json!({
            "review_position_ns": session.review_position_ns,
            "selected_segment": session.selected_segment,
            "original_verified": integrity.verified,
        }),
    )?;
    remember_last_project(&root)?;
    Ok(OpenedProject { session, integrity })
}

pub fn inspect_project(root: &Path) -> Result<OpenedProject> {
    let root = root
        .canonicalize()
        .with_context(|| format!("项目目录不存在：{}", root.display()))?;
    let session_path = root.join(SESSION_FILE);
    let text = fs::read_to_string(&session_path)
        .with_context(|| format!("无法读取项目会话：{}", session_path.display()))?;
    let mut session: ProjectSession = serde_json::from_str(&text).context("项目会话 JSON 损坏")?;
    if session.schema_version > SCHEMA_VERSION {
        bail!(
            "项目版本 {} 高于当前支持版本 {}",
            session.schema_version,
            SCHEMA_VERSION
        );
    }
    session.project_dir = root;
    session.validate().map_err(anyhow::Error::msg)?;
    let integrity = verify_original(&session);
    Ok(OpenedProject { session, integrity })
}

pub fn save_session(session: &ProjectSession) -> Result<()> {
    let root = &session.project_dir;
    fs::create_dir_all(root).context("项目目录不可用")?;
    let mut to_save = session.clone();
    to_save.updated_at = now_rfc3339();
    let text = serde_json::to_vec_pretty(&to_save).context("会话序列化失败")?;
    let temp = root.join(format!(".{SESSION_FILE}.{}.tmp", std::process::id()));
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temp)
        .with_context(|| format!("无法创建临时会话：{}", temp.display()))?;
    file.write_all(&text).context("会话写入失败")?;
    file.write_all(b"\n").context("会话换行写入失败")?;
    file.sync_all().context("会话落盘失败")?;
    drop(file);
    fs::rename(&temp, root.join(SESSION_FILE)).context("会话原子替换失败")?;
    Ok(())
}

pub fn verify_original(session: &ProjectSession) -> IntegrityReport {
    let stored = session
        .project_dir
        .join(&session.original.stored_relative_path);
    let expected = session.original.sha256.clone();
    if !stored.is_file() {
        return IntegrityReport {
            verified: false,
            expected_sha256: expected,
            actual_sha256: None,
            read_only: false,
            stored_path: stored.display().to_string(),
            message: "只读原始副本缺失".to_string(),
        };
    }
    let read_only = !is_writable(&stored);
    match sha256_file(&stored) {
        Ok(actual) => {
            let verified = actual == expected && read_only;
            let message = if actual != expected {
                "原始副本 SHA-256 不匹配，已禁止继续渲染".to_string()
            } else if !read_only {
                "原始副本仍为可写状态，已禁止继续渲染".to_string()
            } else {
                "原始副本字节未修改且为只读".to_string()
            };
            IntegrityReport {
                verified,
                expected_sha256: expected,
                actual_sha256: Some(actual),
                read_only,
                stored_path: stored.display().to_string(),
                message,
            }
        }
        Err(error) => IntegrityReport {
            verified: false,
            expected_sha256: expected,
            actual_sha256: None,
            read_only,
            stored_path: stored.display().to_string(),
            message: format!("无法校验原始副本：{error:#}"),
        },
    }
}

pub fn import_original(source: &Path, project_root: &Path) -> Result<OriginalMaterial> {
    let source = source
        .canonicalize()
        .with_context(|| format!("原始素材不存在：{}", source.display()))?;
    if !source.is_file() {
        bail!("原始素材不是普通文件：{}", source.display());
    }
    let file_name = source
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow::anyhow!("素材文件名不是有效 UTF-8"))?;
    let safe_name = file_name.replace(['/', '\\'], "_");
    let original_dir = project_root.join(ORIGINAL_DIR);
    fs::create_dir_all(&original_dir).context("无法创建原始素材目录")?;
    let temp_path = original_dir.join(format!(".{safe_name}.{}.part", std::process::id()));
    let final_name = format!(
        "{}-{safe_name}",
        &uuid::Uuid::new_v4().simple().to_string()[..12]
    );
    let final_path = original_dir.join(&final_name);

    let source_metadata = fs::metadata(&source).context("无法读取原始素材元信息")?;
    let mut input = File::open(&source).context("无法打开原始素材")?;
    let output = File::create(&temp_path).context("无法创建原始副本")?;
    let mut writer = BufWriter::new(output);
    let mut hasher = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = input.read(&mut buffer).context("读取原始素材失败")?;
        if read == 0 {
            break;
        }
        writer
            .write_all(&buffer[..read])
            .context("写入原始副本失败")?;
        hasher.update(&buffer[..read]);
        size += read as u64;
    }
    writer.flush().context("刷新原始副本失败")?;
    writer.get_ref().sync_all().context("原始副本落盘失败")?;
    drop(writer);
    if let Ok(modified) = source_metadata.modified() {
        if let Ok(file) = OpenOptions::new().write(true).open(&temp_path) {
            let times = fs::FileTimes::new().set_modified(modified);
            let _ = file.set_times(times);
        }
    }
    set_read_only(&temp_path)?;
    fs::rename(&temp_path, &final_path).context("原始副本原子替换失败")?;

    let digest = format!("{:x}", hasher.finalize());
    let copied_digest = sha256_file(&final_path)?;
    if digest != copied_digest {
        bail!("原始副本写入后校验失败");
    }
    if size != source_metadata.len() {
        bail!("复制过程中原始素材大小发生变化");
    }
    Ok(OriginalMaterial {
        source_path: source.display().to_string(),
        stored_relative_path: format!("{ORIGINAL_DIR}/{final_name}"),
        sha256: digest,
        size_bytes: size,
        imported_at: now_rfc3339(),
        read_only: true,
    })
}

pub fn run_dir(session: &ProjectSession, segment_index: usize, run_number: u32) -> PathBuf {
    session
        .project_dir
        .join(RUNS_DIR)
        .join(format!("segment-{:02}", segment_index + 1))
        .join(format!("run-{run_number:04}"))
}

pub fn active_run_count(session: &ProjectSession) -> usize {
    session
        .segments
        .iter()
        .flat_map(|segment| &segment.runs)
        .filter(|run| run.status == RunStatus::Running)
        .count()
}

fn set_read_only(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_mode(0o444);
        fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

fn is_writable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path)
            .map(|metadata| metadata.permissions().mode() & 0o222 != 0)
            .unwrap_or(true)
    }
    #[cfg(not(unix))]
    {
        !path
            .metadata()
            .map(|metadata| metadata.permissions().readonly())
            .unwrap_or(false)
    }
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn data_home() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".local").join("share"))
}
