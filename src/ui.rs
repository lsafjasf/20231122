use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use gtk::prelude::*;
use gtk4 as gtk;

use crate::evidence::read_journal;
use crate::mask::{MaskConfig, MaskShape};
use crate::media::probe_media;
use crate::model::{MediaInfo, ProjectSession, RunStatus};
use crate::player::{FrameMailbox, PreviewPlayer};
use crate::project::{
    create_project, default_projects_root, load_last_project, open_project, save_session,
    IntegrityReport, OpenedProject,
};
use crate::render::RenderOutcome;
use crate::service::{begin_run, compare_latest_runs, export_segment, finish_run, record_export};
use crate::timecode::{format_timecode, frame_duration_ns, parse_timecode};

type SharedState = Rc<RefCell<AppState>>;
type WeakState = Weak<RefCell<AppState>>;

thread_local! {
    static STATE_KEEPALIVE: RefCell<Option<SharedState>> = const { RefCell::new(None) };
}

enum WorkerMsg {
    ProjectOpened {
        result: Box<Result<OpenedProject, String>>,
    },
    RunFinished {
        segment_index: usize,
        run_id: String,
        result: Result<RenderOutcome, String>,
    },
    ExportFinished {
        segment_index: usize,
        result: Result<RenderOutcome, String>,
    },
}

struct AppState {
    project: Option<ProjectSession>,
    integrity: Option<IntegrityReport>,
    selected_segment: usize,
    player: Option<PreviewPlayer>,
    mask: Arc<RwLock<MaskConfig>>,
    mailbox: Arc<FrameMailbox>,
    ui: UiRefs,
    syncing: Rc<Cell<bool>>,
    worker_tx: Sender<WorkerMsg>,
    worker_rx: Receiver<WorkerMsg>,
    dirty: bool,
    tick_count: u64,
    last_saved_position_ns: u64,
    preview_end_ns: Option<u64>,
}

struct UiRefs {
    window: gtk::ApplicationWindow,
    new_button: gtk::Button,
    open_button: gtk::Button,
    evidence_button: gtk::Button,
    folder_button: gtk::Button,
    stack: gtk::Stack,
    notebook: gtk::Notebook,
    picture: gtk::Picture,
    timeline: gtk::Scale,
    time_label: gtk::Label,
    play_button: gtk::Button,
    meta_label: gtk::Label,
    integrity_label: gtk::Label,
    status_label: gtk::Label,
    mask_panel: MaskPanel,
    segments: Vec<SegmentPage>,
}

struct SegmentPage {
    anchor_entry: gtk::Entry,
    offset_in: gtk::SpinButton,
    offset_out: gtk::SpinButton,
    anchor_label: gtk::Label,
    range_label: gtk::Label,
    run_list: gtk::ListBox,
    run_summary: gtk::Label,
    run_button: gtk::Button,
    compare_button: gtk::Button,
    export_button: gtk::Button,
}

struct MaskPanel {
    root: gtk::Box,
    shape: gtk::DropDown,
    center_x: gtk::Scale,
    center_y: gtk::Scale,
    radius_x: gtk::Scale,
    radius_y: gtk::Scale,
    feather: gtk::SpinButton,
    expand: gtk::SpinButton,
    darkness: gtk::Scale,
    summary: gtk::Label,
}

