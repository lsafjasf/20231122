use crate::journal::read_journal;
use gtk4::prelude::*;
use gtk4::{
    Application, ApplicationWindow, Box as GtkBox, Button, CheckButton, Label, Notebook,
    Orientation, Paned, ProgressBar, ScrolledWindow, TextView,
};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

const APP_ID: &str = "dev.proxyrecover.experiment";
const REPORT: &str = "artifacts/latest/REPORT.md";
const JOURNAL: &str = "artifacts/latest/journal.jsonl";

pub fn run() {
    let app = Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_ui);
    app.run_with_args(&[] as &[String]);
}

fn text_view() -> TextView {
    let view = TextView::new();
    view.set_editable(false);
    view.set_monospace(true);
    view.set_wrap_mode(gtk4::WrapMode::WordChar);
    view.set_margin_top(8);
    view.set_margin_bottom(8);
    view.set_margin_start(8);
    view.set_margin_end(8);
    view
}

fn scroll(child: &impl IsA<gtk4::Widget>) -> ScrolledWindow {
    ScrolledWindow::builder()
        .child(child)
        .vexpand(true)
        .hexpand(true)
        .build()
}

fn build_ui(app: &Application) {
    let title = Label::new(Some("素材代理失效恢复 · 可回放实验"));
    title.add_css_class("title-3");
    title.set_xalign(0.0);
    let subtitle = Label::new(Some("Rust + GStreamer 代理事务 · GTK4 回放与复核"));
    subtitle.set_xalign(0.0);
    subtitle.add_css_class("dim-label");

    let run_btn = Button::with_label("运行实验");
    let replay_btn = Button::with_label("回放恢复记录");
    let verify_btn = Button::with_label("校验证据清单");
    let progress = ProgressBar::new();
    progress.set_show_text(true);
    progress.set_fraction(0.0);
    let status = Label::new(Some("就绪"));
    status.set_xalign(0.0);

    let log = text_view();
    let auto_view = text_view();
    let manual_view = text_view();
    let failure_view = text_view();

    let manual_items = GtkBox::new(Orientation::Vertical, 6);
    for label in [
        "已查看初始代理与恢复后代理，内容/尺寸正确",
        "已核对失效样本确实不可直接使用",
        "已确认未知扩展隔离符合产品策略",
        "已检查自动结论与人工确认分区及回放按钮",
    ] {
        manual_items.append(&CheckButton::with_label(label));
    }
    manual_items.append(&Label::new(Some(
        "以上勾选仅用于人工复核，不改变自动检查结果。",
    )));

    let manual_page = GtkBox::new(Orientation::Vertical, 8);
    manual_page.append(&scroll(&manual_view));
    manual_page.append(&manual_items);

    let notebook = Notebook::new();
    notebook.append_page(&scroll(&auto_view), Some(&Label::new(Some("自动结论"))));
    notebook.append_page(&manual_page, Some(&Label::new(Some("人工确认"))));
    notebook.append_page(
        &scroll(&failure_view),
        Some(&Label::new(Some("失败后续操作"))),
    );
    notebook.set_vexpand(true);

    let buttons = GtkBox::new(Orientation::Horizontal, 8);
    buttons.append(&run_btn);
    buttons.append(&replay_btn);
    buttons.append(&verify_btn);

    let left = GtkBox::new(Orientation::Vertical, 8);
    left.set_margin_top(10);
    left.set_margin_bottom(10);
    left.set_margin_start(10);
    left.set_margin_end(10);
    left.append(&title);
    left.append(&subtitle);
    left.append(&buttons);
    left.append(&progress);
    left.append(&status);
    left.append(&scroll(&log));
    left.set_size_request(420, -1);

    let paned = Paned::new(Orientation::Horizontal);
    paned.set_start_child(Some(&left));
    paned.set_end_child(Some(&notebook));
    paned.set_position(450);
    paned.set_vexpand(true);

    let window = ApplicationWindow::builder()
        .application(app)
        .title("素材代理失效恢复实验")
        .default_width(1080)
        .default_height(680)
        .child(&paned)
        .build();

    let (tx, rx) = mpsc::channel::<String>();
    wire_run(&run_btn, tx.clone(), status.clone(), progress.clone());
    wire_replay(&replay_btn, tx.clone(), status.clone());
    wire_verify(&verify_btn, tx.clone(), status.clone());

    {
        let log = log.clone();
        let status = status.clone();
        let progress = progress.clone();
        let auto_view = auto_view.clone();
        let manual_view = manual_view.clone();
        let failure_view = failure_view.clone();
        gtk4::glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
            while let Ok(message) = rx.try_recv() {
                let mut end = log.buffer().end_iter();
                if let Some(rest) = message.strip_prefix("PROGRESS ") {
                    if let Ok(fraction) = rest.trim().parse::<f64>() {
                        progress.set_fraction(fraction);
                    }
                } else if let Some(rest) = message.strip_prefix("STEP ") {
                    status.set_text(rest);
                    log.buffer().insert(&mut end, &format!("• {rest}\n"));
                } else if let Some(rest) = message.strip_prefix("REPORT ") {
                    load_report(
                        Path::new(rest.trim()),
                        &auto_view,
                        &manual_view,
                        &failure_view,
                    );
                    status.set_text("报告已加载");
                } else if let Some(rest) = message.strip_prefix("DONE ") {
                    status.set_text("完成");
                    progress.set_fraction(1.0);
                    log.buffer().insert(&mut end, &format!("{rest}\n"));
                } else if let Some(rest) = message.strip_prefix("ERROR ") {
                    status.set_text("失败");
                    log.buffer().insert(&mut end, &format!("错误: {rest}\n"));
                } else {
                    log.buffer().insert(&mut end, &format!("{message}\n"));
                }
            }
            gtk4::glib::ControlFlow::Continue
        });
    }

    if Path::new(REPORT).is_file() {
        load_report(Path::new(REPORT), &auto_view, &manual_view, &failure_view);
    }
    if Path::new(JOURNAL).is_file() {
        load_journal(Path::new(JOURNAL), &log);
    }

    window.present();
    if std::env::var("PROXYRECOVER_AUTO_RUN").is_ok() {
        run_btn.emit_clicked();
    }
}

