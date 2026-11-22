//! GTK4 widget construction.
//!
//! Layout: header bar with project actions, a three-pane notebook
//! (Segments / Devices & offsets / Comparisons), a keyframe detail panel and
//! a bottom status bar with progress and a Stop button. All mutating actions
//! go through `ProjectService` so the GUI and CLI share one implementation.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::channel;

use gtk::glib;
use gtk::glib::clone;
use gtk::glib::prelude::StaticType;
use gtk::prelude::*;
use gtk::{
    Application, ApplicationWindow, Box as GtkBox, Button, CellRendererText, Entry, Grid, Label,
    ListStore, Notebook, Orientation, Paned, ProgressBar, ScrolledWindow, SignalListItemFactory,
    StringList, TreeView,
};

use crate::dialogs::{
    show_add_device_dialog, show_config_dialog, show_error, show_export_dialog, show_info,
    show_new_project_dialog, show_offset_dialog, show_open_project_dialog, show_register_dialog,
};
use crate::state::{ActiveRun, SharedState, WorkerMessage};
use crate::workers;
use vkri_core::ProjectService;

pub(crate) const ACTOR: &str = "gtk4";

/// Build and return the main application window.
pub fn build_main_window(app: &Application, state: SharedState) -> ApplicationWindow {
    let window = ApplicationWindow::builder()
        .application(app)
        .title("VKRI · 视频关键帧索引重建")
        .default_width(1180)
        .default_height(760)
        .build();

    let root = GtkBox::new(Orientation::Vertical, 0);
    window.set_child(Some(&root));

    root.append(&build_header_bar(&window, state.clone()));

    let notebook = Notebook::new();
    let segments_page = build_segments_page(&window, state.clone());
    let devices_page = build_devices_page(&window, state.clone());
    let comparison_page = build_comparison_page(&window, state.clone());
    notebook.append_page(&segments_page, Some(&Label::new(Some("片段与关键帧"))));
    notebook.append_page(&devices_page, Some(&Label::new(Some("设备时钟"))));
    notebook.append_page(&comparison_page, Some(&Label::new(Some("比对与偏移"))));

    let paned = Paned::new(Orientation::Vertical);
    paned.set_vexpand(true);
    paned.set_start_child(Some(&notebook));
    let (status_container, status) = build_status_bar(state.clone());
    state.borrow_mut().status = Some(status);
    paned.set_end_child(Some(&status_container));
    paned.set_position(560);
    root.append(&paned);

    install_worker_channel(&window, state.clone());
    install_close_cancel(&window, state);

    window
}

fn build_header_bar(window: &ApplicationWindow, state: SharedState) -> gtk::HeaderBar {
    let header = gtk::HeaderBar::new();
    let new_button = Button::with_label("新建项目");
    let open_button = Button::with_label("打开项目");
    let export_button = Button::with_label("导出复核包");
    header.pack_start(&new_button);
    header.pack_start(&open_button);
    header.pack_end(&export_button);

    new_button.connect_clicked(clone!(
        #[weak]
        window,
        #[strong]
        state,
        move |_| {
            show_new_project_dialog(&window, state.clone());
        }
    ));
    open_button.connect_clicked(clone!(
        #[weak]
        window,
        #[strong]
        state,
        move |_| {
            show_open_project_dialog(&window, state.clone());
        }
    ));
    export_button.connect_clicked(clone!(
        #[weak]
        window,
        #[strong]
        state,
        move |_| {
            show_export_dialog(&window, state.clone());
        }
    ));
    header
}

/// Status bar holds the progress text/bar and the Stop button. The widgets are
/// shared through an Rc so worker messages can update them directly.
#[derive(Clone)]
pub(crate) struct StatusWidgets {
    label: Label,
    progress: ProgressBar,
    stop_button: Button,
}