const CSS: &str = r#"
.app-title { font-weight: 800; font-size: 18px; }
.welcome-title { font-weight: 800; font-size: 30px; }
.eyebrow { font-size: 11px; font-weight: 700; letter-spacing: 1.2px; opacity: .62; }
.mono { font-family: monospace; }
.timecode { font-family: monospace; font-size: 22px; font-weight: 700; }
.offset { font-family: monospace; font-size: 14px; }
.video-stage { background: #090b0e; border-radius: 12px; }
.integrity-ok { color: #77d39a; }
.integrity-bad { color: #ff7d7d; font-weight: 700; }
.section-title { font-weight: 700; font-size: 15px; margin-top: 4px; }
.segment-card { background: alpha(currentColor, .035); border-radius: 10px; }
.run-row { padding: 4px 8px; }
.status { font-size: 12px; opacity: .78; }
.status-error { color: #ff7d7d; font-weight: 700; opacity: 1; }
.dim { opacity: .62; }
"#;

pub fn launch(initial_project: Option<PathBuf>) -> anyhow::Result<()> {
    let app = gtk::Application::builder()
        .application_id(crate::APP_ID)
        .build();
    app.connect_startup(|_| {
        load_css();
    });
    app.connect_activate(move |app| {
        build_app(app, initial_project.clone());
    });
    let _ = app.run_with_args::<&str>(&[]);
    Ok(())
}

fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_data(CSS);
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

fn build_app(app: &gtk::Application, initial_project: Option<PathBuf>) {
    let (worker_tx, worker_rx) = mpsc::channel();
    let shared: SharedState = Rc::new_cyclic(|weak| {
        let ui = build_ui(app, weak.clone(), worker_tx.clone());
        RefCell::new(AppState {
            project: None,
            integrity: None,
            selected_segment: 0,
            player: None,
            mask: Arc::new(RwLock::new(MaskConfig::default())),
            mailbox: Arc::new(FrameMailbox::default()),
            ui,
            syncing: Rc::new(Cell::new(false)),
            worker_tx,
            worker_rx,
            dirty: false,
            tick_count: 0,
            last_saved_position_ns: 0,
            preview_end_ns: None,
        })
    });
    STATE_KEEPALIVE.with(|slot| {
        *slot.borrow_mut() = Some(Rc::clone(&shared));
    });
    app.connect_shutdown(|_| {
        STATE_KEEPALIVE.with(|slot| {
            *slot.borrow_mut() = None;
        });
    });

    {
        let state = shared.borrow();
        install_window_actions(&state.ui, Rc::downgrade(&shared));
        state.ui.window.present();
    }

    let initial = initial_project.or_else(load_last_project);
    if let Some(path) = initial {
        spawn_open_project(Rc::downgrade(&shared), path);
    } else {
        if let Ok(mut state) = shared.try_borrow_mut() {
            state.set_status("新建项目或打开已有审查项目", false);
        }
    }
}

fn with_state<F: FnOnce(&mut AppState)>(weak: &WeakState, action: F) {
    if let Some(shared) = weak.upgrade() {
        if let Ok(mut state) = shared.try_borrow_mut() {
            action(&mut state);
        }
    }
}

fn section_label(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.add_css_class("section-title");
    label
}

fn dim_label(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_wrap(true);
    label.add_css_class("dim");
    label
}

fn button(text: &str, suggested: bool) -> gtk::Button {
    let button = gtk::Button::with_label(text);
    if suggested {
        button.add_css_class("suggested-action");
    }
    button
}

fn horizontal_spin(min: f64, max: f64, step: f64, value: f64) -> gtk::SpinButton {
    let spin = gtk::SpinButton::with_range(min, max, step);
    spin.set_digits(0);
    spin.set_numeric(true);
    spin.set_value(value);
    spin
}

fn percent_scale(value: f64) -> gtk::Scale {
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 0.1);
    scale.set_digits(1);
    scale.set_draw_value(true);
    scale.set_value_pos(gtk::PositionType::Right);
    scale.set_hexpand(true);
    scale.set_value(value);
    scale
}

fn build_ui(app: &gtk::Application, weak: WeakState, worker_tx: Sender<WorkerMsg>) -> UiRefs {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title(format!("{} · 三段锚点工作台", crate::APP_NAME))
        .default_width(1440)
        .default_height(900)
        .build();

    let header = gtk::HeaderBar::new();
    let title = gtk::Label::new(Some(crate::APP_NAME));
    title.add_css_class("app-title");
    header.set_title_widget(Some(&title));
    let new_button = button("新建", false);
    let open_button = button("打开", false);
    let evidence_button = button("证据日志", false);
    let folder_button = button("项目目录", false);
    header.pack_start(&new_button);
    header.pack_start(&open_button);
    header.pack_end(&folder_button);
    header.pack_end(&evidence_button);
    window.set_titlebar(Some(&header));

    let stack = gtk::Stack::new();
    stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    stack.add_named(&build_welcome(&weak), Some("welcome"));

    let workspace = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let integrity_label = gtk::Label::new(Some("尚未打开项目"));
    integrity_label.set_xalign(0.0);
    integrity_label.set_margin_top(8);
    integrity_label.set_margin_start(12);
    integrity_label.set_margin_end(12);
    integrity_label.set_wrap(true);
    integrity_label.add_css_class("status");
    workspace.append(&integrity_label);

    let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
    paned.set_position(840);
    paned.set_wide_handle(true);
    paned.set_hexpand(true);
    paned.set_vexpand(true);
    let (preview, picture, timeline, time_label, play_button, meta_label) = build_preview(&weak);
    let (right, notebook, mask_panel, segments) = build_right_pane(&weak, &worker_tx, &window);
    paned.set_start_child(Some(&preview));
    paned.set_end_child(Some(&right));
    paned.set_resize_start_child(true);
    paned.set_resize_end_child(false);
    workspace.append(&paned);

    let status_label = gtk::Label::new(Some("就绪"));
    status_label.set_xalign(0.0);
    status_label.set_margin_top(6);
    status_label.set_margin_bottom(8);
    status_label.set_margin_start(12);
    status_label.set_margin_end(12);
    status_label.add_css_class("status");
    workspace.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    workspace.append(&status_label);
    stack.add_named(&workspace, Some("workspace"));
    stack.set_visible_child_name("welcome");
    window.set_child(Some(&stack));

    schedule_tick(weak.clone());
    UiRefs {
        window,
        new_button,
        open_button,
        evidence_button,
        folder_button,
        stack,
        notebook,
        picture,
        timeline,
        time_label,
        play_button,
        meta_label,
        integrity_label,
        status_label,
        mask_panel,
        segments,
    }
}

fn build_welcome(weak: &WeakState) -> gtk::Widget {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 18);
    root.set_valign(gtk::Align::Center);
    root.set_halign(gtk::Align::Center);
    root.set_margin_top(40);
    root.set_margin_bottom(40);
    root.set_margin_start(40);
    root.set_margin_end(40);

    let eyebrow = gtk::Label::new(Some("LOCAL VIDEO MASK REVIEW"));
    eyebrow.add_css_class("eyebrow");
    let title = gtk::Label::new(Some("视频遮罩边缘审查"));
    title.add_css_class("welcome-title");
    let subtitle = gtk::Label::new(Some(
        "三段锚点、独立重跑、双版本比较与导出。\n原始素材只读保全，会话与证据可中途退出后恢复。",
    ));
    subtitle.set_justify(gtk::Justification::Center);
    subtitle.add_css_class("dim");
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    actions.set_halign(gtk::Align::Center);
    let new_button = button("新建审查", true);
    let open_button = button("打开已有项目", false);
    actions.append(&new_button);
    actions.append(&open_button);
    root.append(&eyebrow);
    root.append(&title);
    root.append(&subtitle);
    root.append(&actions);

    let features = gtk::Label::new(Some(
        "① 三个锚点各自保留原始偏移    ② 每段独立 VP8/WebM 导出    ③ 锚点/入点/出点 PNG 证据并校验 SHA-256",
    ));
    features.set_wrap(true);
    features.set_justify(gtk::Justification::Center);
    features.add_css_class("mono");
    features.add_css_class("dim");
    root.append(&features);

    {
        let weak = weak.clone();
        new_button.connect_clicked(move |_| choose_source_video(&weak));
    }
    {
        let weak = weak.clone();
        open_button.connect_clicked(move |_| choose_project_folder(&weak));
    }
    root.upcast()
}

fn build_preview(
    weak: &WeakState,
) -> (
    gtk::Widget,
    gtk::Picture,
    gtk::Scale,
    gtk::Label,
    gtk::Button,
    gtk::Label,
) {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
    root.set_margin_top(10);
    root.set_margin_bottom(10);
    root.set_margin_start(12);
    root.set_margin_end(8);

    let stage_header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let stage_title = section_label("画面审查");
    stage_title.set_hexpand(true);
    let meta_label = gtk::Label::new(Some("未载入素材"));
    meta_label.set_xalign(1.0);
    meta_label.add_css_class("status");
    stage_header.append(&stage_title);
    stage_header.append(&meta_label);
    root.append(&stage_header);

    let picture = gtk::Picture::new();
    picture.set_content_fit(gtk::ContentFit::Contain);
    picture.set_can_shrink(true);
    picture.set_hexpand(true);
    picture.set_vexpand(true);
    picture.set_size_request(-1, 360);
    picture.add_css_class("video-stage");
    root.append(&picture);

    let timeline = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 1.0);
    timeline.set_draw_value(false);
    timeline.set_hexpand(true);
    timeline.set_sensitive(false);
    {
        let weak = weak.clone();
        timeline.connect_change_value(move |_, _, value| {
            with_state(&weak, |state| {
                state.jump_to(value.max(0.0) as u64);
            });
            gtk::glib::Propagation::Proceed
        });
    }
    root.append(&timeline);

    let transport = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let back_button = button("−1 帧", false);
    let play_button = button("播放", true);
    let forward_button = button("+1 帧", false);
    let time_label = gtk::Label::new(Some("00:00:00.000 / 00:00:00.000"));
    time_label.set_xalign(1.0);
    time_label.set_hexpand(true);
    time_label.add_css_class("mono");
    transport.append(&back_button);
    transport.append(&play_button);
    transport.append(&forward_button);
    transport.append(&time_label);
    root.append(&transport);

    let hint =
        dim_label("预览与导出均由 GStreamer 烧录同一遮罩参数；入点/出点按原片时间轴精确定位。");
    root.append(&hint);

    {
        let weak = weak.clone();
        play_button.connect_clicked(move |_| {
            with_state(&weak, |state| state.toggle_playback());
        });
    }
    {
        let weak = weak.clone();
        back_button.connect_clicked(move |_| {
            with_state(&weak, |state| state.step_frames(-1));
        });
    }
    {
        let weak = weak.clone();
        forward_button.connect_clicked(move |_| {
            with_state(&weak, |state| state.step_frames(1));
        });
    }

    (
        root.upcast(),
        picture,
        timeline,
        time_label,
        play_button,
        meta_label,
    )
}

fn build_right_pane(
    weak: &WeakState,
    worker_tx: &Sender<WorkerMsg>,
    window: &gtk::ApplicationWindow,
) -> (gtk::Widget, gtk::Notebook, MaskPanel, Vec<SegmentPage>) {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
    root.set_margin_top(10);
    root.set_margin_bottom(10);
    root.set_margin_start(8);
    root.set_margin_end(12);
    root.set_size_request(500, -1);

    let notebook = gtk::Notebook::new();
    notebook.set_vexpand(true);
    let mut segments = Vec::new();
    for index in 0..crate::model::SEGMENT_COUNT {
        let (page, refs) = build_segment_page(index, weak, worker_tx, window);
        let tab = gtk::Label::new(Some(&format!("片段 {}", index + 1)));
        notebook.append_page(&page, Some(&tab));
        segments.push(refs);
    }
    {
        let weak = weak.clone();
        notebook.connect_switch_page(move |_, _, page| {
            with_state(&weak, |state| state.select_segment(page as usize));
        });
    }
    root.append(&notebook);

    let mask_panel = build_mask_panel(weak);
    root.append(&mask_panel.root);
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroll.set_child(Some(&root));
    scroll.set_hexpand(false);
    scroll.set_vexpand(true);
    (scroll.upcast(), notebook, mask_panel, segments)
}

fn build_segment_page(
    index: usize,
    weak: &WeakState,
    worker_tx: &Sender<WorkerMsg>,
    window: &gtk::ApplicationWindow,
) -> (gtk::Widget, SegmentPage) {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 9);
    root.add_css_class("segment-card");
    root.set_margin_top(8);
    root.set_margin_bottom(8);
    root.set_margin_start(6);
    root.set_margin_end(6);

    let heading = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let title = section_label(&format!("片段 {} · 锚点", index + 1));
    title.set_hexpand(true);
    let anchor_label = gtk::Label::new(Some("--:--:--.---"));
    anchor_label.add_css_class("timecode");
    heading.append(&title);
    heading.append(&anchor_label);
    root.append(&heading);

    let anchor_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let anchor_entry = gtk::Entry::new();
    anchor_entry.set_placeholder_text(Some("HH:MM:SS.mmm"));
    anchor_entry.set_width_chars(14);
    anchor_entry.set_hexpand(true);
    let set_anchor = button("设为播放头", false);
    let jump_anchor = button("跳到锚点", false);
    anchor_row.append(&anchor_entry);
    anchor_row.append(&set_anchor);
    anchor_row.append(&jump_anchor);
    root.append(&anchor_row);

    let offsets = gtk::Grid::new();
    offsets.set_column_spacing(8);
    offsets.set_row_spacing(4);
    let in_label = gtk::Label::new(Some("入偏移 ms"));
    in_label.set_xalign(0.0);
    let out_label = gtk::Label::new(Some("出偏移 ms"));
    out_label.set_xalign(0.0);
    let offset_in = horizontal_spin(-600_000.0, 600_000.0, 10.0, -1_000.0);
    let offset_out = horizontal_spin(-600_000.0, 600_000.0, 10.0, 1_000.0);
    offset_in.set_hexpand(true);
    offset_out.set_hexpand(true);
    offsets.attach(&in_label, 0, 0, 1, 1);
    offsets.attach(&out_label, 1, 0, 1, 1);
    offsets.attach(&offset_in, 0, 1, 1, 1);
    offsets.attach(&offset_out, 1, 1, 1, 1);
    root.append(&offsets);

    let range_label = gtk::Label::new(Some("入点 -- → 出点 --"));
    range_label.set_xalign(0.0);
    range_label.set_selectable(true);
    range_label.set_wrap(true);
    range_label.add_css_class("mono");
    root.append(&range_label);
    let offset_note = gtk::Label::new(Some("原始偏移 -- / --"));
    offset_note.set_xalign(0.0);
    offset_note.add_css_class("status");
    root.append(&offset_note);

    let jumps = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let jump_in = button("跳到入点", false);
    let jump_out = button("跳到出点", false);
    let preview_range = button("预览本段", false);
    jumps.append(&jump_in);
    jumps.append(&jump_out);
    jumps.append(&preview_range);
    root.append(&jumps);

    let run_summary = gtk::Label::new(Some("尚无运行记录"));
    run_summary.set_xalign(0.0);
    run_summary.set_wrap(true);
    run_summary.add_css_class("status");
    root.append(&run_summary);

    let run_list = gtk::ListBox::new();
    run_list.set_selection_mode(gtk::SelectionMode::None);
    run_list.add_css_class("boxed-list");
    let run_scroll = gtk::ScrolledWindow::new();
    run_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    run_scroll.set_min_content_height(150);
    run_scroll.set_child(Some(&run_list));
    root.append(&run_scroll);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let run_button = button("重跑本段", true);
    let compare_button = button("比较最近两次", false);
    let export_button = button("导出 WebM", false);
    actions.append(&run_button);
    actions.append(&compare_button);
    actions.append(&export_button);
    root.append(&actions);
    root.append(&dim_label(
        "重跑会从只读原片重新渲染，并保存锚点/入点/出点证据帧。",
    ));

    {
        let weak = weak.clone();
        let entry = anchor_entry.clone();
        anchor_entry.connect_activate(move |_| {
            let value = entry.text().to_string();
            with_state(&weak, |state| state.apply_anchor_text(index, &value));
        });
    }
    {
        let weak = weak.clone();
        set_anchor.connect_clicked(move |_| {
            with_state(&weak, |state| state.set_anchor_from_playhead(index));
        });
    }
    {
        let weak = weak.clone();
        jump_anchor.connect_clicked(move |_| {
            with_state(&weak, |state| state.jump_to_segment_anchor(index));
        });
    }
    {
        let weak = weak.clone();
        let spin = offset_in.clone();
        offset_in.connect_value_changed(move |_| {
            with_state(&weak, |state| {
                state.update_offset(index, true, spin.value() as i64)
            });
        });
    }
    {
        let weak = weak.clone();
        let spin = offset_out.clone();
        offset_out.connect_value_changed(move |_| {
            with_state(&weak, |state| {
                state.update_offset(index, false, spin.value() as i64)
            });
        });
    }
    {
        let weak = weak.clone();
        jump_in.connect_clicked(move |_| {
            with_state(&weak, |state| state.jump_to_segment_edge(index, true));
        });
    }
    {
        let weak = weak.clone();
        jump_out.connect_clicked(move |_| {
            with_state(&weak, |state| state.jump_to_segment_edge(index, false));
        });
    }
    {
        let weak = weak.clone();
        preview_range.connect_clicked(move |_| {
            with_state(&weak, |state| state.preview_segment(index));
        });
    }
    {
        let weak = weak.clone();
        let tx = worker_tx.clone();
        run_button.connect_clicked(move |_| start_run_async(&weak, &tx, index));
    }
    {
        let weak = weak.clone();
        compare_button.connect_clicked(move |_| {
            with_state(&weak, |state| state.show_compare_dialog(index));
        });
    }
    {
        let weak = weak.clone();
        let window = window.clone();
        export_button.connect_clicked(move |_| {
            choose_export_file(&weak, &window, index);
        });
    }

    let page = gtk::ScrolledWindow::new();
    page.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    page.set_child(Some(&root));
    (
        page.upcast(),
        SegmentPage {
            anchor_entry,
            offset_in,
            offset_out,
            anchor_label,
            range_label,
            run_list,
            run_summary,
            run_button,
            compare_button,
            export_button,
        },
    )
}

