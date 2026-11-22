//! UI-facing application state.
//!
//! GTK widgets are not `Send`, while extraction runs on a worker thread. The
//! shared mutable state therefore lives in `Rc<RefCell<...>>` on the main
//! thread; workers communicate back through a `glib::Sender` channel. A
//! background run holds only a path and a cancellation token, never a GTK
//! object.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use vkri_core::extract::CancellationToken;
use vkri_core::ProjectService;

use crate::widgets::StatusWidgets;

/// Messages sent from worker threads back to the GTK main loop.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum WorkerMessage {
    Progress {
        frames_seen: u64,
        keyframes_written: u64,
        pts_ns: i64,
    },
    RunFinished {
        segment_id: String,
        interrupted: bool,
        message: String,
    },
    RunFailed {
        segment_id: String,
        error: String,
    },
    ProjectReloaded,
    Info(String),
}

/// Handle to a running extraction, used by the Stop button / window close.
#[derive(Clone)]
#[allow(dead_code)]
pub struct ActiveRun {
    pub segment_id: String,
    pub cancel: CancellationToken,
}

/// Mutable state shared by the window and its callbacks.
pub struct AppState {
    pub service: Option<ProjectService>,
    pub project_dir: Option<PathBuf>,
    pub active_run: Option<ActiveRun>,
    pub status: Option<Rc<StatusWidgets>>,
}

pub type SharedState = Rc<RefCell<AppState>>;

impl AppState {
    pub fn new() -> SharedState {
        Rc::new(RefCell::new(Self {
            service: None,
            project_dir: None,
            active_run: None,
            status: None,
        }))
    }

    pub fn is_running(&self) -> bool {
        self.active_run.is_some()
    }

    pub fn open_project(&mut self, dir: PathBuf, actor: &str) -> anyhow::Result<()> {
        let service = ProjectService::open(&dir, actor)?;
        self.service = Some(service);
        self.project_dir = Some(dir);
        Ok(())
    }

    pub fn create_project(
        &mut self,
        dir: PathBuf,
        name: &str,
        description: &str,
        actor: &str,
    ) -> anyhow::Result<()> {
        let service = ProjectService::create(&dir, name, description, actor)?;
        self.service = Some(service);
        self.project_dir = Some(dir);
        Ok(())
    }
}
