//! GTK4 control panel for the sound-effect batch experiment.
//!
//! The UI only drives the same `svbatch` engine used by the headless CLI:
//! submit a batch, cancel mid-run, and watch per-job state transitions live.

use anyhow::Context;
use gio::prelude::*;
use glib::clone;
use gtk::prelude::*;
use gtk::{glib, Application, ApplicationWindow, Box as GtkBox, Button, Label, ListBox, Orientation, Paned, PolicyType, ScrolledWindow};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use svbatch::audio::{init_gstreamer, CancelToken, Engine};
use svbatch::batch::{SubmissionRegistry, SubmitOutcome};
use svbatch::model::{JobState, JobSpec};
use svbatch::scenarios::canonical_jobs;

type RowRefs = std::sync::Arc<std::sync::Mutex<Vec<(String, glib::SendWeakRef<Label>)>>>;

/// Thread-safe handle to the UI pieces touched from the worker thread.
#[derive(Clone)]
struct WorkerUi {
    window: glib::SendWeakRef<ApplicationWindow>,
    run_btn: glib::SendWeakRef<Button>,
    cancel_btn: glib::SendWeakRef<Button>,
    log_view: glib::SendWeakRef<Label>,
    rows: RowRefs,
}

impl WorkerUi {
    fn invoke<F>(&self, context: &glib::MainContext, f: F)
    where
        F: FnOnce(ApplicationWindow) + Send + 'static,
    {
        let window = self.window.clone();
        context.invoke(move || {
            if let Some(window) = window.upgrade() {
                f(window);
            }
        });
    }

    fn set_row(&self, context: &glib::MainContext, job_id: &str, state: JobState) {
        let rows = self.rows.clone();
        let job_id = job_id.to_string();
        self.invoke(context, move |_| {
            if let Ok(guard) = rows.lock() {
                for (id, weak) in guard.iter() {
                    if id == &job_id {
                        if let Some(label) = weak.upgrade() {
                            label.set_markup(&format!("<tt>{job_id}</tt>  <b>{}</b>", state.as_str()));
                        }
                    }
                }
            }
        });
    }

    fn log(&self, context: &glib::MainContext, line: String, buffer: &std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        let weak = self.log_view.clone();
        let buffer = buffer.clone();
        context.invoke(move || {
            if let Some(label) = weak.upgrade() {
                if let Ok(mut guard) = buffer.lock() {
                    guard.push(line);
                    label.set_text(&guard.join("\n"));
                }
            }
        });
    }

    fn finish(&self, context: &glib::MainContext, running: &std::sync::Arc<std::sync::atomic::AtomicBool>) {
        let run_btn = self.run_btn.clone();
        let cancel_btn = self.cancel_btn.clone();
        let running = running.clone();
        context.invoke(move || {
            running.store(false, std::sync::atomic::Ordering::SeqCst);
            if let Some(b) = run_btn.upgrade() {
                b.set_sensitive(true);
            }
            if let Some(b) = cancel_btn.upgrade() {
                b.set_sensitive(false);
            }
        });
    }
}

fn main() -> anyhow::Result<()> {
    init_gstreamer().context("GStreamer initialisation failed")?;
    let smoke = std::env::args().any(|arg| arg == "--smoke");

    let app = Application::builder()
        .application_id("org.svbatch.experiment")
        .build();

    app.connect_activate(move |app| build_ui(app, smoke));
    let empty: Vec<String> = Vec::new();
    app.run_with_args(&empty);
    Ok(())
}

