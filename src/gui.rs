use crate::engine::{self, ProcessOptions, RunPaths};
use crate::evidence;
use crate::io::{build_anchors, file_sha256, probe_wav};
use crate::model::{CalibrationConfig, RunState};
use anyhow::Result;
use gtk::gdk::Display;
use gtk::glib;
use gtk::prelude::*;
use gtk4 as gtk;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

const APP_CSS: &str = "
window { background: #0f141d; color: #e6edf6; }
.hero { font-size: 24px; font-weight: 800; letter-spacing: 1px; }
.subtitle { color: #8ea0b8; }
.panel { background: #171f2c; border: 1px solid #273449; border-radius: 12px; padding: 14px; }
.section { font-size: 14px; font-weight: 700; color: #d9e6f8; }
.muted { color: #8ea0b8; }
.mono { font-family: monospace; }
.big { font-size: 18px; font-weight: 700; }
.anchor { background: #151d29; border: 1px solid #2a394f; border-radius: 11px; padding: 13px; }
.anchor-done { border-color: #2f9e63; background: #14251e; }
.anchor-paused { border-color: #d29922; background: #292316; }
.badge { font-size: 11px; font-weight: 700; padding: 2px 8px; border-radius: 20px; background: #273449; }
.badge-done { background: #1f6f45; color: #dcffe9; }
.badge-paused { background: #7a5a13; color: #ffe8ad; }
.good { color: #56d68b; }
.warn { color: #f2c94c; }
.error { color: #ff7b72; }
.accent { color: #6cb6ff; }
button { border-radius: 8px; padding: 6px 14px; }
button.suggested-action { background: #1f6feb; color: white; }
button.destructive-action { background: #7a2e2e; color: white; }
progressbar trough { background: #263246; border-radius: 7px; min-height: 10px; }
progressbar progress { background: #2f9e63; border-radius: 7px; min-height: 10px; }
textview { background: #0c1119; color: #c9d6e6; font-family: monospace; }
";

#[derive(Debug)]
enum WorkerMessage {
    Progress(String),
    Done(Box<RunState>),
    Failed(String),
}

struct SourceFacts {
    name: String,
    size: u64,
    duration_ns: u64,
    sample_rate: u32,
    channels: u16,
    bits_per_sample: u16,
    data_offset: u64,
    hash: String,
    anchors: Vec<(String, u64, u64, u64)>,
}

#[derive(Clone)]
struct AnchorView {
    card: gtk::Box,
    badge: gtk::Label,
    time: gtk::Label,
    offset: gtk::Label,
    levels: gtk::Label,
    gain: gtk::Label,
    evidence: gtk::Label,
}

#[derive(Clone)]
struct Widgets {
    panel: gtk::Box,
    status: gtk::Label,
    progress: gtk::ProgressBar,
    mode: gtk::Label,
    resource: gtk::Label,
    run_path: gtk::Label,
    log: gtk::TextBuffer,
    run_button: gtk::Button,
    pause_button: gtk::Button,
    verify_button: gtk::Button,
    force_degrade: gtk::CheckButton,
    anchors: Vec<AnchorView>,
}

pub fn run(source: PathBuf, run: PathBuf) -> Result<ExitCode> {
    let application = gtk::Application::builder()
        .application_id("org.example.loudness.calibrator")
        .build();
    application.connect_activate(move |app| {
        build_ui(app, source.clone(), run.clone());
    });
    application.run_with_args::<&str>(&[]);
    Ok(ExitCode::SUCCESS)
}

fn inspect_source(source: &Path) -> SourceFacts {
    let fallback = SourceFacts {
        name: source
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "原始材料".to_string()),
        size: std::fs::metadata(source)
            .map(|meta| meta.len())
            .unwrap_or(0),
        duration_ns: 0,
        sample_rate: 0,
        channels: 0,
        bits_per_sample: 0,
        data_offset: 44,
        hash: "不可读".to_string(),
        anchors: vec![
            ("A".to_string(), 5_000_000_000, 240_000, 480_044),
            ("B".to_string(), 10_000_000_000, 480_000, 960_044),
            ("C".to_string(), 15_000_000_000, 720_000, 1_440_044),
        ],
    };
    let Ok(wav) = probe_wav(source) else {
        return fallback;
    };
    let Ok(specs) = build_anchors(&wav) else {
        return fallback;
    };
    SourceFacts {
        name: fallback.name,
        size: wav.file_size,
        duration_ns: wav.duration_ns,
        sample_rate: wav.sample_rate,
        channels: wav.channels,
        bits_per_sample: wav.bits_per_sample,
        data_offset: wav.data_offset,
        hash: file_sha256(source).unwrap_or_else(|_| "哈希失败".to_string()),
        anchors: specs
            .into_iter()
            .map(|spec| {
                (
                    spec.id,
                    spec.anchor_ns,
                    spec.anchor_frame,
                    spec.file_end_byte,
                )
            })
            .collect(),
    }
}

fn build_ui(app: &gtk::Application, source: PathBuf, run: PathBuf) {
    let css = gtk::CssProvider::new();
    css.load_from_data(APP_CSS);
    if let Some(display) = Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }

    let facts = inspect_source(&source);
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("响度分段校准 · 可暂停工作台")
        .default_width(1180)
        .default_height(820)
        .build();
    let root = gtk::Box::new(gtk::Orientation::Vertical, 12);
    root.set_margin_top(18);
    root.set_margin_bottom(18);
    root.set_margin_start(18);
    root.set_margin_end(18);
    root.append(&build_header());
    root.append(&build_source_panel(&facts, &source, &run));
    let (anchor_row, anchor_views) = build_anchor_row(&facts);
    root.append(&anchor_row);
    let widgets = build_controls_and_log(&run, anchor_views);
    root.append(&widgets.panel);
    window.set_child(Some(&root));
    window.present();
    if let Ok(path) = std::env::var("LOUDNESS_GUI_SCREENSHOT") {
        let window_clone = window.clone();
        let app_clone = app.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(900), move || {
            let window_ref: &gtk::Window = window_clone.upcast_ref();
            match capture_window(window_ref, Path::new(&path)) {
                Ok(()) => eprintln!("screenshot: {path}"),
                Err(error) => eprintln!("screenshot failed: {error:#}"),
            }
            app_clone.quit();
        });
    }

    let initial_state = if run.join("state.json").exists() {
        engine::load_state(&RunPaths::new(&run)).ok()
    } else {
        None
    };
    if let Some(state) = initial_state {
        refresh_state(&widgets, &state);
        append_log(
            &widgets,
            &format!(
                "已载入断点：status={} next_segment={} resumes={}",
                state.status, state.next_segment, state.resume_count
            ),
        );
    } else {
        append_log(&widgets, "等待开始。暂停请求会在当前锚点完成后生效。");
    }

    let pause_flag = Arc::new(AtomicBool::new(false));
    let busy = Arc::new(AtomicBool::new(false));
    wire_actions(&widgets, source, run, pause_flag, busy);
}

fn capture_window(window: &gtk::Window, path: &Path) -> Result<()> {
    let width = f64::from(window.width().max(1));
    let height = f64::from(window.height().max(1));
    let paintable = gtk::WidgetPaintable::new(Some(window));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, width, height);
    let node = snapshot
        .to_node()
        .ok_or_else(|| anyhow::anyhow!("窗口没有可渲染节点"))?;
    let renderer = gtk::gsk::CairoRenderer::new();
    if !renderer.is_realized() {
        renderer.realize(None)?;
    }
    let viewport = gtk::graphene::Rect::new(0.0, 0.0, width as f32, height as f32);
    let texture = renderer.render_texture(&node, Some(&viewport));
    texture.save_to_png(path)?;
    renderer.unrealize();
    Ok(())
}

fn build_header() -> gtk::Box {
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    let titles = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let title = gtk::Label::new(Some("响度分段校准"));
    title.set_css_classes(&["hero"]);
    title.set_halign(gtk::Align::Start);
    let subtitle = gtk::Label::new(Some("三段锚点检查点 · 原子断点 · 恢复复测 · 资源降级证据"));
    subtitle.set_css_classes(&["subtitle"]);
    subtitle.set_halign(gtk::Align::Start);
    titles.append(&title);
    titles.append(&subtitle);
    header.append(&titles);
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    header.append(&spacer);
    let tech = gtk::Label::new(Some("Rust / GStreamer / GTK4"));
    tech.set_css_classes(&["badge", "accent"]);
    header.append(&tech);
    header
}

fn build_source_panel(facts: &SourceFacts, source: &Path, run: &Path) -> gtk::Box {
    let panel = gtk::Box::new(gtk::Orientation::Horizontal, 20);
    panel.set_css_classes(&["panel"]);
    let left = gtk::Box::new(gtk::Orientation::Vertical, 4);
    left.set_hexpand(true);
    let heading = gtk::Label::new(Some("未修改原始材料"));
    heading.set_css_classes(&["section"]);
    heading.set_halign(gtk::Align::Start);
    let file = gtk::Label::new(Some(&facts.name));
    file.set_css_classes(&["big"]);
    file.set_halign(gtk::Align::Start);
    let path = gtk::Label::new(Some(&source.to_string_lossy()));
    path.set_css_classes(&["muted", "mono"]);
    path.set_selectable(true);
    path.set_halign(gtk::Align::Start);
    let summary = gtk::Label::new(Some(&format!(
        "{} · {} · 三段 · {} Hz / {}-bit / {} ch",
        format_bytes(facts.size),
        format_duration(facts.duration_ns),
        facts.sample_rate,
        facts.bits_per_sample,
        facts.channels
    )));
    summary.set_css_classes(&["muted"]);
    summary.set_halign(gtk::Align::Start);
    let hash = gtk::Label::new(Some(&format!("SHA-256  {}", facts.hash)));
    hash.set_css_classes(&["mono", "good"]);
    hash.set_selectable(true);
    hash.set_halign(gtk::Align::Start);
    left.append(&heading);
    left.append(&file);
    left.append(&path);
    left.append(&summary);
    left.append(&hash);

    let right = gtk::Box::new(gtk::Orientation::Vertical, 4);
    right.set_width_request(390);
    let run_heading = gtk::Label::new(Some("运行 / 证据目录"));
    run_heading.set_css_classes(&["section"]);
    run_heading.set_halign(gtk::Align::Start);
    let run_path = gtk::Label::new(Some(&run.to_string_lossy()));
    run_path.set_css_classes(&["mono", "muted"]);
    run_path.set_selectable(true);
    run_path.set_wrap(true);
    run_path.set_halign(gtk::Align::Start);
    let contract = gtk::Label::new(Some(
        "A · 5.000s / 480044 B\nB · 10.000s / 960044 B\nC · 15.000s / 1440044 B",
    ));
    contract.set_css_classes(&["mono"]);
    contract.set_halign(gtk::Align::Start);
    right.append(&run_heading);
    right.append(&run_path);
    right.append(&contract);
    panel.append(&left);
    panel.append(&right);
    panel
}

fn build_anchor_row(facts: &SourceFacts) -> (gtk::Box, Vec<AnchorView>) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row.set_homogeneous(true);
    let mut views = Vec::new();
    for (id, time_ns, frame, offset) in &facts.anchors {
        let card = gtk::Box::new(gtk::Orientation::Vertical, 5);
        card.set_css_classes(&["anchor"]);
        card.set_hexpand(true);
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let name = gtk::Label::new(Some(&format!("锚点 {id}")));
        name.set_css_classes(&["big"]);
        name.set_halign(gtk::Align::Start);
        let badge = gtk::Label::new(Some("待处理"));
        badge.set_css_classes(&["badge"]);
        badge.set_halign(gtk::Align::End);
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        top.append(&name);
        top.append(&spacer);
        top.append(&badge);
        let time = gtk::Label::new(Some(&format!(
            "t={:.9}s · frame {}",
            *time_ns as f64 / 1e9,
            frame
        )));
        time.set_css_classes(&["mono", "accent"]);
        time.set_halign(gtk::Align::Start);
        let offset_label = gtk::Label::new(Some(&format!(
            "原始文件偏移 {} · data {}",
            offset,
            offset.saturating_sub(facts.data_offset)
        )));
        offset_label.set_css_classes(&["mono", "muted"]);
        offset_label.set_halign(gtk::Align::Start);
        let levels = gtk::Label::new(Some("前 — dBFS  →  后 — dBFS"));
        levels.set_css_classes(&["mono"]);
        levels.set_halign(gtk::Align::Start);
        let gain = gtk::Label::new(Some("增益 — · 目标误差 —"));
        gain.set_css_classes(&["mono", "muted"]);
        gain.set_halign(gtk::Align::Start);
        let evidence = gtk::Label::new(Some("断点与输出哈希待生成"));
        evidence.set_css_classes(&["mono", "muted"]);
        evidence.set_halign(gtk::Align::Start);
        evidence.set_ellipsize(gtk::pango::EllipsizeMode::End);
        card.append(&top);
        card.append(&time);
        card.append(&offset_label);
        card.append(&levels);
        card.append(&gain);
        card.append(&evidence);
        row.append(&card);
        views.push(AnchorView {
            card,
            badge,
            time,
            offset: offset_label,
            levels,
            gain,
            evidence,
        });
    }
    (row, views)
}

fn build_controls_and_log(run: &Path, anchors: Vec<AnchorView>) -> Widgets {
    let panel = gtk::Box::new(gtk::Orientation::Vertical, 10);
    panel.set_css_classes(&["panel"]);
    panel.set_vexpand(true);
    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let run_button = gtk::Button::with_label("开始 / 恢复处理");
    run_button.set_css_classes(&["suggested-action"]);
    let pause_button = gtk::Button::with_label("请求暂停");
    pause_button.set_css_classes(&["destructive-action"]);
    pause_button.set_sensitive(false);
    let verify_button = gtk::Button::with_label("重新验收");
    let force_degrade = gtk::CheckButton::with_label("强制降级路径");
    let status = gtk::Label::new(Some("就绪"));
    status.set_css_classes(&["section"]);
    status.set_halign(gtk::Align::Start);
    status.set_hexpand(true);
    controls.append(&run_button);
    controls.append(&pause_button);
    controls.append(&verify_button);
    controls.append(&force_degrade);
    controls.append(&status);
    panel.append(&controls);
    let progress = gtk::ProgressBar::new();
    progress.set_fraction(0.0);
    progress.set_show_text(true);
    progress.set_text(Some("0 / 3 锚点"));
    panel.append(&progress);

    let metadata = gtk::Box::new(gtk::Orientation::Horizontal, 18);
    let mode = gtk::Label::new(Some("资源模式  待探测"));
    mode.set_css_classes(&["mono", "accent"]);
    let resource = gtk::Label::new(Some("内存 — · 磁盘 — · CPU —"));
    resource.set_css_classes(&["mono", "muted"]);
    let run_path = gtk::Label::new(Some(&run.to_string_lossy()));
    run_path.set_css_classes(&["mono", "muted"]);
    run_path.set_selectable(true);
    run_path.set_hexpand(true);
    run_path.set_halign(gtk::Align::End);
    metadata.append(&mode);
    metadata.append(&resource);
    metadata.append(&run_path);
    panel.append(&metadata);

    let scroller = gtk::ScrolledWindow::new();
    scroller.set_vexpand(true);
    scroller.set_min_content_height(190);
    let log_view = gtk::TextView::new();
    log_view.set_editable(false);
    log_view.set_cursor_visible(false);
    log_view.set_wrap_mode(gtk::WrapMode::WordChar);
    let log = log_view.buffer();
    scroller.set_child(Some(&log_view));
    panel.append(&scroller);

    Widgets {
        panel,
        status,
        progress,
        mode,
        resource,
        run_path,
        log,
        run_button,
        pause_button,
        verify_button,
        force_degrade,
        anchors,
    }
}

fn refresh_state(widgets: &Widgets, state: &RunState) {
    let completed = state.completed.len().min(widgets.anchors.len());
    widgets
        .progress
        .set_fraction(completed as f64 / widgets.anchors.len().max(1) as f64);
    widgets.progress.set_text(Some(&format!(
        "{completed} / {} 锚点",
        widgets.anchors.len()
    )));
    for (index, view) in widgets.anchors.iter().enumerate() {
        if let Some(checkpoint) = state.completed.get(index) {
            let paused = checkpoint.pause_after && state.status == "paused";
            view.card.set_css_classes(&[
                "anchor",
                if paused {
                    "anchor-paused"
                } else {
                    "anchor-done"
                },
            ]);
            view.badge
                .set_css_classes(&["badge", if paused { "badge-paused" } else { "badge-done" }]);
            view.badge
                .set_text(if paused { "已暂停" } else { "已完成" });
            view.time.set_text(&format!(
                "t={:.9}s · frame {}",
                checkpoint.anchor_ns as f64 / 1e9,
                checkpoint.anchor_frame
            ));
            view.offset.set_text(&format!(
                "原始文件偏移 {} · data {}",
                checkpoint.original_anchor_file_byte, checkpoint.payload_end_byte
            ));
            view.levels.set_text(&format!(
                "前 {:+.3} dBFS  →  后 {:+.3} dBFS",
                checkpoint.before.rms_dbfs, checkpoint.after.rms_dbfs
            ));
            view.gain.set_text(&format!(
                "增益 {:+.3} dB · 目标误差 {:+.3} dB",
                checkpoint.gain.applied_gain_db, checkpoint.target_error_db
            ));
            view.gain.set_css_classes(&[
                "mono",
                if checkpoint.target_error_db.abs() <= 0.75 {
                    "good"
                } else {
                    "warn"
                },
            ]);
            view.evidence.set_text(&format!(
                "断点 {} · 输出 {}",
                short_hash(&checkpoint.checkpoint_sha256),
                short_hash(&checkpoint.output_sha256)
            ));
        } else {
            view.card.set_css_classes(&["anchor"]);
            view.badge.set_css_classes(&["badge"]);
            if state.status == "running" && index == state.next_segment {
                view.badge.set_text("处理中");
            } else {
                view.badge.set_text("待处理");
            }
            view.levels.set_text("前 — dBFS  →  后 — dBFS");
            view.gain.set_text("增益 — · 目标误差 —");
            view.gain.set_css_classes(&["mono", "muted"]);
            view.evidence.set_text("断点与输出哈希待生成");
        }
    }
    widgets
        .mode
        .set_text(&format!("资源模式  {}", state.resource.mode));
    widgets.resource.set_text(&format!(
        "内存可用 {} MB / 需求 {} MB · 磁盘 {} MB / 需求 {} MB · CPU {}",
        state.resource.memory_available_mb,
        state.resource.memory_required_mb,
        state.resource.disk_available_mb,
        state.resource.disk_required_mb,
        state.resource.cpu_count
    ));
    if let Some(degradation) = &state.degradation {
        widgets.mode.set_text(&format!(
            "降级路径  {} Hz · {}",
            degradation.output_sample_rate, degradation.strategy
        ));
        widgets.mode.set_css_classes(&["mono", "warn"]);
    } else {
        widgets.mode.set_css_classes(&["mono", "good"]);
    }
    widgets.run_path.set_text(&state.run_id);
    if state.status == "completed" {
        widgets
            .status
            .set_text("三段校准完成 · 可重新验收并查看 acceptance.txt");
        if let Some(final_output) = &state.final_output {
            widgets.status.set_tooltip_text(Some(&format!(
                "{} · {}",
                final_output.path, final_output.sha256
            )));
        }
    } else if state.status == "paused" {
        widgets.status.set_text(&format!(
            "已暂停于锚点 {} · 下一段 {}",
            state
                .completed
                .last()
                .map(|item| item.anchor_id.as_str())
                .unwrap_or("?"),
            state.next_segment + 1
        ));
    } else if state.status == "running" {
        widgets
            .status
            .set_text("处理中 · 暂停将在当前锚点完成后生效");
    } else if state.status == "failed" {
        widgets.status.set_text("运行失败 · 断点与证据已保留");
    }
}

fn short_hash(value: &str) -> String {
    if value.len() <= 14 {
        value.to_string()
    } else {
        format!("{}…{}", &value[..8], &value[value.len() - 4..])
    }
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.2} MiB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{} B", bytes)
    }
}

fn format_duration(duration_ns: u64) -> String {
    format!("{:.3}s", duration_ns as f64 / 1e9)
}

fn append_log(widgets: &Widgets, message: &str) {
    let mut end = widgets.log.end_iter();
    let stamp = chrono::Local::now().format("%H:%M:%S");
    widgets
        .log
        .insert(&mut end, &format!("[{stamp}] {message}\n"));
}

fn set_busy(widgets: &Widgets, busy: bool) {
    widgets.run_button.set_sensitive(!busy);
    widgets.verify_button.set_sensitive(!busy);
    widgets.pause_button.set_sensitive(busy);
    widgets.force_degrade.set_sensitive(!busy);
}

fn wire_actions(
    widgets: &Widgets,
    source: PathBuf,
    run: PathBuf,
    pause_flag: Arc<AtomicBool>,
    busy: Arc<AtomicBool>,
) {
    {
        let widgets = widgets.clone();
        let source = source.clone();
        let run = run.clone();
        let pause_flag = pause_flag.clone();
        let busy = busy.clone();
        widgets.run_button.clone().connect_clicked(move |_| {
            if busy.swap(true, Ordering::SeqCst) {
                return;
            }
            set_busy(&widgets, true);
            pause_flag.store(false, Ordering::SeqCst);
            let paths = RunPaths::new(&run);
            let existing = engine::load_state(&paths).ok();
            if existing
                .as_ref()
                .map(|state| state.status == "completed")
                .unwrap_or(false)
            {
                let state = existing.expect("completed state");
                refresh_state(&widgets, &state);
                append_log(&widgets, "该运行已完成；开始按钮转为重新验收提示");
                widgets.status.set_text("已完成 · 请点击“重新验收”");
                busy.store(false, Ordering::SeqCst);
                set_busy(&widgets, false);
                return;
            }
            let force_degrade = widgets.force_degrade.is_active();
            append_log(
                &widgets,
                if existing.is_some() {
                    "从锚点断点恢复处理"
                } else {
                    "新建运行：三段锚点顺序处理"
                },
            );
            let (tx, rx) = mpsc::channel::<WorkerMessage>();
            let worker_widgets = widgets.clone();
            let worker_busy = busy.clone();
            let worker_source = source.clone();
            let worker_run = run.clone();
            let worker_pause = pause_flag.clone();
            std::thread::spawn(move || {
                let progress_tx = tx.clone();
                let progress = Arc::new(move |update: engine::ProgressUpdate| {
                    let _ = progress_tx.send(WorkerMessage::Progress(update.message));
                });
                let options = ProcessOptions {
                    pause_after: None,
                    pause_flag: Some(worker_pause.clone()),
                    progress: Some(progress),
                };
                let result = if worker_run.join("state.json").exists() {
                    engine::resume(&RunPaths::new(&worker_run), options)
                } else {
                    let config = CalibrationConfig {
                        force_degrade,
                        ..CalibrationConfig::default()
                    };
                    engine::init_run(&worker_source, &worker_run, config)
                        .and_then(|(paths, state)| engine::process(&paths, state, options))
                };
                let _ = match result {
                    Ok(outcome) => tx.send(WorkerMessage::Done(Box::new(outcome.state))),
                    Err(error) => tx.send(WorkerMessage::Failed(format!("{error:#}"))),
                };
            });

            let poll_widgets = worker_widgets;
            let poll_paths = RunPaths::new(&run);
            let poll_busy = worker_busy;
            glib::timeout_add_local(std::time::Duration::from_millis(120), move || {
                match rx.try_recv() {
                    Ok(WorkerMessage::Progress(message)) => {
                        append_log(&poll_widgets, &message);
                        if let Ok(state) = engine::load_state(&poll_paths) {
                            refresh_state(&poll_widgets, &state);
                        }
                        glib::ControlFlow::Continue
                    }
                    Ok(WorkerMessage::Done(state)) => {
                        refresh_state(&poll_widgets, &state);
                        append_log(
                            &poll_widgets,
                            &format!(
                                "处理结束：status={} next_segment={} resumes={}",
                                state.status, state.next_segment, state.resume_count
                            ),
                        );
                        poll_busy.store(false, Ordering::SeqCst);
                        set_busy(&poll_widgets, false);
                        glib::ControlFlow::Break
                    }
                    Ok(WorkerMessage::Failed(error)) => {
                        append_log(&poll_widgets, &format!("失败：{error}"));
                        poll_widgets.status.set_text("处理失败 · 断点与证据已保留");
                        if let Ok(state) = engine::load_state(&poll_paths) {
                            refresh_state(&poll_widgets, &state);
                        }
                        poll_busy.store(false, Ordering::SeqCst);
                        set_busy(&poll_widgets, false);
                        glib::ControlFlow::Break
                    }
                    Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        poll_busy.store(false, Ordering::SeqCst);
                        set_busy(&poll_widgets, false);
                        glib::ControlFlow::Break
                    }
                }
            });
        });
    }

    {
        let widgets = widgets.clone();
        let pause_flag = pause_flag.clone();
        widgets.pause_button.clone().connect_clicked(move |_| {
            pause_flag.store(true, Ordering::SeqCst);
            widgets.status.set_text("已请求暂停 · 当前锚点完成后落盘");
            append_log(
                &widgets,
                "收到暂停请求；不会中断正在写出的段文件，将在锚点边界保存断点",
            );
        });
    }

    {
        let widgets = widgets.clone();
        let run = run.clone();
        widgets.verify_button.clone().connect_clicked(move |_| {
            let paths = RunPaths::new(&run);
            match evidence::verify_run(&paths) {
                Ok(report) => {
                    let failures = report.checks.iter().filter(|check| !check.ok).count();
                    if report.ok {
                        widgets
                            .status
                            .set_text("验收通过 · 原件、断点、段输出与最终输出一致");
                        append_log(&widgets, "验收通过，acceptance.txt 已刷新");
                    } else {
                        widgets.status.set_text("验收失败 · 详见 verification.json");
                        append_log(&widgets, &format!("验收失败：{failures} 项未通过"));
                    }
                    for line in &report.acceptance_lines {
                        append_log(&widgets, line);
                    }
                }
                Err(error) => {
                    widgets.status.set_text("无法验收 · 状态或证据不完整");
                    append_log(&widgets, &format!("验收错误：{error:#}"));
                }
            }
        });
    }
}
