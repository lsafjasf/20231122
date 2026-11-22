//! GTK4 界面：帧列表 + 字段明细 + 逐字段应用/撤销 + 导出对照清单。

use crate::fix::RepairSession;
use crate::meta::*;
use crate::report;
use crate::rules::{Issue, Suggestion};
use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

struct AppState {
    session: RepairSession,
    selected: Option<usize>,
    source_desc: String,
}

impl AppState {
    fn new() -> Self {
        AppState {
            session: RepairSession::new(Vec::new()),
            selected: None,
            source_desc: "未加载".to_string(),
        }
    }
}

/// 界面句柄，刷新时整体重建两侧列表。
#[derive(Clone)]
struct Widgets {
    state: Rc<RefCell<AppState>>,
    frame_list: gtk::ListBox,
    detail: gtk::Box,
    status: gtk::Label,
}

pub fn run() -> glib::ExitCode {
    let app = gtk::Application::new(
        Some("dev.hdrmetafix.App"),
        Default::default(),
    );
    app.connect_activate(build_ui);
    app.run()
}

fn build_ui(app: &gtk::Application) {
    let win = gtk::ApplicationWindow::new(app);
    win.set_title(Some("HDR 元数据修复工具"));
    win.set_default_size(1100, 700);

    let header = gtk::HeaderBar::new();
    let open_btn = gtk::Button::with_label("打开视频…");
    let demo_btn = gtk::Button::with_label("加载示例帧");
    let undo_btn = gtk::Button::with_label("撤销一步");
    let export_btn = gtk::Button::with_label("导出对照清单…");
    header.pack_start(&open_btn);
    header.pack_start(&demo_btn);
    header.pack_start(&undo_btn);
    header.pack_end(&export_btn);
    win.set_titlebar(Some(&header));

    let frame_list = gtk::ListBox::new();
    frame_list.set_selection_mode(gtk::SelectionMode::Single);
    frame_list.add_css_class("navigation-sidebar");
    let left = gtk::ScrolledWindow::new();
    left.set_child(Some(&frame_list));
    left.set_min_content_width(260);

    let detail = gtk::Box::new(gtk::Orientation::Vertical, 8);
    detail.set_margin_top(12);
    detail.set_margin_bottom(12);
    detail.set_margin_start(12);
    detail.set_margin_end(12);
    let right = gtk::ScrolledWindow::new();
    right.set_child(Some(&detail));
    right.set_hexpand(true);

    let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
    paned.set_start_child(Some(&left));
    paned.set_end_child(Some(&right));
    paned.set_position(280);
    paned.set_vexpand(true);

    let status = gtk::Label::new(Some("就绪"));
    status.set_halign(gtk::Align::Start);
    status.set_margin_top(4);
    status.set_margin_bottom(4);
    status.set_margin_start(8);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.append(&paned);
    root.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    root.append(&status);
    win.set_child(Some(&root));

    let widgets = Widgets {
        state: Rc::new(RefCell::new(AppState::new())),
        frame_list: frame_list.clone(),
        detail: detail.clone(),
        status: status.clone(),
    };

    wire_actions(&win, &widgets, &open_btn, &demo_btn, &undo_btn, &export_btn);
    frame_list.connect_row_selected(glib::clone!(
        #[strong]
        widgets,
        move |_, row| {
            let idx = row.map(|r| r.index() as usize);
            widgets.state.borrow_mut().selected = idx;
            refresh_detail(&widgets);
        }
    ));

    refresh_all(&widgets);
    win.present();
}