fn build_mask_panel(weak: &WeakState) -> MaskPanel {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    root.add_css_class("segment-card");
    root.set_margin_top(6);
    root.set_margin_bottom(8);
    root.set_margin_start(6);
    root.set_margin_end(6);
    root.set_sensitive(false);
    let heading = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let title = section_label("当前片段遮罩边缘");
    title.set_hexpand(true);
    let summary = gtk::Label::new(Some("--"));
    summary.set_xalign(1.0);
    summary.add_css_class("status");
    summary.set_ellipsize(gtk::pango::EllipsizeMode::End);
    summary.set_max_width_chars(42);
    heading.append(&title);
    heading.append(&summary);
    root.append(&heading);

    let shape_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let shape_label = gtk::Label::new(Some("形状"));
    shape_label.set_xalign(0.0);
    let shape = gtk::DropDown::from_strings(&["椭圆", "矩形"]);
    shape.set_hexpand(true);
    shape_row.append(&shape_label);
    shape_row.append(&shape);
    root.append(&shape_row);

    let grid = gtk::Grid::new();
    grid.set_column_spacing(8);
    grid.set_row_spacing(5);
    let center_x = percent_scale(50.0);
    let center_y = percent_scale(50.0);
    let radius_x = percent_scale(24.0);
    let radius_y = percent_scale(34.0);
    let darkness = percent_scale(58.0);
    let feather = horizontal_spin(0.0, 120.0, 1.0, 18.0);
    let expand = horizontal_spin(-80.0, 80.0, 1.0, 0.0);
    for (row, (label, widget)) in [
        ("中心 X %", center_x.clone().upcast::<gtk::Widget>()),
        ("中心 Y %", center_y.clone().upcast::<gtk::Widget>()),
        ("半径 X %", radius_x.clone().upcast::<gtk::Widget>()),
        ("半径 Y %", radius_y.clone().upcast::<gtk::Widget>()),
        ("外区压暗 %", darkness.clone().upcast::<gtk::Widget>()),
        ("羽化 px", feather.clone().upcast::<gtk::Widget>()),
        ("边缘位移 px", expand.clone().upcast::<gtk::Widget>()),
    ]
    .into_iter()
    .enumerate()
    {
        let label = gtk::Label::new(Some(label));
        label.set_xalign(0.0);
        grid.attach(&label, 0, row as i32, 1, 1);
        grid.attach(&widget, 1, row as i32, 1, 1);
    }
    root.append(&grid);

    {
        let weak = weak.clone();
        shape.connect_selected_notify(move |dropdown| {
            if dropdown.selected() == 1 {
                with_state(&weak, |state| {
                    state.update_selected_mask(|mask| mask.shape = MaskShape::Rectangle)
                });
            } else {
                with_state(&weak, |state| {
                    state.update_selected_mask(|mask| mask.shape = MaskShape::Ellipse)
                });
            }
        });
    }
    bind_mask_scale(&center_x, weak, |mask, value| mask.center_x = value / 100.0);
    bind_mask_scale(&center_y, weak, |mask, value| mask.center_y = value / 100.0);
    bind_mask_scale(&radius_x, weak, |mask, value| mask.radius_x = value / 100.0);
    bind_mask_scale(&radius_y, weak, |mask, value| mask.radius_y = value / 100.0);
    bind_mask_scale(&darkness, weak, |mask, value| {
        mask.outside_darkness = value / 100.0
    });
    {
        let weak = weak.clone();
        let spin = feather.clone();
        feather.connect_value_changed(move |_| {
            with_state(&weak, |state| {
                state.update_selected_mask(|mask| mask.feather_px = spin.value())
            });
        });
    }
    {
        let weak = weak.clone();
        let spin = expand.clone();
        expand.connect_value_changed(move |_| {
            with_state(&weak, |state| {
                state.update_selected_mask(|mask| mask.expand_px = spin.value())
            });
        });
    }

    MaskPanel {
        root,
        shape,
        center_x,
        center_y,
        radius_x,
        radius_y,
        feather,
        expand,
        darkness,
        summary,
    }
}

