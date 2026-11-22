use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use crate::evidence::{self, AssessOptions, AssessmentOutcome};
use crate::gst_audio;
use crate::report::{AutomatedReport, ManualVerdict, Verdict};
use crate::waveform::WaveformSummary;
use gst::prelude::*;
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use gtk4 as gtk;

#[derive(Debug, Clone, Default)]
struct Selection {
    manifest: Option<PathBuf>,
    audio: Option<PathBuf>,
}

#[derive(Default)]
struct AppState {
    selection: Selection,
    outcome: Option<AssessmentOutcome>,
    selected_event: Option<usize>,
    busy: bool,
}

#[derive(Default)]
struct PlaybackState {
    playbin: Option<gst::Element>,
    uri: Option<String>,
    playing: bool,
}

#[derive(Debug, Clone, Default)]
struct EventMarker {
    start_ms: f64,
    end_ms: f64,
}

#[derive(Default)]
struct WaveState {
    waveform: Option<WaveformSummary>,
    events: Vec<EventMarker>,
    selected: Option<usize>,
    position_ms: f64,
}

#[derive(Clone)]
struct ManualRow {
    id: String,
    combo: gtk::DropDown,
    note: gtk::Entry,
}

#[derive(Clone)]
struct Ui {
    window: gtk::ApplicationWindow,
    case_label: gtk::Label,
    source_label: gtk::Label,
    status_label: gtk::Label,
    event_list: gtk::ListBox,
    wave_area: gtk::DrawingArea,
    time_label: gtk::Label,
    play_button: gtk::Button,
    auto_view: gtk::TextView,
    manual_list: gtk::ListBox,
    manual_controls: Rc<RefCell<Vec<ManualRow>>>,
    reviewer_entry: gtk::Entry,
    manual_notes: gtk::TextView,
    manual_status: gtk::Label,
    evidence_view: gtk::TextView,
    evidence_path: gtk::Label,
    open_manifest: gtk::Button,
    open_audio: gtk::Button,
    run_button: gtk::Button,
    verify_button: gtk::Button,
    save_manual: gtk::Button,
    copy_path: gtk::Button,
    open_folder: gtk::Button,
    spinner: gtk::Spinner,
}

type UiBundle = (
    Ui,
    Rc<RefCell<AppState>>,
    Rc<RefCell<WaveState>>,
    Rc<RefCell<PlaybackState>>,
);

pub fn run(manifest: Option<PathBuf>, audio: Option<PathBuf>) -> glib::ExitCode {
    let app = gtk::Application::builder()
        .application_id("io.audit.alc-review")
        .build();
    app.connect_activate(move |app| {
        install_css();
        let (ui, state, wave, playback) = build_ui(app, manifest.clone(), audio.clone());
        let needs_run =
            state.borrow().selection.manifest.is_some() || state.borrow().selection.audio.is_some();
        ui.window.present();
        if needs_run {
            start_assess(&ui, &state, &wave, &playback);
        }
    });
    app.run_with_args(&["alc-review"])
}