fn build_status_bar(state: SharedState) -> (GtkBox, Rc<StatusWidgets>) {
    let container = GtkBox::new(Orientation::Horizontal, 8);
    container.set_margin_start(10);
    container.set_margin_end(10);
    container.set_margin_top(6);
    container.set_margin_bottom(6);

    let label = Label::new(Some("未打开项目"));
    label.set_halign(gtk::Align::Start);
    label.set_hexpand(true);
    let progress = ProgressBar::new();
    progress.set_hexpand(true);
    progress.set_show_text(true);
    let stop_button = Button::with_label("停止");
    stop_button.set_sensitive(false);

    container.append(&label);
    container.append(&progress);
    container.append(&stop_button);

    let widgets = Rc::new(StatusWidgets {
        label,
        progress,
        stop_button: stop_button.clone(),
    });
    stop_button.connect_clicked(clone!(
        #[strong]
        state,
        #[strong]
        widgets,
        move |_| {
            let running = state.borrow().active_run.clone();
            if let Some(active) = running {
                active.cancel.cancel();
                widgets.label.set_text("正在停止，保留已生成证据…");
                widgets.stop_button.set_sensitive(false);
            }
        }
    ));
    (container, widgets)
}

fn install_worker_channel(window: &ApplicationWindow, state: SharedState) {
    let (sender, receiver) = channel::<WorkerMessage>();
    glib::timeout_add_local(
        std::time::Duration::from_millis(100),
        clone!(
            #[weak]
            window,
            #[strong]
            state,
            #[upgrade_or]
            glib::ControlFlow::Break,
            move || {
                let receiver = &receiver;
                let mut keep_polling = glib::ControlFlow::Continue;
                while let Ok(message) = receiver.try_recv() {
                    handle_worker_message(&window, &state, message);
                    keep_polling = glib::ControlFlow::Continue;
                }
                keep_polling
            }
        ),
    );
    SENDER.with(|cell| *cell.borrow_mut() = Some(sender));
}

thread_local! {
    static SENDER: RefCell<Option<std::sync::mpsc::Sender<WorkerMessage>>> =
        const { RefCell::new(None) };
}

fn handle_worker_message(window: &ApplicationWindow, state: &SharedState, message: WorkerMessage) {
    let status = state.borrow().status.clone();
    match message {
        WorkerMessage::Progress {
            frames_seen,
            keyframes_written,
            pts_ns,
        } => {
            if let Some(status) = status {
                status.progress.set_text(Some(&format!(
                    "frames={frames_seen} keyframes={keyframes_written} pts={pts_ns}ns"
                )));
                status.progress.pulse();
            }
        }
        WorkerMessage::RunFinished {
            segment_id,
            interrupted,
            message,
        } => {
            state.borrow_mut().active_run = None;
            reload_project(state);
            if let Some(status) = &status {
                status
                    .progress
                    .set_fraction(if interrupted { 0.5 } else { 1.0 });
                status.progress.set_text(Some(&message));
                status.stop_button.set_sensitive(false);
            }
            let detail = if interrupted {
                format!("片段 {segment_id} 已中断，部分证据已保留，可再次运行恢复")
            } else {
                format!("片段 {segment_id} 提取完成：{message}")
            };
            show_info(window, &detail);
            refresh_all_views(state);
        }
        WorkerMessage::RunFailed { segment_id, error } => {
            state.borrow_mut().active_run = None;
            if let Some(status) = &status {
                status.progress.set_fraction(0.0);
                status.progress.set_text(Some(&format!("失败: {error}")));
                status.stop_button.set_sensitive(false);
            }
            show_error(window, &format!("片段 {segment_id} 运行失败：{error}"));
        }
        WorkerMessage::ProjectReloaded | WorkerMessage::Info(_) => {
            refresh_all_views(state);
        }
    }
}

fn install_close_cancel(window: &ApplicationWindow, state: SharedState) {
    window.connect_close_request(move |_window| {
        if let Some(active) = state.borrow().active_run.clone() {
            active.cancel.cancel();
        }
        glib::Propagation::Proceed
    });
}