fn bind_mask_scale<F>(scale: &gtk::Scale, weak: &WeakState, apply: F)
where
    F: Fn(&mut MaskConfig, f64) + 'static,
{
    let weak = weak.clone();
    scale.connect_value_changed(move |scale| {
        with_state(&weak, |state| {
            state.update_selected_mask(|mask| apply(mask, scale.value()))
        });
    });
}

impl AppState {
    fn set_status(&mut self, message: impl Into<String>, error: bool) {
        self.ui.status_label.set_text(&message.into());
        self.ui.status_label.remove_css_class("status-error");
        if error {
            self.ui.status_label.add_css_class("status-error");
        }
    }

    fn install_project(&mut self, opened: OpenedProject) {
        self.player = None;
        let review_position = opened
            .session
            .review_position_ns
            .min(opened.session.media.duration_ns.saturating_sub(1));
        let selected = opened
            .session
            .selected_segment
            .min(crate::model::SEGMENT_COUNT - 1);
        let mask = opened.session.segments[selected].mask.clone().sanitized();
        if let Ok(mut handle) = self.mask.write() {
            *handle = mask;
        }
        self.selected_segment = selected;
        self.project = Some(opened.session);
        self.integrity = Some(opened.integrity);
        self.preview_end_ns = None;
        self.dirty = false;
        self.last_saved_position_ns = review_position;

        if let Some(project) = &self.project {
            let source = project
                .project_dir
                .join(&project.original.stored_relative_path);
            match PreviewPlayer::open(
                &source,
                project.media.duration_ns,
                Arc::clone(&self.mask),
                Arc::clone(&self.mailbox),
            ) {
                Ok(player) => {
                    let _ = player.seek_ns(review_position);
                    self.player = Some(player);
                }
                Err(error) => {
                    self.set_status(format!("预览初始化失败：{error:#}"), true);
                }
            }
        }
        self.ui.stack.set_visible_child_name("workspace");
        self.ui.notebook.set_current_page(Some(selected as u32));
        self.refresh_all();
        if self
            .integrity
            .as_ref()
            .is_some_and(|report| report.verified)
        {
            self.set_status("会话已恢复，原始素材校验通过", false);
        } else {
            self.set_status("原始素材校验失败：已禁用重跑与导出", true);
        }
    }

