//! Durable project storage.
//!
//! Layout:
//! ```text
//! <project_dir>/
//!   project.json          # canonical project document (pretty, ordered keys)
//!   evidence/
//!     <segment_id>/
//!       audit.jsonl       # append-only hash-chained audit trail
//!       run-<run_id>/     # one directory per extraction attempt
//!         kf-000001.jpg   # thumbnail evidence, named by keyframe index
//!         manifest.json   # run-local frame manifest
//! ```
//! Writes go through `project.json.tmp` + rename + fsync so a crash at any
//! point leaves either the old or the new document, never a torn file.

use std::fs;
use std::path::{Path, PathBuf};

use crate::model::project::{AppInfo, Project, SCHEMA_VERSION};
use crate::{Error, Result};

/// Manages on-disk persistence of one [`Project`].
#[derive(Debug, Clone)]
pub struct ProjectStore {
    root: PathBuf,
}

impl ProjectStore {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        if !root.join("project.json").exists() {
            return Err(Error::ProjectNotFound(root));
        }
        Ok(Self { root })
    }

    pub fn create(
        root: impl Into<PathBuf>,
        name: impl Into<String>,
        description: impl Into<String>,
        actor: &str,
    ) -> Result<(Self, Project)> {
        let root = root.into();
        fs::create_dir_all(&root).map_err(|e| Error::io(&root, e))?;
        let is_empty = fs::read_dir(&root)
            .map_err(|e| Error::io(&root, e))?
            .next()
            .is_none();
        if !is_empty {
            return Err(Error::ProjectExists(root));
        }
        let evidence_dir = "evidence".to_string();
        fs::create_dir_all(root.join(&evidence_dir))
            .map_err(|e| Error::io(root.join(&evidence_dir), e))?;

        let now = crate::time::format_unix_nanos(
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default(),
        );
        let project = Project {
            app_info: AppInfo {
                app: "vkri".to_string(),
                schema_version: SCHEMA_VERSION,
                app_version: env!("CARGO_PKG_VERSION").to_string(),
                created_at: now.clone(),
                updated_at: now,
            },
            name: name.into(),
            description: description.into(),
            evidence_dir,
            devices: Vec::new(),
            segments: Vec::new(),
            comparisons: Vec::new(),
        };
        let store = Self { root };
        store.save(&project)?;
        store.audit_global(actor, "project.created", &format!("name={}", project.name))?;
        Ok((store, project))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn project_path(&self) -> PathBuf {
        self.root.join("project.json")
    }

    pub fn evidence_root(&self) -> PathBuf {
        self.root.join("evidence")
    }

    /// Load the project, preserving JSON field and array ordering exactly.
    pub fn load(&self) -> Result<Project> {
        let path = self.project_path();
        let bytes = fs::read(&path).map_err(|e| Error::io(&path, e))?;
        let project: Project = serde_json::from_slice(&bytes)?;
        self.validate(&project)?;
        Ok(project)
    }

    /// Persist the project atomically and update `updated_at`.
    pub fn save(&self, project: &Project) -> Result<()> {
        self.validate(project)?;
        let path = self.project_path();
        let tmp = self.root.join("project.json.tmp");
        let body = serde_json::to_vec_pretty(project)?;
        fs::write(&tmp, body).map_err(|e| Error::io(&tmp, e))?;
        if let Some(parent) = tmp.parent() {
            fs::File::open(&tmp)
                .and_then(|file| file.sync_all())
                .map_err(|e| Error::io(&tmp, e))?;
            let dir = fs::File::open(parent).map_err(|e| Error::io(parent, e))?;
            dir.sync_all().map_err(|e| Error::io(parent, e))?;
        }
        fs::rename(&tmp, &path).map_err(|e| Error::io(&path, e))?;
        Ok(())
    }

    pub fn audit_for(&self, segment_id: &str) -> Result<crate::audit::AuditLog> {
        crate::audit::AuditLog::open(&self.evidence_root(), segment_id)
    }

    /// Project-level events are recorded in a global audit log alongside the
    /// per-segment trails.
    pub fn audit_global(&self, actor: &str, action: &str, detail: &str) -> Result<()> {
        let mut log = crate::audit::AuditLog::open(&self.evidence_root(), "_global")?;
        log.append(actor, action, detail, None)?;
        Ok(())
    }

    /// Directory for one extraction attempt. Evidence of every attempt is
    /// retained; nothing is deleted when a segment is re-run.
    pub fn run_dir(&self, segment_id: &str, run_id: &str) -> PathBuf {
        self.evidence_root()
            .join(segment_id)
            .join(format!("run-{run_id}"))
    }

    fn validate(&self, project: &Project) -> Result<()> {
        if project.app_info.schema_version != SCHEMA_VERSION {
            return Err(Error::Validation(format!(
                "unsupported schema_version {}, expected {}",
                project.app_info.schema_version, SCHEMA_VERSION
            )));
        }
        let device_ids: std::collections::HashSet<&str> =
            project.devices.iter().map(|d| d.id.as_str()).collect();
        if device_ids.len() != project.devices.len() {
            return Err(Error::Validation("duplicate device ids".into()));
        }
        let mut segment_ids = std::collections::HashSet::new();
        for segment in &project.segments {
            if !segment_ids.insert(segment.id.as_str()) {
                return Err(Error::Validation(format!(
                    "duplicate segment id {}",
                    segment.id
                )));
            }
            if !device_ids.contains(segment.device_id.as_str()) {
                return Err(Error::Validation(format!(
                    "segment {} references unknown device {}",
                    segment.id, segment.device_id
                )));
            }
            for (position, keyframe) in segment.keyframes.iter().enumerate() {
                if keyframe.index as usize != position {
                    return Err(Error::Validation(format!(
                        "segment {} keyframe indices out of order",
                        segment.id
                    )));
                }
            }
        }
        Ok(())
    }
}