fn build_ui(app: &Application, smoke: bool) {
    let jobs = canonical_jobs(80);

    let root = Paned::new(Orientation::Horizontal);

    // Left pane: job list.
    let left = GtkBox::new(Orientation::Vertical, 8);
    left.set_margin_top(12);
    left.set_margin_bottom(12);
    left.set_margin_start(12);
    left.set_margin_end(12);
    left.append(&Label::builder().label("作业状态（实时）").halign(gtk::Align::Start).build());

    let list = ListBox::new();
    let mut row_refs: Vec<(String, glib::SendWeakRef<Label>)> = Vec::new();
    for job in &jobs {
        let label = Label::builder()
            .label(format!("{}  {}", job.job_id, JobState::Pending.as_str()))
            .halign(gtk::Align::Start)
            .build();
        list.append(&label);
        row_refs.push((job.job_id.clone(), glib::SendWeakRef::from(label.downgrade())));
    }
    let list_scroll = ScrolledWindow::builder()
        .min_content_width(360)
        .hscrollbar_policy(PolicyType::Never)
        .child(&list)
        .build();
    left.append(&list_scroll);
    root.set_start_child(Some(&left));

    // Right pane: controls + log.
    let right = GtkBox::new(Orientation::Vertical, 8);
    right.set_margin_top(12);
    right.set_margin_bottom(12);
    right.set_margin_start(12);
    right.set_margin_end(12);

    let controls = GtkBox::new(Orientation::Horizontal, 8);
    let run_btn = Button::with_label("提交批次（9 作业）");
    let cancel_btn = Button::with_label("中途取消");
    cancel_btn.set_sensitive(false);
    controls.append(&run_btn);
    controls.append(&cancel_btn);
    right.append(&controls);

    let hint = Label::builder()
        .label("重复提交同一作业会被注册表拒绝；取消会在当前作业的轮询点生效。")
        .halign(gtk::Align::Start)
        .wrap(true)
        .build();
    right.append(&hint);

    let log_view = Label::builder()
        .halign(gtk::Align::Start)
        .valign(gtk::Align::Start)
        .selectable(true)
        .wrap(true)
        .build();
    let log_scroll = ScrolledWindow::builder()
        .min_content_height(320)
        .child(&log_view)
        .build();
    right.append(&log_scroll);
    root.set_end_child(Some(&right));

    let window = ApplicationWindow::builder()
        .application(app)
        .title("音效素材批量变体 · 可回放实验控制台")
        .default_width(900)
        .default_height(560)
        .child(&root)
        .build();

    let worker_ui = WorkerUi {
        window: glib::SendWeakRef::from(window.downgrade()),
        run_btn: glib::SendWeakRef::from(run_btn.downgrade()),
        cancel_btn: glib::SendWeakRef::from(cancel_btn.downgrade()),
        log_view: glib::SendWeakRef::from(log_view.downgrade()),
        rows: RowRefs::new(std::sync::Mutex::new(row_refs)),
    };
    let log_buffer: std::sync::Arc<std::sync::Mutex<Vec<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(vec!["就绪：点击“提交批次”开始。".into()]));
    log_view.set_text("就绪：点击“提交批次”开始。");
    let running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let token: Rc<RefCell<Option<CancelToken>>> = Rc::new(RefCell::new(None));
    let smoke_jobs = if smoke { Some(jobs.clone()) } else { None };

    run_btn.connect_clicked(clone!(
        #[strong]
        worker_ui,
        #[strong]
        log_buffer,
        #[strong]
        running,
        #[strong]
        token,
        move |_| {
            if running.load(std::sync::atomic::Ordering::SeqCst) {
                worker_ui.log(&glib::MainContext::default(), "拒绝：批次正在运行中（重复提交被忽略）。".into(), &log_buffer);
                return;
            }
            start_batch(&worker_ui, &log_buffer, &running, &token, &jobs);
        }
    ));

    cancel_btn.connect_clicked(clone!(
        #[strong]
        token,
        #[strong]
        worker_ui,
        #[strong]
        log_buffer,
        move |_| {
            if let Some(token) = token.borrow().as_ref() {
                token.cancel();
                worker_ui.log(
                    &glib::MainContext::default(),
                    "用户请求取消：CancelToken 已置位。".into(),
                    &log_buffer,
                );
            }
        }
    ));
    window.present();

    if smoke {
        // Headless verification: auto-submit, cancel mid-run, then quit.
        start_batch(&worker_ui, &log_buffer, &running, &token, smoke_jobs.as_ref().unwrap());
        let cancel_token = token.clone();
        let cancel_ui = worker_ui.clone();
        let cancel_log = log_buffer.clone();
        glib::timeout_add_local_once(Duration::from_millis(120), move || {
            if let Some(active) = cancel_token.borrow().as_ref() {
                active.cancel();
                cancel_ui.log(
                    &glib::MainContext::default(),
                    "[smoke] 自动取消已触发".into(),
                    &cancel_log,
                );
            }
        });
        let smoke_app = app.clone();
        let summary_ui = worker_ui.clone();
        glib::timeout_add_local_once(Duration::from_millis(900), move || {
            eprintln!("[smoke] final UI states:");
            if let Ok(guard) = summary_ui.rows.lock() {
                for (id, weak) in guard.iter() {
                    if let Some(label) = weak.upgrade() {
                        eprintln!("[smoke]   {id} -> {}", label.text());
                    }
                }
            }
            eprintln!("[smoke] application quitting after auto-cancel test");
            smoke_app.quit();
        });
    }
}