/// Holds the segment list and its per-page widgets for refresh.
#[derive(Clone)]
struct SegmentPage {
    segment_list: StringList,
    detail: Label,
    keyframe_store: ListStore,
    run_button: Button,
    stop_ref_button: Button,
    config_button: Button,
}

thread_local! {
    static SEGMENT_PAGE: RefCell<Option<SegmentPage>> = const { RefCell::new(None) };
    static DEVICE_PAGE: RefCell<Option<DevicePage>> = const { RefCell::new(None) };
    static COMPARISON_PAGE: RefCell<Option<ComparisonPage>> = const { RefCell::new(None) };
}

fn build_segments_page(window: &ApplicationWindow, state: SharedState) -> GtkBox {
    let page = GtkBox::new(Orientation::Horizontal, 8);
    page.set_margin_start(8);
    page.set_margin_end(8);
    page.set_margin_top(8);
    page.set_margin_bottom(8);

    let left = GtkBox::new(Orientation::Vertical, 6);
    left.set_width_request(360);
    let segment_list = StringList::new(&[] as &[&str]);
    let selection = gtk::SingleSelection::new(Some(segment_list.clone()));
    let factory = SignalListItemFactory::new();
    factory.connect_setup(move |_factory, list_item| {
        let label = Label::new(None);
        label.set_halign(gtk::Align::Start);
        list_item.set_child(Some(&label));
    });
    factory.connect_bind(move |_factory, list_item| {
        let string_object = list_item
            .item()
            .and_downcast::<gtk::StringObject>()
            .expect("StringObject");
        let label = list_item
            .child()
            .and_downcast::<Label>()
            .expect("Label child");
        label.set_text(&string_object.string());
    });
    let list_view = gtk::ListView::new(Some(selection.clone()), Some(factory));
    let scroller = ScrolledWindow::new();
    scroller.set_child(Some(&list_view));
    scroller.set_vexpand(true);
    left.append(&scroller);

    let register_button = Button::with_label("注册媒体片段…");
    let run_button = Button::with_label("运行 / 重跑提取");
    let stop_ref_button = Button::with_label("标记为重跑(stale)");
    let config_button = Button::with_label("修改提取配置…");
    for button in [
        &register_button,
        &run_button,
        &stop_ref_button,
        &config_button,
    ] {
        left.append(button);
    }
    page.append(&left);

    let right = GtkBox::new(Orientation::Vertical, 6);
    right.set_hexpand(true);
    let detail = Label::new(Some("选择一个片段查看详情"));
    detail.set_halign(gtk::Align::Start);
    right.append(&detail);

    let keyframe_store = ListStore::new(&[
        String::static_type(),
        String::static_type(),
        String::static_type(),
        String::static_type(),
        String::static_type(),
    ]);
    let tree = TreeView::with_model(&keyframe_store);
    let titles = [
        "序号",
        "PTS(ns)",
        "设备时间(UTC)",
        "参考时间(UTC)",
        "缩略图SHA256",
    ];
    for (index, title) in titles.iter().enumerate() {
        let renderer = CellRendererText::new();
        let column = gtk::TreeViewColumn::new();
        column.set_title(title);
        column.pack_start(&renderer, true);
        column.add_attribute(&renderer, "text", index as i32);
        tree.append_column(&column);
    }
    let keyframe_scroll = ScrolledWindow::new();
    keyframe_scroll.set_child(Some(&tree));
    keyframe_scroll.set_vexpand(true);
    right.append(&keyframe_scroll);
    page.append(&right);

    let page_state = SegmentPage {
        segment_list: segment_list.clone(),
        detail,
        keyframe_store,
        run_button: run_button.clone(),
        stop_ref_button: stop_ref_button.clone(),
        config_button: config_button.clone(),
    };
    SEGMENT_PAGE.with(|cell| *cell.borrow_mut() = Some(page_state));

    register_button.connect_clicked(clone!(
        #[weak]
        window,
        #[strong]
        state,
        move |_| {
            show_register_dialog(&window, state.clone());
        }
    ));
    run_button.connect_clicked(clone!(
        #[weak]
        window,
        #[strong]
        state,
        #[strong]
        selection,
        #[strong]
        segment_list,
        move |_| {
            start_selected_segment(&window, state.clone(), &selection, &segment_list);
        }
    ));
    stop_ref_button.connect_clicked(clone!(
        #[weak]
        window,
        #[strong]
        state,
        #[strong]
        selection,
        #[strong]
        segment_list,
        move |_| {
            mark_selected_stale(&window, state.clone(), &selection, &segment_list);
        }
    ));
    config_button.connect_clicked(clone!(
        #[weak]
        window,
        #[strong]
        state,
        #[strong]
        selection,
        #[strong]
        segment_list,
        move |_| {
            show_config_dialog(&window, state.clone(), &selection, &segment_list);
        }
    ));
    selection.connect_selection_changed(clone!(
        #[strong]
        state,
        move |selection, _position, _n| {
            update_segment_detail(&state, selection);
        }
    ));

    page
}

