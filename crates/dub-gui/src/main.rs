//! dub-gui: GTK4 front-end for the pausable dub-switch flow.
//!
//! One window drives the same repeated object as the CLI: start a job,
//! pause it (checkpoint is written after each segment), resume it, and run
//! the independent review. Processing happens on a worker thread; the UI
//! polls shared state a few times per second.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use dub_core::pipeline::{run_job, Outcome, RunOptions};
use dub_core::{media, report, state, DubJob};
use gtk4::prelude::*;
use gtk4::{
    Application, ApplicationWindow, Box as GtkBox, Button, Entry, Label, Orientation, ProgressBar,
    ScrolledWindow, TextView,
};

#[derive(Default)]
struct Shared {
    log: Vec<String>,
    done: u32,
    total: u32,
    running: bool,
    degraded: usize,
}

struct Ctx {
    shared: Arc<Mutex<Shared>>,
    pause: Arc<AtomicBool>,
}

impl Ctx {
    fn new() -> Self {
        Self {
            shared: Arc::new(Mutex::new(Shared::default())),
            pause: Arc::new(AtomicBool::new(false)),
        }
    }
    fn log(&self, msg: impl Into<String>) {
        self.shared.lock().unwrap().log.push(msg.into());
    }
}

fn spawn_run(ctx: Arc<Ctx>, workdir: PathBuf, job: Option<DubJob>) {
    ctx.pause.store(false, Ordering::SeqCst);
    {
        let mut s = ctx.shared.lock().unwrap();
        s.running = true;
        s.degraded = 0;
    }
    std::thread::spawn(move || {
        let result = (|| -> anyhow::Result<Outcome> {
            if let Some(job) = job {
                state::save_state(&workdir, &job)?;
            }
            let pause = ctx.pause.clone();
            let shared = ctx.shared.clone();
            let opts = RunOptions {
                should_pause: Box::new(move || pause.load(Ordering::SeqCst)),
                on_progress: Box::new(move |done, total| {
                    let mut s = shared.lock().unwrap();
                    s.done = done;
                    s.total = total;
                }),
                ..RunOptions::default()
            };
            run_job(&workdir, &opts)
        })();
        let mut s = ctx.shared.lock().unwrap();
        s.running = false;
        match result {
            Ok(Outcome::Completed) => s.log.push("run completed".into()),
            Ok(Outcome::Paused) => s.log.push("paused — checkpoint saved".into()),
            Err(e) => s.log.push(format!("error: {e:#}")),
        }
        if let Ok(job) = state::load_state(&workdir) {
            s.degraded = job.degradations.len();
        }
    });
}

fn main() -> glib::ExitCode {
    gstreamer::init().expect("gstreamer init");
    let app = Application::builder()
        .application_id("dev.dubswitch.gui")
        .build();
    app.connect_activate(build_ui);
    app.run()
}

fn build_ui(app: &Application) {
    let ctx = Arc::new(Ctx::new());

    let input = Entry::builder().placeholder_text("input .mkv").build();
    let workdir = Entry::builder()
        .placeholder_text("workdir")
        .text("work")
        .build();
    let languages = Entry::builder()
        .placeholder_text("languages, e.g. eng,zho,eng")
        .text("eng,zho,eng")
        .build();
    let start_btn = Button::with_label("Start");
    let pause_btn = Button::with_label("Pause");
    let resume_btn = Button::with_label("Resume");
    let verify_btn = Button::with_label("Verify");
    let progress = ProgressBar::builder().show_text(true).build();
    let degrade_label = Label::new(Some("degradations: 0"));
    let log_view = TextView::builder().editable(false).monospace(true).build();
    let scroller = ScrolledWindow::builder()
        .child(&log_view)
        .vexpand(true)
        .build();

    let buttons = GtkBox::new(Orientation::Horizontal, 6);
    for b in [&start_btn, &pause_btn, &resume_btn, &verify_btn] {
        buttons.append(b);
    }
    let root = GtkBox::new(Orientation::Vertical, 6);
    root.set_margin_top(8);
    root.set_margin_bottom(8);
    root.set_margin_start(8);
    root.set_margin_end(8);
    root.append(&input);
    root.append(&workdir);
    root.append(&languages);
    root.append(&buttons);
    root.append(&progress);
    root.append(&degrade_label);
    root.append(&scroller);

    let window = ApplicationWindow::builder()
        .application(app)
        .title("Dub Switch — pausable pipeline")
        .default_width(640)
        .default_height(480)
        .child(&root)
        .build();

    start_btn.connect_clicked({
        let ctx = ctx.clone();
        let input = input.clone();
        let workdir = workdir.clone();
        let languages = languages.clone();
        move |_| {
            let in_path = PathBuf::from(input.text().as_str());
            let wd = PathBuf::from(workdir.text().as_str());
            let langs: Vec<String> = languages
                .text()
                .split(',')
                .map(|s| s.trim().to_string())
                .collect();
            ctx.log(format!("start: {} -> {}", in_path.display(), wd.display()));
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let job = (|| -> anyhow::Result<DubJob> {
                    let total_ms = media::duration_ms(&in_path)?;
                    let out_dir = wd.join("segments");
                    std::fs::create_dir_all(&out_dir)?;
                    DubJob::new(
                        "gui-job",
                        &in_path.canonicalize()?.to_string_lossy(),
                        &out_dir.to_string_lossy(),
                        total_ms,
                        &langs,
                    )
                })();
                match job {
                    Ok(j) => spawn_run(ctx, wd, Some(j)),
                    Err(e) => ctx.log(format!("error: {e:#}")),
                }
            });
        }
    });

    pause_btn.connect_clicked({
        let ctx = ctx.clone();
        move |_| {
            ctx.pause.store(true, Ordering::SeqCst);
            ctx.log("pause requested");
        }
    });

    resume_btn.connect_clicked({
        let ctx = ctx.clone();
        let workdir = workdir.clone();
        move |_| {
            let wd = PathBuf::from(workdir.text().as_str());
            ctx.log(format!("resume: {}", wd.display()));
            spawn_run(ctx.clone(), wd, None);
        }
    });

    verify_btn.connect_clicked({
        let ctx = ctx.clone();
        let workdir = workdir.clone();
        move |_| {
            let wd = PathBuf::from(workdir.text().as_str());
            let ctx = ctx.clone();
            std::thread::spawn(move || match report::verify_workdir(&wd) {
                Ok(review) => {
                    for (name, ok, detail) in &review.checks {
                        ctx.log(format!(
                            "[{}] {name}: {detail}",
                            if *ok { "ok" } else { "FAIL" }
                        ));
                    }
                    ctx.log(format!(
                        "verify: {}",
                        if review.ok { "OK" } else { "FAILED" }
                    ));
                }
                Err(e) => ctx.log(format!("verify error: {e:#}")),
            });
        }
    });

    // Poll shared state a few times per second.
    glib::timeout_add_local(std::time::Duration::from_millis(150), {
        let ctx = ctx.clone();
        move || {
            let mut s = ctx.shared.lock().unwrap();
            if s.total > 0 {
                progress.set_fraction(s.done as f64 / s.total as f64);
                progress.set_text(Some(&format!("{}/{}", s.done, s.total)));
            }
            degrade_label.set_text(&format!("degradations: {}", s.degraded));
            if !s.log.is_empty() {
                let buf = log_view.buffer();
                for line in s.log.drain(..) {
                    buf.insert(&mut buf.end_iter(), &format!("{line}\n"));
                }
            }
            glib::ControlFlow::Continue
        }
    });

    window.present();
}
