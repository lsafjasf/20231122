//! GTK4 control surface for the fingerprint pipeline.
//!
//! The GUI is intentionally a thin shell over the core library: long running
//! pipeline work executes on a worker thread while the main loop stays
//! responsive, and every state transition is reflected by reloading the
//! checkpoint from disk (the same file a CLI colleague would inspect).

use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use segment_fingerprint_core::model::RunRecord;
use segment_fingerprint_core::pipeline::{resume_run, start_run, RunOptions};
use segment_fingerprint_core::verify::verify;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc::{channel, Receiver, Sender};

#[derive(Debug)]
enum WorkerMessage {
    Log(String),
    Finished(Result<(), String>),
}

#[derive(Default)]
struct AppState {
    case_path: Option<PathBuf>,
    out_dir: Option<PathBuf>,
}

fn main() -> glib::ExitCode {
    segment_fingerprint_core::decode::ensure_initialized();
    let application = gtk::Application::new(
        Some("org.example.segmentfingerprint"),
        gtk::gio::ApplicationFlags::FLAGS_NONE,
    );
    application.connect_activate(move |app| {
        build_ui(app);
        if std::env::var("SF_GTK_SELFTEST").as_deref() == Ok("1") {
            // Headless smoke path: every widget was constructed and signal
            // handlers wired; quit before entering an interactive event loop.
            // Triggered via env because GTK's Application rejects unknown flags.
            let application = app.clone();
            glib::idle_add_local_once(move || application.quit());
        }
    });
    application.run()
}

