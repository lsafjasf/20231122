use crate::gst;
use gtk4::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use submerge_core::{demo_timeline, fmt, Timeline};

pub struct AppState {
    pub timeline: Timeline,
    pub video_label: String,
}

impl Default for AppState {
    fn default() -> Self {
        AppState {
            timeline: demo_timeline(),
            video_label: "示例项目 (120s)".to_string(),
        }
    }
}

#[derive(Clone)]
struct Widgets {
    list: gtk4::ListBox,
    video_info: gtk4::Label,
    status: gtk4::Label,
    gap_spin: gtk4::SpinButton,
    selection_entry: gtk4::Entry,
    order_entry: gtk4::Entry,
}

pub fn build_window(app: &gtk4::Application) -> gtk4::ApplicationWindow {
    let state = Rc::new(RefCell::new(AppState::default()));

    let window = gtk4::ApplicationWindow::builder()
        .application(app)
        .title("字幕时间轴合并工具")
        .default_width(1100)
        .default_height(640)
        .build();

    let open_btn = gtk4::Button::with_label("打开视频…");
    let header = gtk4::HeaderBar::new();
    header.pack_start(&open_btn);
    window.set_titlebar(Some(&header));

    // Left: video preview + metadata.
    let video = gtk4::Video::new();
    video.set_size_request(360, 220);
    let video_info = gtk4::Label::new(Some("未加载视频"));
    video_info.set_selectable(true);
    let left = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
    left.set_margin_start(12);
    left.set_margin_end(12);
    left.set_margin_top(12);
    left.append(&video);
    left.append(&video_info);

    // Right: event list + controls.
    let list = gtk4::ListBox::new();
    list.set_selection_mode(gtk4::SelectionMode::None);
    let scroll = gtk4::ScrolledWindow::builder()
        .child(&list)
        .vexpand(true)
        .build();

    let gap_spin = gtk4::SpinButton::with_range(0.1, 60.0, 0.5);
    gap_spin.set_value(2.0);
    gap_spin.set_digits(1);
    let detect_btn = gtk4::Button::with_label("重新检测冲突");
    let detect_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    detect_row.append(&gtk4::Label::new(Some("空档阈值(秒):")));
    detect_row.append(&gap_spin);
    detect_row.append(&detect_btn);

    let selection_entry = gtk4::Entry::builder()
        .placeholder_text("选中ID, 如 2,3")
        .build();
    let order_entry = gtk4::Entry::builder()
        .placeholder_text("新顺序, 如 1,0")
        .build();
    let reorder_btn = gtk4::Button::with_label("局部重排");
    let reorder_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    reorder_row.append(&selection_entry);
    reorder_row.append(&order_entry);
    reorder_row.append(&reorder_btn);

    let status = gtk4::Label::new(None);
    status.set_xalign(0.0);

    let right = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
    right.set_margin_start(12);
    right.set_margin_end(12);
    right.set_margin_top(12);
    right.set_margin_bottom(12);
    right.append(&gtk4::Label::new(Some(
        "ID | 语言 | 原始时间 | 换算时间 | 冲突原因",
    )));
    right.append(&scroll);
    right.append(&detect_row);
    right.append(&reorder_row);
    right.append(&status);

    let paned = gtk4::Paned::new(gtk4::Orientation::Horizontal);
    paned.set_start_child(Some(&left));
    paned.set_end_child(Some(&right));
    paned.set_position(400);
    window.set_child(Some(&paned));

    let widgets = Widgets {
        list,
        video_info,
        status,
        gap_spin,
        selection_entry,
        order_entry,
    };

    refresh(&state, &widgets);

    connect_open(&window, &open_btn, state.clone(), widgets.clone(), video);
    connect_detect(&detect_btn, state.clone(), widgets.clone());
    connect_reorder(&reorder_btn, state, widgets);

    window
}

