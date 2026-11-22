//! GTK4 user interface.

use crate::export::export_bundle;
use crate::model::{
    APP_VERSION, DeviceId, RunRecord, RunStatus, Segment, SegmentStatus, Session, field,
};
use crate::store::{SessionStore, default_session_path};
use crate::transcode::{self, TranscodeOutput};
use crate::util::{format_bytes, format_ms, now_rfc3339, safe_component};
use gtk::prelude::*;
use gtk::{gio, glib, pango};
use gtk4 as gtk;
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

const APP_ID: &str = "io.local.QueuePriorityCorrector";

pub fn run(session_path: Option<PathBuf>) -> glib::ExitCode {
    let session_path = session_path.unwrap_or_else(default_session_path);
    let application = gtk::Application::builder().application_id(APP_ID).build();
    application.connect_activate(move |app| {
        if let Some(window) = app.active_window() {
            window.present();
            return;
        }
        activate(app, session_path.clone());
    });
    application.run()
}

fn activate(app: &gtk::Application, session_path: PathBuf) {
    if let Err(error) = transcode::ensure_initialized() {
        eprintln!("{error:#}");
    }
    let store = SessionStore::new(&session_path);
    let (session, load_note) = match store.load() {
        Ok(Some(loaded)) => {
            let mut note = if loaded.recovered {
                "上次退出时有片段正在运行，已保留中断证据".to_owned()
            } else {
                format!("已载入 {}", store.path().display())
            };
            if loaded.used_backup {
                note.push_str("；主文件损坏，已从备份恢复");
            }
            (loaded.session, note)
        }
        Ok(None) => {
            let mut session =
                crate::demo::seed_session(store.path().parent().unwrap_or_else(|| Path::new(".")));
            let media_note = match crate::demo::ensure_demo_media(&mut session) {
                Ok(()) => "已生成本地示例媒体".to_owned(),
                Err(error) => format!("示例媒体生成失败：{error:#}"),
            };
            let _ = store.save(&session);
            (session, format!("首次启动；{media_note}"))
        }
        Err(error) => {
            eprintln!("载入会话失败：{error:#}");
            let mut session =
                crate::demo::seed_session(store.path().parent().unwrap_or_else(|| Path::new(".")));
            let _ = crate::demo::ensure_demo_media(&mut session);
            (session, format!("会话损坏，已创建可恢复示例：{error:#}"))
        }
    };
    let ui = AppUi::new(app, store, session);
    ui.refresh_all();
    if ui.persist() {
        ui.set_status(&load_note);
    }
    ui.window.present();
    let autorun = std::env::var("QP_AUTORUN").ok();
    if let Some(segment_id) = autorun.clone() {
        let ui_for_run = ui.clone();
        glib::timeout_add_local_once(Duration::from_millis(350), move || {
            ui_for_run.run_segment(&segment_id);
        });
    }
    if let Ok(path) = std::env::var("QP_SCREENSHOT") {
        let window = ui.window.clone();
        let app = app.clone();
        let delay = if autorun.is_some() { 5_500 } else { 1_500 };
        glib::timeout_add_local_once(Duration::from_millis(delay), move || {
            if let Err(error) = capture_widget(&window, Path::new(&path)) {
                eprintln!("截图失败：{error}");
            }
            app.quit();
        });
    }
}

struct AppUi {
    window: gtk::ApplicationWindow,
    store: SessionStore,
    session: RefCell<Session>,
    selected: RefCell<Option<String>>,
    busy_segment: RefCell<Option<String>>,
    last_export: RefCell<Option<PathBuf>>,
    syncing_clock: Cell<bool>,
    rebuilding_list: Cell<bool>,
    run_dir: PathBuf,
    export_dir: PathBuf,
    object_title: gtk::Label,
    object_meta: gtk::Label,
    object_note: gtk::Label,
    clock_difference: gtk::Label,
    spin_a: gtk::SpinButton,
    spin_b: gtk::SpinButton,
    segment_list: gtk::ListBox,
    detail_box: gtk::Box,
    status_label: gtk::Label,
    save_label: gtk::Label,
    progress: gtk::ProgressBar,
    open_export_button: gtk::Button,
}

