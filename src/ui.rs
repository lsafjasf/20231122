use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

use gtk4 as gtk;
use gtk4::prelude::*;

use crate::evidence::EvidenceLog;
use crate::fixture;
use crate::model::{ExtremeParams, SamplingPlan};
use crate::plan::build_demo_plan;
use crate::runner::{self, RunConfig, RunEvent};

enum UiMessage {
    Status(String),
    Sample {
        seq: usize,
        timestamp_ms: u64,
        raw_offset: u64,
        rgb: [u8; 3],
        next: usize,
        total: usize,
    },
    Done {
        message: String,
        ok: bool,
    },
}

pub fn run() -> gtk::glib::ExitCode {
    let app = gtk::Application::builder()
        .application_id("io.example.ColorSampleFlow")
        .build();
    app.connect_activate(build_ui);
    app.run()
}

fn build_ui(app: &gtk::Application) {
    let (source_path, manifest_path) = fixture::default_paths();
    let source = match fixture::ensure_original(&source_path, &manifest_path) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("source initialization failed: {error:#}");
            return;
        }
    };
    let source_caption = format!(
        "原始素材 {} · sha256 {}… · 320×240 · 10 fps",
        source.path,
        &source.sha256[..12]
    );
    let plan = build_demo_plan(source, ExtremeParams::demo());
    let workspaces = PathBuf::from("artifacts/ui-run");

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("视频截图色彩取样 · 可暂停复核对")
        .default_width(980)
        .default_height(720)
        .build();

    let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
    root.set_margin_top(14);
    root.set_margin_bottom(14);
    root.set_margin_start(14);
    root.set_margin_end(14);

    let title = gtk::Label::new(Some("视频截图色彩取样"));
    title.add_css_class("title-1");
    title.set_halign(gtk::Align::Start);
    root.append(&title);

    let source_label = gtk::Label::new(Some(&source_caption));
    source_label.set_halign(gtk::Align::Start);
    source_label.add_css_class("dim-label");
    root.append(&source_label);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let run_button = gtk::Button::with_label("开始并暂停在 8/25");
    let pause_button = gtk::Button::with_label("暂停");
    let resume_button = gtk::Button::with_label("恢复");
    let reference_button = gtk::Button::with_label("全量参考跑");
    actions.append(&run_button);
    actions.append(&pause_button);
    actions.append(&resume_button);
    actions.append(&reference_button);
    root.append(&actions);

    let status = gtk::Label::new(Some("就绪"));
    status.set_halign(gtk::Align::Start);
    root.append(&status);

    let progress = gtk::ProgressBar::new();
    progress.set_show_text(true);
    root.append(&progress);

    let text = gtk::TextView::new();
    text.set_editable(false);
    text.set_monospace(true);
    let scroller = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .child(&text)
        .build();
    root.append(&scroller);
    window.set_child(Some(&root));

    let buffer = text.buffer();
    let (tx, rx) = mpsc::channel::<UiMessage>();
    install_pump(rx, buffer.clone(), status.clone(), progress.clone());

    let busy = Arc::new(AtomicBool::new(false));
    let pause_flag = Arc::new(AtomicBool::new(false));
    {
        let plan = plan.clone();
        let out_dir = workspaces.clone();
        let tx = tx.clone();
        let busy = Arc::clone(&busy);
        let pause_flag = Arc::clone(&pause_flag);
        run_button.connect_clicked(move |_| {
            if busy.swap(true, Ordering::SeqCst) {
                return;
            }
            pause_flag.store(false, Ordering::SeqCst);
            spawn_run(
                plan.clone(),
                out_dir.clone(),
                false,
                Some(ExtremeParams::demo().pause_after),
                Arc::clone(&pause_flag),
                tx.clone(),
                Arc::clone(&busy),
            );
        });
    }
    {
        let flag = Arc::clone(&pause_flag);
        let tx = tx.clone();
        pause_button.connect_clicked(move |_| {
            flag.store(true, Ordering::SeqCst);
            let _ = tx.send(UiMessage::Status("暂停请求已登记；当前项完成后落盘".into()));
        });
    }
    {
        let plan = plan.clone();
        let out_dir = workspaces.clone();
        let tx = tx.clone();
        let busy = Arc::clone(&busy);
        let pause_flag = Arc::clone(&pause_flag);
        resume_button.connect_clicked(move |_| {
            if busy.swap(true, Ordering::SeqCst) {
                return;
            }
            pause_flag.store(false, Ordering::SeqCst);
            spawn_run(
                plan.clone(),
                out_dir.clone(),
                true,
                None,
                Arc::clone(&pause_flag),
                tx.clone(),
                Arc::clone(&busy),
            );
        });
    }
    {
        let plan = plan.clone();
        let out_dir = PathBuf::from("artifacts/ui-reference");
        let tx = tx.clone();
        let busy = Arc::clone(&busy);
        let pause_flag = Arc::clone(&pause_flag);
        reference_button.connect_clicked(move |_| {
            if busy.swap(true, Ordering::SeqCst) {
                return;
            }
            pause_flag.store(false, Ordering::SeqCst);
            spawn_run(
                plan.clone(),
                out_dir.clone(),
                false,
                None,
                Arc::clone(&pause_flag),
                tx.clone(),
                Arc::clone(&busy),
            );
        });
    }

    window.present();
}