fn wire_actions(
    win: &gtk::ApplicationWindow,
    widgets: &Widgets,
    open_btn: &gtk::Button,
    demo_btn: &gtk::Button,
    undo_btn: &gtk::Button,
    export_btn: &gtk::Button,
) {
    let widgets = widgets.clone();
    let win = win.clone();
    demo_btn.connect_clicked(glib::clone!(
        #[strong]
        widgets,
        move |_| {
            let frames = crate::synth::sample_frames();
            let mut st = widgets.state.borrow_mut();
            st.session = RepairSession::new(frames);
            st.selected = None;
            st.source_desc = "合成示例帧（正常/缺字段/冲突）".to_string();
            drop(st);
            refresh_all(&widgets);
        }
    ));

    open_btn.connect_clicked(glib::clone!(
        #[strong]
        widgets,
        #[strong]
        win,
        move |_| {
            let dialog = gtk::FileDialog::new();
            dialog.set_title("选择视频文件");
            let w = widgets.clone();
            dialog.open(Some(&win), gtk::gio::Cancellable::NONE, move |res| {
                if let Ok(file) = res {
                    if let Some(path) = file.path() {
                        match crate::gst::extract_frames(&path.to_string_lossy(), 300) {
                            Ok(frames) => {
                                let n = frames.len();
                                let mut st = w.state.borrow_mut();
                                st.session = RepairSession::new(frames);
                                st.selected = None;
                                st.source_desc =
                                    format!("{}（前 {n} 帧）", path.display());
                                drop(st);
                                refresh_all(&w);
                            }
                            Err(e) => {
                                w.status.set_text(&format!("打开失败：{e}"));
                            }
                        }
                    }
                }
            });
        }
    ));

    undo_btn.connect_clicked(glib::clone!(
        #[strong]
        widgets,
        move |_| {
            if !widgets.state.borrow_mut().session.undo_last() {
                widgets.status.set_text("没有可撤销的修改");
            }
            refresh_all(&widgets);
        }
    ));

    export_btn.connect_clicked(glib::clone!(
        #[strong]
        widgets,
        #[strong]
        win,
        move |_| {
            let dialog = gtk::FileDialog::new();
            dialog.set_title("导出修复对照清单（Markdown）");
            dialog.set_initial_name(Some("hdr-fix-report.md"));
            let w = widgets.clone();
            dialog.save(Some(&win), gtk::gio::Cancellable::NONE, move |res| {
                if let Ok(file) = res {
                    if let Some(path) = file.path() {
                        let st = w.state.borrow();
                        let md = report::to_markdown(&st.session);
                        let csv = report::to_csv(&st.session);
                        let csv_path = path.with_extension("csv");
                        let r1 = std::fs::write(&path, md);
                        let r2 = std::fs::write(&csv_path, csv);
                        let msg = match (r1, r2) {
                            (Ok(()), Ok(())) => format!(
                                "已导出 {} 与 {}",
                                path.display(),
                                csv_path.display()
                            ),
                            _ => "导出失败".to_string(),
                        };
                        drop(st);
                        w.status.set_text(&msg);
                    }
                }
            });
        }
    ));
}

fn refresh_all(widgets: &Widgets) {
    // 帧列表
    while let Some(child) = widgets.frame_list.first_child() {
        widgets.frame_list.remove(&child);
    }
    let st = widgets.state.borrow();
    for (idx, frame) in st.session.frames.iter().enumerate() {
        let issues = crate::rules::analyze(frame);
        let conflicts = issues
            .iter()
            .filter(|i| i.kind == crate::rules::IssueKind::Conflict)
            .count();
        let missing = issues.len() - conflicts;
        let text = if issues.is_empty() {
            format!("帧 {}  ✓ 正常", frame.frame_no)
        } else {
            format!("帧 {}  ⚠ {conflicts} 冲突 / {missing} 缺字段", frame.frame_no)
        };
        let row = gtk::ListBoxRow::new();
        let label = gtk::Label::new(Some(&text));
        label.set_halign(gtk::Align::Start);
        label.set_margin_top(6);
        label.set_margin_bottom(6);
        label.set_margin_start(8);
        row.set_child(Some(&label));
        row.set_widget_name(&format!("frame-{idx}"));
        widgets.frame_list.append(&row);
    }
    let total = st.session.frames.len();
    let remaining = st.session.remaining_issues();
    let edits = st.session.edits().len();
    let src = st.source_desc.clone();
    drop(st);
    widgets.status.set_text(&format!(
        "来源：{src} ｜ 共 {total} 帧 ｜ 已修复 {edits} 处 ｜ 剩余问题 {remaining} 个"
    ));
    refresh_detail(widgets);
}