impl AppUi {
    fn new(app: &gtk::Application, store: SessionStore, session: Session) -> Rc<Self> {
        install_css();
        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .title("转码队列 · 局部修正")
            .default_width(1240)
            .default_height(820)
            .build();
        window.set_css_classes(&["app-window"]);

        let header = gtk::HeaderBar::new();
        let header_title = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let title = gtk::Label::new(Some("转码队列 · 局部修正"));
        title.set_css_classes(&["header-title"]);
        let subtitle = gtk::Label::new(Some("重复对象 · 双设备时钟不同步"));
        subtitle.set_css_classes(&["header-subtitle"]);
        header_title.append(&title);
        header_title.append(&subtitle);
        header.set_title_widget(Some(&header_title));
        window.set_titlebar(Some(&header));

        let shared = store
            .path()
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        let run_dir = shared.join("runs");
        let export_dir = shared.join("exports");
        let spin_a = axis_spin();
        let spin_b = axis_spin();
        let clock_difference = gtk::Label::new(None);
        clock_difference.set_css_classes(&["clock-difference"]);
        let save_button = gtk::Button::with_label("立即保存");
        save_button.set_css_classes(&["toolbar-button"]);
        let recalc_button = gtk::Button::with_label("重算队列");
        recalc_button.set_css_classes(&["toolbar-button"]);
        let export_button = gtk::Button::with_label("导出复核包");
        export_button.set_css_classes(&["primary-button"]);
        let open_export_button = gtk::Button::with_label("打开导出目录");
        open_export_button.set_css_classes(&["toolbar-button"]);
        open_export_button.set_sensitive(false);

        let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        toolbar.set_css_classes(&["toolbar-card"]);
        toolbar.set_margin_start(16);
        toolbar.set_margin_end(16);
        toolbar.set_margin_top(14);
        let clock_label = gtk::Label::new(Some("时钟偏移"));
        clock_label.set_css_classes(&["toolbar-label"]);
        toolbar.append(&clock_label);
        toolbar.append(&device_clock_box("A", &spin_a));
        toolbar.append(&device_clock_box("B", &spin_b));
        toolbar.append(&clock_difference);
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        toolbar.append(&spacer);
        toolbar.append(&recalc_button);
        toolbar.append(&save_button);
        toolbar.append(&open_export_button);
        toolbar.append(&export_button);

        let object_title = gtk::Label::new(None);
        object_title.set_xalign(0.0);
        object_title.set_wrap(true);
        object_title.set_css_classes(&["object-title"]);
        let object_meta = gtk::Label::new(None);
        object_meta.set_xalign(0.0);
        object_meta.set_wrap(true);
        object_meta.set_css_classes(&["mono", "muted"]);
        let object_note = gtk::Label::new(None);
        object_note.set_xalign(0.0);
        object_note.set_wrap(true);
        object_note.set_css_classes(&["object-note"]);
        let object_card = gtk::Box::new(gtk::Orientation::Vertical, 6);
        object_card.set_css_classes(&["card", "object-card"]);
        object_card.append(&object_title);
        object_card.append(&object_meta);
        object_card.append(&object_note);

        let list_heading = gtk::Label::new(Some("片段 · 原始顺序"));
        list_heading.set_xalign(0.0);
        list_heading.set_css_classes(&["section-label"]);
        let segment_list = gtk::ListBox::new();
        segment_list.set_selection_mode(gtk::SelectionMode::Single);
        segment_list.set_activate_on_single_click(true);
        segment_list.set_css_classes(&["segment-list"]);
        let list_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&segment_list)
            .build();
        list_scroll.set_css_classes(&["list-scroll"]);
        let order_hint =
            gtk::Label::new(Some("行顺序固定；Q 为修正后的队列名次，不改变列表顺序。"));
        order_hint.set_xalign(0.0);
        order_hint.set_wrap(true);
        order_hint.set_css_classes(&["hint"]);
        let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 10);
        sidebar.set_css_classes(&["sidebar"]);
        sidebar.set_size_request(370, -1);
        sidebar.set_margin_start(16);
        sidebar.append(&object_card);
        sidebar.append(&list_heading);
        sidebar.append(&list_scroll);
        sidebar.append(&order_hint);

        let detail_box = gtk::Box::new(gtk::Orientation::Vertical, 14);
        detail_box.set_margin_top(2);
        detail_box.set_margin_bottom(20);
        detail_box.set_margin_start(4);
        detail_box.set_margin_end(16);
        let detail_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&detail_box)
            .build();
        let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
        paned.set_start_child(Some(&sidebar));
        paned.set_end_child(Some(&detail_scroll));
        paned.set_position(370);
        paned.set_resize_start_child(false);
        paned.set_shrink_start_child(false);
        paned.set_vexpand(true);
        paned.set_margin_top(14);

        let status_label = gtk::Label::new(None);
        status_label.set_xalign(0.0);
        status_label.set_hexpand(true);
        status_label.set_ellipsize(pango::EllipsizeMode::Middle);
        status_label.set_css_classes(&["status-text"]);
        let save_label = gtk::Label::new(Some("自动保存已启用"));
        save_label.set_css_classes(&["save-text"]);
        let progress = gtk::ProgressBar::new();
        progress.set_visible(false);
        progress.set_size_request(150, -1);
        let footer = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        footer.set_css_classes(&["footer"]);
        footer.set_margin_start(16);
        footer.set_margin_end(16);
        footer.set_margin_top(8);
        footer.set_margin_bottom(10);
        footer.append(&status_label);
        footer.append(&progress);
        footer.append(&save_label);

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.append(&toolbar);
        root.append(&paned);
        root.append(&footer);
        window.set_child(Some(&root));

        let ui = Rc::new(Self {
            window,
            store,
            session: RefCell::new(session),
            selected: RefCell::new(None),
            busy_segment: RefCell::new(None),
            last_export: RefCell::new(None),
            syncing_clock: Cell::new(false),
            rebuilding_list: Cell::new(false),
            run_dir,
            export_dir,
            object_title,
            object_meta,
            object_note,
            clock_difference,
            spin_a,
            spin_b,
            segment_list,
            detail_box,
            status_label,
            save_label,
            progress,
            open_export_button,
        });
        ui.wire(&save_button, &recalc_button, &export_button);
        ui
    }

    fn wire(self: &Rc<Self>, save: &gtk::Button, recalc: &gtk::Button, export: &gtk::Button) {
        {
            let app = self.clone();
            save.connect_clicked(move |_| {
                if app.persist() {
                    app.set_status("会话已保存；字段与片段顺序保持不变");
                }
            });
        }
        {
            let app = self.clone();
            recalc.connect_clicked(move |_| {
                app.refresh_all();
                app.set_status("队列已按生效优先级、修正后起点、原始顺序重新计算");
            });
        }
        {
            let app = self.clone();
            export.connect_clicked(move |_| app.export_session(None));
        }
        {
            let app = self.clone();
            self.open_export_button.connect_clicked(move |_| {
                let path = app.last_export.borrow().clone();
                if let Some(path) = path
                    && let Ok(uri) = glib::filename_to_uri(&path, None)
                {
                    let _ = gio::AppInfo::launch_default_for_uri(
                        uri.as_str(),
                        None::<&gio::AppLaunchContext>,
                    );
                }
            });
        }
        {
            let app = self.clone();
            self.spin_a.connect_value_changed(move |spin| {
                if app.syncing_clock.get() {
                    return;
                }
                app.change_offset(DeviceId::A, spin.value() as i64);
            });
        }
        {
            let app = self.clone();
            self.spin_b.connect_value_changed(move |spin| {
                if app.syncing_clock.get() {
                    return;
                }
                app.change_offset(DeviceId::B, spin.value() as i64);
            });
        }
        {
            let app = self.clone();
            self.segment_list.connect_row_selected(move |_, row| {
                if app.rebuilding_list.get() {
                    return;
                }
                let Some(row) = row else {
                    return;
                };
                let id = row.widget_name().to_string();
                if id.is_empty() {
                    return;
                }
                *app.selected.borrow_mut() = Some(id);
                app.rebuild_detail();
            });
        }
        {
            let app = self.clone();
            self.window.connect_close_request(move |_| {
                app.persist();
                glib::Propagation::Proceed
            });
        }
    }

    fn refresh_all(self: &Rc<Self>) {
        let session = self.session.borrow().clone();
        if self.selected.borrow().is_none() {
            *self.selected.borrow_mut() =
                session.segments.first().map(|segment| segment.id.clone());
        }
        self.object_title.set_text(&session.duplicate_object.label);
        self.object_meta.set_text(&format!(
            "{} · {} 个片段 · v{}",
            session.duplicate_object.object_id,
            session.segments.len(),
            APP_VERSION
        ));
        self.object_note.set_text(&session.duplicate_object.note);

        self.syncing_clock.set(true);
        self.spin_a
            .set_value(session.devices[0].clock_offset_ms as f64);
        self.spin_b
            .set_value(session.devices[1].clock_offset_ms as f64);
        self.syncing_clock.set(false);
        let delta = session.devices[0].clock_offset_ms - session.devices[1].clock_offset_ms;
        self.clock_difference
            .set_text(&format!("A−B = {} ms", signed_ms(delta)));
        self.rebuild_list(&session);
        self.rebuild_detail();
    }

    fn rebuild_list(self: &Rc<Self>, session: &Session) {
        self.rebuilding_list.set(true);
        while let Some(child) = self.segment_list.first_child() {
            self.segment_list.remove(&child);
        }
        let ranks = session.queue_ranks();
        let selected = self.selected.borrow().clone();
        let mut selected_row = None;
        for segment in &session.segments {
            let rank = ranks.get(&segment.id).copied().unwrap_or_default();
            let row = self.segment_row(
                segment,
                rank,
                session.normalized_start_ms(segment),
                session.latest_run(&segment.id),
            );
            row.set_widget_name(&segment.id);
            self.segment_list.append(&row);
            if selected.as_deref() == Some(segment.id.as_str()) {
                selected_row = Some(row);
            }
        }
        if let Some(row) = selected_row {
            self.segment_list.select_row(Some(&row));
        }
        self.rebuilding_list.set(false);
    }

    fn segment_row(
        self: &Rc<Self>,
        segment: &Segment,
        rank: u32,
        normalized_start_ms: i64,
        latest: Option<&RunRecord>,
    ) -> gtk::ListBoxRow {
        let row = gtk::ListBoxRow::new();
        row.set_activatable(true);
        row.set_selectable(true);
        row.set_css_classes(&["segment-row"]);
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 9);
        content.set_margin_top(9);
        content.set_margin_bottom(9);
        content.set_margin_start(10);
        content.set_margin_end(10);

        let order = gtk::Label::new(Some(&format!("{:02}", segment.ordinal)));
        order.set_css_classes(&["ordinal"]);
        order.set_size_request(28, -1);
        let rank_label = gtk::Label::new(Some(&format!("Q{rank}")));
        rank_label.set_css_classes(&["rank-pill"]);
        let left = gtk::Box::new(gtk::Orientation::Vertical, 4);
        left.append(&order);
        left.append(&rank_label);
        content.append(&left);

        let middle = gtk::Box::new(gtk::Orientation::Vertical, 3);
        middle.set_hexpand(true);
        let title = gtk::Label::new(Some(&segment.label));
        title.set_xalign(0.0);
        title.set_ellipsize(pango::EllipsizeMode::End);
        title.set_css_classes(&["segment-title"]);
        let timeline = gtk::Label::new(Some(&format!(
            "{} · {} → {}",
            segment.device.label(),
            format_ms(normalized_start_ms),
            format_ms(normalized_start_ms + segment.duration_ms())
        )));
        timeline.set_xalign(0.0);
        timeline.set_css_classes(&["segment-meta"]);
        let run_meta_text = match latest {
            Some(run) => format!(
                "{} · {} · {}",
                run.status.label(),
                format_ms(run.duration_ms as i64),
                short_hash(run.output_sha256.as_deref())
            ),
            None => "尚未重跑".to_owned(),
        };
        let run_meta = gtk::Label::new(Some(&run_meta_text));
        run_meta.set_xalign(0.0);
        run_meta.set_css_classes(&["segment-run-meta"]);
        middle.append(&title);
        middle.append(&timeline);
        middle.append(&run_meta);
        content.append(&middle);

        let right = gtk::Box::new(gtk::Orientation::Vertical, 5);
        right.set_valign(gtk::Align::Center);
        let priority = gtk::Label::new(Some(&format!("P {}", segment.effective_priority())));
        priority.set_css_classes(&["priority-pill"]);
        let status = gtk::Label::new(Some(segment.status.label()));
        status.set_css_classes(&["status-pill", status_class(segment.status)]);
        right.append(&priority);
        right.append(&status);
        content.append(&right);

        let run_button = gtk::Button::with_label("重跑");
        run_button.set_css_classes(&["mini-button"]);
        run_button.set_valign(gtk::Align::Center);
        {
            let app = self.clone();
            let id = segment.id.clone();
            run_button.connect_clicked(move |_| app.run_segment(&id));
        }
        content.append(&run_button);
        row.set_child(Some(&content));
        row
    }

    fn rebuild_detail(self: &Rc<Self>) {
        while let Some(child) = self.detail_box.first_child() {
            self.detail_box.remove(&child);
        }
        let session = self.session.borrow().clone();
        let selected = self.selected.borrow().clone();
        let Some(segment) = selected
            .as_deref()
            .and_then(|id| session.segment(id))
            .cloned()
        else {
            let empty = gtk::Box::new(gtk::Orientation::Vertical, 6);
            empty.set_valign(gtk::Align::Center);
            empty.set_halign(gtk::Align::Center);
            empty.set_vexpand(true);
            let title = gtk::Label::new(Some("选择一个媒体片段"));
            title.set_css_classes(&["empty-title"]);
            let hint = gtk::Label::new(Some("可单独重跑、比较和导出，不会改动其他片段。"));
            hint.set_css_classes(&["muted"]);
            empty.append(&title);
            empty.append(&hint);
            self.detail_box.append(&empty);
            return;
        };
        let rank = session
            .queue_ranks()
            .get(&segment.id)
            .copied()
            .unwrap_or_default();
        let offset = session.offset_for(segment.device);
        let normalized = session.normalized_start_ms(&segment);
        let latest = session.latest_run(&segment.id).cloned();

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let heading = gtk::Box::new(gtk::Orientation::Vertical, 4);
        heading.set_hexpand(true);
        let title = gtk::Label::new(Some(&format!("{:02} · {}", segment.ordinal, segment.label)));
        title.set_xalign(0.0);
        title.set_wrap(true);
        title.set_css_classes(&["detail-title"]);
        let subtitle = gtk::Label::new(Some(&format!(
            "{} · 队列 Q{} · 生效优先级 {}",
            segment.device.label(),
            rank,
            segment.effective_priority()
        )));
        subtitle.set_xalign(0.0);
        subtitle.set_css_classes(&["muted"]);
        heading.append(&title);
        heading.append(&subtitle);
        header.append(&heading);

        let run_button = gtk::Button::with_label("重跑此片段");
        run_button.set_css_classes(&["primary-button"]);
        {
            let app = self.clone();
            let id = segment.id.clone();
            run_button.connect_clicked(move |_| app.run_segment(&id));
        }
        let export_button = gtk::Button::with_label("单段导出");
        export_button.set_css_classes(&["toolbar-button"]);
        {
            let app = self.clone();
            let id = segment.id.clone();
            export_button.connect_clicked(move |_| app.export_session(Some(id.clone())));
        }
        header.append(&run_button);
        header.append(&export_button);
        self.detail_box.append(&header);

        let overview = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        overview.set_homogeneous(true);
        overview.append(&info_card(
            "设备时钟",
            &format!("{} · 偏移 {} ms", segment.device.label(), signed_ms(offset)),
            &format!(
                "原始 {} → 修正 {}",
                format_ms(segment.capture_start_ms + segment.in_ms),
                format_ms(normalized)
            ),
        ));
        overview.append(&info_card(
            "区间",
            &format!(
                "{} → {}",
                format_ms(segment.in_ms),
                format_ms(segment.out_ms)
            ),
            &format!("镜头时长 {}", format_ms(segment.duration_ms())),
        ));
        overview.append(&info_card(
            "队列",
            &format!("Q{rank} · 生效优先级 {}", segment.effective_priority()),
            &format!(
                "原始 {} / 局部覆盖 {}",
                segment.base_priority,
                segment
                    .priority_override
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "无".to_owned())
            ),
        ));
        self.detail_box.append(&overview);
        self.detail_box.append(&source_card(&segment));
        self.detail_box.append(&self.priority_card(&segment));
        self.detail_box
            .append(&self.comparison_card(&segment, latest.as_ref()));
        self.detail_box
            .append(&self.evidence_card(&session, &segment));
    }

    fn priority_card(self: &Rc<Self>, segment: &Segment) -> gtk::Widget {
        let card = card_box();
        let title = section_title("局部优先级修正");
        card.append(&title);
        let explanation = gtk::Label::new(Some(
            "这里只改变该片段的覆盖值；原始优先级和列表顺序会保留，并写入证据日志。",
        ));
        explanation.set_xalign(0.0);
        explanation.set_wrap(true);
        explanation.set_css_classes(&["muted"]);
        card.append(&explanation);

        let editor = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let spin = gtk::SpinButton::with_range(-1000.0, 1000.0, 1.0);
        spin.set_value(segment.effective_priority() as f64);
        spin.set_width_chars(6);
        let note = gtk::Entry::new();
        note.set_placeholder_text(Some("修正原因（写入证据）"));
        note.set_hexpand(true);
        note.set_text(&segment.correction_note);
        let apply = gtk::Button::with_label("应用覆盖");
        apply.set_css_classes(&["primary-button"]);
        {
            let app = self.clone();
            let id = segment.id.clone();
            let spin = spin.clone();
            let note = note.clone();
            apply.connect_clicked(move |_| {
                app.apply_priority(&id, spin.value() as i32, note.text().as_str());
            });
        }
        editor.append(&spin);
        editor.append(&note);
        editor.append(&apply);
        card.append(&editor);

        let quick = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        for (label, delta) in [("−10", -10), ("−1", -1), ("+1", 1), ("+10", 10)] {
            let button = gtk::Button::with_label(label);
            button.set_css_classes(&["toolbar-button"]);
            let app = self.clone();
            let id = segment.id.clone();
            button.connect_clicked(move |_| app.nudge_priority(&id, delta));
            quick.append(&button);
        }
        let reset = gtk::Button::with_label("恢复原始优先级");
        reset.set_css_classes(&["toolbar-button"]);
        {
            let app = self.clone();
            let id = segment.id.clone();
            reset.connect_clicked(move |_| app.reset_priority(&id));
        }
        quick.append(&reset);
        card.append(&quick);
        card.upcast()
    }

    fn comparison_card(
        self: &Rc<Self>,
        segment: &Segment,
        latest: Option<&RunRecord>,
    ) -> gtk::Widget {
        let card = card_box();
        let heading = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let title = section_title("单片段比较");
        title.set_hexpand(true);
        heading.append(&title);
        let count = self.session.borrow().runs_for(&segment.id).len();
        let hint = gtk::Label::new(Some(&format!("已保留 {count} 次运行")));
        hint.set_css_classes(&["muted"]);
        heading.append(&hint);
        card.append(&heading);

        let previous = {
            let runs = self.session.borrow();
            let all = runs.runs_for(&segment.id);
            all.len()
                .checked_sub(2)
                .and_then(|index| all.get(index))
                .copied()
                .cloned()
        };
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        row.set_homogeneous(true);
        row.append(&comparison_run_card("最新运行", latest));
        row.append(&comparison_run_card("更早运行", previous.as_ref()));
        card.append(&row);

        if latest.is_none() {
            let empty = gtk::Label::new(Some(
                "此片段尚未生成产物。点击“重跑此片段”后，可在此比较不同尝试的校验和、耗时和首帧。",
            ));
            empty.set_wrap(true);
            empty.set_xalign(0.0);
            empty.set_css_classes(&["hint"]);
            card.append(&empty);
        } else if latest.is_some_and(|run| run.status == RunStatus::Failed) {
            let failed = gtk::Label::new(Some(
                "最新一次运行失败；失败原因和管线已写入 evidence.jsonl，旧产物不会被覆盖记录。",
            ));
            failed.set_wrap(true);
            failed.set_xalign(0.0);
            failed.set_css_classes(&["warning-text"]);
            card.append(&failed);
        }
        card.upcast()
    }

    fn evidence_card(self: &Rc<Self>, session: &Session, segment: &Segment) -> gtk::Widget {
        let card = card_box();
        let heading = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let title = section_title("关键证据 · 最近事件");
        title.set_hexpand(true);
        heading.append(&title);
        let count = gtk::Label::new(Some(&format!("{} 条 · 追加写入", session.evidence.len())));
        count.set_css_classes(&["muted"]);
        heading.append(&count);
        card.append(&heading);
        for event in session.evidence.iter().rev().take(8) {
            let row = gtk::Box::new(gtk::Orientation::Vertical, 2);
            row.set_css_classes(&["evidence-row"]);
            let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let seq = gtk::Label::new(Some(&format!("#{:03}", event.seq)));
            seq.set_css_classes(&["mono", "muted"]);
            let kind = gtk::Label::new(Some(event.kind.label()));
            kind.set_css_classes(&["evidence-kind"]);
            let time = gtk::Label::new(Some(&event.at));
            time.set_css_classes(&["mono", "muted"]);
            time.set_hexpand(true);
            time.set_xalign(1.0);
            top.append(&seq);
            top.append(&kind);
            top.append(&time);
            let summary = gtk::Label::new(Some(
                &event
                    .segment_id
                    .as_deref()
                    .filter(|id| *id != segment.id)
                    .map(|id| format!("[{id}] {}", event.summary))
                    .unwrap_or_else(|| event.summary.clone()),
            ));
            summary.set_xalign(0.0);
            summary.set_wrap(true);
            summary.set_css_classes(&["evidence-summary"]);
            row.append(&top);
            row.append(&summary);
            card.append(&row);
        }
        card.upcast()
    }

    fn change_offset(self: &Rc<Self>, device: DeviceId, offset_ms: i64) {
        {
            let mut session = self.session.borrow_mut();
            if session.offset_for(device) == offset_ms {
                return;
            }
            session.set_device_offset(device, offset_ms, "GTK4 工具栏局部修正");
        }
        self.persist();
        self.refresh_all();
        self.set_status(&format!(
            "{} 时钟偏移已修正为 {} ms；片段顺序不变",
            device.label(),
            signed_ms(offset_ms)
        ));
    }

    fn apply_priority(self: &Rc<Self>, segment_id: &str, priority: i32, note: &str) {
        let result =
            self.session
                .borrow_mut()
                .set_priority_override(segment_id, Some(priority), note);
        match result {
            Ok(()) => {
                self.persist();
                self.refresh_all();
                self.set_status(&format!("片段 {segment_id} 已局部修正为优先级 {priority}"));
            }
            Err(error) => self.set_status(&error),
        }
    }

    fn nudge_priority(self: &Rc<Self>, segment_id: &str, delta: i32) {
        let current = self
            .session
            .borrow()
            .segment(segment_id)
            .map(Segment::effective_priority);
        if let Some(current) = current {
            let value = current.saturating_add(delta).clamp(-1000, 1000);
            let note = format!("快捷调整 {delta:+}（{current} → {value}）");
            let result =
                self.session
                    .borrow_mut()
                    .set_priority_override(segment_id, Some(value), &note);
            if result.is_ok() {
                self.persist();
                self.refresh_all();
                self.set_status(&format!("片段 {segment_id} 优先级已调整为 {value}"));
            }
        }
    }

    fn reset_priority(self: &Rc<Self>, segment_id: &str) {
        let result =
            self.session
                .borrow_mut()
                .set_priority_override(segment_id, None, "恢复原始优先级");
        if result.is_ok() {
            self.persist();
            self.refresh_all();
            self.set_status(&format!("片段 {segment_id} 已恢复原始优先级"));
        }
    }

    fn persist(self: &Rc<Self>) -> bool {
        let session = self.session.borrow().clone();
        match self.store.save(&session) {
            Ok(()) => {
                self.save_label.set_text(&format!(
                    "已保存 {}",
                    chrono::Local::now().format("%H:%M:%S")
                ));
                self.save_label.set_css_classes(&["save-text"]);
                true
            }
            Err(error) => {
                self.save_label.set_text("保存失败");
                self.save_label
                    .set_css_classes(&["save-text", "error-text"]);
                self.set_status(&format!("保存失败：{error:#}"));
                false
            }
        }
    }

    fn set_status(&self, message: &str) {
        self.status_label.set_text(message);
    }

    fn run_segment(self: &Rc<Self>, segment_id: &str) {
        if self.busy_segment.borrow().is_some() {
            self.set_status("已有片段在运行，请等待当前任务结束");
            return;
        }
        let prepared = {
            let mut session = self.session.borrow_mut();
            let Some(segment) = session.segment(segment_id).cloned() else {
                return;
            };
            let run_seq = session.next_run_seq;
            session.next_run_seq += 1;
            let run_id = format!("run-{run_seq:04}");
            let attempt = session.runs_for(segment_id).len() as u32 + 1;
            let output_path = self
                .run_dir
                .join(safe_component(segment_id))
                .join(format!("{run_id}.webm"));
            let preview_path = self
                .run_dir
                .join(safe_component(segment_id))
                .join(format!("{run_id}.preview.png"));
            let pipeline =
                transcode::pipeline_description(Path::new(&segment.source_path), &output_path);
            if let Some(segment) = session.segment_mut(segment_id) {
                segment.status = SegmentStatus::Running;
                segment.latest_run_id = Some(run_id.clone());
            }
            session.touch();
            session.append_evidence(
                crate::model::EvidenceKind::RunStarted,
                Some(segment_id.to_owned()),
                Some(run_id.clone()),
                format!("开始第 {attempt} 次单段重跑"),
                vec![
                    field("attempt", attempt),
                    field("input_path", &segment.source_path),
                    field(
                        "input_sha256",
                        segment.source_sha256.as_deref().unwrap_or("—"),
                    ),
                    field("output_path", output_path.display()),
                    field("in_ms", segment.in_ms),
                    field("out_ms", segment.out_ms),
                    field("effective_priority", segment.effective_priority()),
                    field("pipeline", &pipeline),
                ],
            );
            PreparedRun {
                run_id,
                segment_id: segment_id.to_owned(),
                attempt,
                started_at: now_rfc3339(),
                started: Instant::now(),
                input_path: PathBuf::from(&segment.source_path),
                output_path,
                preview_path,
                pipeline,
                input_sha256: segment.source_sha256.clone(),
                in_ms: segment.in_ms,
                out_ms: segment.out_ms,
                device_offset_ms: session.offset_for(segment.device),
                effective_priority: segment.effective_priority(),
                gst_version: transcode::gstreamer_version(),
            }
        };
        *self.busy_segment.borrow_mut() = Some(segment_id.to_owned());
        self.progress.set_visible(true);
        self.progress.pulse();
        self.persist();
        self.refresh_all();
        self.set_status(&format!(
            "正在重跑 {}（第 {} 次）…",
            segment_id, prepared.attempt
        ));

        let (sender, receiver) =
            mpsc::channel::<(PreparedRun, Result<TranscodeOutput, anyhow::Error>)>();
        let input = prepared.input_path.clone();
        let output = prepared.output_path.clone();
        let preview = prepared.preview_path.clone();
        let trim = Some((prepared.in_ms, prepared.out_ms));
        std::thread::spawn(move || {
            let result = transcode::transcode_segment(&input, &output, Some(&preview), trim);
            let _ = sender.send((prepared, result));
        });

        let app = self.clone();
        let progress = self.progress.clone();
        glib::timeout_add_local(Duration::from_millis(120), move || {
            progress.pulse();
            match receiver.try_recv() {
                Ok((prepared, result)) => {
                    app.finish_run(prepared, result);
                    glib::ControlFlow::Break
                }
                Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(mpsc::TryRecvError::Disconnected) => {
                    app.set_status("转码线程意外断开，未写入成功记录");
                    *app.busy_segment.borrow_mut() = None;
                    progress.set_visible(false);
                    app.refresh_all();
                    glib::ControlFlow::Break
                }
            }
        });
    }

    fn finish_run(
        self: &Rc<Self>,
        prepared: PreparedRun,
        result: Result<TranscodeOutput, anyhow::Error>,
    ) {
        let elapsed_ms = prepared.started.elapsed().as_millis() as u64;
        let mut session = self.session.borrow_mut();
        let (
            status,
            input_sha,
            output_path,
            output_sha,
            output_bytes,
            preview_path,
            gst_version,
            error,
        ) = match result {
            Ok(output) => (
                RunStatus::Succeeded,
                output.input_sha256,
                Some(output.output_path.to_string_lossy().into_owned()),
                Some(output.output_sha256),
                Some(output.output_bytes),
                output
                    .preview_path
                    .map(|path| path.to_string_lossy().into_owned()),
                output.gst_version,
                None,
            ),
            Err(error) => (
                RunStatus::Failed,
                prepared.input_sha256.clone(),
                Some(prepared.output_path.to_string_lossy().into_owned()),
                None,
                None,
                None,
                prepared.gst_version.clone(),
                Some(format!("{error:#}")),
            ),
        };
        if let Some(segment) = session.segment_mut(&prepared.segment_id) {
            segment.status = if status == RunStatus::Succeeded {
                SegmentStatus::Ready
            } else {
                SegmentStatus::Failed
            };
            segment.latest_run_id = Some(prepared.run_id.clone());
            if status == RunStatus::Succeeded {
                segment.source_sha256 = input_sha.clone();
            }
        }
        let record = RunRecord {
            run_id: prepared.run_id.clone(),
            segment_id: prepared.segment_id.clone(),
            attempt: prepared.attempt,
            in_ms: prepared.in_ms,
            out_ms: prepared.out_ms,
            device_offset_ms: prepared.device_offset_ms,
            effective_priority: prepared.effective_priority,
            status,
            started_at: prepared.started_at.clone(),
            finished_at: now_rfc3339(),
            duration_ms: elapsed_ms,
            input_path: prepared.input_path.to_string_lossy().into_owned(),
            input_sha256: input_sha,
            output_path: output_path.clone(),
            output_sha256: output_sha.clone(),
            output_bytes,
            preview_path: preview_path.clone(),
            pipeline: prepared.pipeline.clone(),
            gst_version,
            tool_version: APP_VERSION.to_owned(),
            error: error.clone(),
        };
        session.runs.push(record);
        session.append_evidence(
            crate::model::EvidenceKind::RunFinished,
            Some(prepared.segment_id.clone()),
            Some(prepared.run_id.clone()),
            format!(
                "第 {} 次运行{}",
                prepared.attempt,
                if status == RunStatus::Succeeded {
                    "成功"
                } else {
                    "失败"
                }
            ),
            vec![
                field("status", run_status_key(status)),
                field("elapsed_ms", elapsed_ms),
                field("output_path", output_path.as_deref().unwrap_or("—")),
                field("output_sha256", output_sha.as_deref().unwrap_or("—")),
                field("output_bytes", output_bytes.unwrap_or_default()),
                field("precision", "milliseconds"),
                field("preview_path", preview_path.as_deref().unwrap_or("—")),
                field("gst_version", &prepared.gst_version),
                field("error", error.as_deref().unwrap_or("—")),
            ],
        );
        session.touch();
        drop(session);

        *self.busy_segment.borrow_mut() = None;
        self.progress.set_visible(false);
        self.persist();
        self.refresh_all();
        let message = match status {
            RunStatus::Succeeded => format!(
                "{} 第 {} 次运行成功；产物与哈希已留证",
                prepared.segment_id, prepared.attempt
            ),
            _ => format!(
                "{} 第 {} 次运行失败；错误已写入证据",
                prepared.segment_id, prepared.attempt
            ),
        };
        self.set_status(&message);
    }

    fn export_session(self: &Rc<Self>, segment_id: Option<String>) {
        let mut snapshot = self.session.borrow().clone();
        if let Some(id) = &segment_id {
            snapshot.segments.retain(|segment| &segment.id == id);
            snapshot.runs.retain(|run| &run.segment_id == id);
            snapshot.session_id = format!("{}-{}", snapshot.session_id, safe_component(id));
        }
        let directory = unique_export_dir(&self.export_dir, segment_id.as_deref());
        let export_fields = vec![
            field("export_dir", directory.display()),
            field("manifest", directory.join("manifest.json").display()),
            field("checksums", directory.join("checksums.sha256").display()),
            field("scope", segment_id.as_deref().unwrap_or("entire_session")),
        ];
        snapshot.append_evidence(
            crate::model::EvidenceKind::ExportCreated,
            segment_id.clone(),
            None,
            format!("导出复核包：{}", directory.display()),
            export_fields.clone(),
        );
        match export_bundle(&snapshot, &directory) {
            Ok(bundle) => {
                {
                    let mut session = self.session.borrow_mut();
                    session.append_evidence(
                        crate::model::EvidenceKind::ExportCreated,
                        segment_id.clone(),
                        None,
                        format!("导出复核包：{}", bundle.directory.display()),
                        export_fields,
                    );
                    session.touch();
                }
                *self.last_export.borrow_mut() = Some(bundle.directory.clone());
                self.open_export_button.set_sensitive(true);
                self.persist();
                self.refresh_all();
                self.set_status(&format!(
                    "复核包已导出：{}（{} 个证据文件，{} 个媒体副本）",
                    bundle.directory.display(),
                    bundle.file_count,
                    bundle.media_count
                ));
            }
            Err(error) => self.set_status(&format!("导出失败：{error:#}")),
        }
    }
}