#[derive(Clone)]
struct DevicePage {
    store: ListStore,
}

fn build_devices_page(window: &ApplicationWindow, state: SharedState) -> GtkBox {
    let page = GtkBox::new(Orientation::Vertical, 8);
    page.set_margin_start(8);
    page.set_margin_end(8);
    page.set_margin_top(8);
    page.set_margin_bottom(8);

    let store = ListStore::new(&[
        String::static_type(),
        String::static_type(),
        String::static_type(),
        String::static_type(),
        String::static_type(),
    ]);
    let tree = TreeView::with_model(&store);
    let titles = ["设备ID", "名称", "偏移(ns)", "不确定度(ns)", "备注"];
    for (index, title) in titles.iter().enumerate() {
        let renderer = CellRendererText::new();
        let column = gtk::TreeViewColumn::new();
        column.set_title(title);
        column.pack_start(&renderer, true);
        column.add_attribute(&renderer, "text", index as i32);
        tree.append_column(&column);
    }
    let scroller = ScrolledWindow::new();
    scroller.set_child(Some(&tree));
    scroller.set_vexpand(true);
    page.append(&scroller);

    let buttons = GtkBox::new(Orientation::Horizontal, 6);
    let add_button = Button::with_label("添加设备…");
    let offset_button = Button::with_label("设置选中设备时钟偏移…");
    buttons.append(&add_button);
    buttons.append(&offset_button);
    page.append(&buttons);

    DEVICE_PAGE.with(|cell| *cell.borrow_mut() = Some(DevicePage { store }));

    add_button.connect_clicked(clone!(
        #[weak]
        window,
        #[strong]
        state,
        move |_| {
            show_add_device_dialog(&window, state.clone());
        }
    ));
    offset_button.connect_clicked(clone!(
        #[weak]
        window,
        #[strong]
        state,
        #[strong]
        tree,
        move |_| {
            show_offset_dialog(&window, state.clone(), &tree);
        }
    ));
    page
}

#[derive(Clone)]
struct ComparisonPage {
    store: ListStore,
    segment_a: StringList,
    segment_b: StringList,
}