fn spawn_run(
    plan: SamplingPlan,
    out_dir: PathBuf,
    resume: bool,
    pause_after: Option<usize>,
    pause_flag: Arc<AtomicBool>,
    tx: Sender<UiMessage>,
    busy: Arc<AtomicBool>,
) {
    std::thread::spawn(move || {
        let result = (|| -> anyhow::Result<bool> {
            std::fs::create_dir_all(&out_dir)?;
            let evidence_path = out_dir.join("evidence.jsonl");
            let mut evidence = if resume {
                EvidenceLog::open_append(&evidence_path)?
            } else {
                EvidenceLog::create(&evidence_path)?
            };
            let config = RunConfig {
                out_dir: out_dir.clone(),
                resume,
                pause_after,
                pause_flag: Some(pause_flag),
            };
            let total = plan.samples.len();
            let mut paused = false;
            let outcome = runner::run(&plan, &config, &mut evidence, |event| match event {
                RunEvent::Resumed { from_index, total } => {
                    let _ = tx.send(UiMessage::Status(format!(
                        "从检查点恢复：{from_index}/{total}"
                    )));
                }
                RunEvent::Sample { result } => {
                    let _ = tx.send(UiMessage::Sample {
                        seq: result.seq,
                        timestamp_ms: result.timestamp_ms,
                        raw_offset: result.raw_offset,
                        rgb: result.rgb,
                        next: result.seq + 1,
                        total,
                    });
                }
                RunEvent::Paused { next_index, reason } => {
                    paused = true;
                    let _ = tx.send(UiMessage::Status(format!(
                        "已暂停于 {next_index}/{total}（{reason}），检查点已落盘"
                    )));
                }
                RunEvent::CheckpointSaved { .. } | RunEvent::Completed { .. } => {}
            })?;
            let cp = outcome.checkpoint();
            runner::write_results(&out_dir, cp)?;
            let message = if paused {
                format!("暂停完成：{} 项已保存", cp.results.len())
            } else {
                format!(
                    "处理完成：{} 项，scale={}%，decoder={}",
                    cp.results.len(),
                    cp.scale_percent,
                    cp.decoder
                )
            };
            let _ = tx.send(UiMessage::Done { message, ok: true });
            Ok(!paused)
        })();
        if let Err(error) = result {
            let _ = tx.send(UiMessage::Done {
                message: format!("失败，检查点与证据已保留：{error:#}"),
                ok: false,
            });
        }
        busy.store(false, Ordering::SeqCst);
    });
}

fn install_pump(
    rx: Receiver<UiMessage>,
    buffer: gtk::TextBuffer,
    status: gtk::Label,
    progress: gtk::ProgressBar,
) {
    gtk::glib::timeout_add_local(Duration::from_millis(80), move || {
        while let Ok(message) = rx.try_recv() {
            match message {
                UiMessage::Status(text) => {
                    status.set_text(&text);
                    append_line(&buffer, &text);
                }
                UiMessage::Sample {
                    seq,
                    timestamp_ms,
                    raw_offset,
                    rgb,
                    next,
                    total,
                } => {
                    let text = format!(
                        "#{seq:02} t={timestamp_ms:4} ms raw_off={raw_offset:6} rgb=({:3},{:3},{:3})",
                        rgb[0], rgb[1], rgb[2]
                    );
                    append_line(&buffer, &text);
                    progress.set_fraction(next as f64 / total.max(1) as f64);
                    progress.set_text(Some(&format!("{next}/{total}")));
                }
                UiMessage::Done { message, ok } => {
                    status.set_text(&message);
                    let prefix = if ok { "完成" } else { "中止" };
                    append_line(&buffer, &format!("[{prefix}] {message}"));
                }
            }
        }
        gtk::glib::ControlFlow::Continue
    });
}

fn append_line(buffer: &gtk::TextBuffer, text: &str) {
    let mut end = buffer.end_iter();
    buffer.insert(&mut end, text);
    buffer.insert(&mut end, "\n");
}