#[derive(Debug)]
struct PreparedRun {
    run_id: String,
    segment_id: String,
    attempt: u32,
    started_at: String,
    started: Instant,
    input_path: PathBuf,
    output_path: PathBuf,
    preview_path: PathBuf,
    pipeline: String,
    input_sha256: Option<String>,
    in_ms: i64,
    out_ms: i64,
    device_offset_ms: i64,
    effective_priority: i32,
    gst_version: String,
}

fn axis_spin() -> gtk::SpinButton {
    let spin = gtk::SpinButton::with_range(-60_000.0, 60_000.0, 100.0);
    spin.set_width_chars(7);
    spin.set_tooltip_text(Some("设备时钟读数减去共享基准时钟；正值表示偏快"));
    spin
}

fn device_clock_box(label: &str, spin: &gtk::SpinButton) -> gtk::Box {
    let container = gtk::Box::new(gtk::Orientation::Horizontal, 5);
    let name = gtk::Label::new(Some(&format!("{label} 偏移")));
    name.set_css_classes(&["toolbar-label"]);
    let unit = gtk::Label::new(Some("ms"));
    unit.set_css_classes(&["muted"]);
    container.append(&name);
    container.append(spin);
    container.append(&unit);
    container
}

fn signed_ms(value: i64) -> String {
    if value > 0 {
        format!("+{value}")
    } else {
        value.to_string()
    }
}