fn build_comparison_page(window: &ApplicationWindow, state: SharedState) -> GtkBox {
    let page = GtkBox::new(Orientation::Vertical, 8);
    page.set_margin_start(8);
    page.set_margin_end(8);
    page.set_margin_top(8);
    page.set_margin_bottom(8);

    let controls = Grid::new();
    controls.set_row_spacing(6);
    controls.set_column_spacing(6);
    let segment_a = StringList::new(&[] as &[&str]);
    let segment_b = StringList::new(&[] as &[&str]);
    let dropdown_a = gtk::DropDown::new(Some(segment_a.clone()), gtk::Expression::NONE);
    let dropdown_b = gtk::DropDown::new(Some(segment_b.clone()), gtk::Expression::NONE);
    let window_entry = Entry::new();
    window_entry.set_text("500000000");
    let compare_button = Button::with_label("运行比对并生成偏移建议");
    controls.attach(&Label::new(Some("片段A")), 0, 0, 1, 1);
    controls.attach(&dropdown_a, 1, 0, 1, 1);
    controls.attach(&Label::new(Some("片段B")), 2, 0, 1, 1);
    controls.attach(&dropdown_b, 3, 0, 1, 1);
    controls.attach(&Label::new(Some("匹配窗口(ns)")), 0, 1, 1, 1);
    controls.attach(&window_entry, 1, 1, 1, 1);
    controls.attach(&compare_button, 3, 1, 1, 1);
    page.append(&controls);

    let store = ListStore::new(&[
        String::static_type(),
        String::static_type(),
        String::static_type(),
        String::static_type(),
        String::static_type(),
        String::static_type(),
    ]);
    let tree = TreeView::with_model(&store);
    let titles = [
        "比对ID",
        "组",
        "配对数",
        "中位时差(ns)",
        "建议B偏移(ns)",
        "备注",
    ];
    for (index, title) in titles.iter().enumerate() {
        let renderer = CellRendererText::new();
        let column = gtk::TreeViewColumn::new();
        column.set_title(title);
        column.pack_start(&renderer, true);
        column.add_attribute(&renderer, "text", index as i32);
        tree.append_column(&column);
    }
    let scroller = ScrolledWindow::new();
    scroller.set_child(Some(&tree));
    scroller.set_vexpand(true);
    page.append(&scroller);

    let page_state = ComparisonPage {
        store,
        segment_a: segment_a.clone(),
        segment_b: segment_b.clone(),
    };
    COMPARISON_PAGE.with(|cell| *cell.borrow_mut() = Some(page_state));

    compare_button.connect_clicked(clone!(
        #[weak]
        window,
        #[strong]
        state,
        #[strong]
        dropdown_a,
        #[strong]
        dropdown_b,
        #[strong]
        window_entry,
        move |_| {
            run_comparison(
                &window,
                state.clone(),
                &dropdown_a,
                &dropdown_b,
                &window_entry,
                &segment_a,
                &segment_b,
            );
        }
    ));
    page
}

fn reload_project(state: &SharedState) {
    let dir = state.borrow().project_dir.clone();
    if let Some(dir) = dir {
        match ProjectService::open(&dir, ACTOR) {
            Ok(service) => {
                state.borrow_mut().service = Some(service);
            }
            Err(error) => {
                eprintln!("reload failed: {error}");
            }
        }
    }
}

pub(crate) fn refresh_all_views(state: &SharedState) {
    let borrowed = state.borrow();
    let Some(service) = borrowed.service.as_ref() else {
        return;
    };
    let project = &service.project;

    SEGMENT_PAGE.with(|cell| {
        if let Some(page) = cell.borrow().as_ref() {
            page.segment_list
                .splice(0, page.segment_list.n_items(), &[]);
            for segment in &project.segments {
                page.segment_list.append(&format!(
                    "{} [{}] {} · 组={} · 关键帧={}",
                    segment.id,
                    status_label(segment.status),
                    segment.label,
                    segment.group_id,
                    segment.keyframes.len()
                ));
            }
            page.keyframe_store.clear();
            page.run_button.set_sensitive(!borrowed.is_running());
            page.stop_ref_button.set_sensitive(!borrowed.is_running());
            page.config_button.set_sensitive(!borrowed.is_running());
        }
    });

    DEVICE_PAGE.with(|cell| {
        if let Some(page) = cell.borrow().as_ref() {
            page.store.clear();
            for device in &project.devices {
                set_row(
                    &page.store,
                    &[
                        &device.id,
                        &device.label,
                        &device.clock_offset.offset_ns.to_string(),
                        &device.clock_offset.uncertainty_ns.to_string(),
                        &device.notes,
                    ],
                );
            }
        }
    });

    COMPARISON_PAGE.with(|cell| {
        if let Some(page) = cell.borrow().as_ref() {
            page.segment_a.splice(0, page.segment_a.n_items(), &[]);
            page.segment_b.splice(0, page.segment_b.n_items(), &[]);
            for segment in &project.segments {
                let entry = format!("{} ({})", segment.id, segment.label);
                page.segment_a.append(&entry);
                page.segment_b.append(&entry);
            }
            page.store.clear();
            for comparison in &project.comparisons {
                let median = comparison
                    .median_delta_ns
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "—".to_string());
                let suggested = comparison
                    .suggested_offset_b_ns
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "—".to_string());
                set_row(
                    &page.store,
                    &[
                        &comparison.id,
                        &comparison.group_id,
                        &comparison.entries.len().to_string(),
                        &median,
                        &suggested,
                        &comparison.notes,
                    ],
                );
            }
        }
    });

    if let Some(status) = borrowed.status.as_ref() {
        status.label.set_text(&format!(
            "项目: {} · 设备 {} · 片段 {} · 比对 {}",
            project.name,
            project.devices.len(),
            project.segments.len(),
            project.comparisons.len()
        ));
    }
}