fn build_ui(app: &gtk::Application) {
    let state = Rc::new(RefCell::new(AppState::default()));
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("片段内容指纹控制台")
        .default_width(980)
        .default_height(720)
        .build();

    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    root.set_margin_top(12);
    root.set_margin_bottom(12);
    root.set_margin_start(12);
    root.set_margin_end(12);

    let case_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let case_button = gtk::Button::with_label("选择 case.json");
    let case_label = gtk::Label::new(Some("未选择案例"));
    case_label.set_hexpand(true);
    case_label.set_xalign(0.0);
    case_row.append(&case_button);
    case_row.append(&case_label);

    let out_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let out_button = gtk::Button::with_label("选择输出目录");
    let out_label = gtk::Label::new(Some("未选择输出目录"));
    out_label.set_hexpand(true);
    out_label.set_xalign(0.0);
    out_row.append(&out_button);
    out_row.append(&out_label);

    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let run_button = gtk::Button::with_label("运行（可暂停）");
    let pause_button = gtk::Button::with_label("暂停");
    let resume_button = gtk::Button::with_label("恢复（降级）");
    let verify_button = gtk::Button::with_label("独立复核");
    let degrade_switch = gtk::Switch::new();
    let degrade_label = gtk::Label::new(Some("强制降级"));
    let pause_spin = gtk::SpinButton::with_range(0.0, 100.0, 1.0);
    pause_spin.set_value(4.0);
    controls.append(&run_button);
    controls.append(&pause_button);
    controls.append(&resume_button);
    controls.append(&verify_button);
    controls.append(&degrade_label);
    controls.append(&degrade_switch);
    controls.append(&gtk::Label::new(Some("暂停于片段数")));
    controls.append(&pause_spin);

    let progress = gtk::ProgressBar::new();
    let status_label = gtk::Label::new(Some("就绪"));
    status_label.set_xalign(0.0);

    let columns = [
        ("片段", 90),
        ("决策", 90),
        ("相似度‰", 90),
        ("覆盖率‰", 90),
        ("设备A 指纹", 200),
        ("设备B 指纹", 200),
    ];
    let list_store = gtk::ListStore::new(&[
        glib::Type::STRING;
        6
    ]);
    let tree_view = gtk::TreeView::with_model(&list_store);
    for (index, (title, width)) in columns.iter().enumerate() {
        let renderer = gtk::CellRendererText::new();
        let column = gtk::TreeViewColumn::with_attributes(title, &renderer, &[("text", index as i32)]);
        column.set_fixed_width(*width);
        tree_view.append_column(&column);
    }
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_vexpand(true);
    scroll.set_child(Some(&tree_view));

    let log_buffer = gtk::TextBuffer::new(None);
    let log_view = gtk::TextView::with_buffer(&log_buffer);
    log_view.set_editable(false);
    log_view.set_height_request(140);

    root.append(&case_row);
    root.append(&out_row);
    root.append(&controls);
    root.append(&progress);
    root.append(&status_label);
    root.append(&scroll);
    root.append(&log_view);
    window.set_child(Some(&root));

    let (sender, receiver): (Sender<WorkerMessage>, Receiver<WorkerMessage>) = channel();
    let log_sender = sender.clone();
    let append_log = move |line: String| {
        let _ = log_sender.send(WorkerMessage::Log(line));
    };

    case_button.connect_clicked({
        let state = state.clone();
        let case_label = case_label.clone();
        let window = window.clone();
        move |_| {
            let dialog = gtk::FileChooserDialog::new(
                Some("选择案例清单"),
                Some(&window),
                gtk::FileChooserAction::Open,
                &[
                    ("取消", gtk::ResponseType::Cancel),
                    ("打开", gtk::ResponseType::Accept),
                ],
            );
            let filter = gtk::FileFilter::new();
            filter.add_pattern("*.json");
            dialog.add_filter(&filter);
            let state = state.clone();
            let case_label = case_label.clone();
            dialog.connect_response(move |dialog, response| {
                if response == gtk::ResponseType::Accept {
                    if let Some(file) = dialog.file() {
                        if let Some(path) = file.path() {
                            case_label.set_label(&path.display().to_string());
                            state.borrow_mut().case_path = Some(path);
                        }
                    }
                }
                dialog.close();
            });
            dialog.present();
        }
    });

    out_button.connect_clicked({
        let state = state.clone();
        let out_label = out_label.clone();
        let window = window.clone();
        move |_| {
            let dialog = gtk::FileChooserDialog::new(
                Some("选择输出目录"),
                Some(&window),
                gtk::FileChooserAction::SelectFolder,
                &[
                    ("取消", gtk::ResponseType::Cancel),
                    ("选择", gtk::ResponseType::Accept),
                ],
            );
            let state = state.clone();
            let out_label = out_label.clone();
            dialog.connect_response(move |dialog, response| {
                if response == gtk::ResponseType::Accept {
                    if let Some(file) = dialog.file() {
                        if let Some(path) = file.path() {
                            out_label.set_label(&path.display().to_string());
                            state.borrow_mut().out_dir = Some(path);
                        }
                    }
                }
                dialog.close();
            });
            dialog.present();
        }
    });

    run_button.connect_clicked({
        let state = state.clone();
        let sender = sender.clone();
        let pause_spin = pause_spin.clone();
        let degrade_switch = degrade_switch.clone();
        move |_| {
            let borrowed = state.borrow();
            let (Some(case_path), Some(out_dir)) =
                (borrowed.case_path.clone(), borrowed.out_dir.clone())
            else {
                let _ = sender.send(WorkerMessage::Log("请先选择案例和输出目录".into()));
                return;
            };
            drop(borrowed);
            let pause_value = pause_spin.value() as usize;
            let pause_after = if pause_value == 0 { None } else { Some(pause_value) };
            let force_degrade = degrade_switch.is_active();
            let sender = sender.clone();
            std::thread::spawn(move || {
                let result = start_run(RunOptions {
                    case_path,
                    output_dir: out_dir,
                    pause_after,
                    force_degrade,
                    command_line: vec!["segment-fingerprint-gtk".into(), "run".into()],
                })
                .map(|_| ())
                .map_err(|error| format!("{error:#}"));
                let _ = sender.send(WorkerMessage::Finished(result));
            });
        }
    });

    pause_button.connect_clicked({
        let append_log = append_log.clone();
        move |_| append_log("暂停请求会在当前片段完成后生效；CLI 等价参数为 --pause-after N".to_string())
    });

    resume_button.connect_clicked({
        let state = state.clone();
        let sender = sender.clone();
        let degrade_switch = degrade_switch.clone();
        move |_| {
            let Some(out_dir) = state.borrow().out_dir.clone() else {
                let _ = sender.send(WorkerMessage::Log("请先选择包含 checkpoint.json 的输出目录".into()));
                return;
            };
            let force_degrade = degrade_switch.is_active() || true;
            let sender = sender.clone();
            std::thread::spawn(move || {
                let case_path = out_dir.join("input/case.json");
                let result = resume_run(RunOptions {
                    case_path,
                    output_dir: out_dir,
                    pause_after: None,
                    force_degrade,
                    command_line: vec!["segment-fingerprint-gtk".into(), "resume".into()],
                })
                .map(|_| ())
                .map_err(|error| format!("{error:#}"));
                let _ = sender.send(WorkerMessage::Finished(result));
            });
        }
    });

    verify_button.connect_clicked({
        let state = state.clone();
        let sender = sender.clone();
        move |_| {
            let Some(out_dir) = state.borrow().out_dir.clone() else {
                let _ = sender.send(WorkerMessage::Log("请先选择包含 run-record.json 的输出目录".into()));
                return;
            };
            let sender = sender.clone();
            std::thread::spawn(move || {
                let result = verify(&out_dir.join("run-record.json"), true).map(|report| {
                    let _ = sender.send(WorkerMessage::Log(format!(
                        "复核结论：{}（{} 项检查）",
                        report.verdict,
                        report.checks.len()
                    )));
                })
                .map(|_| ())
                .map_err(|error| format!("{error:#}"));
                let _ = sender.send(WorkerMessage::Finished(result));
            });
        }
    });

    let receiver = std::sync::Arc::new(std::sync::Mutex::new(receiver));
    let tick_receiver = receiver.clone();
    gtk::glib::idle_add_local(move || {
        let Ok(guard) = tick_receiver.lock() else {
            return glib::ControlFlow::Continue;
        };
        while let Ok(message) = guard.try_recv() {
            match message {
            WorkerMessage::Log(line) => {
                let mut end = log_buffer.end_iter();
                log_buffer.insert(&mut end, &format!("{line}\n"));
            }
            WorkerMessage::Finished(result) => match result {
                Ok(()) => {
                    status_label.set_label("工作线程完成");
                    if let Some(out_dir) = state.borrow().out_dir.clone() {
                        refresh_views(&out_dir, &list_store, &progress, &status_label);
                    }
                }
                Err(error) => {
                    status_label.set_label("失败");
                    let mut end = log_buffer.end_iter();
                    log_buffer.insert(&mut end, &format!("错误：{error:#}\n"));
                }
            },
        }
        }
        glib::ControlFlow::Continue
    });
    window.present();
}

