use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow, Box as GtkBox, Button, Label, Orientation,
           ProgressBar, ScrolledWindow, TextView};
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::atomic::Ordering;

const APP_ID: &str = "dev.ditherdiag.app";

pub fn run() {
    let app = Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_ui);
    app.run_with_args(&[] as &[String]);
}

fn build_ui(app: &Application) {
    let title = Label::new(Some("音频导出抖动诊断 (Rust + GStreamer + GTK4)"));
    title.add_css_class("title-4");

    let run_btn = Button::with_label("运行实验");
    let replay_btn = Button::with_label("回放日志");
    let cancel_btn = Button::with_label("中途取消(手动)");
    cancel_btn.set_tooltip_text(Some("请求当前导出立即中止(置 NULL,不发 EOS)"));

    let progress = ProgressBar::new();
    progress.set_show_text(true);
    let status = Label::new(Some("就绪"));
    status.set_xalign(0.0);

    let log = TextView::new();
    log.set_editable(false);
    log.set_monospace(true);
    let scroll = ScrolledWindow::new();
    scroll.set_child(Some(&log));
    scroll.set_vexpand(true);
    scroll.set_min_content_height(320);

    let buttons = GtkBox::new(Orientation::Horizontal, 8);
    buttons.append(&run_btn);
    buttons.append(&replay_btn);
    buttons.append(&cancel_btn);

    let root = GtkBox::new(Orientation::Vertical, 8);
    root.set_margin_top(12);
    root.set_margin_bottom(12);
    root.set_margin_start(12);
    root.set_margin_end(12);
    root.append(&title);
    root.append(&buttons);
    root.append(&progress);
    root.append(&status);
    root.append(&scroll);

    let window = ApplicationWindow::builder()
        .application(app)
        .title("音频导出抖动诊断")
        .default_width(760)
        .default_height(520)
        .child(&root)
        .build();

    let (tx, rx) = mpsc::channel::<String>();

    {
        let tx = tx.clone();
        let status = status.clone();
        let progress = progress.clone();
        run_btn.connect_clicked(move |_| {
            status.set_text("实验运行中…");
            progress.set_fraction(0.0);
            let tx = tx.clone();
            std::thread::spawn(move || {
                let out = PathBuf::from("artifacts/latest");
                let total = 10.0f64;
                let mut n = 0.0f64;
                let txp = tx.clone();
                let mut cb = move |msg: &str| {
                    n += 1.0;
                    let _ = txp.send(format!("STEP {msg}"));
                    let _ = txp.send(format!("PROGRESS {:.3}", (n / total).min(1.0)));
                };
                match crate::experiment::run_experiment(&out, &mut cb) {
                    Ok(output) => {
                        let _ = tx.send(format!("DONE {}", crate::experiment::run_summary(&output)));
                    }
                    Err(e) => {
                        let _ = tx.send(format!("ERROR {e}"));
                    }
                }
            });
        });
    }

    {
        let tx = tx.clone();
        replay_btn.connect_clicked(move |_| {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let journal = PathBuf::from("artifacts/latest/journal.jsonl");
                if !journal.exists() {
                    let _ = tx.send("ERROR 未找到 artifacts/latest/journal.jsonl,请先运行实验".into());
                    return;
                }
                match crate::experiment::replay_journal(&journal) {
                    Ok(true) => {
                        let _ = tx.send("DONE 回放完成: 全部步骤与记录一致".into());
                    }
                    Ok(false) => {
                        let _ = tx.send("ERROR 回放完成: 存在不一致步骤(见终端输出)".into());
                    }
                    Err(e) => {
                        let _ = tx.send(format!("ERROR 回放失败: {e}"));
                    }
                }
            });
        });
    }

    cancel_btn.connect_clicked(move |_| {
        crate::pipeline::MANUAL_CANCEL.store(true, Ordering::SeqCst);
    });

    {
        let log = log.clone();
        let status = status.clone();
        let progress = progress.clone();
        gtk4::glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
            while let Ok(msg) = rx.try_recv() {
                let buf = log.buffer();
                let mut end = buf.end_iter();
                if let Some(rest) = msg.strip_prefix("PROGRESS ") {
                    if let Ok(f) = rest.trim().parse::<f64>() {
                        progress.set_fraction(f);
                    }
                } else if let Some(rest) = msg.strip_prefix("STEP ") {
                    status.set_text(rest);
                    buf.insert(&mut end, &format!("• {rest}\n"));
                } else if let Some(rest) = msg.strip_prefix("DONE ") {
                    status.set_text("完成");
                    progress.set_fraction(1.0);
                    buf.insert(&mut end, &format!("{rest}\n"));
                } else if let Some(rest) = msg.strip_prefix("ERROR ") {
                    status.set_text("失败");
                    buf.insert(&mut end, &format!("错误: {rest}\n"));
                } else {
                    buf.insert(&mut end, &format!("{msg}\n"));
                }
            }
            gtk4::glib::ControlFlow::Continue
        });
    }

    window.present();

    if std::env::var("DITHERDIAG_AUTO").is_ok() {
        run_btn.emit_clicked();
    }
}
