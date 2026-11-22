// GTK 4.10 deprecates the old native chooser/combo API, but it remains the
// most portable way to use host-native dialogs across X11 and Wayland.
#![allow(deprecated)]

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use gtk::prelude::*;
use gtk::{cairo, gdk, glib, Application, ApplicationWindow, Orientation, Picture};

use crate::analysis::{self, AnalysisRequest, KeyFrame, ProgressUpdate};
use crate::evidence;
use crate::failure::AnalysisFailure;
use crate::gst_backend;
use crate::model::{
    extension_record, format_timecode, ActionKind, DetectorParams, ExtensionStatus, FailureRecord,
    IntervalState, ManualCheck, ManualStatus, ReviewReport, Verdict,
};
use crate::report::{now_utc, read_report};

const APP_ID: &str = "dev.blackframe.review";
const CSS: &str = include_str!("../assets/style.css");

#[derive(Clone)]
struct ManualWidgets {
    reviewer: gtk::Entry,
    note: gtk::TextView,
}

enum WorkerMessage {
    Progress(ProgressUpdate),
    Finished(Box<WorkerResult>),
}

struct WorkerResult {
    report: ReviewReport,
    keyframes: Vec<KeyFrame>,
    bundle_path: Option<PathBuf>,
    failure: Option<AnalysisFailure>,
    bundle_error: Option<String>,
}

struct Ui {
    window: ApplicationWindow,
    source_path: RefCell<Option<PathBuf>>,
    report: RefCell<Option<ReviewReport>>,
    keyframes: RefCell<Vec<KeyFrame>>,
    selected_interval: RefCell<Option<usize>>,
    selected_frame: RefCell<Option<usize>>,
    evidence_root: PathBuf,
    source_label: gtk::Label,
    source_meta: gtk::Label,
    extension_chip: gtk::Label,
    hash_label: gtk::Label,
    recovery_switch: gtk::Switch,
    recovery_note: gtk::Entry,
    threshold: gtk::SpinButton,
    black_ratio: gtk::SpinButton,
    min_duration: gtk::SpinButton,
    fps: gtk::SpinButton,
    track: gtk::SpinButton,
    params_chip: gtk::Label,
    format_chip: gtk::Label,
    banner: gtk::Label,
    run_button: gtk::Button,
    cancel_button: gtk::Button,
    progress: gtk::ProgressBar,
    progress_label: gtk::Label,
    waveform: gtk::DrawingArea,
    picture: Picture,
    preview_label: gtk::Label,
    detail_label: gtk::Label,
    timeline_box: gtk::Box,
    auto_box: gtk::Box,
    manual_box: gtk::Box,
    evidence_box: gtk::Box,
    failure_box: gtk::Box,
    failure_revealer: gtk::Revealer,
    status_label: gtk::Label,
    worker_rx: RefCell<Option<Arc<Mutex<mpsc::Receiver<WorkerMessage>>>>>,
    cancel: RefCell<Option<Arc<AtomicBool>>>,
    busy: Cell<bool>,
    manual_widgets: RefCell<Option<ManualWidgets>>,
}

pub fn run() {
    install_bundled_font();
    let app = Application::builder().application_id(APP_ID).build();
    app.connect_activate(build);
    let _ = app.run();
}