    fn refresh_all(&mut self) {
        self.syncing.set(true);
        let Some(project) = self.project.as_ref() else {
            self.syncing.set(false);
            return;
        };
        let duration = project.media.duration_ns;
        let integrity_ok = self
            .integrity
            .as_ref()
            .is_some_and(|report| report.verified);
        self.ui.meta_label.set_text(&format!(
            "{} × {} · {:.3} fps · {} · {}",
            project.media.width,
            project.media.height,
            project.media.fps_num as f64 / project.media.fps_den.max(1) as f64,
            project.media.video_codec,
            project.media.container
        ));
        self.ui.timeline.set_range(0.0, duration.max(1) as f64);
        self.ui.timeline.set_sensitive(true);
        self.ui
            .timeline
            .set_value(project.review_position_ns as f64);
        self.ui.time_label.set_text(&format!(
            "{} / {}",
            format_timecode(project.review_position_ns),
            format_timecode(duration)
        ));
        self.ui
            .window
            .set_title(Some(&format!("{} · {}", crate::APP_NAME, project.title)));
        if let Some(report) = &self.integrity {
            self.ui.integrity_label.set_text(&format!(
                "原始副本：{}  ·  {}",
                report.message, report.stored_path
            ));
            self.ui.integrity_label.remove_css_class("integrity-ok");
            self.ui.integrity_label.remove_css_class("integrity-bad");
            self.ui.integrity_label.add_css_class(if report.verified {
                "integrity-ok"
            } else {
                "integrity-bad"
            });
        }
        for index in 0..crate::model::SEGMENT_COUNT {
            self.refresh_segment(index, integrity_ok);
        }
        self.refresh_mask_panel(integrity_ok);
        self.syncing.set(false);
    }

    fn refresh_segment(&self, index: usize, integrity_ok: bool) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let Some(segment) = project.segments.get(index) else {
            return;
        };
        let Some(view) = self.ui.segments.get(index) else {
            return;
        };
        view.anchor_entry
            .set_text(&format_timecode(segment.anchor_ns));
        view.anchor_label
            .set_text(&format_timecode(segment.anchor_ns));
        view.offset_in
            .set_value((segment.in_offset_ns / 1_000_000) as f64);
        view.offset_out
            .set_value((segment.out_offset_ns / 1_000_000) as f64);
        match segment.range(project.media.duration_ns) {
            Ok(range) => {
                view.range_label.set_text(&format!(
                    "入点 {}  [{} ns]\n出点 {}  [{} ns]",
                    range.start_timecode(),
                    range.start_ns,
                    range.end_timecode(),
                    range.end_ns
                ));
                view.range_label.remove_css_class("integrity-bad");
            }
            Err(error) => {
                view.range_label.set_text(&error);
                view.range_label.add_css_class("integrity-bad");
            }
        }
        view.run_summary.set_text(&run_summary(segment));
        clear_list(&view.run_list);
        for run in segment.runs.iter().rev().take(8) {
            let text = format!(
                "第 {} 次 · {} · {} → {} · 证据 {} 帧",
                run.number,
                run.status.label(),
                format_timecode(run.range_start_ns),
                format_timecode(run.range_end_ns),
                run.evidence.len()
            );
            let row = gtk::Label::new(Some(&text));
            row.set_xalign(0.0);
            row.set_selectable(true);
            row.add_css_class("run-row");
            row.add_css_class("mono");
            if let Some(error) = &run.error {
                row.set_tooltip_text(Some(error));
            } else if let Some(path) = &run.output_relative_path {
                row.set_tooltip_text(Some(path));
            }
            view.run_list.append(&row);
        }
        let busy = segment
            .runs
            .iter()
            .any(|run| run.status == RunStatus::Running);
        view.run_button.set_sensitive(integrity_ok && !busy);
        view.run_button
            .set_label(if busy { "运行中…" } else { "重跑本段" });
        let successes = segment
            .runs
            .iter()
            .filter(|run| run.status == RunStatus::Succeeded)
            .count();
        view.compare_button.set_sensitive(successes >= 2);
        view.export_button.set_sensitive(integrity_ok);
    }

    fn refresh_mask_panel(&self, integrity_ok: bool) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let Some(segment) = project.segments.get(self.selected_segment) else {
            return;
        };
        let mask = segment.mask.clone().sanitized();
        let panel = &self.ui.mask_panel;
        panel.root.set_sensitive(integrity_ok);
        panel
            .shape
            .set_selected(if mask.shape == MaskShape::Rectangle {
                1
            } else {
                0
            });
        panel.center_x.set_value(mask.center_x * 100.0);
        panel.center_y.set_value(mask.center_y * 100.0);
        panel.radius_x.set_value(mask.radius_x * 100.0);
        panel.radius_y.set_value(mask.radius_y * 100.0);
        panel.feather.set_value(mask.feather_px);
        panel.expand.set_value(mask.expand_px);
        panel.darkness.set_value(mask.outside_darkness * 100.0);
        panel.summary.set_text(&mask.summary());
        if let Ok(mut handle) = self.mask.write() {
            *handle = mask;
        }
    }

    fn update_selected_mask<F: FnOnce(&mut MaskConfig)>(&mut self, apply: F) {
        if self.syncing.get() {
            return;
        }
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let Some(segment) = project.segments.get_mut(self.selected_segment) else {
            return;
        };
        apply(&mut segment.mask);
        segment.mask = segment.mask.clone().sanitized();
        let mask = segment.mask.clone();
        self.ui.mask_panel.summary.set_text(&mask.summary());
        if let Ok(mut handle) = self.mask.write() {
            *handle = mask;
        }
        self.dirty = true;
        if let Some(player) = &self.player {
            if !player.is_playing() {
                let _ = player.seek_ns(self.current_position());
            }
        }
    }

    fn current_position(&self) -> u64 {
        self.player
            .as_ref()
            .map(PreviewPlayer::position_ns)
            .or_else(|| {
                self.project
                    .as_ref()
                    .map(|project| project.review_position_ns)
            })
            .unwrap_or(0)
    }

    fn mark_position(&mut self, position_ns: u64) {
        if let Some(project) = self.project.as_mut() {
            project.review_position_ns =
                position_ns.min(project.media.duration_ns.saturating_sub(1));
        }
        self.refresh_playback_labels();
    }

    fn refresh_playback_labels(&self) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        self.ui
            .timeline
            .set_value(project.review_position_ns as f64);
        self.ui.time_label.set_text(&format!(
            "{} / {}   {}",
            format_timecode(project.review_position_ns),
            format_timecode(project.media.duration_ns),
            project.media.frame_label(project.review_position_ns)
        ));
        self.ui.play_button.set_label(
            if self.player.as_ref().is_some_and(PreviewPlayer::is_playing) {
                "暂停"
            } else {
                "播放"
            },
        );
    }
}