fn install_css() {
    if let Some(settings) = gtk::Settings::default() {
        settings.set_property("gtk-application-prefer-dark-theme", true);
    }
    let css = "
        window { background: #0f151b; color: #e6edf3; }
        headerbar { background: #151f28; border-bottom: 1px solid #2b3b47; }
        .panel { background: #17212b; border: 1px solid #2b3b47; border-radius: 8px; }
        .brand { font-size: 17px; font-weight: 700; }
        .title { font-size: 14px; font-weight: 700; }
        .muted { color: #8fa3b5; }
        .verdict { font-size: 13px; font-weight: 700; }
        .pass { color: #2ed6a1; }
        .attention { color: #ffb84d; }
        .fail { color: #ff6b8a; }
        .pending { color: #8fa3b5; }
        textview, textview text { background: #0d1318; color: #d7e2ea; }
        entry { background: #0d1318; color: #e6edf3; }
        listbox { background: transparent; }
        listbox row { padding: 4px; }
        notebook > stack { background: #0d1318; }
        dropdown > button { background: #1b2732; color: #e6edf3; }
        button { padding: 6px 10px; }
    ";
    let provider = gtk::CssProvider::new();
    provider.load_from_string(css);
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

fn build_ui(app: &gtk::Application, manifest: Option<PathBuf>, audio: Option<PathBuf>) -> UiBundle {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("音频丢包掩蔽评估")
        .default_width(1360)
        .default_height(900)
        .build();
    let header = gtk::HeaderBar::new();
    let brand = gtk::Label::new(Some("音频丢包掩蔽评估 · 可复核重放"));
    brand.add_css_class("brand");
    header.set_title_widget(Some(&brand));
    window.set_titlebar(Some(&header));

    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    root.set_margin_top(10);
    root.set_margin_bottom(10);
    root.set_margin_start(10);
    root.set_margin_end(10);

    let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let open_manifest = gtk::Button::with_label("打开清单…");
    let open_audio = gtk::Button::with_label("打开重放…");
    let run_button = gtk::Button::with_label("运行评估");
    let verify_button = gtk::Button::with_label("校验证据");
    let spinner = gtk::Spinner::new();
    spinner.set_visible(false);
    toolbar.append(&open_manifest);
    toolbar.append(&open_audio);
    toolbar.append(&run_button);
    toolbar.append(&verify_button);
    toolbar.append(&spinner);

    let case_label = gtk::Label::new(Some("尚未加载案件"));
    case_label.set_hexpand(true);
    case_label.set_xalign(0.0);
    case_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    case_label.add_css_class("title");
    toolbar.append(&case_label);
    let status_label = gtk::Label::new(Some("选择清单或重放文件后开始"));
    status_label.add_css_class("muted");
    toolbar.append(&status_label);
    root.append(&toolbar);

    let source_label = gtk::Label::new(Some("输入：未选择"));
    source_label.set_xalign(0.0);
    source_label.set_wrap(true);
    source_label.add_css_class("muted");
    root.append(&source_label);

    let top_paned = gtk::Paned::new(gtk::Orientation::Horizontal);
    top_paned.set_vexpand(true);
    let event_list = gtk::ListBox::new();
    event_list.set_selection_mode(gtk::SelectionMode::Single);
    event_list.add_css_class("panel");
    let event_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&event_list)
        .build();
    let event_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    event_box.set_width_request(430);
    let event_title = gtk::Label::new(Some("事件 · 时间点 / 轨道 / 帧"));
    event_title.set_xalign(0.0);
    event_title.add_css_class("title");
    event_box.append(&event_title);
    event_box.append(&event_scroll);
    event_box.add_css_class("panel");
    top_paned.set_start_child(Some(&event_box));

    let right = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let wave_title = gtk::Label::new(Some("恢复后输出波形 · 点击定位"));
    wave_title.set_xalign(0.0);
    wave_title.add_css_class("title");
    right.append(&wave_title);
    let wave_area = gtk::DrawingArea::new();
    wave_area.set_content_height(300);
    wave_area.set_hexpand(true);
    wave_area.set_vexpand(true);
    wave_area.add_css_class("panel");
    right.append(&wave_area);

    let transport = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let play_button = gtk::Button::with_label("播放");
    play_button.set_sensitive(false);
    let time_label = gtk::Label::new(Some("0:00.000 / 0:00.000"));
    time_label.add_css_class("muted");
    transport.append(&play_button);
    transport.append(&time_label);
    right.append(&transport);
    top_paned.set_end_child(Some(&right));
    top_paned.set_position(430);
    root.append(&top_paned);

    let notebook = gtk::Notebook::new();
    notebook.set_vexpand(true);
    let auto_view = text_view();
    notebook.append_page(
        &scrolled(&auto_view),
        Some(&gtk::Label::new(Some("自动结论"))),
    );

    let manual_page = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let manual_header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let reviewer_entry = gtk::Entry::new();
    reviewer_entry.set_placeholder_text(Some("复核人"));
    reviewer_entry.set_width_chars(18);
    let manual_status = gtk::Label::new(Some("人工确认：待确认"));
    manual_status.add_css_class("verdict");
    manual_status.add_css_class("pending");
    manual_header.append(&reviewer_entry);
    manual_header.append(&manual_status);
    manual_page.append(&manual_header);
    let manual_list = gtk::ListBox::new();
    manual_list.set_selection_mode(gtk::SelectionMode::None);
    let manual_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&manual_list)
        .build();
    manual_page.append(&manual_scroll);
    let manual_notes = text_view();
    let notes_scroll = scrolled(&manual_notes);
    notes_scroll.set_vexpand(false);
    notes_scroll.set_height_request(74);
    manual_page.append(&notes_scroll);
    let save_manual = gtk::Button::with_label("保存人工确认（不修改自动结论）");
    save_manual.set_halign(gtk::Align::End);
    manual_page.append(&save_manual);
    notebook.append_page(&manual_page, Some(&gtk::Label::new(Some("人工确认"))));

    let evidence_page = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let evidence_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let evidence_path = gtk::Label::new(Some("证据包：未生成"));
    evidence_path.set_xalign(0.0);
    evidence_path.set_hexpand(true);
    evidence_path.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    let copy_path = gtk::Button::with_label("复制路径");
    let open_folder = gtk::Button::with_label("打开目录");
    evidence_row.append(&evidence_path);
    evidence_row.append(&copy_path);
    evidence_row.append(&open_folder);
    evidence_page.append(&evidence_row);
    let evidence_view = text_view();
    evidence_page.append(&scrolled(&evidence_view));
    notebook.append_page(&evidence_page, Some(&gtk::Label::new(Some("证据"))));
    root.append(&notebook);
    window.set_child(Some(&root));

    let ui = Ui {
        window: window.clone(),
        case_label,
        source_label,
        status_label,
        event_list: event_list.clone(),
        wave_area: wave_area.clone(),
        time_label,
        play_button: play_button.clone(),
        auto_view,
        manual_list,
        manual_controls: Rc::new(RefCell::new(Vec::new())),
        reviewer_entry,
        manual_notes,
        manual_status,
        evidence_view,
        evidence_path,
        open_manifest: open_manifest.clone(),
        open_audio: open_audio.clone(),
        run_button,
        verify_button,
        save_manual: save_manual.clone(),
        copy_path,
        open_folder,
        spinner,
    };
    let state = Rc::new(RefCell::new(AppState {
        selection: Selection { manifest, audio },
        ..AppState::default()
    }));
    let wave = Rc::new(RefCell::new(WaveState::default()));
    let playback = Rc::new(RefCell::new(PlaybackState::default()));

    let wave_for_draw = wave.clone();
    wave_area.set_draw_func(move |_, context, width, height| {
        draw_wave(context, width, height, &wave_for_draw.borrow());
    });
    connect_actions(&ui, &state, &wave, &playback);
    update_selection_label(&ui, &state);
    (ui, state, wave, playback)
}

fn text_view() -> gtk::TextView {
    let view = gtk::TextView::new();
    view.set_editable(false);
    view.set_cursor_visible(false);
    view.set_monospace(true);
    view.set_wrap_mode(gtk::WrapMode::WordChar);
    view.set_top_margin(8);
    view.set_bottom_margin(8);
    view.set_left_margin(10);
    view.set_right_margin(10);
    view
}

fn scrolled(widget: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(widget)
        .build()
}

fn update_selection_label(ui: &Ui, state: &Rc<RefCell<AppState>>) {
    let state = state.borrow();
    let mut parts = Vec::new();
    if let Some(path) = &state.selection.manifest {
        parts.push(format!("清单 {}", path.display()));
    }
    if let Some(path) = &state.selection.audio {
        parts.push(format!("重放 {}", path.display()));
    }
    ui.source_label.set_text(&if parts.is_empty() {
        "输入：未选择。可打开旧版/新版 JSON 清单，或直接打开未知扩展名的重放文件。".to_string()
    } else {
        format!("输入：{}", parts.join("  |  "))
    });
}

fn start_assess(
    ui: &Ui,
    state: &Rc<RefCell<AppState>>,
    wave: &Rc<RefCell<WaveState>>,
    playback: &Rc<RefCell<PlaybackState>>,
) {
    let selection = {
        let mut state = state.borrow_mut();
        if state.busy {
            return;
        }
        if state.selection.manifest.is_none() && state.selection.audio.is_none() {
            ui.status_label.set_text("请先打开清单或重放文件");
            return;
        }
        state.busy = true;
        state.selection.clone()
    };
    ui.spinner.set_visible(true);
    ui.spinner.start();
    ui.run_button.set_sensitive(false);
    ui.status_label.set_text("正在解码、生成波形并固化证据…");
    let options = AssessOptions {
        manifest: selection.manifest,
        audio: selection.audio,
        out: None,
        copy_audio: true,
        force: false,
        max_seconds: gst_audio::max_seconds(),
    };
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(evidence::assess(&options));
    });
    let ui = ui.clone();
    let state = state.clone();
    let wave = wave.clone();
    let playback = playback.clone();
    glib::timeout_add_local(Duration::from_millis(100), move || {
        match receiver.try_recv() {
            Ok(Ok(outcome)) => {
                finish_busy(&ui, &state);
                render_outcome(&ui, &state, &wave, &playback, outcome);
                glib::ControlFlow::Break
            }
            Ok(Err(failure)) => {
                finish_busy(&ui, &state);
                ui.status_label
                    .set_text(&format!("评估启动失败：{}", failure.message));
                ui.auto_view.buffer().set_text(&format!(
                    "评估失败\n\n{failure}\n\n后续操作：{}",
                    failure.action
                ));
                glib::ControlFlow::Break
            }
            Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => {
                finish_busy(&ui, &state);
                ui.status_label.set_text("评估线程意外结束");
                glib::ControlFlow::Break
            }
        }
    });
}

fn finish_busy(ui: &Ui, state: &Rc<RefCell<AppState>>) {
    state.borrow_mut().busy = false;
    ui.spinner.stop();
    ui.spinner.set_visible(false);
    ui.run_button.set_sensitive(true);
}

fn render_outcome(
    ui: &Ui,
    state: &Rc<RefCell<AppState>>,
    wave: &Rc<RefCell<WaveState>>,
    playback: &Rc<RefCell<PlaybackState>>,
    outcome: AssessmentOutcome,
) {
    let case_id = outcome.case_id.clone();
    let evidence_dir = outcome.evidence_dir.clone();
    let verdict = outcome
        .automated
        .as_ref()
        .map(|report| report.verdict)
        .unwrap_or(Verdict::Inconclusive);
    ui.case_label
        .set_text(&format!("案件 {} · 自动 {}", case_id, verdict.label()));
    let status = if outcome.success {
        format!("评估完成 · 自动结论 {}", verdict.label())
    } else {
        "评估失败 · 证据已保留".to_string()
    };
    ui.status_label.set_text(&status);
    ui.evidence_path
        .set_text(&format!("证据包：{}", evidence_dir.display()));
    ui.auto_view
        .buffer()
        .set_text(&format_automated(outcome.automated.as_ref()));
    ui.evidence_view
        .buffer()
        .set_text(&format_evidence(&outcome));
    render_events(ui, state, wave, &outcome);
    render_manual(ui, state, &outcome);

    if let Some(manifest) = outcome.manifest.as_ref()
        && let Err(error) = configure_playback(playback, &manifest.audio_path)
    {
        ui.status_label
            .set_text(&format!("回放初始化失败：{error}"));
        ui.play_button.set_sensitive(false);
    } else {
        ui.play_button.set_sensitive(true);
        ui.play_button.set_label("播放");
    }
    {
        let mut playback = playback.borrow_mut();
        playback.playing = false;
    }
    let duration = outcome
        .analysis
        .as_ref()
        .map(|analysis| analysis.waveform.duration_ms)
        .unwrap_or(0.0);
    ui.time_label
        .set_text(&format!("0:00.000 / {}", format_time(duration)));
    state.borrow_mut().outcome = Some(outcome);
    ui.wave_area.queue_draw();
}

fn format_automated(report: Option<&AutomatedReport>) -> String {
    let Some(report) = report else {
        return "尚无自动结论。".to_string();
    };
    let mut output = format!(
        "自动结论：{}\n{}\n\n",
        report.verdict.label(),
        report.summary
    );
    output.push_str("检查项\n");
    for check in &report.checks {
        output.push_str(&format!(
            "[{}] {}\n  {}\n",
            check.result.label(),
            check.title,
            check.detail
        ));
        if let Some(action) = &check.action {
            output.push_str(&format!("  后续：{action}\n"));
        }
    }
    if !report.events.is_empty() {
        output.push_str("\n事件与输出波形\n");
        for event in &report.events {
            output.push_str(&format!(
                "[{}] {}  {:.3}s–{:.3}s  轨道 {} / 帧 {}\n  参数 {}\n",
                event.result.label(),
                event.event_id,
                event.start_ms / 1000.0,
                event.end_ms / 1000.0,
                event.track_id.as_deref().unwrap_or("未知"),
                event
                    .frame
                    .map(|frame| frame.to_string())
                    .unwrap_or_else(|| "未知".to_string()),
                event.parameters
            ));
            if let Some(metrics) = &event.metrics {
                output.push_str(&format!(
                    "  波形 peak {:.1} dBFS / RMS {:.1} dBFS / 静音 {:.1}% / 削波 {:.3}%\n",
                    metrics.peak_dbfs,
                    metrics.rms_dbfs,
                    metrics.silent_fraction * 100.0,
                    metrics.clipped_fraction * 100.0
                ));
            }
            for note in &event.notes {
                output.push_str(&format!("  注意：{note}\n"));
            }
        }
    }
    output.push_str("\n局限\n");
    for limitation in &report.limitations {
        output.push_str(&format!("- {limitation}\n"));
    }
    output
}

fn format_evidence(outcome: &AssessmentOutcome) -> String {
    let Some(report) = &outcome.automated else {
        return format!("证据目录：{}\n", outcome.evidence_dir.display());
    };
    let mut output = format!(
        "证据目录：{}\n未知扩展：{}\n\n",
        report.evidence.root.display(),
        report.unknown_extension.as_deref().unwrap_or("无")
    );
    for file in &report.evidence.files {
        output.push_str(&format!(
            "{}  {}\n  {}\n  SHA-256 {}\n",
            file.path,
            file.name,
            file.description,
            file.sha256.as_deref().unwrap_or("—")
        ));
    }
    output.push_str("\n证据校验命令：\n");
    output.push_str(&format!(
        "alc-review verify --evidence {}",
        report.evidence.root.display()
    ));
    output
}

fn render_events(
    ui: &Ui,
    state: &Rc<RefCell<AppState>>,
    wave: &Rc<RefCell<WaveState>>,
    outcome: &AssessmentOutcome,
) {
    while let Some(child) = ui.event_list.first_child() {
        ui.event_list.remove(&child);
    }
    let mut markers = Vec::new();
    if let Some(automated) = &outcome.automated {
        for (index, event) in automated.events.iter().enumerate() {
            markers.push(EventMarker {
                start_ms: event.start_ms,
                end_ms: event.end_ms,
            });
            let row = gtk::ListBoxRow::new();
            let content = gtk::Box::new(gtk::Orientation::Vertical, 2);
            content.set_margin_top(4);
            content.set_margin_bottom(4);
            content.set_margin_start(6);
            content.set_margin_end(6);
            let title = gtk::Label::new(Some(&format!(
                "#{:02}  {}  {}  ·  {}",
                index + 1,
                format_time(event.start_ms),
                event.event_id,
                event.result.label()
            )));
            title.set_xalign(0.0);
            title.set_hexpand(true);
            title.set_ellipsize(gtk::pango::EllipsizeMode::End);
            title.set_max_width_chars(46);
            title.add_css_class("title");
            let detail = gtk::Label::new(Some(&format!(
                "轨道 {} / 帧 {}  ·  {}\n参数 {}",
                event.track_id.as_deref().unwrap_or("未知"),
                event
                    .frame
                    .map(|frame| frame.to_string())
                    .unwrap_or_else(|| "未知".to_string()),
                event.kind,
                event.parameters
            )));
            detail.set_xalign(0.0);
            detail.set_wrap(true);
            detail.set_wrap_mode(gtk::pango::WrapMode::WordChar);
            detail.set_max_width_chars(50);
            detail.add_css_class("muted");
            content.append(&title);
            content.append(&detail);
            content.set_hexpand(true);
            content.set_halign(gtk::Align::Fill);
            row.set_child(Some(&content));
            ui.event_list.append(&row);
        }
    }
    if markers.is_empty() {
        let label = gtk::Label::new(Some("没有可定位事件。自动流程不会据此判定通过。"));
        label.set_wrap(true);
        label.add_css_class("attention");
        ui.event_list.append(&label);
    }
    let mut wave = wave.borrow_mut();
    wave.waveform = outcome
        .analysis
        .as_ref()
        .map(|analysis| analysis.waveform.clone());
    wave.events = markers;
    wave.selected = None;
    wave.position_ms = 0.0;
    state.borrow_mut().selected_event = None;
}

fn render_manual(ui: &Ui, state: &Rc<RefCell<AppState>>, outcome: &AssessmentOutcome) {
    while let Some(child) = ui.manual_list.first_child() {
        ui.manual_list.remove(&child);
    }
    ui.manual_controls.borrow_mut().clear();
    let Some(manual) = &outcome.manual else {
        return;
    };
    ui.reviewer_entry.set_text(&manual.reviewer);
    ui.manual_notes.buffer().set_text(&manual.notes);
    set_manual_status(ui, manual.status);
    for item in &manual.items {
        let row = gtk::ListBoxRow::new();
        let content = gtk::Box::new(gtk::Orientation::Vertical, 4);
        content.set_margin_top(4);
        content.set_margin_bottom(4);
        content.set_margin_start(6);
        content.set_margin_end(6);
        let label = gtk::Label::new(Some(&item.label));
        label.set_xalign(0.0);
        label.set_wrap(true);
        content.append(&label);
        let controls = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let model = gtk::StringList::new(&["待确认", "通过", "需关注", "不通过"]);
        let combo = gtk::DropDown::builder()
            .model(&model)
            .selected(verdict_index(item.verdict))
            .build();
        let note = gtk::Entry::new();
        note.set_hexpand(true);
        note.set_placeholder_text(Some("人工备注（可选）"));
        note.set_text(&item.note);
        controls.append(&combo);
        controls.append(&note);
        content.append(&controls);
        row.set_child(Some(&content));
        ui.manual_list.append(&row);
        ui.manual_controls.borrow_mut().push(ManualRow {
            id: item.id.clone(),
            combo: combo.clone(),
            note: note.clone(),
        });
        let ui_clone = ui.clone();
        let state_clone = state.clone();
        combo.connect_selected_notify(move |_| {
            if state_clone.borrow().outcome.is_some() {
                set_manual_status(&ui_clone, ManualVerdict::Pending);
                ui_clone.manual_status.set_text("人工确认：有未保存修改");
            }
        });
    }
}

fn set_manual_status(ui: &Ui, verdict: ManualVerdict) {
    ui.manual_status
        .set_text(&format!("人工确认：{}", verdict.label()));
    for class in ["pass", "attention", "fail", "pending"] {
        ui.manual_status.remove_css_class(class);
    }
    ui.manual_status.add_css_class(match verdict {
        ManualVerdict::Pending => "pending",
        ManualVerdict::Pass => "pass",
        ManualVerdict::Attention => "attention",
        ManualVerdict::Fail => "fail",
    });
}

fn verdict_index(verdict: ManualVerdict) -> u32 {
    match verdict {
        ManualVerdict::Pending => 0,
        ManualVerdict::Pass => 1,
        ManualVerdict::Attention => 2,
        ManualVerdict::Fail => 3,
    }
}

fn verdict_from_index(index: u32) -> ManualVerdict {
    match index {
        1 => ManualVerdict::Pass,
        2 => ManualVerdict::Attention,
        3 => ManualVerdict::Fail,
        _ => ManualVerdict::Pending,
    }
}

fn configure_playback(
    playback: &Rc<RefCell<PlaybackState>>,
    path: &std::path::Path,
) -> Result<(), String> {
    let uri = gst_audio::file_uri(path).map_err(|failure| failure.to_string())?;
    let mut playback = playback.borrow_mut();
    if playback.uri.as_deref() == Some(uri.as_str()) {
        return Ok(());
    }
    if let Some(old) = playback.playbin.take() {
        let _ = old.set_state(gst::State::Null);
    }
    gst::init().map_err(|error| error.to_string())?;
    let playbin = gst::ElementFactory::make("playbin")
        .build()
        .map_err(|error| format!("无法创建 GStreamer playbin：{error}"))?;
    playbin.set_property("uri", &uri);
    playback.playbin = Some(playbin);
    playback.uri = Some(uri);
    playback.playing = false;
    Ok(())
}

fn seek_to(playback: &Rc<RefCell<PlaybackState>>, position_ms: f64) {
    let playback = playback.borrow();
    if let Some(playbin) = &playback.playbin {
        let _ = playbin.seek_simple(
            gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT,
            gst::ClockTime::from_mseconds(position_ms.max(0.0) as u64),
        );
    }
}

fn format_time(milliseconds: f64) -> String {
    let total = milliseconds.max(0.0) / 1000.0;
    let minutes = (total / 60.0).floor() as u64;
    let seconds = total - minutes as f64 * 60.0;
    format!("{minutes}:{seconds:06.3}")
}

fn connect_actions(
    ui: &Ui,
    state: &Rc<RefCell<AppState>>,
    wave: &Rc<RefCell<WaveState>>,
    playback: &Rc<RefCell<PlaybackState>>,
) {
    {
        let ui = ui.clone();
        let state = state.clone();
        let wave = wave.clone();
        let playback = playback.clone();
        ui.run_button.clone().connect_clicked(move |_| {
            start_assess(&ui, &state, &wave, &playback);
        });
    }
    {
        let ui = ui.clone();
        let state = state.clone();
        let wave = wave.clone();
        let playback = playback.clone();
        ui.open_manifest.clone().connect_clicked(move |_| {
            choose_manifest(&ui, &state, &wave, &playback);
        });
    }
    {
        let ui = ui.clone();
        let state = state.clone();
        let wave = wave.clone();
        let playback = playback.clone();
        ui.open_audio.clone().connect_clicked(move |_| {
            choose_audio(&ui, &state, &wave, &playback);
        });
    }
    {
        let ui = ui.clone();
        let state = state.clone();
        ui.verify_button.clone().connect_clicked(move |_| {
            let evidence_dir = state
                .borrow()
                .outcome
                .as_ref()
                .map(|outcome| outcome.evidence_dir.clone());
            let Some(evidence_dir) = evidence_dir else {
                ui.status_label.set_text("尚无证据包可校验");
                return;
            };
            match evidence::verify_evidence(&evidence_dir) {
                Ok(report) if report.ok() => {
                    ui.status_label
                        .set_text(&format!("证据校验通过：{} 个哈希一致", report.checked));
                }
                Ok(report) => {
                    ui.status_label.set_text(&format!(
                        "证据校验失败：{} 个不一致，{} 个缺失",
                        report.mismatched, report.missing
                    ));
                }
                Err(failure) => {
                    ui.status_label
                        .set_text(&format!("证据校验失败：{}", failure.message));
                }
            }
        });
    }
    {
        let ui = ui.clone();
        let state = state.clone();
        let wave = wave.clone();
        let playback = playback.clone();
        ui.event_list.clone().connect_row_activated(move |_, row| {
            let index = row.index().max(0) as usize;
            select_event(&ui, &state, &wave, &playback, index, true);
        });
    }
    {
        let ui = ui.clone();
        let wave = wave.clone();
        let playback = playback.clone();
        let wave_area = ui.wave_area.clone();
        let gesture = gtk::GestureClick::new();
        gesture.connect_pressed(move |_, _, x, _| {
            let width = ui.wave_area.width().max(1) as f64;
            let (duration, position, nearest) = {
                let wave = wave.borrow();
                let duration = wave
                    .waveform
                    .as_ref()
                    .map(|waveform| waveform.duration_ms)
                    .unwrap_or(0.0);
                if duration <= 0.0 {
                    return;
                }
                let position = (x / width).clamp(0.0, 1.0) * duration;
                let nearest = wave
                    .events
                    .iter()
                    .enumerate()
                    .min_by(|(_, left), (_, right)| {
                        (left.start_ms - position)
                            .abs()
                            .total_cmp(&(right.start_ms - position).abs())
                    })
                    .and_then(|(index, event)| {
                        ((event.start_ms - position).abs() < 80.0).then_some(index)
                    });
                (duration, position, nearest)
            };
            let actual = nearest
                .and_then(|index| wave.borrow().events.get(index).map(|event| event.start_ms))
                .unwrap_or(position);
            seek_to(&playback, actual);
            let mut wave = wave.borrow_mut();
            wave.position_ms = actual;
            wave.selected = nearest;
            ui.time_label.set_text(&format!(
                "{} / {}",
                format_time(actual),
                format_time(duration)
            ));
            ui.wave_area.queue_draw();
        });
        wave_area.add_controller(gesture);
    }
    {
        let ui = ui.clone();
        let playback = playback.clone();
        ui.play_button
            .clone()
            .connect_clicked(move |_| toggle_playback(&ui, &playback));
    }
    {
        let ui = ui.clone();
        let state = state.clone();
        let wave = wave.clone();
        let playback = playback.clone();
        ui.save_manual.clone().connect_clicked(move |_| {
            save_manual(&ui, &state, &wave, &playback);
        });
    }
    {
        let ui = ui.clone();
        let state = state.clone();
        ui.copy_path.clone().connect_clicked(move |_| {
            if let Some(outcome) = &state.borrow().outcome {
                ui.window
                    .clipboard()
                    .set_text(&outcome.evidence_dir.display().to_string());
                ui.status_label.set_text("证据目录路径已复制");
            }
        });
    }
    {
        let ui = ui.clone();
        let state = state.clone();
        ui.open_folder.clone().connect_clicked(move |_| {
            let path = state
                .borrow()
                .outcome
                .as_ref()
                .map(|outcome| outcome.evidence_dir.clone());
            if let Some(path) = path {
                let uri = gio::File::for_path(path).uri();
                if let Err(error) =
                    gio::AppInfo::launch_default_for_uri(&uri, None::<&gio::AppLaunchContext>)
                {
                    ui.status_label
                        .set_text(&format!("无法打开证据目录：{error}"));
                }
            }
        });
    }
    start_playback_timer(ui, wave, playback);
}

fn choose_manifest(
    ui: &Ui,
    state: &Rc<RefCell<AppState>>,
    wave: &Rc<RefCell<WaveState>>,
    playback: &Rc<RefCell<PlaybackState>>,
) {
    let dialog = gtk::FileDialog::builder()
        .title("打开旧版或新版评估清单")
        .modal(true)
        .build();
    let filter = gtk::FileFilter::new();
    filter.set_name(Some("JSON 清单"));
    filter.add_pattern("*.json");
    let all = gtk::FileFilter::new();
    all.set_name(Some("所有文件"));
    all.add_pattern("*");
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);
    filters.append(&all);
    dialog.set_filters(Some(&filters));
    dialog.set_default_filter(Some(&filter));
    let ui_clone = ui.clone();
    let state_clone = state.clone();
    let wave_clone = wave.clone();
    let playback_clone = playback.clone();
    dialog.open(Some(&ui.window), None::<&gio::Cancellable>, move |result| {
        if let Ok(file) = result
            && let Some(path) = file.path()
        {
            let mut state = state_clone.borrow_mut();
            state.selection.manifest = Some(path);
            state.selection.audio = None;
            drop(state);
            update_selection_label(&ui_clone, &state_clone);
            start_assess(&ui_clone, &state_clone, &wave_clone, &playback_clone);
        }
    });
}

fn choose_audio(
    ui: &Ui,
    state: &Rc<RefCell<AppState>>,
    wave: &Rc<RefCell<WaveState>>,
    playback: &Rc<RefCell<PlaybackState>>,
) {
    let dialog = gtk::FileDialog::builder()
        .title("打开重放音频（未知扩展名也可）")
        .modal(true)
        .build();
    let all = gtk::FileFilter::new();
    all.set_name(Some("所有文件（按内容解码）"));
    all.add_pattern("*");
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&all);
    dialog.set_filters(Some(&filters));
    dialog.set_default_filter(Some(&all));
    let ui_clone = ui.clone();
    let state_clone = state.clone();
    let wave_clone = wave.clone();
    let playback_clone = playback.clone();
    dialog.open(Some(&ui.window), None::<&gio::Cancellable>, move |result| {
        if let Ok(file) = result
            && let Some(path) = file.path()
        {
            let mut state = state_clone.borrow_mut();
            state.selection.audio = Some(path);
            state.selection.manifest = None;
            drop(state);
            update_selection_label(&ui_clone, &state_clone);
            start_assess(&ui_clone, &state_clone, &wave_clone, &playback_clone);
        }
    });
}

fn select_event(
    ui: &Ui,
    state: &Rc<RefCell<AppState>>,
    wave: &Rc<RefCell<WaveState>>,
    playback: &Rc<RefCell<PlaybackState>>,
    index: usize,
    seek: bool,
) {
    let position = {
        let wave = wave.borrow();
        let Some(event) = wave.events.get(index) else {
            return;
        };
        event.start_ms
    };
    {
        let mut wave = wave.borrow_mut();
        wave.selected = Some(index);
        wave.position_ms = position;
    }
    state.borrow_mut().selected_event = Some(index);
    if seek {
        seek_to(playback, position);
    }
    let duration = wave
        .borrow()
        .waveform
        .as_ref()
        .map(|waveform| waveform.duration_ms)
        .unwrap_or(0.0);
    ui.time_label.set_text(&format!(
        "{} / {}",
        format_time(position),
        format_time(duration)
    ));
    ui.wave_area.queue_draw();
}

fn toggle_playback(ui: &Ui, playback: &Rc<RefCell<PlaybackState>>) {
    let mut playback = playback.borrow_mut();
    let Some(playbin) = playback.playbin.as_ref() else {
        ui.status_label.set_text("没有可播放的音频");
        return;
    };
    if playback.playing {
        let _ = playbin.set_state(gst::State::Paused);
        playback.playing = false;
        ui.play_button.set_label("播放");
        ui.status_label.set_text("回放已暂停");
    } else {
        let result = playbin.set_state(gst::State::Playing);
        if let Some(clock) = playbin.query_position::<gst::ClockTime>()
            && clock.mseconds() > 0
            && ui.play_button.label().as_deref() == Some("播放")
            && matches!(result, Ok(gst::StateChangeSuccess::Async))
        {
            // State changes are asynchronous; errors are surfaced by the bus timer.
        }
        match result {
            Ok(_) => {
                playback.playing = true;
                ui.play_button.set_label("暂停");
                ui.status_label.set_text("正在回放恢复后的音频");
            }
            Err(error) => {
                ui.status_label.set_text(&format!("回放启动失败：{error}"));
            }
        }
    }
}

fn start_playback_timer(
    ui: &Ui,
    wave: &Rc<RefCell<WaveState>>,
    playback: &Rc<RefCell<PlaybackState>>,
) {
    let ui = ui.clone();
    let wave = wave.clone();
    let playback = playback.clone();
    glib::timeout_add_local(Duration::from_millis(100), move || {
        let (position, eos, error) = {
            let playback = playback.borrow();
            let Some(playbin) = playback.playbin.as_ref() else {
                return glib::ControlFlow::Continue;
            };
            let mut eos = false;
            let mut error = None;
            if let Some(bus) = playbin.bus() {
                while let Some(message) = bus.pop() {
                    match message.view() {
                        gst::MessageView::Eos(_) => eos = true,
                        gst::MessageView::Error(message) => {
                            error = Some(format!("{} ({:?})", message.error(), message.debug()));
                        }
                        _ => {}
                    }
                }
            }
            let position = playbin
                .query_position::<gst::ClockTime>()
                .map(|clock| clock.mseconds() as f64)
                .unwrap_or(0.0);
            (position, eos, error)
        };
        if let Some(error) = error {
            ui.status_label.set_text(&format!("回放错误：{error}"));
            let mut playback = playback.borrow_mut();
            playback.playing = false;
            ui.play_button.set_label("播放");
        }
        if eos {
            let mut playback = playback.borrow_mut();
            playback.playing = false;
            ui.play_button.set_label("播放");
            ui.status_label
                .set_text("回放结束；可再次播放或点击事件复核");
        }
        let duration = wave
            .borrow()
            .waveform
            .as_ref()
            .map(|waveform| waveform.duration_ms)
            .unwrap_or(0.0);
        if position > 0.0 || eos {
            {
                let mut wave = wave.borrow_mut();
                wave.position_ms = position.min(duration.max(position));
            }
            ui.time_label.set_text(&format!(
                "{} / {}",
                format_time(position),
                format_time(duration)
            ));
            ui.wave_area.queue_draw();
        }
        glib::ControlFlow::Continue
    });
}

fn save_manual(
    ui: &Ui,
    state: &Rc<RefCell<AppState>>,
    wave: &Rc<RefCell<WaveState>>,
    playback: &Rc<RefCell<PlaybackState>>,
) {
    let reviewer = ui.reviewer_entry.text().to_string();
    let notes = ui
        .manual_notes
        .buffer()
        .text(
            &ui.manual_notes.buffer().start_iter(),
            &ui.manual_notes.buffer().end_iter(),
            true,
        )
        .to_string();
    let verdicts: Vec<_> = ui
        .manual_controls
        .borrow()
        .iter()
        .map(|row| {
            (
                row.id.clone(),
                verdict_from_index(row.combo.selected()),
                row.note.text().to_string(),
            )
        })
        .collect();
    let mut state_guard = state.borrow_mut();
    let Some(outcome) = state_guard.outcome.as_mut() else {
        ui.status_label.set_text("尚无评估案件，无法保存人工确认");
        return;
    };
    match evidence::mark_manual_review(outcome, reviewer, notes, &verdicts) {
        Ok(()) => {
            let status = outcome
                .manual
                .as_ref()
                .map(|manual| manual.status)
                .unwrap_or(ManualVerdict::Pending);
            set_manual_status(ui, status);
            ui.status_label.set_text(&format!(
                "人工确认已独立保存：{} · 自动结论未改动",
                status.label()
            ));
        }
        Err(failure) => {
            ui.status_label
                .set_text(&format!("人工确认保存失败：{}", failure.message));
        }
    }
    let _ = (wave, playback);
}

fn draw_wave(context: &gtk::cairo::Context, width: i32, height: i32, state: &WaveState) {
    let width = width.max(1) as f64;
    let height = height.max(1) as f64;
    context.set_source_rgb(0.071, 0.102, 0.125);
    context.rectangle(0.0, 0.0, width, height);
    let _ = context.fill();

    let left = 46.0;
    let right = width - 16.0;
    let top = 18.0;
    let bottom = height - 30.0;
    let center = (top + bottom) / 2.0;
    let amplitude = (bottom - top) / 2.0 - 4.0;
    context.set_source_rgb(0.102, 0.137, 0.169);
    context.rectangle(left, top, right - left, bottom - top);
    let _ = context.fill();

    let Some(waveform) = state.waveform.as_ref() else {
        context.set_source_rgb(0.56, 0.64, 0.71);
        context.select_font_face(
            "Sans",
            gtk::cairo::FontSlant::Normal,
            gtk::cairo::FontWeight::Normal,
        );
        context.set_font_size(13.0);
        context.move_to(left + 18.0, top + 34.0);
        let _ = context.show_text("加载并评估后显示恢复后的输出波形");
        return;
    };
    let duration = waveform.duration_ms.max(1.0);

    context.set_line_width(1.0);
    for step in 0..=5 {
        let ratio = step as f64 / 5.0;
        let x = left + (right - left) * ratio;
        context.set_source_rgb(0.145, 0.196, 0.239);
        context.move_to(x, top);
        context.line_to(x, bottom);
        let _ = context.stroke();
        context.set_source_rgb(0.56, 0.64, 0.71);
        context.set_font_size(10.0);
        let label = format!("{:.2}s", duration * ratio / 1000.0);
        let text_x = if step == 5 {
            x - context
                .text_extents(&label)
                .map(|extents| extents.width())
                .unwrap_or(28.0)
                - 2.0
        } else {
            x + 3.0
        };
        context.move_to(text_x, height - 10.0);
        let _ = context.show_text(&label);
    }
    context.set_source_rgb(0.19, 0.25, 0.30);
    context.move_to(left, center);
    context.line_to(right, center);
    let _ = context.stroke();

    for (index, event) in state.events.iter().enumerate() {
        let x = left + (right - left) * (event.start_ms / duration).clamp(0.0, 1.0);
        let end_x = left + (right - left) * (event.end_ms / duration).clamp(0.0, 1.0);
        if state.selected == Some(index) {
            context.set_source_rgba(1.0, 0.72, 0.30, 0.28);
            context.rectangle(x, top, (end_x - x).max(2.0), bottom - top);
            let _ = context.fill();
        }
        context.set_source_rgba(1.0, 0.72, 0.30, 0.62);
        context.set_line_width(1.0);
        context.move_to(x, top);
        context.line_to(x, bottom);
        let _ = context.stroke();
    }

    let plot_width = (right - left).max(1.0) as usize;
    let bins = waveform.bins.len().max(1);
    let step = (bins as f64 / plot_width as f64).max(1.0);
    let mut index = 0;
    context.set_source_rgb(0.38, 0.82, 1.0);
    context.set_line_width(1.0);
    while index < waveform.bins.len() {
        let end = ((index as f64 + step) as usize).min(waveform.bins.len());
        let slice = &waveform.bins[index..end];
        let min = slice.iter().map(|bin| bin.min).fold(0.0_f32, f32::min);
        let max = slice.iter().map(|bin| bin.max).fold(0.0_f32, f32::max);
        let x = left + plot_width as f64 * (index as f64 / bins as f64);
        let y1 = center - f64::from(max) * amplitude;
        let y2 = center - f64::from(min) * amplitude;
        context.move_to(x + 0.5, y1);
        context.line_to(x + 0.5, y2.max(y1 + 0.8));
        let _ = context.stroke();
        index = end;
    }

    if state.position_ms > 0.0 {
        let x = left + (right - left) * (state.position_ms / duration).clamp(0.0, 1.0);
        context.set_source_rgb(1.0, 0.32, 0.43);
        context.set_line_width(1.5);
        context.move_to(x, top);
        context.line_to(x, bottom);
        let _ = context.stroke();
    }
}