fn status_label(status: vkri_core::SegmentStatus) -> &'static str {
    match status {
        vkri_core::SegmentStatus::Pending => "pending",
        vkri_core::SegmentStatus::Extracted => "extracted",
        vkri_core::SegmentStatus::Interrupted => "interrupted",
        vkri_core::SegmentStatus::Stale => "stale",
    }
}

/// Populate one `ListStore` row from a slice of glib values.
fn set_row(store: &ListStore, values: &[&dyn glib::value::ToValue]) {
    let iter = store.append();
    let columns: Vec<(u32, &dyn glib::value::ToValue)> = values
        .iter()
        .enumerate()
        .map(|(index, value)| (index as u32, *value))
        .collect();
    store.set(&iter, &columns);
}

fn selected_segment_id(selection: &gtk::SingleSelection, list: &StringList) -> Option<String> {
    let position = selection.selected();
    if position == gtk::INVALID_LIST_POSITION {
        return None;
    }
    let text = list.string(position)?.to_string();
    text.split_whitespace().next().map(str::to_string)
}

fn update_segment_detail(state: &SharedState, selection: &gtk::SingleSelection) {
    let segment_id = SEGMENT_PAGE.with(|cell| {
        let page = cell.borrow();
        page.as_ref().and_then(|page| {
            let position = selection.selected();
            if position == gtk::INVALID_LIST_POSITION {
                None
            } else {
                let text = page.segment_list.string(position)?.to_string();
                text.split_whitespace().next().map(str::to_string)
            }
        })
    });
    let Some(segment_id) = segment_id else {
        return;
    };
    let borrowed = state.borrow();
    let Some(service) = borrowed.service.as_ref() else {
        return;
    };
    let Some(segment) = service.project.segment(&segment_id) else {
        return;
    };
    SEGMENT_PAGE.with(|cell| {
        if let Some(page) = cell.borrow().as_ref() {
            page.detail.set_text(&format!(
                "片段 {} · {} · 源 {} · 状态 {} · 运行次数 {} · 时长 {:?}ns",
                segment.id,
                segment.label,
                segment.source_path.display(),
                status_label(segment.status),
                segment.runs.len(),
                segment.duration_ns
            ));
            page.keyframe_store.clear();
            for keyframe in &segment.keyframes {
                let device_time = vkri_core::time::format_unix_nanos(keyframe.device_time_unix_ns);
                let reference_time =
                    vkri_core::time::format_unix_nanos(keyframe.reference_time_unix_ns);
                set_row(
                    &page.keyframe_store,
                    &[
                        &keyframe.index.to_string(),
                        &keyframe.pts_ns.to_string(),
                        &device_time,
                        &reference_time,
                        &keyframe.thumbnail_sha256,
                    ],
                );
            }
        }
    });
}