fn short_hash(value: Option<&str>) -> String {
    match value {
        Some(hash) if hash.len() >= 12 => format!("{}…", &hash[..12]),
        Some(hash) => hash.to_owned(),
        None => "无哈希".to_owned(),
    }
}

fn status_class(status: SegmentStatus) -> &'static str {
    match status {
        SegmentStatus::Pending => "status-pending",
        SegmentStatus::Running => "status-running",
        SegmentStatus::Ready => "status-ready",
        SegmentStatus::Failed => "status-failed",
    }
}

fn run_status_key(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Succeeded => "succeeded",
        RunStatus::Failed => "failed",
        RunStatus::Interrupted => "interrupted",
    }
}

fn card_box() -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 9);
    card.set_css_classes(&["card"]);
    card
}

fn section_title(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_css_classes(&["section-title"]);
    label
}

fn info_card(title: &str, primary: &str, secondary: &str) -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 4);
    card.set_css_classes(&["info-card"]);
    card.set_hexpand(true);
    let title = gtk::Label::new(Some(title));
    title.set_xalign(0.0);
    title.set_css_classes(&["info-label"]);
    let primary = gtk::Label::new(Some(primary));
    primary.set_xalign(0.0);
    primary.set_wrap(true);
    primary.set_css_classes(&["info-value"]);
    let secondary = gtk::Label::new(Some(secondary));
    secondary.set_xalign(0.0);
    secondary.set_wrap(true);
    secondary.set_css_classes(&["muted"]);
    card.append(&title);
    card.append(&primary);
    card.append(&secondary);
    card
}

