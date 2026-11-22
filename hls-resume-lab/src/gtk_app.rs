//! GTK4 front end for the HLS resume-point diagnosis lab.
//!
//! The window is intentionally thin: all logic lives in [`crate::core`],
//! GTK only renders configuration, progress and the evidence summary. The
//! same code path runs under X11, Wayland and the headless Broadway backend.

use crate::core::runner::{self, RunConfig, RunOutcome};
use glib::clone;
use gtk::glib;
use gtk::prelude::*;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

pub fn run_app() -> glib::ExitCode {
    let app = gtk::Application::builder()
        .application_id("org.lab.hlsresume.HlsResumeLab")
        .build();
    app.connect_activate(build_ui);
    // Strip the CLI subcommand ("gui") so GTK does not treat it as a file
    // to open: GApplication only accepts positional file arguments when the
    // HANDLES_OPEN flag is set.
    app.run_with_args(&Vec::<String>::new())
}

#[derive(Clone)]
struct UiState {
    config_path: Rc<RefCell<PathBuf>>,
    last_outcome: Rc<RefCell<Option<RunOutcome>>>,
}

fn build_ui(app: &gtk::Application) {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("HLS 断点续播诊断实验室")
        .default_width(860)
        .default_height(640)
        .build();

    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    root.set_margin_top(12);
    root.set_margin_bottom(12);
    root.set_margin_start(12);
    root.set_margin_end(12);

    let header = gtk::Label::new(Some(
        "两个版本快照 + 截断对照 · 顺序/重复提交验证 · 全步骤留证",
    ));
    header.set_halign(gtk::Align::Start);
    root.append(&header);

    let config_buffer = gtk::TextBuffer::new(None);
    let config_view = gtk::TextView::builder()
        .buffer(&config_buffer)
        .monospace(true)
        .editable(true)
        .height_request(220)
        .build();
    root.append(&config_view);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let run_button = gtk::Button::with_label("运行可回放实验");
    let reload_button = gtk::Button::with_label("载入配置文件");
    let clean_button = gtk::Button::with_label("清理临时状态");
    buttons.append(&run_button);
    buttons.append(&reload_button);
    buttons.append(&clean_button);
    root.append(&buttons);

    let progress = gtk::ProgressBar::new();
    progress.set_show_text(true);
    root.append(&progress);

    let output_buffer = gtk::TextBuffer::new(None);
    let output_view = gtk::TextView::builder()
        .buffer(&output_buffer)
        .monospace(true)
        .editable(false)
        .vexpand(true)
        .build();
    root.append(&output_view);

    window.set_child(Some(&root));

    let state = UiState {
        config_path: Rc::new(RefCell::new(PathBuf::from("run-config.json"))),
        last_outcome: Rc::new(RefCell::new(None)),
    };

    reload_button.connect_clicked(clone!(
        #[weak]
        config_buffer,
        #[weak]
        output_buffer,
        #[strong]
        state,
        move |_| {
            let path = state.config_path.borrow().clone();
            match std::fs::read_to_string(&path) {
                Ok(text) => config_buffer.set_text(&text),
                Err(err) => {
                    output_buffer.set_text(&format!("无法读取配置 {}: {err}", path.display()))
                }
            }
        }
    ));

    run_button.connect_clicked(clone!(
        #[weak]
        config_buffer,
        #[weak]
        output_buffer,
        #[weak]
        progress,
        #[strong]
        state,
        move |_| {
            let start = config_buffer.start_iter();
            let end = config_buffer.end_iter();
            let json_text = config_buffer.text(&start, &end, false);
            progress.set_fraction(0.15);
            let config: RunConfig = match serde_json::from_str(&json_text) {
                Ok(config) => config,
                Err(err) => {
                    output_buffer.set_text(&format!("配置解析失败: {err}"));
                    progress.set_fraction(0.0);
                    return;
                }
            };
            let run_id = format!("gtk-{}", chrono::Local::now().format("%Y%m%d-%H%M%S"));
            progress.set_fraction(0.55);
            match runner::run(&config, &run_id) {
                Ok(outcome) => {
                    let summary = serde_json::to_string_pretty(&outcome.totals)
                        .unwrap_or_else(|_| "{}".to_string());
                    output_buffer.set_text(&format!(
                        "实验完成\n证据目录: {}\n报告: {}\n汇总:\n{summary}",
                        outcome.journal_path.display(),
                        outcome.report_path.display()
                    ));
                    progress.set_fraction(1.0);
                    *state.last_outcome.borrow_mut() = Some(outcome);
                }
                Err(err) => {
                    output_buffer.set_text(&format!("实验执行失败: {err}"));
                    progress.set_fraction(0.0);
                }
            }
        }
    ));

    clean_button.connect_clicked(clone!(
        #[weak]
        output_buffer,
        #[strong]
        state,
        move |_| {
            if let Some(outcome) = state.last_outcome.borrow().as_ref() {
                let removed =
                    crate::cleanup::remove_temp_dir(&crate::cleanup::temp_dir_for(&outcome.run_id));
                output_buffer.set_text(&format!("临时状态清理: {removed}"));
            } else {
                output_buffer.set_text("尚无运行记录，无需清理");
            }
        }
    ));

    window.present();
}
