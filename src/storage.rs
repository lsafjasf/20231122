//! Project persistence with atomic writes and deterministic field order.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::model::{Project, PROJECT_SCHEMA_VERSION};

pub const PROJECT_FILE_NAME: &str = "project.srcrepair.json";
pub const OUTPUT_DIR: &str = "fragments";
pub const EXPORT_DIR: &str = "exports";

pub fn project_path(project_dir: &Path) -> PathBuf {
    project_dir.join(PROJECT_FILE_NAME)
}

/// Load and sanity-check a project file. Field and fragment order come back
/// exactly as written because serde emits structs/Vec in declaration order.
pub fn load_project(project_dir: &Path) -> Result<Project> {
    let path = project_path(project_dir);
    let text = fs::read_to_string(&path)
        .with_context(|| format!("failed to read project file {}", path.display()))?;
    let project: Project = serde_json::from_str(&text)
        .with_context(|| format!("failed to parse project file {}", path.display()))?;
    if project.schema_version != PROJECT_SCHEMA_VERSION {
        bail!(
            "unsupported project schema version {} (supported: {})",
            project.schema_version,
            PROJECT_SCHEMA_VERSION
        );
    }
    Ok(project)
}

/// Pretty-serialize then atomically replace the project file so a crash at
/// any point leaves either the old or the new complete document.
pub fn save_project(project_dir: &Path, project: &Project) -> Result<()> {
    fs::create_dir_all(project_dir)
        .with_context(|| format!("cannot create project dir {}", project_dir.display()))?;
    let json = serde_json::to_string_pretty(project)?;
    let final_path = project_path(project_dir);
    let tmp_path = project_dir.join(format!("{PROJECT_FILE_NAME}.tmp"));
    fs::write(&tmp_path, format!("{json}\n"))
        .with_context(|| format!("cannot write {}", tmp_path.display()))?;
    fs::rename(&tmp_path, &final_path)
        .with_context(|| format!("cannot replace {}", final_path.display()))?;
    Ok(())
}

pub fn output_dir(project_dir: &Path) -> PathBuf {
    project_dir.join(OUTPUT_DIR)
}

pub fn export_dir(project_dir: &Path) -> PathBuf {
    project_dir.join(EXPORT_DIR)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Fragment, Project};

    fn temp_dir() -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("srcrepair-store-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn round_trip_keeps_fragment_order_and_fields() {
        let dir = temp_dir();
        let mut project = Project::new("order-case".into(), 48000, "2026-11-22T00:00:00Z".into());
        for index in 0..3u32 {
            let id = project.next_fragment_id();
            project.fragments.push(Fragment::new(
                id,
                format!("device-{index}"),
                format!("/tmp/source-{index}.wav"),
                44100,
                48000,
                u64::from(index) * 1000,
                500,
                "test".into(),
                "2026-11-22T00:00:00Z".into(),
            ));
        }
        save_project(&dir, &project).unwrap();
        let reloaded = load_project(&dir).unwrap();
        assert_eq!(reloaded, project);
        let ids: Vec<String> = reloaded.fragments.iter().map(|f| f.id.clone()).collect();
        assert_eq!(ids, vec!["f0001", "f0002", "f0003"]);

        let text = fs::read_to_string(project_path(&dir)).unwrap();
        let schema = text.find("\"schema_version\"").unwrap();
        let project_id = text.find("\"project_id\"").unwrap();
        let fragments = text.find("\"fragments\"").unwrap();
        assert!(schema < project_id && project_id < fragments);
    }

    #[test]
    fn rejects_foreign_schema() {
        let dir = temp_dir();
        fs::write(project_path(&dir), "{\"schema_version\":99}").unwrap();
        assert!(load_project(&dir).is_err());
    }
}