fn install_window_actions(ui: &UiRefs, weak: WeakState) {
    {
        let weak = weak.clone();
        ui.new_button
            .connect_clicked(move |_| choose_source_video(&weak));
    }
    {
        let weak = weak.clone();
        ui.open_button
            .connect_clicked(move |_| choose_project_folder(&weak));
    }
    {
        let weak = weak.clone();
        ui.evidence_button.connect_clicked(move |_| {
            with_state(&weak, AppState::show_evidence_dialog);
        });
    }
    {
        let weak = weak.clone();
        ui.folder_button.connect_clicked(move |_| {
            with_state(&weak, |state| {
                if let Some(project) = &state.project {
                    open_path(&project.project_dir);
                }
            });
        });
    }
    {
        let weak = weak.clone();
        ui.window.connect_close_request(move |_| {
            with_state(&weak, |state| {
                state.save_now();
                state.player = None;
            });
            gtk::glib::Propagation::Proceed
        });
    }
}

fn schedule_tick(weak: WeakState) {
    gtk::glib::timeout_add_local(Duration::from_millis(40), move || {
        with_state(&weak, AppState::tick);
        gtk::glib::ControlFlow::Continue
    });
}

fn choose_source_video(weak: &WeakState) {
    let Some(shared) = weak.upgrade() else {
        return;
    };
    let window = shared.borrow().ui.window.clone();
    let dialog = gtk::FileDialog::builder()
        .title("选择原始视频")
        .accept_label("导入并新建")
        .build();
    let weak = Rc::downgrade(&shared);
    dialog.open(
        Some(&window),
        None::<&gtk::gio::Cancellable>,
        move |result| {
            let Ok(file) = result else {
                return;
            };
            let Some(source) = file.path() else {
                return;
            };
            let stem = source
                .file_stem()
                .and_then(|name| name.to_str())
                .unwrap_or("review");
            let title = stem.to_string();
            let safe: String = stem
                .chars()
                .map(|character| {
                    if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                        character
                    } else {
                        '-'
                    }
                })
                .collect();
            let root = default_projects_root().join(format!(
                "{}-{}-{}",
                safe.trim_matches('-'),
                chrono::Utc::now().format("%Y%m%d-%H%M%S"),
                &uuid::Uuid::new_v4().simple().to_string()[..8]
            ));
            spawn_new_project(&weak, source, root, title);
        },
    );
}

fn choose_project_folder(weak: &WeakState) {
    let Some(shared) = weak.upgrade() else {
        return;
    };
    let window = shared.borrow().ui.window.clone();
    let dialog = gtk::FileDialog::builder()
        .title("打开审查项目目录")
        .accept_label("打开")
        .build();
    let weak = Rc::downgrade(&shared);
    dialog.select_folder(
        Some(&window),
        None::<&gtk::gio::Cancellable>,
        move |result| {
            let Ok(folder) = result else {
                return;
            };
            if let Some(path) = folder.path() {
                spawn_open_project(weak.clone(), path);
            }
        },
    );
}

fn spawn_open_project(weak: WeakState, root: PathBuf) {
    let Some(shared) = weak.upgrade() else {
        return;
    };
    let (tx, window) = {
        let mut state = shared.borrow_mut();
        state.set_status(format!("正在打开并校验项目：{}", root.display()), false);
        (state.worker_tx.clone(), state.ui.window.clone())
    };
    let _ = window;
    std::thread::spawn(move || {
        let result = open_project(&root).map_err(|error| format!("{error:#}"));
        let _ = tx.send(WorkerMsg::ProjectOpened {
            result: Box::new(result),
        });
    });
}

fn spawn_new_project(weak: &WeakState, source: PathBuf, root: PathBuf, title: String) {
    let Some(shared) = weak.upgrade() else {
        return;
    };
    let tx = shared.borrow().worker_tx.clone();
    shared.borrow_mut().set_status(
        format!("正在探测并保全原始素材：{}", source.display()),
        false,
    );
    std::thread::spawn(move || {
        let result = (|| -> anyhow::Result<OpenedProject> {
            let media: MediaInfo = probe_media(&source)?;
            create_project(&root, &source, title, media)
        })()
        .map_err(|error| format!("{error:#}"));
        let _ = tx.send(WorkerMsg::ProjectOpened {
            result: Box::new(result),
        });
    });
}

fn start_run_async(weak: &WeakState, tx: &Sender<WorkerMsg>, segment_index: usize) {
    let Some(shared) = weak.upgrade() else {
        return;
    };
    let job = {
        let mut state = match shared.try_borrow_mut() {
            Ok(state) => state,
            Err(_) => return,
        };
        if !state
            .integrity
            .as_ref()
            .is_some_and(|report| report.verified)
        {
            state.set_status("原始素材校验未通过，禁止重跑", true);
            return;
        }
        let Some(project) = state.project.as_mut() else {
            return;
        };
        match begin_run(project, segment_index) {
            Ok(job) => {
                state.refresh_all();
                state.set_status(
                    format!(
                        "片段 {} 第 {} 次重跑中，原始素材只读",
                        segment_index + 1,
                        job.run_number
                    ),
                    false,
                );
                job
            }
            Err(error) => {
                state.set_status(format!("无法开始重跑：{error:#}"), true);
                return;
            }
        }
    };
    let tx = tx.clone();
    std::thread::spawn(move || {
        let result = crate::service::execute_run(&job).map_err(|error| format!("{error:#}"));
        let _ = tx.send(WorkerMsg::RunFinished {
            segment_index,
            run_id: job.run_id,
            result,
        });
    });
}

fn choose_export_file(weak: &WeakState, window: &gtk::ApplicationWindow, segment_index: usize) {
    let dialog = gtk::FileDialog::builder()
        .title(format!("导出片段 {} 为 WebM", segment_index + 1))
        .accept_label("导出")
        .initial_name(format!("segment-{}-masked.webm", segment_index + 1))
        .build();
    let weak = weak.clone();
    let window = window.clone();
    dialog.save(
        Some(&window),
        None::<&gtk::gio::Cancellable>,
        move |result| {
            let Ok(file) = result else {
                return;
            };
            let Some(mut output) = file.path() else {
                return;
            };
            if output.extension().and_then(|value| value.to_str()) != Some("webm") {
                output.set_extension("webm");
            }
            spawn_export(&weak, segment_index, output);
        },
    );
}

fn spawn_export(weak: &WeakState, segment_index: usize, output: PathBuf) {
    let Some(shared) = weak.upgrade() else {
        return;
    };
    let (session, tx) = {
        let mut state = shared.borrow_mut();
        if !state
            .integrity
            .as_ref()
            .is_some_and(|report| report.verified)
        {
            state.set_status("原始素材校验未通过，禁止导出", true);
            return;
        }
        let Some(project) = state.project.as_ref() else {
            return;
        };
        let session = project.clone();
        let tx = state.worker_tx.clone();
        state.set_status(
            format!("片段 {} 正在导出：{}", segment_index + 1, output.display()),
            false,
        );
        (session, tx)
    };
    std::thread::spawn(move || {
        let result =
            export_segment(&session, segment_index, &output).map_err(|error| format!("{error:#}"));
        let _ = tx.send(WorkerMsg::ExportFinished {
            segment_index,
            result,
        });
    });
}

fn open_path(path: &Path) {
    let _ = std::process::Command::new("xdg-open").arg(path).spawn();
}

fn clear_list(list: &gtk::ListBox) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
}

