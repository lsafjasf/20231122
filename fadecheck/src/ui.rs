//! GTK4 来源检查页。
//!
//! 页面分三栏：
//! 1. 左侧：原始输入登记区（添加文件、未知扩展标记、指纹信息）；
//! 2. 中间：检查结论列表，点击可查看证据链与两个版本快照；
//! 3. 右侧：选中对象的溯源详情（原始输入 → 处理步骤 → 关键证据）。

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

use fadecheck::model_public::{Conclusion, Report};

struct AppState {
    paths: Vec<String>,
    report: Option<Report>,
}

/// 构建并显示主窗口。
pub fn run() -> glib::ExitCode {
    let app = gtk::Application::new(
        Some("org.example.fadecheck"),
        gio::ApplicationFlags::FLAGS_NONE,
    );
    app.connect_activate(build_window);
    app.run()
}

fn build_window(app: &gtk::Application) {
    let state = Rc::new(RefCell::new(AppState {
        paths: Vec::new(),
        report: None,
    }));

    let window = gtk::ApplicationWindow::new(app);
    window.set_title(Some("音频片段淡化检查 — 来源检查页"));
    window.set_default_size(1180, 760);

    let header = gtk::HeaderBar::new();
    window.set_titlebar(Some(&header));

    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    root.set_margin_top(10);
    root.set_margin_bottom(10);
    root.set_margin_start(10);
    root.set_margin_end(10);

    let banner = gtk::Label::new(Some(
        "每个结论均可回溯：原始输入（路径/大小/SHA-256）→ 两个版本快照（处理步骤）→ 关键证据",
    ));
    banner.add_css_class("title-4");
    banner.set_xalign(0.0);
    root.append(&banner);

    let outer_paned = gtk::Paned::new(gtk::Orientation::Horizontal);
    let inner_paned = gtk::Paned::new(gtk::Orientation::Horizontal);

    // ---- 左栏：输入登记 ----
    let left = gtk::Box::new(gtk::Orientation::Vertical, 8);
    left.set_width_request(340);
    let left_title = gtk::Label::new(Some("① 原始输入登记"));
    left_title.add_css_class("heading");
    left_title.set_xalign(0.0);
    left.append(&left_title);

    let input_list = gtk::ListBox::new();
    input_list.add_css_class("card");
    input_list.set_selection_mode(gtk::SelectionMode::None);
    let input_scroll = gtk::ScrolledWindow::new();
    input_scroll.set_child(Some(&input_list));
    input_scroll.set_vexpand(true);
    left.append(&input_scroll);

    let btn_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let add_btn = gtk::Button::with_label("添加文件…");
    let run_btn = gtk::Button::with_label("开始检查");
    run_btn.add_css_class("suggested-action");
    let clear_btn = gtk::Button::with_label("清空");
    btn_row.append(&add_btn);
    btn_row.append(&run_btn);
    btn_row.append(&clear_btn);
    left.append(&btn_row);

    // ---- 中栏：结论列表 ----
    let middle = gtk::Box::new(gtk::Orientation::Vertical, 8);
    middle.set_width_request(300);
    let mid_title = gtk::Label::new(Some("② 检查结论"));
    mid_title.add_css_class("heading");
    mid_title.set_xalign(0.0);
    middle.append(&mid_title);

    let result_list = gtk::ListBox::new();
    result_list.add_css_class("card");
    let result_scroll = gtk::ScrolledWindow::new();
    result_scroll.set_child(Some(&result_list));
    result_scroll.set_vexpand(true);
    middle.append(&result_scroll);

    let status = gtk::Label::new(Some("尚未运行检查"));
    status.set_xalign(0.0);
    status.add_css_class("dim-label");
    middle.append(&status);

    // ---- 右栏：溯源详情 ----
    let right = gtk::Box::new(gtk::Orientation::Vertical, 8);
    right.set_width_request(460);
    let right_title = gtk::Label::new(Some("③ 溯源详情"));
    right_title.add_css_class("heading");
    right_title.set_xalign(0.0);
    right.append(&right_title);

    let detail = gtk::TextView::new();
    detail.set_editable(false);
    detail.set_monospace(true);
    detail.set_wrap_mode(gtk::WrapMode::Word);
    let detail_scroll = gtk::ScrolledWindow::new();
    detail_scroll.set_child(Some(&detail));
    detail_scroll.set_vexpand(true);
    right.append(&detail_scroll);

    inner_paned.set_start_child(Some(&middle));
    inner_paned.set_resize_start_child(false);
    inner_paned.set_end_child(Some(&right));
    inner_paned.set_vexpand(true);
    outer_paned.set_start_child(Some(&left));
    outer_paned.set_resize_start_child(false);
    outer_paned.set_end_child(Some(&inner_paned));
    outer_paned.set_vexpand(true);
    root.append(&outer_paned);
    window.set_child(Some(&root));

    // ---- 交互接线 ----
    let window_weak = window.downgrade();
    let state_for_chooser = state.clone();
    let list_for_chooser = input_list.downgrade();
    add_btn.connect_clicked(move |_| {
        if let Some(window) = window_weak.upgrade() {
            let dialog = gtk::FileDialog::new();
            dialog.set_title("选择待检查的音频片段（可多选）");
            dialog.set_modal(true);
            let state = state_for_chooser.clone();
            let list = list_for_chooser.clone();
            dialog.open_multiple(Some(&window), None::<&gio::Cancellable>, move |result| {
                if let Ok(files) = result {
                    let mut added = Vec::new();
                    for index in 0..files.n_items() {
                        if let Some(object) = files.item(index) {
                            if let Ok(file) = object.downcast::<gio::File>() {
                                if let Some(path) = file.path() {
                                    added.push(path);
                                }
                            }
                        }
                    }
                    if let Some(list) = list.upgrade() {
                        append_inputs(&state, &list, added);
                    }
                }
            });
        }
    });

    clear_btn.connect_clicked({
        let state = state.clone();
        let input_list = input_list.clone();
        let result_list = result_list.clone();
        let status = status.clone();
        let detail = detail.clone();
        move |_| {
            state.borrow_mut().paths.clear();
            state.borrow_mut().report = None;
            while let Some(row) = input_list.first_child() {
                input_list.remove(&row);
            }
            while let Some(row) = result_list.first_child() {
                result_list.remove(&row);
            }
            status.set_text("尚未运行检查");
            detail.buffer().set_text("");
        }
    });

    run_btn.connect_clicked({
        let state = state.clone();
        let result_list = result_list.clone();
        let status = status.clone();
        let detail = detail.clone();
        move |_| {
            let paths = state.borrow().paths.clone();
            if paths.is_empty() {
                status.set_text("请先添加至少一个输入对象");
                return;
            }
            status.set_text("检查进行中…");
            // 引擎在独立线程运行，避免阻塞 GTK 主循环；
            // 通过 std 通道回传报告，主循环用 timeout 轮询接收。
            let (sender, receiver) = mpsc::channel::<Report>();
            std::thread::spawn(move || {
                let (report, temp) = fadecheck::engine_public::run_batch(&paths);
                let _ = sender.send(report);
                // 临时状态在此显式清理；TempState::drop 会再兜底一次。
                temp.cleanup();
                assert!(!temp.path().exists(), "临时状态清理失败");
            });
            let state_for_done = state.clone();
            let result_list_for_done = result_list.clone();
            let status_for_done = status.clone();
            let detail_for_done = detail.clone();
            glib::timeout_add_local(Duration::from_millis(100), move || {
                let report = match receiver.try_recv() {
                    Ok(report) => report,
                    Err(mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
                    Err(mpsc::TryRecvError::Disconnected) => return glib::ControlFlow::Break,
                };
                state_for_done.borrow_mut().report = Some(report.clone());
                render_results(
                    &result_list_for_done,
                    &status_for_done,
                    &detail_for_done,
                    &report,
                );
                glib::ControlFlow::Break
            });
        }
    });

    result_list.connect_row_selected(move |_, row| {
        if let Some(row) = row {
            let idx = row.index() as usize;
            if let Some(report) = &state.borrow().report {
                if let Some(conclusion) = report.conclusions.get(idx) {
                    detail.buffer().set_text(&render_detail(conclusion));
                }
            }
        }
    });

    window.present();
}

fn append_inputs(state: &Rc<RefCell<AppState>>, list: &gtk::ListBox, paths: Vec<PathBuf>) {
    for path in paths {
        let text = path.display().to_string();
        state.borrow_mut().paths.push(text.clone());
        let row = gtk::ListBoxRow::new();
        let label = gtk::Label::new(Some(&text));
        label.set_xalign(0.0);
        label.set_margin_top(4);
        label.set_margin_bottom(4);
        label.set_margin_start(8);
        label.set_margin_end(8);
        row.set_child(Some(&label));
        list.append(&row);
    }
}

fn render_results(
    result_list: &gtk::ListBox,
    status: &gtk::Label,
    detail: &gtk::TextView,
    report: &Report,
) {
    while let Some(row) = result_list.first_child() {
        result_list.remove(&row);
    }
    let inconclusive = report
        .conclusions
        .iter()
        .filter(|c| c.verdict == fadecheck::model_public::Verdict::Inconclusive)
        .count();
    status.set_text(&format!(
        "共 {} 个对象；其中无法判断 {} 个；临时状态已清理",
        report.total_objects, inconclusive
    ));
    detail.buffer().set_text("");

    for conclusion in &report.conclusions {
        let row = gtk::ListBoxRow::new();
        let label = gtk::Label::new(Some(&format!(
            "#{}  [{}]  {}",
            conclusion.object_id,
            conclusion.verdict.as_str(),
            conclusion.path
        )));
        label.set_xalign(0.0);
        label.set_margin_top(4);
        label.set_margin_bottom(4);
        label.set_margin_start(8);
        label.set_margin_end(8);
        row.set_child(Some(&label));
        result_list.append(&row);
    }
}

fn render_detail(c: &Conclusion) -> String {
    let mut out = String::new();
    out.push_str(&format!("对象 #{}: {}\n", c.object_id, c.path));
    out.push_str(&format!("结论: {}\n\n", c.verdict.as_str()));

    out.push_str("== 版本快照与处理步骤 ==\n");
    for snap in &c.snapshots {
        out.push_str(&format!(
            "\n[{}] 采样率={:?} 声道={:?} 帧数={:?}\n",
            snap.label, snap.sample_rate, snap.channels, snap.frame_count
        ));
        out.push_str(&format!("  输入指纹 SHA-256: {}\n", snap.input_sha256));
        for step in &snap.steps {
            out.push_str(&format!(
                "  {}. {} — {}{}\n",
                step.order,
                step.stage,
                step.detail,
                step.observation
                    .as_deref()
                    .map(|o| format!(" ({o})"))
                    .unwrap_or_default()
            ));
        }
    }

    out.push_str("\n== 关键证据 ==\n");
    for ev in &c.evidence {
        out.push_str(&format!(
            "\n• {} = {:.4} [快照 {}]\n  阈值: {}\n  窗口帧: {:?}\n  {}\n",
            ev.metric, ev.value, ev.snapshot, ev.threshold, ev.window_frames, ev.note
        ));
    }

    if !c.warnings.is_empty() {
        out.push_str("\n== 已恢复的异常 ==\n");
        for warning in &c.warnings {
            out.push_str(&format!("⚠ {warning}\n"));
        }
    }
    out
}