fn start_batch(
    worker_ui: &WorkerUi,
    log_buffer: &std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    running: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    token_slot: &Rc<RefCell<Option<CancelToken>>>,
    jobs: &[JobSpec],
) {
    running.store(true, std::sync::atomic::Ordering::SeqCst);
    if let Some(btn) = worker_ui.run_btn.upgrade() {
        btn.set_sensitive(false);
    }
    if let Some(btn) = worker_ui.cancel_btn.upgrade() {
        btn.set_sensitive(true);
    }
    let context = glib::MainContext::default();
    if let Ok(guard) = worker_ui.rows.lock() {
        for (id, weak) in guard.iter() {
            if let Some(label) = weak.upgrade() {
                label.set_markup(&format!("<tt>{id}</tt>  {}", JobState::Pending.as_str()));
            }
        }
    }

    let token = CancelToken::new();
    *token_slot.borrow_mut() = Some(token.clone());
    worker_ui.log(
        &context,
        format!("批次提交：{} 个作业，注册表为空。", jobs.len()),
        log_buffer,
    );

    let jobs_owned = jobs.to_vec();
    let worker_ui = worker_ui.clone();
    let log_buffer = log_buffer.clone();
    let running = running.clone();
    std::thread::spawn(move || {
        run_worker(jobs_owned, token, context, worker_ui, log_buffer, running);
    });
}

fn run_worker(
    jobs: Vec<JobSpec>,
    token: CancelToken,
    context: glib::MainContext,
    ui: WorkerUi,
    log_buffer: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    running: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    let started = Instant::now();
    let tmp = std::env::temp_dir().join("svbatch-gtk-output");
    let engine = match Engine::new(&tmp) {
        Ok(e) => e,
        Err(err) => {
            ui.log(&context, format!("引擎初始化失败：{err}"), &log_buffer);
            ui.finish(&context, &running);
            return;
        }
    };
    let mut registry = SubmissionRegistry::new();

    for (index, job) in jobs.iter().enumerate() {
        let outcome = registry.submit(job);
        match outcome {
            SubmitOutcome::Duplicate => {
                let job_id = job.job_id.clone();
                let line = format!("[{}ms] 作业 {} 被拒绝：重复提交", started.elapsed().as_millis(), job_id);
                ui.set_row(&context, &job_id, JobState::Failed);
                ui.log(&context, line, &log_buffer);
                continue;
            }
            SubmitOutcome::Accepted => {}
        }
        let job_id = job.job_id.clone();
        let line = format!(
            "[{}ms] 开始作业 {}/{}：{}",
            started.elapsed().as_millis(),
            index + 1,
            jobs.len(),
            job.job_id
        );
        ui.set_row(&context, &job_id, JobState::Running);
        ui.log(&context, line, &log_buffer);

        match engine.run_job(job, &token) {
            Ok((JobState::Cancelled, _)) => {
                let job_id = job.job_id.clone();
                let line = format!(
                    "[{}ms] 作业 {} 已取消，部分输出已清理",
                    started.elapsed().as_millis(),
                    job.job_id
                );
                ui.set_row(&context, &job_id, JobState::Cancelled);
                ui.log(&context, line, &log_buffer);
                for remaining in jobs.iter().skip(index + 1) {
                    ui.set_row(&context, &remaining.job_id, JobState::Pending);
                }
                ui.log(&context, "批次因取消提前结束。".into(), &log_buffer);
                break;
            }
            Ok((state, Some((file, sha, bytes)))) => {
                let job_id = job.job_id.clone();
                let line = format!(
                    "[{}ms] 完成 {} → {} ({} 字节, sha256 {}…)",
                    started.elapsed().as_millis(),
                    job.job_id,
                    file,
                    bytes,
                    &sha[..12]
                );
                ui.set_row(&context, &job_id, state);
                ui.log(&context, line, &log_buffer);
            }
            Ok((state, None)) => {
                ui.set_row(&context, &job.job_id, state);
            }
            Err(err) => {
                let job_id = job.job_id.clone();
                let line = format!("作业 {} 失败：{err}", job_id);
                ui.set_row(&context, &job_id, JobState::Failed);
                ui.log(&context, line, &log_buffer);
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    ui.finish(&context, &running);
}