fn refresh_detail(widgets: &Widgets) {
    while let Some(child) = widgets.detail.first_child() {
        widgets.detail.remove(&child);
    }
    let st = widgets.state.borrow();
    let Some(idx) = st.selected else {
        let hint = gtk::Label::new(Some("从左侧选择一帧查看字段明细"));
        hint.set_margin_top(40);
        widgets.detail.append(&hint);
        return;
    };
    if idx >= st.session.frames.len() {
        return;
    }
    let frame = &st.session.frames[idx];
    let title = gtk::Label::new(Some(&format!("帧 {} 字段明细", frame.frame_no)));
    title.add_css_class("title-3");
    title.set_halign(gtk::Align::Start);
    widgets.detail.append(&title);

    let issues = st.session.issues(idx);
    // 字段 → 涉及它的问题与建议
    let mut by_field: HashMap<FieldKey, Vec<(&Issue, Option<&Suggestion>)>> = HashMap::new();
    for issue in &issues {
        for f in &issue.fields {
            let sug = issue.suggestions.iter().find(|s| s.field == *f);
            by_field.entry(*f).or_default().push((issue, sug));
        }
    }

    let grid = gtk::Grid::new();
    grid.set_column_spacing(12);
    grid.set_row_spacing(6);
    let headers = ["字段", "原始值", "当前值", "问题", "建议值", "操作"];
    for (col, h) in headers.iter().enumerate() {
        let l = gtk::Label::new(Some(h));
        l.add_css_class("heading");
        l.set_halign(gtk::Align::Start);
        grid.attach(&l, col as i32, 0, 1, 1);
    }
    for (row, key) in FieldKey::ALL.iter().enumerate() {
        let row = row as i32 + 1;
        build_field_row(widgets, &grid, row, idx, *key, frame, &by_field);
    }
    widgets.detail.append(&grid);
}

fn build_field_row(
    widgets: &Widgets,
    grid: &gtk::Grid,
    row: i32,
    frame_idx: usize,
    key: FieldKey,
    frame: &FrameMeta,
    by_field: &HashMap<FieldKey, Vec<(&Issue, Option<&Suggestion>)>>,
) {
    let name = gtk::Label::new(Some(key.label()));
    name.set_halign(gtk::Align::Start);
    grid.attach(&name, 0, row, 1, 1);

    let original = frame
        .get_original(key)
        .map(|v| v.to_string())
        .unwrap_or_else(|| "（缺失）".into());
    let current = frame
        .get(key)
        .map(|v| v.to_string())
        .unwrap_or_else(|| "（缺失）".into());
    let orig_l = gtk::Label::new(Some(&original));
    orig_l.set_halign(gtk::Align::Start);
    grid.attach(&orig_l, 1, row, 1, 1);
    let cur_l = gtk::Label::new(Some(&current));
    cur_l.set_halign(gtk::Align::Start);
    if frame.is_modified(key) {
        cur_l.add_css_class("accent");
    }
    grid.attach(&cur_l, 2, row, 1, 1);

    let empty = Vec::new();
    let related = by_field.get(&key).unwrap_or(&empty);
    let problem_text = related
        .iter()
        .map(|(i, _)| format!("[{}] {}", i.kind.label(), i.description))
        .collect::<Vec<_>>()
        .join("\n");
    let prob_l = gtk::Label::new(Some(&problem_text));
    prob_l.set_halign(gtk::Align::Start);
    prob_l.set_wrap(true);
    prob_l.set_max_width_chars(48);
    grid.attach(&prob_l, 3, row, 1, 1);

    let suggestion = related.iter().find_map(|(_, s)| *s).cloned();
    let sug_text = suggestion
        .as_ref()
        .map(|s| format!("{}（{}）", s.value, s.rationale))
        .unwrap_or_default();
    let sug_l = gtk::Label::new(Some(&sug_text));
    sug_l.set_halign(gtk::Align::Start);
    sug_l.set_wrap(true);
    sug_l.set_max_width_chars(40);
    grid.attach(&sug_l, 4, row, 1, 1);

    let ops = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    if let Some(sug) = suggestion {
        let apply = gtk::Button::with_label("应用");
        let w = widgets.clone();
        apply.connect_clicked(move |_| {
            w.state.borrow_mut().session.apply(frame_idx, &sug);
            refresh_all(&w);
        });
        ops.append(&apply);
    }
    if frame.is_modified(key) {
        let undo = gtk::Button::with_label("撤销");
        let w = widgets.clone();
        undo.connect_clicked(move |_| {
            w.state.borrow_mut().session.undo_field(frame_idx, key);
            refresh_all(&w);
        });
        ops.append(&undo);
    }
    grid.attach(&ops, 5, row, 1, 1);
}
