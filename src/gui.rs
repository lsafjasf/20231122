//! GTK4 front end for the pausable coverage pipeline.
//!
//! The heavy work runs on a worker thread; progress and log lines travel
//! back over an mpsc channel drained by a GLib timeout. Pause is a shared
//! atomic flag checked between chunks; Resume reloads the checkpoint and
//! continues the same repeated media object.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use gtk4::prelude::*;
use gtk4::{self as gtk, glib};

use crate::pipeline::{Pipeline, ResourceBudget, RunOutcome};

enum Msg {
    Log(String),
    Progress(u64, u64),
    Done(String),
    Failed(String),
}

struct UiState {
    object_path: Rc<RefCell<Option<PathBuf>>>,
    pause_flag: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
}

pub fn run_gui() -> i32 {
    let app = gtk::Application::builder()
        .application_id("dev.sfc.SubtitleFontCoverage")
        .build();
    app.connect_activate(build_ui);
    app.run().into()
}

fn build_ui(app: &gtk::Application) {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("字幕字体覆盖报告 — Subtitle Font Coverage")
        .default_width(720)
        .default_height(520)
        .build();

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 8);
    vbox.set_margin_top(12);
    vbox.set_margin_bottom(12);
    vbox.set_margin_start(12);
    vbox.set_margin_end(12);

    let file_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let pick_btn = gtk::Button::with_label("选择媒体/字幕文件…");
    let file_label = gtk::Label::new(Some("未选择 no file"));
    file_label.set_hexpand(true);
    file_label.set_xalign(0.0);
    file_row.append(&pick_btn);
    file_row.append(&file_label);
    vbox.append(&file_row);

    let out_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    out_row.append(&gtk::Label::new(Some("输出目录 output dir:")));
    let out_entry = gtk::Entry::new();
    out_entry.set_text("sfc-output");
    out_entry.set_hexpand(true);
    out_row.append(&out_entry);
    vbox.append(&out_row);

    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let start_btn = gtk::Button::with_label("开始 Start");
    let pause_btn = gtk::Button::with_label("暂停 Pause");
    let resume_btn = gtk::Button::with_label("恢复 Resume");
    pause_btn.set_sensitive(false);
    resume_btn.set_sensitive(false);
    controls.append(&start_btn);
    controls.append(&pause_btn);
    controls.append(&resume_btn);
    vbox.append(&controls);

    let progress = gtk::ProgressBar::new();
    progress.set_show_text(true);
    vbox.append(&progress);

    let log_view = gtk::TextView::new();
    log_view.set_editable(false);
    log_view.set_monospace(true);
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_child(Some(&log_view));
    scroll.set_vexpand(true);
    vbox.append(&scroll);

    window.set_child(Some(&vbox));

    let state = Rc::new(UiState {
        object_path: Rc::new(RefCell::new(None)),
        pause_flag: Arc::new(AtomicBool::new(false)),
        running: Arc::new(AtomicBool::new(false)),
    });
    let (tx, rx) = mpsc::channel::<Msg>();

    {
        let state = state.clone();
        let file_label = file_label.clone();
        let window = window.clone();
        pick_btn.connect_clicked(move |_| {
            let dialog = gtk::FileDialog::new();
            let state = state.clone();
            let file_label = file_label.clone();
            dialog.open(Some(&window), gtk::gio::Cancellable::NONE, move |res| {
                if let Ok(file) = res {
                    if let Some(path) = file.path() {
                        file_label.set_text(&path.display().to_string());
                        *state.object_path.borrow_mut() = Some(path);
                    }
                }
            });
        });
    }

    // Shared worker launcher: `resume` selects Pipeline::resume vs ::new.
    let launch = {
        let state = state.clone();
        let out_entry = out_entry.clone();
        let pause_btn = pause_btn.clone();
        let resume_btn = resume_btn.clone();
        let start_btn = start_btn.clone();
        move |resume: bool, tx: mpsc::Sender<Msg>| {
            let Some(obj) = state.object_path.borrow().clone() else {
                let _ = tx.send(Msg::Failed("请先选择媒体文件 pick a file first".into()));
                return;
            };
            let out_dir = PathBuf::from(out_entry.text().as_str());
            state.pause_flag.store(false, Ordering::SeqCst);
            state.running.store(true, Ordering::SeqCst);
            start_btn.set_sensitive(false);
            resume_btn.set_sensitive(false);
            pause_btn.set_sensitive(true);
            let pause_flag = state.pause_flag.clone();
            let running = state.running.clone();
            std::thread::spawn(move || {
                let result = (|| -> anyhow::Result<RunOutcome> {
                    let budget = ResourceBudget::default();
                    let mut pipe = if resume {
                        Pipeline::resume(&obj, &out_dir, budget, pause_flag, None)?
                    } else {
                        Pipeline::new(&obj, &out_dir, budget, pause_flag, None)?
                    };
                    let txp = tx.clone();
                    pipe.run(move |p| {
                        let _ = txp.send(Msg::Progress(p.processed, p.total));
                        let _ = txp.send(Msg::Log(p.message));
                    })
                })();
                running.store(false, Ordering::SeqCst);
                match result {
                    Ok(RunOutcome::Completed) => {
                        let _ = tx.send(Msg::Done(format!(
                            "完成 Completed. 报告在 report.json / report.md ({})",
                            out_dir.display()
                        )));
                    }
                    Ok(RunOutcome::Paused) => {
                        let _ = tx.send(Msg::Done(
                            "已暂停 Paused. 中间状态已保存 checkpoint.json，可恢复 Resume.".into(),
                        ));
                    }
                    Err(e) => {
                        let _ = tx.send(Msg::Failed(format!("{e:#}")));
                    }
                }
            });
        }
    };
    let launch = Rc::new(launch);

    {
        let launch = launch.clone();
        let tx = tx.clone();
        start_btn.connect_clicked(move |_| launch(false, tx.clone()));
    }
    {
        let launch = launch.clone();
        let tx = tx.clone();
        resume_btn.connect_clicked(move |_| launch(true, tx.clone()));
    }
    {
        let state = state.clone();
        pause_btn.connect_clicked(move |_| {
            state.pause_flag.store(true, Ordering::SeqCst);
        });
    }

    // Drain worker messages on the main loop.
    {
        let log_view = log_view.clone();
        let progress = progress.clone();
        let start_btn = start_btn.clone();
        let pause_btn = pause_btn.clone();
        let resume_btn = resume_btn.clone();
        let state = state.clone();
        glib::timeout_add_local(Duration::from_millis(100), move || {
            while let Ok(msg) = rx.try_recv() {
                let buf = log_view.buffer();
                match msg {
                    Msg::Log(line) => {
                        buf.insert(&mut buf.end_iter(), &format!("{line}\n"));
                    }
                    Msg::Progress(done, total) => {
                        let frac = if total == 0 { 0.0 } else { done as f64 / total as f64 };
                        progress.set_fraction(frac);
                        progress.set_text(Some(&format!("{done}/{total}")));
                    }
                    Msg::Done(line) => {
                        buf.insert(&mut buf.end_iter(), &format!("{line}\n"));
                        pause_btn.set_sensitive(false);
                        start_btn.set_sensitive(true);
                        resume_btn.set_sensitive(true);
                        state.running.store(false, Ordering::SeqCst);
                    }
                    Msg::Failed(line) => {
                        buf.insert(&mut buf.end_iter(), &format!("错误 ERROR: {line}\n"));
                        pause_btn.set_sensitive(false);
                        start_btn.set_sensitive(true);
                        resume_btn.set_sensitive(true);
                        state.running.store(false, Ordering::SeqCst);
                    }
                }
            }
            glib::ControlFlow::Continue
        });
    }

    window.present();
}