fn start_selected_segment(
    window: &ApplicationWindow,
    state: SharedState,
    selection: &gtk::SingleSelection,
    list: &StringList,
) {
    let Some(segment_id) = selected_segment_id(selection, list) else {
        show_error(window, "请先选择一个片段");
        return;
    };
    if state.borrow().is_running() {
        show_error(window, "已有提取任务在运行，请先停止或等待结束");
        return;
    }
    let (project_dir, actor) = {
        let borrowed = state.borrow();
        let Some(dir) = borrowed.project_dir.clone() else {
            show_error(window, "请先打开或创建项目");
            return;
        };
        (dir, ACTOR.to_string())
    };
    let cancel = vkri_core::extract::CancellationToken::new();
    state.borrow_mut().active_run = Some(ActiveRun {
        segment_id: segment_id.clone(),
        cancel: cancel.clone(),
    });
    if let Some(status) = state.borrow().status.clone() {
        status.stop_button.set_sensitive(true);
        status.progress.set_fraction(0.0);
        status.progress.set_text(Some("提取进行中…"));
    }
    let sender = SENDER
        .with(|cell| cell.borrow().clone())
        .expect("worker channel installed");
    workers::spawn_extraction(project_dir, segment_id, actor, cancel, sender);
}

fn mark_selected_stale(
    window: &ApplicationWindow,
    state: SharedState,
    selection: &gtk::SingleSelection,
    list: &StringList,
) {
    let Some(segment_id) = selected_segment_id(selection, list) else {
        show_error(window, "请先选择一个片段");
        return;
    };
    let result = match state.borrow_mut().service.as_mut() {
        Some(service) => service
            .mark_rerun_requested(&segment_id)
            .map_err(anyhow::Error::from),
        None => Err(anyhow::anyhow!("项目未打开")),
    };
    match result {
        Ok(()) => {
            refresh_all_views(&state);
            show_info(
                window,
                "已标记为 stale；旧证据保留，下次运行生成新 run 目录",
            );
        }
        Err(error) => show_error(window, &error.to_string()),
    }
}

fn run_comparison(
    window: &ApplicationWindow,
    state: SharedState,
    dropdown_a: &gtk::DropDown,
    dropdown_b: &gtk::DropDown,
    window_entry: &Entry,
    list_a: &StringList,
    list_b: &StringList,
) {
    let parse_id = |dropdown: &gtk::DropDown, list: &StringList| -> Option<String> {
        let position = dropdown.selected();
        if position == gtk::INVALID_LIST_POSITION {
            return None;
        }
        list.string(position)?
            .as_str()
            .split_whitespace()
            .next()
            .map(str::to_string)
    };
    let Some(segment_a) = parse_id(dropdown_a, list_a) else {
        show_error(window, "请选择片段 A");
        return;
    };
    let Some(segment_b) = parse_id(dropdown_b, list_b) else {
        show_error(window, "请选择片段 B");
        return;
    };
    let match_window_ns: i64 = match window_entry.text().parse() {
        Ok(value) if value > 0 => value,
        _ => {
            show_error(window, "匹配窗口必须是正整数纳秒");
            return;
        }
    };
    let result = match state.borrow_mut().service.as_mut() {
        Some(service) => service
            .compare(&segment_a, &segment_b, match_window_ns, "GTK4 触发比对")
            .map_err(anyhow::Error::from),
        None => Err(anyhow::anyhow!("项目未打开")),
    };
    match result {
        Ok(comparison) => {
            refresh_all_views(&state);
            show_info(
                window,
                &format!(
                    "比对完成：配对 {} 对，中位时差 {:?} ns，建议 B 偏移 {:?} ns",
                    comparison.entries.len(),
                    comparison.median_delta_ns,
                    comparison.suggested_offset_b_ns
                ),
            );
        }
        Err(error) => show_error(window, &error.to_string()),
    }
}