fn refresh(state: &Rc<RefCell<AppState>>, w: &Widgets) {
    while let Some(row) = w.list.first_child() {
        w.list.remove(&row);
    }

    let s = state.borrow();
    w.video_info.set_text(&format!(
        "视频: {} | 时长 {} | 空档阈值 {}",
        s.video_label,
        fmt::ns_to_timecode(s.timeline.video_duration_ns),
        fmt::ns_to_duration(s.timeline.gap_threshold_ns),
    ));

    let reasons = s.timeline.reasons_by_event();
    let mut events = s.timeline.events().to_vec();
    events.sort_by_key(|e| (e.unified_start_ns(), e.id));

    let mut n_conflicts = 0usize;
    for e in &events {
        let reason = reasons
            .get(&e.id)
            .map(|r| r.join("; "))
            .unwrap_or_else(|| "无".to_string());
        if reason != "无" {
            n_conflicts += 1;
        }
        let row = gtk4::Label::new(Some(&format!(
            "#{} | {} | {} | {} | {}",
            e.id,
            e.language,
            e.original_timing(),
            e.unified_timing(),
            reason,
        )));
        row.set_xalign(0.0);
        row.set_wrap(true);
        if reason != "无" {
            row.add_css_class("error");
        }
        w.list.append(&row);
    }
    w.status.set_text(&format!(
        "共 {} 条字幕，{} 条存在冲突",
        events.len(),
        n_conflicts
    ));
}

fn connect_open(
    window: &gtk4::ApplicationWindow,
    btn: &gtk4::Button,
    state: Rc<RefCell<AppState>>,
    widgets: Widgets,
    video: gtk4::Video,
) {
    let window = window.clone();
    btn.connect_clicked(move |_| {
        let dialog = gtk4::FileDialog::new();
        let state = state.clone();
        let widgets = widgets.clone();
        let video = video.clone();
        let window = window.clone();
        dialog.open(Some(&window), None::<&gtk4::gio::Cancellable>, move |res| {
            if let Ok(file) = res {
                let path = file.path().expect("local file");
                let uri = format!("file://{}", path.display());
                match gst::probe(&uri) {
                    Ok(meta) => {
                        let mut s = state.borrow_mut();
                        s.timeline.video_duration_ns = meta.duration_ns;
                        s.video_label = format!(
                            "{} ({}fps)",
                            path.display(),
                            meta.framerate
                                .map(|(n, d)| format!("{}/{}", n, d))
                                .unwrap_or_else(|| "?".into())
                        );
                        drop(s);
                        gst::load_into_preview(&video, &file);
                        refresh(&state, &widgets);
                    }
                    Err(e) => widgets.status.set_text(&e),
                }
            }
        });
    });
}

fn connect_detect(btn: &gtk4::Button, state: Rc<RefCell<AppState>>, widgets: Widgets) {
    btn.connect_clicked(move |_| {
        let threshold = (widgets.gap_spin.value() * 1e9) as u64;
        state.borrow_mut().timeline.gap_threshold_ns = threshold;
        refresh(&state, &widgets);
    });
}

fn connect_reorder(btn: &gtk4::Button, state: Rc<RefCell<AppState>>, widgets: Widgets) {
    btn.connect_clicked(move |_| {
        let parse = |e: &gtk4::Entry| -> Result<Vec<usize>, String> {
            e.text()
                .split(',')
                .map(|s| {
                    s.trim()
                        .parse::<usize>()
                        .map_err(|_| format!("无法解析 '{}'", s.trim()))
                })
                .collect()
        };
        let result = (|| {
            let selection = parse(&widgets.selection_entry)?;
            let order = parse(&widgets.order_entry)?;
            state
                .borrow_mut()
                .timeline
                .reorder_range(&selection, &order)
        })();
        match result {
            Ok(ids) => {
                widgets
                    .status
                    .set_text(&format!("重排完成，新顺序: {:?}", ids));
                refresh(&state, &widgets);
            }
            Err(e) => widgets.status.set_text(&format!("重排失败: {}", e)),
        }
    });
}
