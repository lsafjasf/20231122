mod actions;
mod style;
mod view;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{self, Receiver, Sender};

use anyhow::{Context, Result, bail};
use gtk::prelude::*;
use gtk4 as gtk;

use clockbridge::model::Workspace;
use clockbridge::runner::{self, RunResult};
use clockbridge::store::{WORKSPACE_FILE, WorkspaceStore};

pub(crate) struct JobOutcome {
    pub segment_id: String,
    pub attempt_id: String,
    pub result: RunResult,
}

pub(crate) struct AppState {
    pub window: gtk::ApplicationWindow,
    pub store: WorkspaceStore,
    pub selected_segment_id: Option<String>,
    pub segment_list: gtk::ListBox,
    pub detail_box: gtk::Box,
    pub title_label: gtk::Label,
    pub subtitle_label: gtk::Label,
    pub summary_label: gtk::Label,
    pub status_label: gtk::Label,
    pub device_a_path: gtk::Label,
    pub device_b_path: gtk::Label,
    pub offset_b: gtk::SpinButton,
    pub start_ms: gtk::SpinButton,
    pub duration_ms: gtk::SpinButton,
    pub reason: gtk::Entry,
    pub auto_save_label: gtk::Label,
    pub running_jobs: HashSet<String>,
    pub tx: Sender<JobOutcome>,
    pub rx: Receiver<JobOutcome>,
    pub cancel: Arc<AtomicBool>,
    pub exporting: bool,
}

pub fn run() -> gtk::glib::ExitCode {
    let application = gtk::Application::builder()
        .application_id("io.clockbridge.retry")
        .build();
    application.connect_startup(|_| style::install());
    application.connect_activate(build_window);
    application.run_with_args::<&str>(&["clockbridge"])
}

fn build_window(application: &gtk::Application) {
    let window = gtk::ApplicationWindow::builder()
        .application(application)
        .title("ClockBridge · 转码错误重试策略")
        .default_width(1440)
        .default_height(900)
        .build();
    window.add_css_class("app-window");

    match load_initial_store() {
        Ok(store) => {
            let _ = runner::initialize();
            let (tx, rx) = mpsc::channel();
            let title = store.workspace().title.clone();
            let state = std::rc::Rc::new(std::cell::RefCell::new(AppState {
                window: window.clone(),
                store,
                selected_segment_id: None,
                segment_list: gtk::ListBox::new(),
                detail_box: gtk::Box::new(gtk::Orientation::Vertical, 18),
                title_label: gtk::Label::new(None),
                subtitle_label: gtk::Label::new(None),
                summary_label: gtk::Label::new(None),
                status_label: gtk::Label::new(None),
                device_a_path: gtk::Label::new(None),
                device_b_path: gtk::Label::new(None),
                offset_b: gtk::SpinButton::with_range(-60_000.0, 60_000.0, 1.0),
                start_ms: gtk::SpinButton::with_range(0.0, 86_400_000.0, 100.0),
                duration_ms: gtk::SpinButton::with_range(100.0, 86_400_000.0, 100.0),
                reason: gtk::Entry::new(),
                auto_save_label: gtk::Label::new(None),
                running_jobs: HashSet::new(),
                tx,
                rx,
                cancel: Arc::new(AtomicBool::new(false)),
                exporting: false,
            }));
            view::build(&state, &title);
            view::refresh(&state);
            actions::connect_window_events(&state);
            actions::start_job_poll(&state);
            window.present();
        }
        Err(error) => {
            let heading = gtk::Label::new(Some("无法载入工作区"));
            heading.add_css_class("detail-title");
            let body = gtk::Label::new(Some(&format!("{error:#}")));
            body.set_wrap(true);
            body.set_selectable(true);
            let content = gtk::Box::new(gtk::Orientation::Vertical, 16);
            content.set_margin_top(48);
            content.set_margin_bottom(48);
            content.set_margin_start(48);
            content.set_margin_end(48);
            content.append(&heading);
            content.append(&body);
            window.set_child(Some(&content));
            window.present();
        }
    }
}

fn load_initial_store() -> Result<WorkspaceStore> {
    let args: Vec<String> = std::env::args().collect();
    if let Some(index) = args.iter().position(|arg| arg == "--workspace")
        && let Some(value) = args.get(index + 1)
    {
        return open_location(Path::new(value), false);
    }
    if let Ok(value) = std::env::var("CLOCKBRIDGE_HOME") {
        return open_location(Path::new(&value), true);
    }
    let root = std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("retry-workspace");
    open_location(&root, true)
}

fn open_location(location: &Path, create_demo: bool) -> Result<WorkspaceStore> {
    if location.is_dir() || location.extension().is_none() {
        let workspace_file = location.join(WORKSPACE_FILE);
        if workspace_file.exists() {
            return WorkspaceStore::open(workspace_file);
        }
        if !create_demo {
            bail!("工作区不存在: {}", workspace_file.display());
        }
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let media_dir = cwd.join("examples/media");
        let workspace = Workspace::demo(
            media_dir.join("device-a.mp4").to_string_lossy().as_ref(),
            media_dir.join("device-b.mp4").to_string_lossy().as_ref(),
        );
        return WorkspaceStore::create(location, workspace)
            .with_context(|| format!("无法在 {} 创建工作区", location.display()));
    }
    WorkspaceStore::open(location)
}