fn wire_run(button: &Button, tx: mpsc::Sender<String>, status: Label, progress: ProgressBar) {
    let button = button.clone();
    button.connect_clicked(move |_| {
        status.set_text("实验运行中…");
        progress.set_fraction(0.0);
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut current = 0.0f64;
            let mut callback = |message: &str| {
                current += 1.0;
                let _ = tx.send(format!("STEP {message}"));
                let _ = tx.send(format!("PROGRESS {:.3}", (current / 22.0).min(0.95)));
            };
            match crate::experiment::run_experiment(Path::new("artifacts/latest"), &mut callback) {
                Ok(output) => {
                    let _ = tx.send(format!("REPORT {}", output.report_path.display()));
                    let _ = tx.send(format!("DONE {}", crate::experiment::run_summary(&output)));
                }
                Err(error) => {
                    let _ = tx.send(format!("ERROR {error}"));
                }
            }
        });
    });
}

fn wire_replay(button: &Button, tx: mpsc::Sender<String>, status: Label) {
    let button = button.clone();
    button.connect_clicked(move |_| {
        let journal = PathBuf::from(JOURNAL);
        if !journal.is_file() {
            let _ = tx.send("ERROR 未找到 artifacts/latest/journal.jsonl，请先运行实验".into());
            return;
        }
        status.set_text("回放中…");
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut callback = |message: &str| {
                let _ = tx.send(format!("STEP {message}"));
            };
            match crate::replay::replay_journal(&journal, &mut callback) {
                Ok(outcome) if outcome.mismatches.is_empty() => {
                    let _ = tx.send(format!(
                        "DONE 回放 {}/{} 步一致; {}",
                        outcome.matched,
                        outcome.total,
                        outcome.report_path.display()
                    ));
                }
                Ok(outcome) => {
                    let _ = tx.send(format!(
                        "ERROR 回放 {}/{} 步一致，{} 处不一致; {}",
                        outcome.matched,
                        outcome.total,
                        outcome.mismatches.len(),
                        outcome.report_path.display()
                    ));
                }
                Err(error) => {
                    let _ = tx.send(format!("ERROR 回放失败: {error}"));
                }
            }
        });
    });
}

fn wire_verify(button: &Button, tx: mpsc::Sender<String>, status: Label) {
    let button = button.clone();
    button.connect_clicked(move |_| {
        let manifest = PathBuf::from("artifacts/latest/evidence/manifest.json");
        status.set_text("校验证据…");
        match crate::report::verify_manifest(&manifest) {
            Ok(count) => {
                let _ = tx.send(format!("DONE 证据清单校验通过: {count} 个文件"));
            }
            Err(error) => {
                let _ = tx.send(format!("ERROR 证据清单校验失败: {error}"));
            }
        }
    });
}

fn load_report(path: &Path, auto: &TextView, manual: &TextView, failure: &TextView) {
    let Ok(report) = std::fs::read_to_string(path) else {
        return;
    };
    let auto_text = section(&report, "## 自动结论", "## 人工确认");
    let manual_text = section(&report, "## 人工确认", "## 失败时的可执行后续操作");
    let failure_text = section(&report, "## 失败时的可执行后续操作", "## 回放方法");
    auto.buffer().set_text(if auto_text.is_empty() {
        &report
    } else {
        auto_text
    });
    manual.buffer().set_text(manual_text);
    failure.buffer().set_text(failure_text);
}

fn load_journal(path: &Path, log: &TextView) {
    let Ok(records) = read_journal(path) else {
        return;
    };
    let mut text = String::from("已加载历史步骤（可以回放）：\n");
    for record in records {
        text.push_str(&format!(
            "#{:02} {}\n    {} -> {}\n",
            record.seq, record.step, record.state_before, record.state_after
        ));
    }
    log.buffer().set_text(&text);
}

fn section<'a>(report: &'a str, start: &str, end: &str) -> &'a str {
    let Some(from) = report.find(start) else {
        return "";
    };
    let rest = &report[from..];
    let until = rest.find(end).unwrap_or(rest.len());
    &rest[..until]
}