fn run_summary(segment: &crate::model::Segment) -> String {
    let Some(run) = segment.runs.last() else {
        return "尚无运行记录；先设锚点与偏移，再重跑本段".to_string();
    };
    match run.status {
        RunStatus::Succeeded => format!(
            "最近：第 {} 次已完成 · 实测 {} → {} · 证据 {} 帧 · SHA-256 {}",
            run.number,
            format_timecode(run.measured_start_ns.unwrap_or(run.range_start_ns)),
            format_timecode(run.measured_end_ns.unwrap_or(run.range_end_ns)),
            run.evidence.len(),
            run.output_sha256.as_deref().map(short_hash).unwrap_or("--")
        ),
        RunStatus::Failed => format!(
            "最近：第 {} 次失败 · {}",
            run.number,
            run.error.as_deref().unwrap_or("未知错误")
        ),
        RunStatus::Interrupted => format!(
            "最近：第 {} 次中途中断 · 会话已恢复，可重新重跑",
            run.number
        ),
        RunStatus::Running => format!("第 {} 次运行中…", run.number),
    }
}

fn short_hash(hash: &str) -> &str {
    hash.get(..12).unwrap_or(hash)
}

impl AppState {
    fn select_segment(&mut self, index: usize) {
        if index >= crate::model::SEGMENT_COUNT {
            return;
        }
        self.selected_segment = index;
        if let Some(project) = self.project.as_ref() {
            if let Some(segment) = project.segments.get(index) {
                if let Ok(mut handle) = self.mask.write() {
                    *handle = segment.mask.clone().sanitized();
                }
            }
        }
        let integrity_ok = self
            .integrity
            .as_ref()
            .is_some_and(|report| report.verified);
        self.refresh_mask_panel(integrity_ok);
        if let Some(project) = self.project.as_mut() {
            project.selected_segment = index;
            self.dirty = true;
        }
    }