#[cfg(target_os = "linux")]
fn install_bundled_font() {
    let Some(path) = bundled_font_path() else {
        return;
    };
    use std::ffi::{c_char, c_int, c_void, CString};
    #[link(name = "fontconfig")]
    extern "C" {
        fn FcConfigGetCurrent() -> *mut c_void;
        fn FcConfigAppFontAddFile(config: *mut c_void, file: *const c_char) -> c_int;
    }
    let Ok(path) = CString::new(path.to_string_lossy().as_bytes()) else {
        return;
    };
    unsafe {
        let config = FcConfigGetCurrent();
        if !config.is_null() {
            let _ = FcConfigAppFontAddFile(config, path.as_ptr());
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn install_bundled_font() {}

#[cfg(target_os = "linux")]
fn bundled_font_path() -> Option<PathBuf> {
    const NAME: &str = "assets/fonts/NotoSansCJKsc-Regular.otf";
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(NAME);
    if manifest.is_file() {
        return Some(manifest);
    }
    let executable = std::env::current_exe().ok()?;
    let directory = executable.parent()?;
    [
        directory.join(NAME),
        directory.join("../").join(NAME),
        directory.join("../../").join(NAME),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
}

fn build(app: &Application) {
    install_css();
    let window = ApplicationWindow::builder()
        .application(app)
        .title("黑场复核台 · Recovery Replay")
        .default_width(1500)
        .default_height(920)
        .build();

    let header = gtk::HeaderBar::new();
    let title = gtk::Label::new(Some("黑场复核台"));
    title.add_css_class("app-title");
    header.set_title_widget(Some(&title));

    let open_button = gtk::Button::with_label("载入媒体");
    let report_button = gtk::Button::with_label("载入复核包");
    let demo_button = gtk::Button::with_label("恢复样例");
    let export_button = gtk::Button::with_label("封存当前复核");
    export_button.add_css_class("primary");
    header.pack_start(&open_button);
    header.pack_start(&report_button);
    header.pack_start(&demo_button);
    header.pack_end(&export_button);
    window.set_titlebar(Some(&header));

    let root = gtk::Box::new(Orientation::Vertical, 0);
    window.set_child(Some(&root));
    let body = gtk::Box::new(Orientation::Horizontal, 0);
    body.set_vexpand(true);
    root.append(&body);

    let sidebar = build_sidebar();
    sidebar.box_widget.set_size_request(330, -1);
    body.append(&sidebar.box_widget);
    let center = build_center();
    center.box_widget.set_hexpand(true);
    body.append(&center.box_widget);
    let right = build_right_panel();
    right.box_widget.set_size_request(410, -1);
    body.append(&right.box_widget);

    let statusbar = gtk::Box::new(Orientation::Horizontal, 12);
    statusbar.add_css_class("statusbar");
    statusbar.set_margin_start(12);
    statusbar.set_margin_end(12);
    statusbar.set_margin_top(5);
    statusbar.set_margin_bottom(5);
    let status_label = gtk::Label::new(Some("就绪 · Rust / GStreamer / GTK4"));
    status_label.set_hexpand(true);
    status_label.set_halign(gtk::Align::Start);
    statusbar.append(&status_label);
    let evidence_label = gtk::Label::new(Some("证据目录: review-evidence"));
    evidence_label.add_css_class("mono");
    statusbar.append(&evidence_label);
    root.append(&statusbar);

    let ui = Rc::new(Ui {
        window: window.clone(),
        source_path: RefCell::new(None),
        report: RefCell::new(None),
        keyframes: RefCell::new(Vec::new()),
        selected_interval: RefCell::new(None),
        selected_frame: RefCell::new(None),
        evidence_root: std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join("review-evidence"),
        source_label: sidebar.source_label,
        source_meta: sidebar.source_meta,
        extension_chip: sidebar.extension_chip,
        hash_label: sidebar.hash_label,
        recovery_switch: sidebar.recovery_switch,
        recovery_note: sidebar.recovery_note,
        threshold: sidebar.threshold,
        black_ratio: sidebar.black_ratio,
        min_duration: sidebar.min_duration,
        fps: sidebar.fps,
        track: sidebar.track,
        params_chip: center.params_chip,
        format_chip: center.format_chip,
        banner: center.banner,
        run_button: sidebar.run_button,
        cancel_button: sidebar.cancel_button,
        progress: sidebar.progress,
        progress_label: sidebar.progress_label,
        waveform: center.waveform,
        picture: center.picture,
        preview_label: center.preview_label,
        detail_label: center.detail_label,
        timeline_box: center.timeline_box,
        auto_box: right.auto_box,
        manual_box: right.manual_box,
        evidence_box: right.evidence_box,
        failure_box: right.failure_box,
        failure_revealer: right.failure_revealer,
        status_label,
        worker_rx: RefCell::new(None),
        cancel: RefCell::new(None),
        busy: Cell::new(false),
        manual_widgets: RefCell::new(None),
    });

    connect_actions(
        &ui,
        &open_button,
        &report_button,
        &demo_button,
        &export_button,
    );
    connect_waveform(&ui);
    start_worker_poll(&ui);
    refresh_all(&ui);
    load_demo(&ui);
    window.present();
    let _ = evidence_label;
}

struct Sidebar {
    box_widget: gtk::Box,
    source_label: gtk::Label,
    source_meta: gtk::Label,
    extension_chip: gtk::Label,
    hash_label: gtk::Label,
    recovery_switch: gtk::Switch,
    recovery_note: gtk::Entry,
    threshold: gtk::SpinButton,
    black_ratio: gtk::SpinButton,
    min_duration: gtk::SpinButton,
    fps: gtk::SpinButton,
    track: gtk::SpinButton,
    run_button: gtk::Button,
    cancel_button: gtk::Button,
    progress: gtk::ProgressBar,
    progress_label: gtk::Label,
}

struct Center {
    box_widget: gtk::Box,
    params_chip: gtk::Label,
    format_chip: gtk::Label,
    banner: gtk::Label,
    waveform: gtk::DrawingArea,
    picture: Picture,
    preview_label: gtk::Label,
    detail_label: gtk::Label,
    timeline_box: gtk::Box,
}

struct RightPanel {
    box_widget: gtk::Box,
    auto_box: gtk::Box,
    manual_box: gtk::Box,
    evidence_box: gtk::Box,
    failure_box: gtk::Box,
    failure_revealer: gtk::Revealer,
}

fn build_sidebar() -> Sidebar {
    let outer = gtk::Box::new(Orientation::Vertical, 0);
    outer.add_css_class("sidebar");
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .build();
    let content = gtk::Box::new(Orientation::Vertical, 12);
    content.set_margin_top(14);
    content.set_margin_bottom(14);
    content.set_margin_start(14);
    content.set_margin_end(14);
    scroll.set_child(Some(&content));
    outer.append(&scroll);

    let source_panel = panel();
    source_panel.append(&section_title("恢复现场"));
    let source_label = gtk::Label::new(Some("未载入媒体"));
    source_label.add_css_class("panel-title");
    source_label.set_halign(gtk::Align::Start);
    source_label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    source_label.set_max_width_chars(28);
    source_panel.append(&source_label);
    let source_meta = gtk::Label::new(Some("支持任意扩展名；内容由 decodebin 探测"));
    source_meta.add_css_class("muted");
    source_meta.add_css_class("tiny");
    source_meta.set_wrap(true);
    source_meta.set_halign(gtk::Align::Start);
    source_panel.append(&source_meta);
    let chips = gtk::Box::new(Orientation::Horizontal, 6);
    let extension_chip = chip("扩展名 —", "chip");
    chips.append(&extension_chip);
    let recovery_chip = chip("恢复后重放", "chip-ok");
    chips.append(&recovery_chip);
    source_panel.append(&chips);
    let hash_label = gtk::Label::new(Some("SHA-256: 待计算"));
    hash_label.add_css_class("mono");
    hash_label.add_css_class("muted");
    hash_label.set_halign(gtk::Align::Start);
    hash_label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    source_panel.append(&hash_label);
    let recovery_row = gtk::Box::new(Orientation::Horizontal, 8);
    recovery_row.append(&gtk::Label::new(Some("恢复状态")));
    let recovery_switch = gtk::Switch::new();
    recovery_switch.set_active(true);
    recovery_switch.set_hexpand(true);
    recovery_switch.set_halign(gtk::Align::End);
    recovery_row.append(&recovery_switch);
    source_panel.append(&recovery_row);
    let recovery_note = gtk::Entry::builder()
        .placeholder_text("恢复来源 / 交接说明")
        .text("从恢复索引重建并重放")
        .build();
    source_panel.append(&recovery_note);
    content.append(&source_panel);

    let params_panel = panel();
    params_panel.append(&section_title("检测参数"));
    let threshold = spin(0.0, 1.0, 0.01, 3, 0.10);
    let black_ratio = spin(0.0, 1.0, 0.01, 3, 0.95);
    let min_duration = spin(100.0, 60_000.0, 100.0, 0, 500.0);
    let fps = spin(1.0, 60.0, 1.0, 0, 5.0);
    let track = spin(0.0, 16.0, 1.0, 0, 0.0);
    params_panel.append(&param_row("亮度阈值", &threshold, "0–1"));
    params_panel.append(&param_row("黑像素比例", &black_ratio, "0–1"));
    params_panel.append(&param_row("最短黑场 ms", &min_duration, "duration"));
    params_panel.append(&param_row("采样 fps", &fps, "frame rate"));
    params_panel.append(&param_row("视频轨索引", &track, "track"));
    let run_button = gtk::Button::with_label("开始恢复后重放");
    run_button.add_css_class("primary");
    params_panel.append(&run_button);
    let cancel_button = gtk::Button::with_label("停止");
    cancel_button.set_sensitive(false);
    params_panel.append(&cancel_button);
    let progress = gtk::ProgressBar::new();
    progress.set_show_text(false);
    params_panel.append(&progress);
    let progress_label = gtk::Label::new(Some("等待重放"));
    progress_label.add_css_class("muted");
    progress_label.add_css_class("tiny");
    progress_label.set_halign(gtk::Align::Start);
    progress_label.set_wrap(true);
    params_panel.append(&progress_label);
    content.append(&params_panel);

    let note_panel = panel();
    note_panel.append(&section_title("复核约定"));
    let note = gtk::Label::new(Some(
        "自动结论只读；人工确认单独记录。每次重放与封存都会生成新的证据包，不覆盖旧证据。",
    ));
    note.add_css_class("muted");
    note.add_css_class("tiny");
    note.set_wrap(true);
    note.set_halign(gtk::Align::Start);
    note_panel.append(&note);
    content.append(&note_panel);

    Sidebar {
        box_widget: outer,
        source_label,
        source_meta,
        extension_chip,
        hash_label,
        recovery_switch,
        recovery_note,
        threshold,
        black_ratio,
        min_duration,
        fps,
        track,
        run_button,
        cancel_button,
        progress,
        progress_label,
    }
}

fn build_center() -> Center {
    let outer = gtk::Box::new(Orientation::Vertical, 10);
    outer.add_css_class("content");
    outer.set_margin_top(14);
    outer.set_margin_bottom(14);
    outer.set_margin_start(14);
    outer.set_margin_end(14);

    let heading = gtk::Box::new(Orientation::Horizontal, 8);
    let title = gtk::Label::new(Some("恢复后重放 · 波形主线"));
    title.add_css_class("section-title");
    heading.append(&title);
    let format_chip = chip("格式 v2", "chip");
    heading.append(&format_chip);
    let params_chip = chip("参数 —", "chip");
    heading.append(&params_chip);
    let spacer = gtk::Box::new(Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    heading.append(&spacer);
    let hint = gtk::Label::new(Some("点击波形或区间核对帧"));
    hint.add_css_class("muted");
    hint.add_css_class("tiny");
    heading.append(&hint);
    outer.append(&heading);

    let banner = gtk::Label::new(None);
    banner.add_css_class("banner");
    banner.set_halign(gtk::Align::Fill);
    banner.set_wrap(true);
    banner.set_visible(false);
    outer.append(&banner);

    let waveform_panel = panel();
    let wave_head = gtk::Box::new(Orientation::Horizontal, 8);
    let wave_title = gtk::Label::new(Some("输出波形（逐采样亮度 Y）"));
    wave_title.add_css_class("panel-title");
    wave_head.append(&wave_title);
    let wave_spacer = gtk::Box::new(Orientation::Horizontal, 0);
    wave_spacer.set_hexpand(true);
    wave_head.append(&wave_spacer);
    let red = chip("自动黑场区间", "chip-auto");
    wave_head.append(&red);
    waveform_panel.append(&wave_head);
    let waveform = gtk::DrawingArea::new();
    waveform.set_content_height(220);
    waveform.set_hexpand(true);
    waveform.set_cursor_from_name(Some("crosshair"));
    waveform_panel.append(&waveform);
    outer.append(&waveform_panel);

    let inspect_panel = panel();
    let inspect_head = gtk::Box::new(Orientation::Horizontal, 8);
    let inspect_title = gtk::Label::new(Some("时间点 / 轨道 / 帧 对应"));
    inspect_title.add_css_class("panel-title");
    inspect_head.append(&inspect_title);
    let preview_label = gtk::Label::new(Some("尚未选择区间或采样"));
    preview_label.add_css_class("muted");
    preview_label.add_css_class("tiny");
    preview_label.set_hexpand(true);
    preview_label.set_halign(gtk::Align::End);
    inspect_head.append(&preview_label);
    inspect_panel.append(&inspect_head);
    let inspect_row = gtk::Box::new(Orientation::Horizontal, 12);
    let picture = Picture::new();
    picture.add_css_class("keyframe");
    picture.set_content_fit(gtk::ContentFit::Contain);
    picture.set_size_request(320, 180);
    inspect_row.append(&picture);
    let detail_label = gtk::Label::new(Some(
        "选择黑场区间或点击波形后，这里显示时间码、帧号、轨道、参数指纹与亮度统计。",
    ));
    detail_label.set_wrap(true);
    detail_label.set_hexpand(true);
    detail_label.set_halign(gtk::Align::Start);
    detail_label.set_valign(gtk::Align::Center);
    detail_label.set_xalign(0.0);
    detail_label.add_css_class("mono");
    inspect_row.append(&detail_label);
    inspect_panel.append(&inspect_row);
    outer.append(&inspect_panel);

    let timeline_panel = panel();
    timeline_panel.set_vexpand(true);
    let timeline_head = gtk::Box::new(Orientation::Horizontal, 8);
    let timeline_title = gtk::Label::new(Some("复核索引"));
    timeline_title.add_css_class("panel-title");
    timeline_head.append(&timeline_title);
    let timeline_hint = gtk::Label::new(Some("时间段 · 帧区间 · 参数 · 证据"));
    timeline_hint.add_css_class("muted");
    timeline_hint.add_css_class("tiny");
    timeline_head.append(&timeline_hint);
    timeline_panel.append(&timeline_head);
    let timeline_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .min_content_height(140)
        .build();
    let timeline_box = gtk::Box::new(Orientation::Vertical, 6);
    timeline_box.set_vexpand(true);
    timeline_scroll.set_child(Some(&timeline_box));
    timeline_panel.append(&timeline_scroll);
    outer.append(&timeline_panel);

    Center {
        box_widget: outer,
        params_chip,
        format_chip,
        banner,
        waveform,
        picture,
        preview_label,
        detail_label,
        timeline_box,
    }
}

fn build_right_panel() -> RightPanel {
    let outer = gtk::Box::new(Orientation::Vertical, 0);
    outer.add_css_class("sidebar");
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .build();
    let content = gtk::Box::new(Orientation::Vertical, 12);
    content.set_margin_top(14);
    content.set_margin_bottom(14);
    content.set_margin_start(14);
    content.set_margin_end(14);
    scroll.set_child(Some(&content));
    outer.append(&scroll);

    let auto_box = gtk::Box::new(Orientation::Vertical, 8);
    auto_box.add_css_class("panel");
    auto_box.add_css_class("auto-card");
    auto_box.set_margin_top(2);
    auto_box.set_margin_bottom(2);
    auto_box.set_margin_start(2);
    auto_box.set_margin_end(2);
    content.append(&auto_box);

    let manual_box = gtk::Box::new(Orientation::Vertical, 8);
    manual_box.add_css_class("panel");
    manual_box.add_css_class("manual-card");
    manual_box.set_margin_top(2);
    manual_box.set_margin_bottom(2);
    manual_box.set_margin_start(2);
    manual_box.set_margin_end(2);
    content.append(&manual_box);

    let failure_box = gtk::Box::new(Orientation::Vertical, 8);
    failure_box.add_css_class("panel");
    failure_box.add_css_class("failure-card");
    let failure_revealer = gtk::Revealer::new();
    failure_revealer.set_transition_type(gtk::RevealerTransitionType::SlideDown);
    failure_revealer.set_reveal_child(false);
    failure_revealer.set_child(Some(&failure_box));
    content.append(&failure_revealer);

    let evidence_box = gtk::Box::new(Orientation::Vertical, 8);
    evidence_box.add_css_class("panel");
    content.append(&evidence_box);

    RightPanel {
        box_widget: outer,
        auto_box,
        manual_box,
        evidence_box,
        failure_box,
        failure_revealer,
    }
}

fn install_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_data(CSS);
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

fn panel() -> gtk::Box {
    let panel = gtk::Box::new(Orientation::Vertical, 10);
    panel.add_css_class("panel");
    panel.set_margin_top(2);
    panel.set_margin_bottom(2);
    panel.set_margin_start(2);
    panel.set_margin_end(2);
    panel.set_valign(gtk::Align::Start);
    panel
}

fn section_title(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class("panel-title");
    label.set_halign(gtk::Align::Start);
    label
}

fn chip(text: &str, class: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class("chip");
    label.add_css_class(class);
    label.set_halign(gtk::Align::Start);
    label
}

fn spin(min: f64, max: f64, step: f64, digits: u32, value: f64) -> gtk::SpinButton {
    let spin = gtk::SpinButton::with_range(min, max, step);
    spin.set_digits(digits);
    spin.set_value(value);
    spin.set_hexpand(true);
    spin
}

fn param_row<W: IsA<gtk::Widget>>(label: &str, widget: &W, hint: &str) -> gtk::Box {
    let row = gtk::Box::new(Orientation::Horizontal, 8);
    let name = gtk::Label::new(Some(label));
    name.set_width_chars(14);
    name.set_xalign(0.0);
    row.append(&name);
    widget.set_hexpand(true);
    row.append(widget);
    let hint = gtk::Label::new(Some(hint));
    hint.add_css_class("muted");
    hint.add_css_class("tiny");
    row.append(&hint);
    row
}

fn clear_box(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

fn muted(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class("muted");
    label.add_css_class("tiny");
    label.set_wrap(true);
    label.set_xalign(0.0);
    label.set_halign(gtk::Align::Start);
    label
}

fn add_label(container: &gtk::Box, text: &str, class: Option<&str>) {
    let label = gtk::Label::new(Some(text));
    label.set_wrap(true);
    label.set_xalign(0.0);
    label.set_halign(gtk::Align::Start);
    if let Some(class) = class {
        label.add_css_class(class);
    }
    container.append(&label);
}

fn connect_actions(
    ui: &Rc<Ui>,
    open_button: &gtk::Button,
    report_button: &gtk::Button,
    demo_button: &gtk::Button,
    export_button: &gtk::Button,
) {
    let widget = Rc::clone(ui);
    open_button.connect_clicked(move |_| choose_media(&widget));
    let widget = Rc::clone(ui);
    report_button.connect_clicked(move |_| choose_report(&widget));
    let widget = Rc::clone(ui);
    demo_button.connect_clicked(move |_| load_demo(&widget));
    let widget = Rc::clone(ui);
    export_button.connect_clicked(move |_| seal_current(&widget));
    let widget = Rc::clone(ui);
    ui.run_button
        .connect_clicked(move |_| start_analysis(&widget));
    let widget = Rc::clone(ui);
    ui.cancel_button.connect_clicked(move |_| {
        if let Some(cancel) = widget.cancel.borrow().as_ref() {
            cancel.store(true, Ordering::SeqCst);
            set_banner(
                &widget,
                "正在停止重放；已生成的采样会写入失败证据包。",
                "banner-warn",
            );
        }
    });
}

fn choose_media(ui: &Rc<Ui>) {
    let dialog = gtk::FileChooserNative::builder()
        .title("选择媒体文件（任意扩展名）")
        .action(gtk::FileChooserAction::Open)
        .transient_for(&ui.window)
        .modal(true)
        .build();
    let widget = Rc::clone(ui);
    dialog.connect_response(move |dialog, response| {
        if response == gtk::ResponseType::Accept {
            if let Some(path) = dialog.file().and_then(|file| file.path()) {
                set_source(&widget, path);
            }
        }
        dialog.destroy();
    });
    dialog.show();
}

fn choose_report(ui: &Rc<Ui>) {
    let dialog = gtk::FileChooserNative::builder()
        .title("载入复核报告（v1 / v2 JSON）")
        .action(gtk::FileChooserAction::Open)
        .transient_for(&ui.window)
        .modal(true)
        .build();
    let filter = gtk::FileFilter::new();
    filter.set_name(Some("黑场复核报告"));
    filter.add_pattern("*.json");
    dialog.set_filter(&filter);
    let widget = Rc::clone(ui);
    dialog.connect_response(move |dialog, response| {
        if response == gtk::ResponseType::Accept {
            if let Some(path) = dialog.file().and_then(|file| file.path()) {
                match read_report(&path) {
                    Ok(report) => {
                        let source = PathBuf::from(&report.source.path);
                        if source.is_file() {
                            widget.source_path.replace(Some(source));
                        }
                        widget.selected_interval.replace(
                            (!report.auto_conclusion.intervals.is_empty()).then_some(0),
                        );
                        widget.selected_frame.replace(None);
                        widget.report.replace(Some(report.clone()));
                        widget.keyframes.replace(Vec::new());
                        refresh_all(&widget);
                        if report.compatibility.migrated {
                            set_banner(
                                &widget,
                                "已迁移旧格式报告。波形为区间重建示意，不替代原始证据；建议重新重放。",
                                "banner-warn",
                            );
                        } else {
                            set_banner(&widget, "已载入复核包，可继续人工确认或重新重放。", "banner-ok");
                        }
                    }
                    Err(error) => {
                        set_banner(&widget, &format!("无法载入报告：{error}"), "banner-error");
                    }
                }
            }
        }
        dialog.destroy();
    });
    dialog.show();
}

fn set_source(ui: &Rc<Ui>, path: PathBuf) {
    let extension = extension_record(&path);
    ui.source_path.replace(Some(path.clone()));
    ui.report.replace(None);
    ui.keyframes.replace(Vec::new());
    ui.selected_interval.replace(None);
    ui.selected_frame.replace(None);
    ui.source_label.set_text(
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("media"),
    );
    ui.source_meta.set_text(&path.display().to_string());
    match extension.status {
        ExtensionStatus::Known => {
            ui.extension_chip.set_text(&format!(
                ".{} · 已知",
                extension.raw_extension.as_deref().unwrap_or("?")
            ));
            ui.extension_chip.remove_css_class("chip-warn");
            ui.extension_chip.add_css_class("chip-ok");
        }
        ExtensionStatus::Unknown => {
            ui.extension_chip.set_text(&format!(
                ".{} · 未知扩展（将记录）",
                extension.raw_extension.as_deref().unwrap_or("?")
            ));
            ui.extension_chip.remove_css_class("chip-ok");
            ui.extension_chip.add_css_class("chip-warn");
        }
        ExtensionStatus::NoExtension => {
            ui.extension_chip.set_text("无扩展名 · 内容探测");
            ui.extension_chip.add_css_class("chip-warn");
        }
    }
    ui.hash_label.set_text("SHA-256: 重放时计算并封存");
    refresh_all(ui);
    set_banner(ui, "源文件已载入；开始恢复后重放。", "banner");
}

fn read_params(ui: &Rc<Ui>) -> DetectorParams {
    DetectorParams {
        luma_threshold: ui.threshold.value() as f32,
        black_pixel_ratio: ui.black_ratio.value() as f32,
        min_duration_ms: ui.min_duration.value().round() as u64,
        sample_fps: ui.fps.value().round() as u32,
        track_index: ui.track.value().round() as u32,
        ..DetectorParams::default()
    }
}

fn start_analysis(ui: &Rc<Ui>) {
    if ui.busy.get() {
        return;
    }
    let Some(source) = ui.source_path.borrow().clone() else {
        choose_media(ui);
        return;
    };
    let params = read_params(ui);
    if let Err(detail) = params.validate() {
        set_banner(ui, &format!("参数无效：{detail}"), "banner-error");
        return;
    }
    let request = AnalysisRequest {
        source,
        params,
        recovered: ui.recovery_switch.is_active(),
        recovery_note: ui.recovery_note.text().to_string(),
        replayed_from_report: ui
            .report
            .borrow()
            .as_ref()
            .map(|report| report.report_id.clone()),
        replay_number: ui
            .report
            .borrow()
            .as_ref()
            .map(|report| report.replay.replay_number.saturating_add(1))
            .unwrap_or(1),
    };
    ui.busy.set(true);
    ui.run_button.set_sensitive(false);
    ui.cancel_button.set_sensitive(true);
    ui.progress.set_fraction(0.0);
    ui.progress_label.set_text("准备恢复后重放…");
    ui.failure_revealer.set_reveal_child(false);
    set_banner(ui, "正在按内容探测并解码；未知扩展不会阻止分析。", "banner");

    let (tx, rx) = mpsc::channel();
    let (progress_tx, progress_rx) = mpsc::channel::<ProgressUpdate>();
    let forward = tx.clone();
    std::thread::spawn(move || {
        while let Ok(update) = progress_rx.recv() {
            let _ = forward.send(WorkerMessage::Progress(update));
        }
    });
    ui.worker_rx.replace(Some(Arc::new(Mutex::new(rx))));
    let cancel = Arc::new(AtomicBool::new(false));
    ui.cancel.replace(Some(cancel.clone()));
    let evidence_root = ui.evidence_root.clone();
    std::thread::spawn(move || {
        let outcome = gst_backend::analyze(&request, cancel, progress_tx);
        let result = match outcome {
            Ok(output) => {
                let keyframes = output.keyframes;
                match evidence::write_bundle(&evidence_root, output.report.clone(), &keyframes) {
                    Ok(bundle) => WorkerResult {
                        report: bundle.report,
                        keyframes,
                        bundle_path: Some(bundle.root),
                        failure: None,
                        bundle_error: None,
                    },
                    Err(error) => WorkerResult {
                        report: output.report,
                        keyframes,
                        bundle_path: None,
                        failure: None,
                        bundle_error: Some(format!("自动证据封存失败：{error}")),
                    },
                }
            }
            Err(failure) => {
                let mut report = analysis::failure_report(&request, &failure);
                if let Some(record) = report.failure.as_mut() {
                    record.evidence_retained = true;
                }
                let (report, bundle_path, bundle_error) =
                    match evidence::write_bundle(&evidence_root, report, &[]) {
                        Ok(bundle) => (bundle.report, Some(bundle.root), None),
                        Err(error) => (
                            analysis::failure_report(&request, &failure),
                            None,
                            Some(format!("失败证据仍无法写入：{error}")),
                        ),
                    };
                WorkerResult {
                    report,
                    keyframes: Vec::new(),
                    bundle_path,
                    failure: Some(failure),
                    bundle_error,
                }
            }
        };
        let _ = tx.send(WorkerMessage::Finished(Box::new(result)));
    });
}

fn start_worker_poll(ui: &Rc<Ui>) {
    let weak = Rc::downgrade(ui);
    glib::timeout_add_local(Duration::from_millis(100), move || {
        let Some(ui) = weak.upgrade() else {
            return glib::ControlFlow::Break;
        };
        let Some(receiver) = ui.worker_rx.borrow().clone() else {
            return glib::ControlFlow::Continue;
        };
        let messages: Vec<WorkerMessage> = receiver
            .lock()
            .map(|guard| guard.try_iter().collect())
            .unwrap_or_default();
        for message in messages {
            match message {
                WorkerMessage::Progress(update) => {
                    ui.progress.set_fraction(update.fraction as f64);
                    if update.fraction <= 0.01 {
                        ui.progress.pulse();
                    }
                    ui.progress_label.set_text(&format!(
                        "{} · {} · frame {}",
                        update.message,
                        format_timecode(update.time_ms),
                        update.frame
                    ));
                }
                WorkerMessage::Finished(result) => {
                    ui.worker_rx.replace(None);
                    finish_worker(&ui, *result);
                }
            }
        }
        glib::ControlFlow::Continue
    });
}

fn finish_worker(ui: &Rc<Ui>, result: WorkerResult) {
    ui.busy.set(false);
    ui.run_button.set_sensitive(true);
    ui.cancel_button.set_sensitive(false);
    ui.cancel.replace(None);
    ui.progress.set_fraction(1.0);
    ui.report.replace(Some(result.report.clone()));
    ui.keyframes.replace(result.keyframes);
    if ui.selected_interval.borrow().is_none()
        && !result.report.auto_conclusion.intervals.is_empty()
    {
        ui.selected_interval.replace(Some(0));
    }
    ui.selected_frame.replace(None);
    refresh_all(ui);

    if let Some(failure) = &result.failure {
        ui.progress_label
            .set_text(&format!("失败：{}", failure.title));
        set_banner(
            ui,
            &format!(
                "重放失败。{}",
                result
                    .bundle_path
                    .as_ref()
                    .map(|path| format!("失败证据已保留在 {}", path.display()))
                    .unwrap_or_else(|| "证据目录写入失败，请按后续操作处理。".into())
            ),
            "banner-error",
        );
        show_failure(ui, failure);
    } else if let Some(error) = &result.bundle_error {
        ui.progress_label.set_text("分析完成，但证据封存失败");
        set_banner(ui, error, "banner-error");
        ui.failure_revealer.set_reveal_child(false);
    } else {
        ui.progress_label.set_text("重放与证据封存完成");
        set_banner(
            ui,
            &format!(
                "自动分析完成；证据已封存在 {}。自动结论仍需人工确认。",
                result
                    .bundle_path
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| ui.evidence_root.display().to_string())
            ),
            "banner-ok",
        );
        ui.failure_revealer.set_reveal_child(false);
    }
    ui.status_label
        .set_text("重放结束 · 自动结论与人工确认分栏显示");
}

fn load_demo(ui: &Rc<Ui>) {
    let output = analysis::demo_output();
    ui.source_path.replace(None);
    ui.report.replace(Some(output.report));
    ui.keyframes.replace(output.keyframes);
    ui.selected_interval.replace(Some(0));
    ui.selected_frame.replace(None);
    ui.failure_revealer.set_reveal_child(false);
    refresh_all(ui);
    set_banner(
        ui,
        "当前为恢复后重放演示：未知扩展 .recx、两次黑场与证据对应关系均可点击核对。",
        "banner-warn",
    );
    ui.status_label
        .set_text("演示数据 · 载入媒体后可开始真实重放");
    ui.progress.set_fraction(1.0);
    ui.progress_label.set_text("演示恢复重放 · 120 个采样");
}

fn seal_current(ui: &Rc<Ui>) {
    let Some(report) = ui.report.borrow().clone() else {
        set_banner(ui, "没有可封存的报告。", "banner-error");
        return;
    };
    let keyframes = ui.keyframes.borrow().clone();
    ui.status_label.set_text("正在封存证据包…");
    match evidence::write_bundle(&ui.evidence_root, report, &keyframes) {
        Ok(bundle) => {
            ui.report.replace(Some(bundle.report));
            refresh_all(ui);
            set_banner(
                ui,
                &format!("已封存新证据包：{}", bundle.root.display()),
                "banner-ok",
            );
            ui.status_label.set_text("证据封存完成");
        }
        Err(error) => {
            let failure = AnalysisFailure::output_failure(
                &ui.evidence_root.display().to_string(),
                error.to_string(),
            );
            set_banner(ui, &format!("证据写入失败：{error}"), "banner-error");
            show_failure(ui, &failure);
        }
    }
}

fn refresh_all(ui: &Rc<Ui>) {
    let report = ui.report.borrow().clone();
    if let Some(report) = &report {
        ui.threshold
            .set_value(report.detector.luma_threshold as f64);
        ui.black_ratio
            .set_value(report.detector.black_pixel_ratio as f64);
        ui.min_duration
            .set_value(report.detector.min_duration_ms as f64);
        ui.fps.set_value(report.detector.sample_fps as f64);
        ui.track.set_value(report.detector.track_index as f64);
        ui.recovery_switch.set_active(report.replay.recovered);
        refresh_source_header(ui, report);
        ui.params_chip
            .set_text(&format!("参数 {}", report.detector.fingerprint()));
        if report.compatibility.migrated {
            ui.format_chip.set_text(&format!(
                "{} → v2 · 已迁移",
                report.compatibility.input_schema
            ));
            ui.format_chip.remove_css_class("chip");
            ui.format_chip.add_css_class("chip-warn");
        } else {
            ui.format_chip.set_text("新格式 v2");
            ui.format_chip.remove_css_class("chip-warn");
            ui.format_chip.add_css_class("chip");
        }
        if let Some(failure) = &report.failure {
            show_failure(ui, &failure_to_analysis(failure));
        } else {
            ui.failure_revealer.set_reveal_child(false);
        }
    } else if let Some(path) = ui.source_path.borrow().as_ref() {
        ui.source_label.set_text(
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("media"),
        );
        ui.source_meta.set_text(&path.display().to_string());
        ui.hash_label.set_text("SHA-256: 重放时计算");
        ui.params_chip
            .set_text(&format!("参数 {}", read_params(ui).fingerprint()));
        ui.format_chip.set_text("等待分析 · 输出 v2");
    } else {
        ui.source_label.set_text("未载入媒体");
        ui.source_meta
            .set_text("支持任意扩展名；内容由 decodebin 探测");
        ui.extension_chip.set_text("扩展名 —");
        ui.hash_label.set_text("SHA-256: 待计算");
        ui.params_chip
            .set_text(&format!("参数 {}", read_params(ui).fingerprint()));
        ui.format_chip.set_text("兼容 v1 / v2");
    }
    refresh_timeline(ui, report.as_ref());
    refresh_auto(ui, report.as_ref());
    refresh_manual(ui, report.as_ref());
    refresh_evidence(ui, report.as_ref());
    refresh_detail(ui);
    ui.waveform.queue_draw();
}

fn refresh_source_header(ui: &Rc<Ui>, report: &ReviewReport) {
    ui.source_label.set_text(&report.source.file_name);
    ui.source_meta.set_text(&report.source.path);
    ui.hash_label
        .set_text(&format!("SHA-256: {}", short_hash(&report.source.sha256)));
    let ext = report
        .source
        .extension
        .raw_extension
        .as_deref()
        .unwrap_or("none");
    match report.source.extension.status {
        ExtensionStatus::Known => {
            ui.extension_chip.set_text(&format!(".{ext} · 已知"));
            ui.extension_chip.remove_css_class("chip-warn");
            ui.extension_chip.add_css_class("chip-ok");
        }
        ExtensionStatus::Unknown => {
            ui.extension_chip
                .set_text(&format!(".{ext} · 未知扩展已记录"));
            ui.extension_chip.remove_css_class("chip-ok");
            ui.extension_chip.add_css_class("chip-warn");
        }
        ExtensionStatus::NoExtension => {
            ui.extension_chip.set_text("无扩展名 · 已记录");
            ui.extension_chip.remove_css_class("chip-ok");
            ui.extension_chip.add_css_class("chip-warn");
        }
    }
}

fn short_hash(hash: &str) -> String {
    if hash.len() > 20 {
        format!("{}…{}", &hash[..12], &hash[hash.len() - 8..])
    } else {
        hash.to_string()
    }
}

fn refresh_timeline(ui: &Rc<Ui>, report: Option<&ReviewReport>) {
    clear_box(&ui.timeline_box);
    let Some(report) = report else {
        ui.timeline_box.append(&muted(
            "尚无重放结果。载入媒体后可生成逐采样波形与可复核区间。",
        ));
        return;
    };
    let header = gtk::Grid::new();
    header.set_column_spacing(12);
    header.add_css_class("timeline-head");
    for (column, text) in ["区间", "时间码", "帧", "轨道", "亮度", "证据"]
        .iter()
        .enumerate()
    {
        let label = gtk::Label::new(Some(text));
        label.set_xalign(0.0);
        header.attach(&label, column as i32, 0, 1, 1);
    }
    ui.timeline_box.append(&header);
    for (index, interval) in report.auto_conclusion.intervals.iter().enumerate() {
        let button = gtk::Button::new();
        button.set_has_frame(false);
        button.add_css_class("timeline-row");
        if ui.selected_interval.borrow().as_ref() == Some(&index) {
            button.add_css_class("timeline-selected");
        }
        let grid = gtk::Grid::new();
        grid.set_column_spacing(12);
        let id = gtk::Label::new(Some(&interval.id));
        id.set_xalign(0.0);
        id.add_css_class("mono");
        let time = gtk::Label::new(Some(&format!(
            "{}  →  {}",
            format_timecode(interval.start_ms),
            format_timecode(interval.end_ms)
        )));
        time.set_xalign(0.0);
        time.add_css_class("mono");
        let frames = gtk::Label::new(Some(&format!(
            "{}–{}",
            interval.start_frame, interval.end_frame
        )));
        frames.set_xalign(0.0);
        frames.add_css_class("mono");
        let track = gtk::Label::new(Some(&interval.track_id));
        track.set_xalign(0.0);
        let luma = gtk::Label::new(Some(&format!(
            "Y {:.3} / min {:.3}",
            interval.mean_luma, interval.lowest_luma
        )));
        luma.set_xalign(0.0);
        let evidence = gtk::Label::new(Some(&format!("{} 项", interval.evidence_refs.len())));
        evidence.set_xalign(0.0);
        grid.attach(&id, 0, 0, 1, 1);
        grid.attach(&time, 1, 0, 1, 1);
        grid.attach(&frames, 2, 0, 1, 1);
        grid.attach(&track, 3, 0, 1, 1);
        grid.attach(&luma, 4, 0, 1, 1);
        grid.attach(&evidence, 5, 0, 1, 1);
        button.set_child(Some(&grid));
        let widget = Rc::clone(ui);
        button.connect_clicked(move |_| select_interval(&widget, index));
        ui.timeline_box.append(&button);
    }
    if report.auto_conclusion.intervals.is_empty() {
        ui.timeline_box
            .append(&muted("没有持续超过阈值的黑场区间。"));
    }
    let events = gtk::Expander::new(Some("恢复后重放事件"));
    let event_box = gtk::Box::new(Orientation::Vertical, 5);
    for event in &report.replay.events {
        let text = format!(
            "#{:02}  {}  [{}]  {}",
            event.sequence,
            format_timecode(event.at_ms),
            event.kind,
            event.detail
        );
        event_box.append(&muted(&text));
    }
    events.set_child(Some(&event_box));
    ui.timeline_box.append(&events);
}

fn select_interval(ui: &Rc<Ui>, index: usize) {
    ui.selected_interval.replace(Some(index));
    ui.selected_frame.replace(None);
    if let Some(report) = ui.report.borrow().clone() {
        refresh_timeline(ui, Some(&report));
    }
    refresh_detail(ui);
    ui.waveform.queue_draw();
}

fn refresh_detail(ui: &Rc<Ui>) {
    let Some(report) = ui.report.borrow().clone() else {
        ui.detail_label
            .set_text("选择黑场区间或点击波形后，这里显示时间码、帧号、轨道、参数指纹与亮度统计。");
        ui.preview_label.set_text("尚未选择区间或采样");
        ui.picture.set_paintable(None::<&gdk::Texture>);
        return;
    };
    if let Some(index) = *ui.selected_frame.borrow() {
        if let Some(sample) = report.waveform.samples.get(index) {
            ui.detail_label.set_text(&format!(
                "时间码  {}\n帧号    {}   轨道  {}\n采样    #{}   PTS {} ns\n亮度 Y  mean {:.4}  min {}  max {}  黑像素 {:.1}%\n判定    {}   参数 {}\n波形    waveform.csv 第 {} 行",
                format_timecode(sample.time_ms),
                sample.frame,
                sample.track_id,
                sample.sample,
                sample.pts_ns,
                sample.y_mean,
                sample.y_min,
                sample.y_max,
                sample.black_pixel_ratio * 100.0,
                if sample.is_black { "BLACK" } else { "NORMAL" },
                sample.params_fingerprint,
                sample.sample + 2
            ));
            let keyframe = ui
                .keyframes
                .borrow()
                .iter()
                .min_by_key(|frame| frame.frame.abs_diff(sample.frame))
                .cloned();
            set_preview(ui, keyframe.as_ref(), Some(sample.time_ms));
            return;
        }
    }
    if let Some(index) = *ui.selected_interval.borrow() {
        if let Some(interval) = report.auto_conclusion.intervals.get(index) {
            ui.detail_label.set_text(&format!(
                "区间    {}   状态 {:?}\n时间码  {}  →  {}\n帧号    {}  →  {}   采样 #{}–#{}\n轨道    {}\n亮度 Y  mean {:.4}  最低 {:.4}\n参数    {}\n证据    {}",
                interval.id,
                interval.state,
                format_timecode(interval.start_ms),
                format_timecode(interval.end_ms),
                interval.start_frame,
                interval.end_frame,
                interval.sample_start,
                interval.sample_end,
                interval.track_id,
                interval.mean_luma,
                interval.lowest_luma,
                interval.params_fingerprint,
                if interval.evidence_refs.is_empty() {
                    "证据引用见 report.json".into()
                } else {
                    interval.evidence_refs.join(", ")
                }
            ));
            let keyframe = ui
                .keyframes
                .borrow()
                .iter()
                .find(|frame| frame.interval_id.as_deref() == Some(interval.id.as_str()))
                .cloned();
            set_preview(ui, keyframe.as_ref(), Some(interval.start_ms));
            return;
        }
    }
    ui.detail_label
        .set_text("拖动或点击波形可定位采样；点击上方区间可查看起点帧与最暗帧。");
    ui.preview_label.set_text("尚未选择区间或采样");
    ui.picture.set_paintable(None::<&gdk::Texture>);
}

fn set_preview(ui: &Rc<Ui>, keyframe: Option<&KeyFrame>, time_ms: Option<u64>) {
    let Some(keyframe) = keyframe else {
        ui.preview_label.set_text(
            &time_ms
                .map(|time| format!("{} · 无关键帧证据", format_timecode(time)))
                .unwrap_or_else(|| "无关键帧证据".into()),
        );
        ui.picture.set_paintable(None::<&gdk::Texture>);
        return;
    };
    match keyframe.to_png() {
        Ok(bytes) => {
            let bytes = glib::Bytes::from(&bytes);
            match gdk::Texture::from_bytes(&bytes) {
                Ok(texture) => ui.picture.set_paintable(Some(&texture)),
                Err(_) => ui.picture.set_paintable(None::<&gdk::Texture>),
            }
        }
        Err(_) => ui.picture.set_paintable(None::<&gdk::Texture>),
    }
    ui.preview_label.set_text(&format!(
        "{} · 帧 {} · {}",
        format_timecode(keyframe.time_ms),
        keyframe.frame,
        keyframe.reason
    ));
}

fn connect_waveform(ui: &Rc<Ui>) {
    let widget = Rc::clone(ui);
    ui.waveform.set_draw_func(move |area, cr, width, height| {
        draw_waveform(&widget, area, cr, width, height);
    });
    let gesture = gtk::GestureClick::new();
    gesture.set_button(1);
    let widget = Rc::clone(ui);
    gesture.connect_pressed(move |_, _, x, _| {
        let Some(report) = widget.report.borrow().clone() else {
            return;
        };
        let width = widget.waveform.width().max(1) as f64;
        let covered = report.waveform.covered_ms.max(1) as f64;
        let time_ms = ((x / width) * covered).clamp(0.0, covered) as u64;
        if let Some((index, _)) = report
            .waveform
            .samples
            .iter()
            .enumerate()
            .min_by_key(|(_, sample)| sample.time_ms.abs_diff(time_ms))
        {
            widget.selected_frame.replace(Some(index));
            let interval = report
                .auto_conclusion
                .intervals
                .iter()
                .position(|interval| time_ms >= interval.start_ms && time_ms <= interval.end_ms);
            widget.selected_interval.replace(interval);
            refresh_timeline(&widget, Some(&report));
            refresh_detail(&widget);
            widget.waveform.queue_draw();
        }
    });
    ui.waveform.add_controller(gesture);
}

fn draw_waveform(
    ui: &Rc<Ui>,
    _area: &gtk::DrawingArea,
    cr: &cairo::Context,
    width: i32,
    height: i32,
) {
    let width = width as f64;
    let height = height as f64;
    cr.set_source_rgb(0.055, 0.068, 0.09);
    cr.rectangle(0.0, 0.0, width, height);
    let _ = cr.fill();
    let left = 46.0;
    let right = 14.0;
    let top = 18.0;
    let bottom = 30.0;
    let plot_width = (width - left - right).max(1.0);
    let plot_height = (height - top - bottom).max(1.0);

    cr.set_line_width(1.0);
    for step in 0..=4 {
        let fraction = step as f64 / 4.0;
        let y = top + plot_height * (1.0 - fraction);
        cr.set_source_rgba(0.25, 0.30, 0.38, 0.45);
        cr.move_to(left, y);
        cr.line_to(left + plot_width, y);
        let _ = cr.stroke();
        cr.set_source_rgba(0.55, 0.61, 0.72, 0.9);
        cr.set_font_size(10.0);
        cr.move_to(7.0, y + 3.0);
        let _ = cr.show_text(&format!("{:.1}", fraction));
    }
    let Some(report) = ui.report.borrow().clone() else {
        cr.set_source_rgba(0.55, 0.61, 0.72, 0.9);
        cr.set_font_size(13.0);
        cr.move_to(left + 30.0, top + plot_height / 2.0);
        let _ = cr.show_text("载入媒体并开始恢复后重放，波形将在这里出现");
        return;
    };
    let covered = report.waveform.covered_ms.max(1) as f64;
    let selected_interval = *ui.selected_interval.borrow();
    for (index, interval) in report.auto_conclusion.intervals.iter().enumerate() {
        let x0 = left + plot_width * interval.start_ms as f64 / covered;
        let x1 = left + plot_width * interval.end_ms as f64 / covered;
        if selected_interval == Some(index) {
            cr.set_source_rgba(0.96, 0.25, 0.38, 0.30);
        } else {
            cr.set_source_rgba(0.96, 0.25, 0.38, 0.14);
        }
        cr.rectangle(x0, top, (x1 - x0).max(1.0), plot_height);
        let _ = cr.fill();
    }
    if let Some(sample) = ui
        .selected_frame
        .borrow()
        .as_ref()
        .and_then(|index| report.waveform.samples.get(*index))
    {
        let x = left + plot_width * sample.time_ms as f64 / covered;
        cr.set_source_rgba(0.38, 0.62, 1.0, 0.75);
        cr.set_line_width(1.5);
        cr.move_to(x, top);
        cr.line_to(x, top + plot_height);
        let _ = cr.stroke();
    }

    let stride = (report.waveform.samples.len() / 2400).max(1);
    let mut first = true;
    for sample in report.waveform.samples.iter().step_by(stride) {
        let x = left + plot_width * sample.time_ms as f64 / covered;
        let y = top + plot_height * (1.0 - sample.y_mean.clamp(0.0, 1.0) as f64);
        if first {
            cr.move_to(x, y);
            first = false;
        } else {
            cr.line_to(x, y);
        }
    }
    cr.set_source_rgba(0.35, 0.88, 0.68, 0.95);
    cr.set_line_width(1.35);
    let _ = cr.stroke();

    cr.set_source_rgba(0.6, 0.66, 0.76, 0.9);
    cr.set_font_size(10.0);
    for step in 0..=4 {
        let fraction = step as f64 / 4.0;
        let x = left + plot_width * fraction;
        let label = format_timecode((covered * fraction) as u64);
        let approximate = if step == 0 {
            0.0
        } else if step == 4 {
            width - right - 72.0
        } else {
            x - 31.0
        };
        cr.move_to(approximate, height - 9.0);
        let _ = cr.show_text(&label);
    }
    cr.set_source_rgba(0.55, 0.61, 0.72, 0.9);
    cr.set_font_size(10.0);
    cr.move_to(left, 12.0);
    let _ = cr.show_text(&format!(
        "{} samples · {} ms · {} fps",
        report.waveform.samples.len(),
        report.waveform.covered_ms,
        report.detector.sample_fps
    ));
}

fn refresh_auto(ui: &Rc<Ui>, report: Option<&ReviewReport>) {
    clear_box(&ui.auto_box);
    let head = gtk::Box::new(Orientation::Horizontal, 8);
    let title = section_title("自动结论");
    head.append(&title);
    let chip = chip("AUTO · 只读", "chip-auto");
    head.append(&chip);
    ui.auto_box.append(&head);
    let Some(report) = report else {
        ui.auto_box
            .append(&muted("等待恢复后重放。自动结论只会由检测器写入。"));
        return;
    };
    let verdict = gtk::Label::new(Some(&report.auto_conclusion.headline));
    verdict.add_css_class("verdict");
    verdict.set_wrap(true);
    verdict.set_xalign(0.0);
    verdict.set_halign(gtk::Align::Start);
    ui.auto_box.append(&verdict);
    let confidence = gtk::ProgressBar::new();
    confidence.set_fraction(report.auto_conclusion.confidence as f64);
    confidence.set_show_text(true);
    confidence.set_text(Some(&format!(
        "置信度 {:.0}%",
        report.auto_conclusion.confidence * 100.0
    )));
    ui.auto_box.append(&confidence);
    let metrics = gtk::Box::new(Orientation::Horizontal, 18);
    for (value, label) in [
        (
            report.auto_conclusion.intervals.len().to_string(),
            "黑场区间",
        ),
        (
            report.auto_conclusion.analyzed_samples.to_string(),
            "采样点",
        ),
        (
            format!("{:.1}s", report.auto_conclusion.analyzed_ms as f64 / 1000.0),
            "覆盖时长",
        ),
    ] {
        let item = gtk::Box::new(Orientation::Vertical, 1);
        let value_label = gtk::Label::new(Some(&value));
        value_label.add_css_class("metric");
        value_label.set_halign(gtk::Align::Start);
        item.append(&value_label);
        item.append(&muted(label));
        metrics.append(&item);
    }
    ui.auto_box.append(&metrics);
    ui.auto_box.append(&muted(&format!(
        "判定 {:?} · 参数 {}",
        report.auto_conclusion.verdict,
        report.detector.fingerprint()
    )));
    if !report.auto_conclusion.reason_codes.is_empty() {
        add_label(
            &ui.auto_box,
            &format!(
                "原因码  {}",
                report.auto_conclusion.reason_codes.join(" · ")
            ),
            Some("mono"),
        );
    }
    for limitation in &report.auto_conclusion.limitations {
        add_label(&ui.auto_box, &format!("• {limitation}"), None);
    }
    if report.auto_conclusion.verdict == Verdict::Inconclusive {
        add_label(
            &ui.auto_box,
            "结论不确定：请先按失败/限制操作补齐采样，再做人工确认。",
            Some("banner-warn"),
        );
    }
}

fn refresh_manual(ui: &Rc<Ui>, report: Option<&ReviewReport>) {
    clear_box(&ui.manual_box);
    let head = gtk::Box::new(Orientation::Horizontal, 8);
    head.append(&section_title("人工确认"));
    head.append(&chip("MANUAL · 独立", "chip-manual"));
    ui.manual_box.append(&head);
    let Some(report) = report else {
        ui.manual_box
            .append(&muted("完成重放后可记录复核人、结论与备注。"));
        return;
    };
    add_label(
        &ui.manual_box,
        "人工确认不会改写自动结论；两者分别写入 report.json 与 review.md。",
        Some("muted"),
    );
    let status = gtk::ComboBoxText::new();
    for (id, label) in [
        ("pending", "待复核"),
        ("confirmed", "确认自动结论"),
        ("rejected", "驳回自动结论"),
        ("needs_review", "需要补充复核"),
    ] {
        status.append(Some(id), label);
    }
    status.set_active_id(Some(match report.manual_confirmation.status {
        ManualStatus::Pending => "pending",
        ManualStatus::Confirmed => "confirmed",
        ManualStatus::Rejected => "rejected",
        ManualStatus::NeedsReview => "needs_review",
    }));
    ui.manual_box.append(&status);
    let reviewer = gtk::Entry::builder()
        .placeholder_text("复核人 / 工号")
        .text(&report.manual_confirmation.reviewer)
        .build();
    ui.manual_box.append(&reviewer);
    let note = gtk::TextView::new();
    note.set_wrap_mode(gtk::WrapMode::WordChar);
    note.set_accepts_tab(false);
    note.set_size_request(-1, 74);
    note.buffer().set_text(&report.manual_confirmation.note);
    let note_scroll = gtk::ScrolledWindow::builder()
        .child(&note)
        .min_content_height(74)
        .build();
    ui.manual_box.append(&note_scroll);
    let buttons = gtk::Box::new(Orientation::Horizontal, 6);
    let confirm = gtk::Button::with_label("确认");
    let reject = gtk::Button::with_label("驳回");
    let review = gtk::Button::with_label("待复核");
    let save = gtk::Button::with_label("保存并封存");
    save.add_css_class("primary");
    buttons.append(&confirm);
    buttons.append(&reject);
    buttons.append(&review);
    buttons.append(&save);
    ui.manual_box.append(&buttons);
    let current = muted(&format!(
        "当前状态 {:?} · 复核人 {} · 时间 {}",
        report.manual_confirmation.status,
        if report.manual_confirmation.reviewer.is_empty() {
            "未填写"
        } else {
            &report.manual_confirmation.reviewer
        },
        report
            .manual_confirmation
            .reviewed_at
            .as_deref()
            .unwrap_or("未确认")
    ));
    ui.manual_box.append(&current);
    ui.manual_widgets.replace(Some(ManualWidgets {
        reviewer: reviewer.clone(),
        note: note.clone(),
    }));
    for (button, value) in [
        (&confirm, ManualStatus::Confirmed),
        (&reject, ManualStatus::Rejected),
        (&review, ManualStatus::NeedsReview),
    ] {
        let widget = Rc::clone(ui);
        let combo = status.clone();
        button.connect_clicked(move |_| {
            combo.set_active_id(Some(match value {
                ManualStatus::Confirmed => "confirmed",
                ManualStatus::Rejected => "rejected",
                ManualStatus::NeedsReview => "needs_review",
                ManualStatus::Pending => "pending",
            }));
            save_manual(&widget, value);
        });
    }
    let widget = Rc::clone(ui);
    let combo = status;
    save.connect_clicked(move |_| {
        let value = match combo.active_id().as_deref() {
            Some("confirmed") => ManualStatus::Confirmed,
            Some("rejected") => ManualStatus::Rejected,
            Some("needs_review") => ManualStatus::NeedsReview,
            _ => ManualStatus::Pending,
        };
        save_manual(&widget, value);
    });
}

fn save_manual(ui: &Rc<Ui>, status: ManualStatus) {
    let Some(widgets) = ui.manual_widgets.borrow().clone() else {
        set_banner(ui, "人工确认控件尚未就绪。", "banner-error");
        return;
    };
    let reviewer = widgets.reviewer.text().trim().to_string();
    if reviewer.is_empty() {
        set_banner(ui, "请先填写复核人，再保存人工确认。", "banner-error");
        return;
    }
    let note = widgets
        .note
        .buffer()
        .text(
            &widgets.note.buffer().start_iter(),
            &widgets.note.buffer().end_iter(),
            false,
        )
        .to_string();
    let Some(mut report) = ui.report.borrow().clone() else {
        set_banner(ui, "没有可复核的报告。", "banner-error");
        return;
    };
    report.manual_confirmation.status = status;
    report.manual_confirmation.reviewer = reviewer.clone();
    report.manual_confirmation.reviewed_at = Some(now_utc());
    report.manual_confirmation.note = note;
    report.manual_confirmation.checks = report
        .auto_conclusion
        .intervals
        .iter()
        .map(|interval| ManualCheck {
            interval_id: interval.id.clone(),
            decision: match status {
                ManualStatus::Confirmed => IntervalState::ManualConfirmed,
                ManualStatus::Rejected => IntervalState::ManualRejected,
                ManualStatus::NeedsReview => IntervalState::NeedsReview,
                ManualStatus::Pending => IntervalState::AutoDetected,
            },
            note: String::new(),
        })
        .collect();
    report.generated_at = now_utc();
    report.replay.events.push(crate::model::ReplayEvent {
        sequence: report.replay.events.len() as u64 + 1,
        at_ms: report.auto_conclusion.analyzed_ms,
        frame: None,
        track_id: report.replay.selected_track.clone(),
        kind: "manual_confirmation_recorded".into(),
        detail: format!(
            "人工状态 {:?}；复核人 {}；备注 {}",
            status,
            reviewer,
            if report.manual_confirmation.note.is_empty() {
                "无"
            } else {
                &report.manual_confirmation.note
            }
        ),
        params_fingerprint: report.detector.fingerprint(),
        waveform_sample: report
            .waveform
            .samples
            .len()
            .checked_sub(1)
            .map(|value| value as u64),
        evidence_refs: Vec::new(),
    });
    let keyframes = ui.keyframes.borrow().clone();
    match evidence::write_bundle(&ui.evidence_root, report, &keyframes) {
        Ok(bundle) => {
            ui.report.replace(Some(bundle.report));
            refresh_all(ui);
            set_banner(
                ui,
                &format!(
                    "人工确认已独立封存：{}。自动结论未被改写。",
                    bundle.root.display()
                ),
                "banner-ok",
            );
            ui.status_label.set_text("人工确认已写入新证据包");
        }
        Err(error) => {
            let failure = AnalysisFailure::output_failure(
                &ui.evidence_root.display().to_string(),
                error.to_string(),
            );
            set_banner(ui, &format!("人工确认封存失败：{error}"), "banner-error");
            show_failure(ui, &failure);
        }
    }
}

fn refresh_evidence(ui: &Rc<Ui>, report: Option<&ReviewReport>) {
    clear_box(&ui.evidence_box);
    ui.evidence_box.append(&section_title("证据封存与兼容性"));
    let Some(report) = report else {
        ui.evidence_box.append(&muted(
            "重放完成后自动写入关键帧、波形、事件日志与封存清单。",
        ));
        return;
    };
    add_label(
        &ui.evidence_box,
        &format!(
            "格式  {} → {}{}",
            report.compatibility.input_schema,
            report.compatibility.output_schema,
            if report.compatibility.migrated {
                " · 已迁移"
            } else {
                ""
            }
        ),
        Some("mono"),
    );
    if report.compatibility.migrated {
        for warning in &report.compatibility.warnings {
            add_label(&ui.evidence_box, &format!("• {warning}"), Some("muted"));
        }
    }
    add_label(
        &ui.evidence_box,
        &format!("扩展策略  {}", report.source.extension.policy),
        Some("muted"),
    );
    add_label(
        &ui.evidence_box,
        &format!("内容探测  {}", report.source.probe_caps),
        Some("mono"),
    );
    if report.evidence.is_empty() {
        add_label(
            &ui.evidence_box,
            "尚未封存；点击“封存当前复核”生成证据包。",
            Some("chip-warn"),
        );
    } else {
        add_label(
            &ui.evidence_box,
            &format!(
                "已封存 {} 个证据文件（manifest 另含 report.json）",
                report.evidence.len()
            ),
            Some("muted"),
        );
        for evidence in &report.evidence {
            let row = gtk::Box::new(Orientation::Vertical, 2);
            row.add_css_class("evidence-row");
            let name = gtk::Label::new(Some(&evidence.relative_path));
            name.set_halign(gtk::Align::Start);
            name.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            name.add_css_class("mono");
            row.append(&name);
            let detail = gtk::Label::new(Some(&format!(
                "{:?} · {} bytes · {}",
                evidence.kind,
                evidence.bytes,
                short_hash(&evidence.sha256)
            )));
            detail.set_halign(gtk::Align::Start);
            detail.add_css_class("muted");
            detail.add_css_class("tiny");
            row.append(&detail);
            ui.evidence_box.append(&row);
        }
    }
    let buttons = gtk::Box::new(Orientation::Horizontal, 6);
    let open = gtk::Button::with_label("打开证据目录");
    let reseal = gtk::Button::with_label("重新封存");
    buttons.append(&open);
    buttons.append(&reseal);
    ui.evidence_box.append(&buttons);
    let widget = Rc::clone(ui);
    open.connect_clicked(move |_| {
        let path = widget.evidence_root.clone();
        let _ = std::fs::create_dir_all(&path);
        open_path(&path);
    });
    let widget = Rc::clone(ui);
    reseal.connect_clicked(move |_| seal_current(&widget));
}

fn show_failure(ui: &Rc<Ui>, failure: &AnalysisFailure) {
    clear_box(&ui.failure_box);
    let head = gtk::Box::new(Orientation::Horizontal, 8);
    head.append(&section_title("失败 · 可执行后续操作"));
    head.append(&chip(
        if failure.code.is_empty() {
            "ERROR"
        } else {
            &failure.code
        },
        "chip-warn",
    ));
    ui.failure_box.append(&head);
    add_label(&ui.failure_box, &failure.title, Some("panel-title"));
    add_label(&ui.failure_box, &failure.detail, None);
    add_label(
        &ui.failure_box,
        if failure.retryable {
            "此失败可重试；以下操作按顺序执行。"
        } else {
            "此失败不可安全重试，先恢复源文件再继续。"
        },
        Some("muted"),
    );
    for (index, action) in failure.actions.iter().enumerate() {
        let item = gtk::Box::new(Orientation::Vertical, 4);
        item.add_css_class("evidence-row");
        add_label(
            &item,
            &format!("{}. {}", index + 1, action.label),
            Some("panel-title"),
        );
        add_label(&item, &action.detail, Some("muted"));
        if let Some(command) = &action.command {
            add_label(&item, command, Some("mono"));
        }
        let row = gtk::Box::new(Orientation::Horizontal, 6);
        match action.kind {
            ActionKind::CopyCommand => {
                if let Some(command) = &action.command {
                    let button = gtk::Button::with_label("复制命令");
                    let clipboard = ui.window.clipboard();
                    let command = command.clone();
                    button.connect_clicked(move |_| {
                        clipboard.set_text(&command);
                    });
                    row.append(&button);
                }
            }
            ActionKind::ChooseFile => {
                let button = gtk::Button::with_label("重新选择文件");
                let widget = Rc::clone(ui);
                button.connect_clicked(move |_| choose_media(&widget));
                row.append(&button);
            }
            ActionKind::Retry => {
                let button = gtk::Button::with_label("重试当前重放");
                let widget = Rc::clone(ui);
                button.connect_clicked(move |_| start_analysis(&widget));
                row.append(&button);
            }
            ActionKind::OpenPath => {
                if let Some(path) = &action.target_path {
                    let button = gtk::Button::with_label("打开位置");
                    let path = PathBuf::from(path);
                    button.connect_clicked(move |_| open_path(&path));
                    row.append(&button);
                }
            }
            ActionKind::Guidance => {}
        }
        if let Some(target) = &action.target_path {
            let label = gtk::Label::new(Some(target));
            label.add_css_class("mono");
            label.add_css_class("muted");
            label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            row.append(&label);
        }
        if row.first_child().is_some() {
            item.append(&row);
        }
        ui.failure_box.append(&item);
    }
    ui.failure_revealer.set_reveal_child(true);
}

fn failure_to_analysis(failure: &FailureRecord) -> AnalysisFailure {
    AnalysisFailure {
        code: failure.code.clone(),
        title: failure.title.clone(),
        detail: if failure.evidence_retained {
            format!("{}（证据已保留）", failure.detail)
        } else {
            failure.detail.clone()
        },
        retryable: failure.retryable,
        actions: failure.actions.clone(),
    }
}

fn set_banner(ui: &Rc<Ui>, text: &str, class: &str) {
    for name in ["banner", "banner-ok", "banner-warn", "banner-error"] {
        ui.banner.remove_css_class(name);
    }
    ui.banner.add_css_class("banner");
    ui.banner.add_css_class(class);
    ui.banner.set_text(text);
    ui.banner.set_visible(true);
}

fn open_path(path: &std::path::Path) {
    let _ = std::process::Command::new("xdg-open").arg(path).spawn();
}