fn refresh_views(
    out_dir: &std::path::Path,
    list_store: &gtk::ListStore,
    progress: &gtk::ProgressBar,
    status_label: &gtk::Label,
) {
    list_store.clear();
    let record_path = out_dir.join("run-record.json");
    let checkpoint_path = out_dir.join("checkpoint.json");
    let path = if record_path.exists() {
        record_path
    } else {
        checkpoint_path
    };
    let Ok(bytes) = std::fs::read(path) else {
        status_label.set_label("尚无检查点");
        return;
    };
    let Ok(record): Result<RunRecord, _> = serde_json::from_slice(&bytes) else {
        status_label.set_label("检查点解析失败");
        return;
    };
    let fraction = if record.summary.segments_total == 0 {
        0.0
    } else {
        record.summary.segments_completed as f64 / record.summary.segments_total as f64
    };
    progress.set_fraction(fraction);
    status_label.set_label(&format!(
        "状态={:?} 模式={:?} 完成 {}/{} 匹配={} 缺失={}",
        record.run.status,
        record.run.mode,
        record.summary.segments_completed,
        record.summary.segments_total,
        record.summary.matches,
        record.summary.missing
    ));
    for segment in &record.segments {
        let fingerprint_a = segment.devices[0].fingerprint_sha256[..16].to_string();
        let fingerprint_b = segment
            .devices
            .get(1)
            .map(|device| device.fingerprint_sha256[..16].to_string())
            .unwrap_or_else(|| "-".to_string());
        list_store.set(
            &list_store.append(),
            &[
                (0, &segment.segment_id as &dyn glib::value::ToValue),
                (1, &segment.decision.as_str()),
                (2, &segment.similarity_milli.to_string()),
                (3, &segment.coverage_milli.to_string()),
                (4, &fingerprint_a),
                (
                    5,
                    &fingerprint_b,
                ),
            ],
        );
    }
}
