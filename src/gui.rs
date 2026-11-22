use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gtk4::prelude::*;
use gtk4::{
    Application, ApplicationWindow, Box as GtkBox, Button, Entry, Grid, Label, Orientation,
    ProgressBar, ScrolledWindow, TextView,
};
use timecode_drift_replay::engine::{compare_runs, run_controlled, RunOptions, RunOutcome};
use timecode_drift_replay::evidence;
use timecode_drift_replay::model::Config;

#[derive(Default)]
struct Shared {
    log: Vec<String>,
    running: bool,
    paused: bool,
    stage: String,
    metrics: String,
    verdict: String,
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

    fn log(&self, message: impl Into<String>) {
        self.shared.lock().unwrap().log.push(message.into());
    }

    fn set(&self, stage: &str, running: bool, paused: bool) {
        let mut shared = self.shared.lock().unwrap();
        shared.stage = stage.into();
        shared.running = running;
        shared.paused = paused;
    }
}

pub fn run_gui() -> glib::ExitCode {
    let _ = gstreamer::init();
    let app = Application::builder()
        .application_id("dev.timecode.drift.replay")
        .build();
    app.connect_activate(build_ui);
    app.run()
}

fn build_ui(app: &Application) {
    let ctx = Arc::new(Ctx::new());
    let out = Entry::builder()
        .text("artifacts/evidence")
        .hexpand(true)
        .build();
    let pause_frame = Entry::builder().text("120").width_chars(8).build();
    let start = Button::with_label("Start / rebuild");
    let pause = Button::with_label("Pause");
    let resume = Button::with_label("Resume + compare");
    let stage = Label::new(Some("idle"));
    let metrics = Label::new(Some("no checkpoint"));
    let verdict = Label::new(Some("comparison not run"));
    verdict.set_wrap(true);
    verdict.set_xalign(0.0);
    let progress = ProgressBar::builder().show_text(true).build();
    let log_view = TextView::builder()
        .editable(false)
        .monospace(true)
        .wrap_mode(gtk4::WrapMode::WordChar)
        .build();
    let scroller = ScrolledWindow::builder()
        .child(&log_view)
        .vexpand(true)
        .min_content_height(240)
        .build();

    let grid = Grid::builder()
        .column_spacing(8)
        .row_spacing(6)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();
    grid.attach(&Label::new(Some("Evidence root")), 0, 0, 1, 1);
    grid.attach(&out, 1, 0, 3, 1);
    grid.attach(&Label::new(Some("Pause frame")), 0, 1, 1, 1);
    grid.attach(&pause_frame, 1, 1, 1, 1);
    grid.attach(
        &Label::new(Some("Missing span: 00:00:04:00 → 00:00:05:15")),
        2,
        1,
        2,
        1,
    );
    let actions = GtkBox::new(Orientation::Horizontal, 6);
    actions.append(&start);
    actions.append(&pause);
    actions.append(&resume);
    grid.attach(&actions, 0, 2, 4, 1);
    grid.attach(&stage, 0, 3, 4, 1);
    grid.attach(&progress, 0, 4, 4, 1);
    grid.attach(&metrics, 0, 5, 4, 1);
    grid.attach(&verdict, 0, 6, 4, 1);
    grid.attach(&scroller, 0, 7, 4, 1);

    let window = ApplicationWindow::builder()
        .application(app)
        .title("Timecode Drift Replay — pausable GTK4 control")
        .default_width(760)
        .default_height(620)
        .child(&grid)
        .build();

    start.connect_clicked({
        let ctx = ctx.clone();
        let out = out.clone();
        let pause_frame = pause_frame.clone();
        move |_| {
            let root = PathBuf::from(out.text().as_str());
            let pause_at = pause_frame.text().parse::<u64>().unwrap_or(120);
            let config = Config::default();
            ctx.pause.store(false, Ordering::SeqCst);
            ctx.set("continuous replay", true, false);
            ctx.log(format!("evidence root: {}", root.display()));
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let continuous = root.join("continuous");
                let pausable = root.join("pausable");
                let continuous_result = run_controlled(
                    &config,
                    &continuous,
                    RunOptions {
                        resume: false,
                        pause_at_frame: None,
                        gst_probe: true,
                    },
                    &|| false,
                );
                if let Err(error) = continuous_result {
                    ctx.log(format!("continuous error: {error:#}"));
                    ctx.set("error", false, false);
                    return;
                }
                ctx.log("continuous run complete");
                ctx.set("pausable replay — pause requested at boundary", true, false);
                let pause = ctx.pause.clone();
                let result = run_controlled(
                    &config,
                    &pausable,
                    RunOptions {
                        resume: false,
                        pause_at_frame: Some(pause_at),
                        gst_probe: true,
                    },
                    &|| pause.load(Ordering::SeqCst),
                );
                let mut shared = ctx.shared.lock().unwrap();
                shared.running = false;
                match result {
                    Ok(RunOutcome::Paused) => {
                        shared.paused = true;
                        shared.stage = "paused — checkpoint saved".into();
                        shared.metrics = checkpoint_metrics(&pausable);
                        shared.log.push("checkpoint saved atomically".into());
                    }
                    Ok(RunOutcome::Completed) => {
                        shared.stage = "pausable run completed without checkpoint".into();
                    }
                    Err(error) => {
                        shared.stage = "error".into();
                        shared.log.push(format!("pausable error: {error:#}"));
                    }
                }
            });
        }
    });

    pause.connect_clicked({
        let ctx = ctx.clone();
        move |_| {
            ctx.pause.store(true, Ordering::SeqCst);
            ctx.log("pause requested; stopping after the current frame");
        }
    });

    resume.connect_clicked({
        let ctx = ctx.clone();
        let out = out.clone();
        move |_| {
            let root = PathBuf::from(out.text().as_str());
            let config = Config::default();
            ctx.pause.store(false, Ordering::SeqCst);
            ctx.set("resuming from checkpoint", true, false);
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let pausable = root.join("pausable");
                let continuous = root.join("continuous");
                let result = run_controlled(
                    &config,
                    &pausable,
                    RunOptions {
                        resume: true,
                        pause_at_frame: None,
                        gst_probe: true,
                    },
                    &|| false,
                );
                let mut shared = ctx.shared.lock().unwrap();
                match result {
                    Ok(RunOutcome::Completed) => {
                        shared.log.push("resumed run completed".into());
                        match compare_runs(
                            &continuous,
                            &pausable,
                            &root.join("compare").join("compare_report.json"),
                        ) {
                            Ok(report) => {
                                shared.verdict = report.verdict.clone();
                                shared.log.push(report.verdict);
                            }
                            Err(error) => {
                                let message = format!("compare error: {error:#}");
                                shared.verdict = message.clone();
                                shared.log.push(message);
                            }
                        }
                        shared.stage = "completed".into();
                    }
                    Ok(RunOutcome::Paused) => {
                        shared.stage = "paused again".into();
                    }
                    Err(error) => {
                        shared.stage = "resume error".into();
                        shared.log.push(format!("resume error: {error:#}"));
                    }
                }
                shared.running = false;
                shared.paused = false;
                shared.metrics = checkpoint_metrics(&pausable);
            });
        }
    });

    glib::timeout_add_local(Duration::from_millis(200), {
        let ctx = ctx.clone();
        let stage = stage.clone();
        let metrics = metrics.clone();
        let verdict = verdict.clone();
        let progress = progress.clone();
        let log_view = log_view.clone();
        move || {
            let shared = ctx.shared.lock().unwrap();
            stage.set_text(if shared.stage.is_empty() {
                "idle"
            } else {
                &shared.stage
            });
            metrics.set_text(if shared.metrics.is_empty() {
                "no checkpoint"
            } else {
                &shared.metrics
            });
            verdict.set_text(if shared.verdict.is_empty() {
                "comparison not run"
            } else {
                &shared.verdict
            });
            if shared.running {
                progress.pulse();
            } else if shared.paused {
                progress.set_fraction(0.5);
            } else if shared.stage == "completed" {
                progress.set_fraction(1.0);
            } else {
                progress.set_fraction(0.0);
            }
            let text = shared.log.join("\n");
            let buffer = log_view.buffer();
            if buffer.text(&buffer.start_iter(), &buffer.end_iter(), false) != text {
                buffer.set_text(&text);
                let end = buffer.end_iter();
                let mark = buffer.create_mark(None, &end, false);
                log_view.scroll_to_mark(&mark, 0.0, true, 0.0, 1.0);
            }
            glib::ControlFlow::Continue
        }
    });

    window.present();
}

fn checkpoint_metrics(dir: &std::path::Path) -> String {
    match evidence::load_checkpoint(dir) {
        Ok(checkpoint) => format!(
            "next frame #{} · processed {} · dropped {} · drift events {} · pool {} B · {} · crc {:08x}",
            checkpoint.next_frame_idx,
            checkpoint.processed,
            checkpoint.dropped,
            checkpoint.drift_events,
            checkpoint.pool_bytes,
            if checkpoint.degraded {
                "degraded"
            } else {
                "full quality"
            },
            checkpoint.crc_chain
        ),
        Err(_) => "no checkpoint".into(),
    }
}