fn source_card(segment: &Segment) -> gtk::Box {
    let card = card_box();
    let title = section_title("输入与可追溯性");
    card.append(&title);
    let path = gtk::Label::new(Some(&segment.source_path));
    path.set_xalign(0.0);
    path.set_selectable(true);
    path.set_wrap(true);
    path.set_css_classes(&["mono", "path-text"]);
    card.append(&path);
    let digest = gtk::Label::new(Some(&format!(
        "源文件 SHA-256  {}",
        segment.source_sha256.as_deref().unwrap_or("尚未计算")
    )));
    digest.set_xalign(0.0);
    digest.set_selectable(true);
    digest.set_wrap(true);
    digest.set_css_classes(&["mono", "muted"]);
    card.append(&digest);
    if !segment.correction_note.is_empty() {
        let note = gtk::Label::new(Some(&format!("修正备注：{}", segment.correction_note)));
        note.set_xalign(0.0);
        note.set_wrap(true);
        note.set_css_classes(&["hint"]);
        card.append(&note);
    }
    card
}

fn comparison_run_card(title: &str, run: Option<&RunRecord>) -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 7);
    card.set_css_classes(&["comparison-card"]);
    card.set_hexpand(true);
    let heading = gtk::Box::new(gtk::Orientation::Horizontal, 7);
    let title = gtk::Label::new(Some(title));
    title.set_xalign(0.0);
    title.set_hexpand(true);
    title.set_css_classes(&["comparison-title"]);
    heading.append(&title);
    if let Some(run) = run {
        let badge = gtk::Label::new(Some(run.status.label()));
        badge.set_css_classes(&[
            "status-pill",
            match run.status {
                RunStatus::Succeeded => "status-ready",
                RunStatus::Failed => "status-failed",
                RunStatus::Interrupted => "status-running",
            },
        ]);
        heading.append(&badge);
    }
    card.append(&heading);
    if let Some(path) = run
        .and_then(|run| run.preview_path.as_deref())
        .filter(|path| Path::new(path).is_file())
    {
        let picture = gtk::Picture::new();
        picture.set_content_fit(gtk::ContentFit::Contain);
        picture.set_size_request(280, 120);
        picture.set_css_classes(&["preview"]);
        picture.set_filename(Some(path));
        card.append(&picture);
    } else {
        let placeholder = gtk::Box::new(gtk::Orientation::Vertical, 0);
        placeholder.set_size_request(280, 120);
        placeholder.set_valign(gtk::Align::Center);
        placeholder.set_halign(gtk::Align::Fill);
        placeholder.set_css_classes(&["preview-placeholder"]);
        let label = gtk::Label::new(Some(if run.is_some() {
            "暂无首帧预览"
        } else {
            "等待首次运行"
        }));
        label.set_valign(gtk::Align::Center);
        label.set_css_classes(&["hint"]);
        placeholder.append(&label);
        card.append(&placeholder);
    }
    match run {
        Some(run) => {
            let metrics = gtk::Label::new(Some(&format!(
                "第 {} 次 · {} · {}\nSHA {}",
                run.attempt,
                format_ms(run.duration_ms as i64),
                run.output_bytes
                    .map(format_bytes)
                    .unwrap_or_else(|| "无产物".to_owned()),
                short_hash(run.output_sha256.as_deref())
            )));
            metrics.set_xalign(0.0);
            metrics.set_wrap(true);
            metrics.set_css_classes(&["mono", "comparison-metrics"]);
            card.append(&metrics);
            if let Some(error) = &run.error {
                let error = gtk::Label::new(Some(error));
                error.set_xalign(0.0);
                error.set_wrap(true);
                error.set_css_classes(&["error-text"]);
                card.append(&error);
            }
        }
        None => {
            let empty = gtk::Label::new(Some(
                "没有上一版可比较。\n再次重跑同一片段后，这里会并排显示两次运行的哈希、耗时和首帧。",
            ));
            empty.set_xalign(0.0);
            empty.set_wrap(true);
            empty.set_css_classes(&["hint"]);
            card.append(&empty);
        }
    }
    card
}