    fn jump_to(&mut self, ns: u64) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let target = ns.min(project.media.duration_ns.saturating_sub(1));
        if let Some(player) = &self.player {
            if let Err(error) = player.seek_ns(target) {
                self.set_status(format!("跳转失败：{error:#}"), true);
                return;
            }
        }
        self.preview_end_ns = None;
        self.mark_position(target);
    }

    fn jump_to_segment_anchor(&mut self, index: usize) {
        if let Ok(range) = self.segment_range(index) {
            self.jump_to(range.anchor_ns);
        }
    }

    fn jump_to_segment_edge(&mut self, index: usize, incoming: bool) {
        if let Ok(range) = self.segment_range(index) {
            self.jump_to(if incoming {
                range.start_ns
            } else {
                range.end_ns
            });
        }
    }

    fn preview_segment(&mut self, index: usize) {
        let Ok(range) = self.segment_range(index) else {
            self.set_status("片段范围无效，无法预览", true);
            return;
        };
        self.selected_segment = index;
        self.refresh_mask_panel(
            self.integrity
                .as_ref()
                .is_some_and(|report| report.verified),
        );
        self.jump_to(range.start_ns);
        self.preview_end_ns = Some(range.end_ns);
        if let Some(player) = &self.player {
            let _ = player.play();
        }
        self.refresh_playback_labels();
    }

    fn segment_range(&self, index: usize) -> Result<crate::model::SegmentRange, String> {
        let project = self.project.as_ref().ok_or("尚未打开项目")?;
        project.range_for(index)
    }

    fn set_anchor_from_playhead(&mut self, index: usize) {
        let position = self.current_position();
        self.set_anchor(index, position);
    }

    fn apply_anchor_text(&mut self, index: usize, text: &str) {
        match parse_timecode(text) {
            Ok(ns) => {
                let duration = self
                    .project
                    .as_ref()
                    .map(|project| project.media.duration_ns)
                    .unwrap_or(0);
                if ns >= duration {
                    self.set_status("锚点超出原片时长", true);
                    return;
                }
                self.set_anchor(index, ns);
            }
            Err(error) => self.set_status(format!("锚点时间码无效：{error}"), true),
        }
    }

    fn set_anchor(&mut self, index: usize, ns: u64) {
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let Some(segment) = project.segments.get_mut(index) else {
            return;
        };
        segment.anchor_ns = ns;
        self.dirty = true;
        self.refresh_all();
        self.set_status(
            format!(
                "片段 {} 锚点已设为 {}（{} ns）",
                index + 1,
                format_timecode(ns),
                ns
            ),
            false,
        );
    }

    fn update_offset(&mut self, index: usize, incoming: bool, millis: i64) {
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let Some(segment) = project.segments.get_mut(index) else {
            return;
        };
        let value = millis.saturating_mul(1_000_000);
        if incoming {
            segment.in_offset_ns = value;
        } else {
            segment.out_offset_ns = value;
        }
        self.dirty = true;
        self.refresh_all();
    }

    fn toggle_playback(&mut self) {
        let Some(player) = &self.player else {
            self.set_status("尚未载入可播放素材", true);
            return;
        };
        if player.is_playing() {
            let _ = player.pause();
        } else {
            self.preview_end_ns = None;
            let _ = player.play();
        }
        self.refresh_playback_labels();
    }

    fn step_frames(&mut self, direction: i64) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let frame =
            frame_duration_ns(project.media.fps_num, project.media.fps_den).unwrap_or(40_000_000);
        let current = self.current_position() as i128;
        let target = (current + direction as i128 * frame as i128)
            .clamp(0, project.media.duration_ns.saturating_sub(1) as i128)
            as u64;
        if let Some(player) = &self.player {
            let _ = player.pause();
        }
        self.jump_to(target);
    }

    fn tick(&mut self) {
        self.drain_workers();
        if let Some(frame) = self.mailbox.take_latest() {
            if frame.stride > 0 && frame.bytes.len() >= frame.stride * frame.height as usize {
                let bytes = gtk::glib::Bytes::from_owned(frame.bytes);
                let texture = gtk::gdk::MemoryTexture::new(
                    frame.width as i32,
                    frame.height as i32,
                    gtk::gdk::MemoryFormat::R8g8b8a8,
                    &bytes,
                    frame.stride,
                );
                self.ui.picture.set_paintable(Some(&texture));
            }
        }
        let (position, player_error, reached_end) = if let Some(player) = &self.player {
            (
                player.position_ns(),
                player.take_error(),
                self.preview_end_ns
                    .is_some_and(|end| player.position_ns() >= end),
            )
        } else {
            (0, None, false)
        };
        if let Some(error) = player_error {
            self.set_status(format!("GStreamer 预览错误：{error}"), true);
        }
        if self.player.is_some() {
            self.mark_position(position);
        }
        if reached_end {
            if let Some(player) = &self.player {
                let _ = player.pause();
            }
            self.preview_end_ns = None;
            self.refresh_playback_labels();
        }
        self.tick_count = self.tick_count.wrapping_add(1);
        let position = self
            .project
            .as_ref()
            .map(|project| project.review_position_ns)
            .unwrap_or(0);
        let position_moved = position.abs_diff(self.last_saved_position_ns) > 750_000_000;
        if self.project.is_some()
            && self.tick_count.is_multiple_of(25)
            && (self.dirty || position_moved)
        {
            self.save_now();
        }
    }

    fn save_now(&mut self) {
        let position = self.current_position();
        let Some(project) = self.project.as_mut() else {
            return;
        };
        project.review_position_ns = position;
        project.selected_segment = self.selected_segment;
        match save_session(project) {
            Ok(()) => {
                self.last_saved_position_ns = project.review_position_ns;
                self.dirty = false;
            }
            Err(error) => self.set_status(format!("自动保存失败：{error:#}"), true),
        }
    }

    fn drain_workers(&mut self) {
        while let Ok(message) = self.worker_rx.try_recv() {
            match message {
                WorkerMsg::ProjectOpened { result } => match *result {
                    Ok(opened) => self.install_project(opened),
                    Err(error) => self.set_status(error, true),
                },
                WorkerMsg::RunFinished {
                    segment_index,
                    run_id,
                    result,
                } => {
                    let Some(project) = self.project.as_mut() else {
                        continue;
                    };
                    let status_message = match &result {
                        Ok(outcome) => format!(
                            "片段 {} 重跑完成 · {} · SHA-256 {}",
                            segment_index + 1,
                            outcome.output_path.display(),
                            short_hash(&outcome.output_sha256)
                        ),
                        Err(error) => format!("片段 {} 重跑失败：{error}", segment_index + 1),
                    };
                    let failed = result.is_err();
                    if let Err(error) = finish_run(project, segment_index, &run_id, result) {
                        self.set_status(format!("保存运行结果失败：{error:#}"), true);
                    } else {
                        self.refresh_all();
                        self.set_status(status_message, failed);
                    }
                }
                WorkerMsg::ExportFinished {
                    segment_index,
                    result,
                } => match result {
                    Ok(outcome) => {
                        let Some(project) = self.project.as_mut() else {
                            continue;
                        };
                        let path = outcome.output_path.display().to_string();
                        if let Err(error) = record_export(project, segment_index, &outcome) {
                            self.set_status(format!("记录导出失败：{error:#}"), true);
                        } else {
                            self.refresh_all();
                            self.set_status(
                                format!("片段 {} 已导出：{path}", segment_index + 1),
                                false,
                            );
                        }
                    }
                    Err(error) => {
                        self.set_status(format!("导出失败：{error}"), true);
                    }
                },
            }
        }
    }

    fn show_compare_dialog(&mut self, segment_index: usize) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let text = match compare_latest_runs(project, segment_index) {
            Ok(text) => text,
            Err(error) => {
                self.set_status(error.to_string(), true);
                return;
            }
        };
        let successful: Vec<_> = project
            .segments
            .get(segment_index)
            .into_iter()
            .flat_map(|segment| segment.runs.iter())
            .filter(|run| run.status == RunStatus::Succeeded)
            .collect();
        let compared = successful[successful.len() - 2..]
            .iter()
            .map(|run| {
                (
                    run.number,
                    run.output_relative_path.clone(),
                    run.evidence
                        .iter()
                        .find(|frame| frame.kind == "anchor")
                        .or_else(|| run.evidence.first())
                        .map(|frame| project.project_dir.join(&frame.relative_path)),
                )
            })
            .collect::<Vec<_>>();

        let window = gtk::Window::builder()
            .title(format!("片段 {} · 最近两次比较", segment_index + 1))
            .transient_for(&self.ui.window)
            .modal(true)
            .default_width(960)
            .default_height(700)
            .build();
        let header = gtk::HeaderBar::new();
        window.set_titlebar(Some(&header));
        let root = gtk::Box::new(gtk::Orientation::Vertical, 12);
        root.set_margin_top(12);
        root.set_margin_bottom(12);
        root.set_margin_start(12);
        root.set_margin_end(12);
        let diff = gtk::Label::new(Some(&text));
        diff.set_xalign(0.0);
        diff.set_selectable(true);
        diff.set_wrap(true);
        diff.add_css_class("mono");
        root.append(&diff);
        let frames = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        frames.set_homogeneous(true);
        for (number, output, image) in compared {
            let column = gtk::Box::new(gtk::Orientation::Vertical, 6);
            let title = gtk::Label::new(Some(&format!("第 {number} 次锚点证据")));
            title.set_xalign(0.0);
            title.add_css_class("section-title");
            column.append(&title);
            if let Some(path) = image {
                let picture = gtk::Picture::for_filename(path);
                picture.set_content_fit(gtk::ContentFit::Contain);
                picture.set_size_request(420, 280);
                picture.add_css_class("video-stage");
                column.append(&picture);
            }
            if let Some(output) = output {
                let label = gtk::Label::new(Some(&output));
                label.set_xalign(0.0);
                label.set_wrap(true);
                label.set_selectable(true);
                label.add_css_class("status");
                column.append(&label);
            }
            frames.append(&column);
        }
        root.append(&frames);
        let close = button("关闭", false);
        close.set_halign(gtk::Align::End);
        let window_for_close = window.clone();
        close.connect_clicked(move |_| window_for_close.close());
        root.append(&close);
        window.set_child(Some(&root));
        window.present();
    }

    fn show_evidence_dialog(&mut self) {
        let Some(project) = self.project.as_ref() else {
            self.set_status("尚未打开项目", true);
            return;
        };
        let events = read_journal(&project.project_dir);
        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        root.set_margin_top(10);
        root.set_margin_bottom(10);
        root.set_margin_start(10);
        root.set_margin_end(10);
        let heading = gtk::Label::new(Some(&format!(
            "关键证据日志 · {} 条 · journal.jsonl",
            events.len()
        )));
        heading.set_xalign(0.0);
        heading.add_css_class("section-title");
        root.append(&heading);
        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::None);
        for event in events.iter().rev().take(250) {
            let segment = event
                .segment_index
                .map(|index| format!("片段 {} · ", index + 1))
                .unwrap_or_default();
            let text = format!("{}  {}{}  {}", event.at, segment, event.kind, event.message);
            let row = gtk::Label::new(Some(&text));
            row.set_xalign(0.0);
            row.set_wrap(true);
            row.set_selectable(true);
            row.add_css_class("mono");
            row.add_css_class("run-row");
            list.append(&row);
        }
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroll.set_vexpand(true);
        scroll.set_child(Some(&list));
        root.append(&scroll);
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let folder = button("打开证据目录", false);
        let close = button("关闭", false);
        actions.append(&folder);
        actions.set_halign(gtk::Align::End);
        let project_dir = project.project_dir.clone();
        folder.connect_clicked(move |_| open_path(&project_dir.join("evidence")));
        root.append(&actions);
        let window = gtk::Window::builder()
            .title("关键证据")
            .transient_for(&self.ui.window)
            .modal(true)
            .default_width(980)
            .default_height(620)
            .build();
        let header = gtk::HeaderBar::new();
        window.set_titlebar(Some(&header));
        let window_for_close = window.clone();
        close.connect_clicked(move |_| window_for_close.close());
        actions.append(&close);
        window.set_child(Some(&root));
        window.present();
    }
}