fn unique_export_dir(root: &Path, segment_id: Option<&str>) -> PathBuf {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let scope = segment_id
        .map(safe_component)
        .unwrap_or_else(|| "session".to_owned());
    let base = format!("review-{stamp}-{scope}");
    for suffix in 0..1000 {
        let name = if suffix == 0 {
            base.clone()
        } else {
            format!("{base}-{suffix}")
        };
        let candidate = root.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    root.join(format!("{base}-overflow"))
}

fn install_css() {
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };
    let provider = gtk::CssProvider::new();
    provider.load_from_string(CSS);
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

fn capture_widget(widget: &gtk::ApplicationWindow, path: &Path) -> anyhow::Result<()> {
    let width = widget.width().max(1) as f64;
    let height = widget.height().max(1) as f64;
    let paintable = gtk::WidgetPaintable::new(Some(widget));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, width, height);
    let Some(node) = snapshot.to_node() else {
        anyhow::bail!("控件没有可渲染节点");
    };
    let Some(renderer) = widget.renderer() else {
        anyhow::bail!("窗口尚未获得渲染器");
    };
    let texture = renderer.render_texture(&node, None);
    texture.save_to_png(path)?;
    Ok(())
}

const CSS: &str = r#"
.app-window { background: #f2f5f7; color: #17222d; }
headerbar { background: #10242f; color: #f7fbfd; min-height: 62px; box-shadow: none; }
.header-title { color: #ffffff; font-size: 18px; font-weight: 700; }
.header-subtitle { color: #a9c0ca; font-size: 12px; }
.toolbar-card { background: #ffffff; border: 1px solid #dbe3e8; border-radius: 12px; padding: 10px 12px; }
.toolbar-label { color: #52636e; font-size: 12px; font-weight: 600; }
.clock-difference { color: #0b6070; background: #e2f4f6; border-radius: 999px; padding: 4px 9px; font-size: 12px; font-weight: 700; }
.sidebar { background: #e9eef2; border-radius: 12px; padding: 10px; }
.card { background: #ffffff; border: 1px solid #dbe3e8; border-radius: 12px; padding: 14px; }
.object-card { background: #f8fbfc; }
.object-title { font-size: 18px; font-weight: 700; }
.object-note { color: #596b76; font-size: 12px; }
.section-label { color: #52636e; font-size: 12px; font-weight: 700; margin-top: 2px; }
.hint { color: #6d7d87; font-size: 12px; line-height: 1.35; }
.muted { color: #6d7d87; font-size: 12px; }
.mono { font-family: monospace; font-size: 12px; }
.segment-list { background: transparent; }
.list-scroll { border-radius: 10px; }
.segment-row { background: #f9fbfc; border-radius: 10px; margin-bottom: 4px; }
.segment-row:hover { background: #f0f6fa; }
.segment-row:selected { background: #dcefff; border: 1px solid #8dc4eb; }
.ordinal { color: #6d7d87; font-size: 11px; font-weight: 700; }
.rank-pill { background: #173f53; color: #ffffff; border-radius: 999px; padding: 2px 6px; font-size: 10px; font-weight: 700; }
.segment-title { font-size: 13px; font-weight: 700; }
.segment-meta { color: #52636e; font-size: 11px; }
.segment-run-meta { color: #758791; font-size: 10px; }
.priority-pill { background: #edf4e9; color: #315b24; border-radius: 999px; padding: 2px 8px; font-size: 11px; font-weight: 700; }
.status-pill { border-radius: 999px; padding: 2px 8px; font-size: 10px; font-weight: 700; }
.status-pending { background: #eef1f3; color: #5d6b74; }
.status-running { background: #fff1cf; color: #815600; }
.status-ready { background: #d8f4e2; color: #146438; }
.status-failed { background: #ffe1e1; color: #942727; }
.mini-button { background: #173f53; color: #ffffff; border-radius: 7px; padding: 4px 9px; font-size: 11px; }
.primary-button { background: #0b6070; color: #ffffff; border-radius: 8px; padding: 7px 13px; font-weight: 700; }
.primary-button:hover { background: #0a5260; }
.toolbar-button { background: #eef3f6; color: #263b47; border-radius: 8px; padding: 7px 11px; }
.toolbar-button:hover { background: #e1eaf0; }
.detail-title { font-size: 22px; font-weight: 700; }
.empty-title { font-size: 20px; font-weight: 700; }
.info-card { background: #f7fafb; border-radius: 9px; padding: 10px; }
.info-label { color: #6d7d87; font-size: 10px; font-weight: 700; }
.info-value { font-size: 13px; font-weight: 700; }
.section-title { font-size: 14px; font-weight: 700; }
.path-text { background: #f4f7f9; border-radius: 6px; padding: 6px; }
.comparison-card { background: #f7fafb; border: 1px solid #e2e9ed; border-radius: 9px; padding: 9px; }
.comparison-title { font-size: 12px; font-weight: 700; color: #435560; }
.preview { background: #12232c; border-radius: 7px; }
.preview-placeholder { background: #eaf0f3; border: 1px dashed #c6d3da; border-radius: 7px; }
.comparison-metrics { color: #42545f; }
.evidence-row { background: #f7fafb; border-radius: 7px; padding: 7px 9px; }
.evidence-kind { background: #e4f0f3; color: #0b6070; border-radius: 999px; padding: 1px 7px; font-size: 10px; font-weight: 700; }
.evidence-summary { color: #334752; font-size: 12px; }
.warning-text { color: #8b5d0a; background: #fff7e0; border-radius: 7px; padding: 7px; }
.error-text { color: #9a2828; }
.footer { background: #e9eef2; border-radius: 9px; padding: 4px 10px; }
.status-text { color: #455a65; font-size: 12px; }
.save-text { color: #37634a; font-size: 11px; font-weight: 600; }
progressbar trough { background: #d7e1e7; border-radius: 999px; min-height: 6px; }
progressbar progress { background: #0b6070; border-radius: 999px; min-height: 6px; }
"#;
