//! GTK4 界面：来源检查页（运行列表 + 溯源链 + 报告 + 后续操作）。

use crate::analysis;
use crate::demo;
use crate::model::*;
use gtk4::prelude::*;
use gtk4::{self as gtk, gio, glib};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

pub struct AppState {
    pub journal: Journal,
    pub selected: Option<usize>,
    pub input_file: Option<PathBuf>,
    pub cancel: Option<Arc<AtomicBool>>,
    pub progress: Option<Arc<AtomicU32>>,
    pub done: Arc<Mutex<Option<RunRecord>>>,
    pub busy: bool,
}

static INITIAL_SELECTION: Mutex<Option<usize>> = Mutex::new(None);

/// 截图模式下指定初始选中的运行记录。
pub fn set_initial_selection(idx: usize) {
    *INITIAL_SELECTION.lock().unwrap() = Some(idx);
}

impl AppState {
    fn new() -> Self {
        let journal = Journal::load(&journal_path());
        // 默认选中最近一次已完成的运行，便于直接查看报告
        let selected = INITIAL_SELECTION.lock().unwrap().or_else(|| {
            journal
                .runs
                .iter()
                .rposition(|r| r.status == RunStatus::Completed)
                .or(if journal.runs.is_empty() { None } else { Some(0) })
        });
        AppState {
            journal,
            selected,
            input_file: None,
            cancel: None,
            progress: None,
            done: Arc::new(Mutex::new(None)),
            busy: false,
        }
    }
}

pub struct Widgets {
    pub window: gtk::ApplicationWindow,
    pub run_list: gtk::ListBox,
    pub detail: gtk::Box,
    pub progress: gtk::ProgressBar,
    pub status: gtk::Label,
    pub reviewer: gtk::Entry,
    pub analyze_btn: gtk::Button,
    pub cancel_btn: gtk::Button,
}

fn margins<W: IsA<gtk::Widget>>(w: &W, v: i32) {
    w.set_margin_top(v);
    w.set_margin_bottom(v);
    w.set_margin_start(v);
    w.set_margin_end(v);
}

pub fn build_ui(app: &gtk::Application) -> gtk::ApplicationWindow {
    let state = Rc::new(RefCell::new(AppState::new()));

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("音频噪声频谱标注 · 来源检查")
        .default_width(1180)
        .default_height(760)
        .build();

    let header = gtk::HeaderBar::new();
    let open_btn = gtk::Button::with_label("选择音频…");
    let analyze_btn = gtk::Button::with_label("开始分析");
    let cancel_btn = gtk::Button::with_label("取消");
    cancel_btn.set_sensitive(false);
    let demo_btn = gtk::Button::with_label("生成演示数据");
    header.pack_start(&open_btn);
    header.pack_start(&analyze_btn);
    header.pack_start(&cancel_btn);
    header.pack_end(&demo_btn);

    let run_list = gtk::ListBox::new();
    run_list.set_selection_mode(gtk::SelectionMode::Single);
    run_list.add_css_class("navigation-sidebar");
    let left_scroll = gtk::ScrolledWindow::builder()
        .child(&run_list)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .min_content_width(240)
        .build();

    let detail = gtk::Box::new(gtk::Orientation::Vertical, 12);
    margins(&detail, 16);
    let right_scroll = gtk::ScrolledWindow::builder()
        .child(&detail)
        .hexpand(true)
        .build();

    let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
    paned.set_start_child(Some(&left_scroll));
    paned.set_end_child(Some(&right_scroll));
    paned.set_position(260);
    paned.set_vexpand(true);

    let progress = gtk::ProgressBar::new();
    progress.set_show_text(true);
    let status = gtk::Label::new(Some("就绪"));
    status.set_xalign(0.0);
    let reviewer = gtk::Entry::new();
    reviewer.set_placeholder_text(Some("人工确认人姓名"));

    let bottom = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let reviewer_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    reviewer_row.append(&gtk::Label::new(Some("人工确认人:")));
    reviewer_row.append(&reviewer);
    reviewer_row.append(&status);
    status.set_hexpand(true);
    bottom.append(&progress);
    bottom.append(&reviewer_row);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("background");
    root.append(&header);
    root.append(&paned);
    root.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    root.append(&bottom);
    window.set_child(Some(&root));

    let widgets = Rc::new(Widgets {
        window: window.clone(),
        run_list,
        detail,
        progress,
        status,
        reviewer,
        analyze_btn: analyze_btn.clone(),
        cancel_btn: cancel_btn.clone(),
    });

    wire_actions(&state, &widgets, &open_btn, &analyze_btn, &cancel_btn, &demo_btn);
    refresh_run_list(&state, &widgets);
    start_poll_timer(&state, &widgets);
    window.present();
    window
}

fn wire_actions(
    state: &Rc<RefCell<AppState>>,
    widgets: &Rc<Widgets>,
    open_btn: &gtk::Button,
    analyze_btn: &gtk::Button,
    cancel_btn: &gtk::Button,
    demo_btn: &gtk::Button,
) {
    // 选择音频文件
    {
        let state = state.clone();
        let widgets = widgets.clone();
        open_btn.connect_clicked(move |_| {
            let dialog = gtk::FileDialog::new();
            dialog.set_title("选择要分析的音频文件");
            let state = state.clone();
            let widgets = widgets.clone();
            let win = widgets.window.clone();
            dialog.open(Some(&win), None::<&gio::Cancellable>, move |res| {
                if let Ok(file) = res {
                    if let Some(path) = file.path() {
                        state.borrow_mut().input_file = Some(path.clone());
                        widgets
                            .status
                            .set_text(&format!("已选择: {}", path.display()));
                    }
                }
            });
        });
    }

    // 开始分析（后台线程运行 GStreamer）
    {
        let state = state.clone();
        let widgets = widgets.clone();
        analyze_btn.connect_clicked(move |_| {
            let input = state.borrow().input_file.clone();
            let Some(input) = input else {
                widgets.status.set_text("请先选择音频文件");
                return;
            };
            let cancel = Arc::new(AtomicBool::new(false));
            let progress = Arc::new(AtomicU32::new(0));
            let done = state.borrow().done.clone();
            {
                let mut s = state.borrow_mut();
                s.cancel = Some(cancel.clone());
                s.progress = Some(progress.clone());
                s.busy = true;
            }
            widgets.analyze_btn.set_sensitive(false);
            widgets.cancel_btn.set_sensitive(true);
            widgets.status.set_text("分析中…");
            let digest = analysis::sha256_file(&input).unwrap_or_default();
            let size = std::fs::metadata(&input).map(|m| m.len()).unwrap_or(0);
            std::thread::spawn(move || {
                let outcome = analysis::run_analysis(&input, cancel, Some(progress));
                let cancelled = outcome.status == RunStatus::Cancelled;
                let record = RunRecord {
                    id: format!("R{:03}", now_ts() % 1000),
                    input_path: input.display().to_string(),
                    input_sha256: digest,
                    input_size_bytes: size,
                    created_ts: now_ts(),
                    status: outcome.status,
                    progress: outcome.progress,
                    cancel_reason: if cancelled {
                        Some("用户中途取消".to_string())
                    } else {
                        None
                    },
                    error: outcome.error,
                    next_actions: outcome.next_actions,
                    steps: outcome.steps,
                    conclusions: outcome.conclusions,
                    reviews: Vec::new(),
                };
                *done.lock().unwrap() = Some(record);
            });
        });
    }

    // 中途取消
    {
        let state = state.clone();
        cancel_btn.connect_clicked(move |_| {
            if let Some(flag) = &state.borrow().cancel {
                flag.store(true, Ordering::SeqCst);
            }
        });
    }

    // 生成演示数据（真实运行：两次中途取消 + 完成 + 失败）
    {
        let state = state.clone();
        let widgets = widgets.clone();
        demo_btn.connect_clicked(move |_| {
            widgets.status.set_text("正在生成演示数据（约 12 秒）…");
            let state = state.clone();
            let widgets = widgets.clone();
            glib::spawn_future_local(async move {
                let result = gio::spawn_blocking(demo::build_demo_journal).await;
                match result {
                    Ok(Ok(journal)) => {
                        let n = journal.runs.len();
                        state.borrow_mut().journal = journal;
                        refresh_run_list(&state, &widgets);
                        widgets
                            .status
                            .set_text(&format!("演示数据已生成：{n} 条运行记录"));
                    }
                    Ok(Err(e)) => widgets.status.set_text(&format!("演示数据失败: {e}")),
                    Err(_) => widgets.status.set_text("演示任务被中断"),
                }
            });
        });
    }

    // 运行列表选择
    {
        let state = state.clone();
        let widgets = widgets.clone();
        let list = widgets.run_list.clone();
        list.connect_row_selected(move |_, row| {
            let mut s = state.borrow_mut();
            s.selected = row.map(|r| r.index() as usize);
            drop(s);
            refresh_detail(&state, &widgets);
        });
    }
}

fn refresh_run_list(state: &Rc<RefCell<AppState>>, widgets: &Rc<Widgets>) {
    while let Some(child) = widgets.run_list.first_child() {
        widgets.run_list.remove(&child);
    }
    // 先取出摘要数据再建行，避免 select_row 回调触发二次借用
    let rows: Vec<(String, String)> = state
        .borrow()
        .journal
        .runs
        .iter()
        .map(|run| {
            (
                format!(
                    "{} · {} · {:.0}%",
                    run.id,
                    run.status.label(),
                    run.progress * 100.0
                ),
                format!("{} · {}", fmt_ts(run.created_ts), run.input_path),
            )
        })
        .collect();
    for (i, (title_text, sub_text)) in rows.iter().enumerate() {
        let title = gtk::Label::new(Some(title_text));
        title.set_xalign(0.0);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let sub = gtk::Label::new(Some(sub_text));
        sub.set_xalign(0.0);
        sub.set_ellipsize(gtk::pango::EllipsizeMode::End);
        sub.add_css_class("dim-label");
        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 2);
        margins(&vbox, 6);
        vbox.append(&title);
        vbox.append(&sub);
        let row = gtk::ListBoxRow::new();
        row.set_child(Some(&vbox));
        widgets.run_list.insert(&row, i as i32);
    }
    let sel = state.borrow().selected;
    if let Some(sel) = sel {
        if let Some(row) = widgets.run_list.row_at_index(sel as i32) {
            widgets.run_list.select_row(Some(&row));
        }
    }
    refresh_detail(state, widgets);
}

fn start_poll_timer(state: &Rc<RefCell<AppState>>, widgets: &Rc<Widgets>) {
    let state = state.clone();
    let widgets = widgets.clone();
    glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
        let (busy, prog, done) = {
            let s = state.borrow();
            (
                s.busy,
                s.progress.clone(),
                s.done.clone(),
            )
        };
        if busy {
            if let Some(p) = prog {
                let permille = p.load(Ordering::SeqCst);
                widgets.progress.set_fraction(permille as f64 / 1000.0);
            }
        }
        let record = done.lock().unwrap().take();
        if let Some(record) = record {
            let mut s = state.borrow_mut();
            s.busy = false;
            s.cancel = None;
            s.progress = None;
            let status_label = record.status.label().to_string();
            s.journal.runs.push(record);
            s.selected = Some(s.journal.runs.len() - 1);
            if let Err(e) = s.journal.save(&journal_path()) {
                widgets.status.set_text(&format!("保存日志失败: {e}"));
            }
            drop(s);
            widgets.analyze_btn.set_sensitive(true);
            widgets.cancel_btn.set_sensitive(false);
            widgets.progress.set_fraction(1.0);
            widgets.status.set_text(&format!("分析结束：{status_label}"));
            refresh_run_list(&state, &widgets);
        }
        glib::ControlFlow::Continue
    });
}

fn section_title(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    l.add_css_class("heading");
    l
}

fn kv_row(key: &str, value: &str) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let k = gtk::Label::new(Some(key));
    k.add_css_class("dim-label");
    k.set_width_chars(14);
    k.set_xalign(0.0);
    let v = gtk::Label::new(Some(value));
    v.set_xalign(0.0);
    v.set_selectable(true);
    v.set_wrap(true);
    row.append(&k);
    row.append(&v);
    row
}

fn refresh_detail(state: &Rc<RefCell<AppState>>, widgets: &Rc<Widgets>) {
    while let Some(child) = widgets.detail.first_child() {
        widgets.detail.remove(&child);
    }
    let s = state.borrow();
    let Some(idx) = s.selected else {
        widgets
            .detail
            .append(&gtk::Label::new(Some("请选择或生成一条运行记录")));
        return;
    };
    let Some(run) = s.journal.runs.get(idx).cloned() else {
        return;
    };
    drop(s);

    // 1. 运行状态
    widgets.detail.append(&section_title(&format!(
        "运行 {} — {}",
        run.id,
        run.status.label()
    )));
    widgets.detail.append(&kv_row("状态", run.status.label()));
    widgets
        .detail
        .append(&kv_row("进度", &format!("{:.0}%", run.progress * 100.0)));
    if let Some(reason) = &run.cancel_reason {
        widgets.detail.append(&kv_row("取消原因", reason));
    }
    if let Some(err) = &run.error {
        widgets.detail.append(&kv_row("错误", err));
    }

    // 2. 原始输入（来源）
    widgets.detail.append(&section_title("原始输入"));
    widgets.detail.append(&kv_row("文件", &run.input_path));
    widgets.detail.append(&kv_row("SHA-256", &run.input_sha256));
    widgets
        .detail
        .append(&kv_row("大小", &format!("{} 字节", run.input_size_bytes)));
    widgets
        .detail
        .append(&kv_row("创建时间", &fmt_ts(run.created_ts)));

    // 3. 处理步骤（溯源链）
    widgets.detail.append(&section_title("处理步骤"));
    for st in &run.steps {
        let params: Vec<String> = st
            .params
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        let exp = gtk::Expander::new(Some(&format!(
            "[{}] {} — {}",
            st.id, st.kind, st.description
        )));
        let body = gtk::Box::new(gtk::Orientation::Vertical, 4);
        body.set_margin_start(18);
        body.append(&kv_row("时间", &format!("{} → {}", fmt_ts(st.started_ts), fmt_ts(st.ended_ts))));
        for p in params {
            body.append(&kv_row("参数", &p));
        }
        exp.set_child(Some(&body));
        widgets.detail.append(&exp);
    }

    build_boundary_section(state, widgets, &run);
    build_report_section(state, widgets, idx, &run);
    build_actions_section(widgets, &run);
}

/// 4. 无法判断的边界频段：保留关键证据。
fn build_boundary_section(_state: &Rc<RefCell<AppState>>, widgets: &Rc<Widgets>, run: &RunRecord) {
    let inconclusive: Vec<&BandConclusion> = run
        .conclusions
        .iter()
        .filter(|c| c.verdict == Verdict::Inconclusive)
        .collect();
    widgets.detail.append(&section_title(&format!(
        "无法判断的边界（{} 个频段）",
        inconclusive.len()
    )));
    if inconclusive.is_empty() {
        let l = gtk::Label::new(Some("本次运行没有落在阈值边界带内的频段"));
        l.set_xalign(0.0);
        l.add_css_class("dim-label");
        widgets.detail.append(&l);
        return;
    }
    for c in inconclusive {
        let exp = gtk::Expander::new(Some(&format!(
            "频段 #{:02}  {:.0}–{:.0} Hz · 距阈值 {:+.2} dB",
            c.band_index, c.freq_lo_hz, c.freq_hi_hz, c.margin_db
        )));
        let body = gtk::Box::new(gtk::Orientation::Vertical, 4);
        body.set_margin_start(18);
        for (k, v) in &c.evidence {
            body.append(&kv_row(k, v));
        }
        body.append(&kv_row("溯源步骤", &c.step_refs.join(", ")));
        exp.set_child(Some(&body));
        widgets.detail.append(&exp);
    }
}

/// 5. 报告：自动结论与人工确认分两栏呈现。
fn build_report_section(
    state: &Rc<RefCell<AppState>>,
    widgets: &Rc<Widgets>,
    run_idx: usize,
    run: &RunRecord,
) {
    widgets
        .detail
        .append(&section_title("报告 · 自动结论 vs 人工确认"));
    if run.conclusions.is_empty() {
        let l = gtk::Label::new(Some("无结论数据（运行未完成测量）"));
        l.set_xalign(0.0);
        l.add_css_class("dim-label");
        widgets.detail.append(&l);
        return;
    }
    let grid = gtk::Grid::new();
    grid.set_column_spacing(12);
    grid.set_row_spacing(2);
    for (col, head) in ["频段", "自动结论", "关键数据", "人工确认", "操作"].iter().enumerate() {
        let h = gtk::Label::new(Some(head));
        h.add_css_class("heading");
        h.set_xalign(0.0);
        grid.attach(&h, col as i32, 0, 1, 1);
    }
    for (row, c) in run.conclusions.iter().enumerate() {
        let r = row as i32 + 1;
        let band = gtk::Label::new(Some(&format!(
            "#{:02} {:.0}–{:.0}Hz",
            c.band_index, c.freq_lo_hz, c.freq_hi_hz
        )));
        band.set_xalign(0.0);
        let auto = gtk::Label::new(Some(c.verdict.label()));
        auto.set_xalign(0.0);
        match c.verdict {
            Verdict::Noise => auto.add_css_class("error"),
            Verdict::Inconclusive => auto.add_css_class("warning"),
            Verdict::Clean => auto.add_css_class("success"),
        }
        let data = gtk::Label::new(Some(&format!(
            "{:.1}dB / 阈值 {:.1}dB",
            c.magnitude_db, c.threshold_db
        )));
        data.set_xalign(0.0);
        data.add_css_class("dim-label");

        let review_text = match run.review_for(c.band_index) {
            Some(rv) => format!("{} · {} · {}", rv.decision.label(), rv.reviewer, fmt_ts(rv.ts)),
            None => ReviewDecision::Pending.label().to_string(),
        };
        let review = gtk::Label::new(Some(&review_text));
        review.set_xalign(0.0);

        let ops = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        for (label, decision) in [("确认", ReviewDecision::Confirmed), ("驳回", ReviewDecision::Rejected)] {
            let btn = gtk::Button::with_label(label);
            btn.add_css_class("flat");
            let state = state.clone();
            let widgets = widgets.clone();
            let band_index = c.band_index;
            let decision = decision.clone();
            btn.connect_clicked(move |_| {
                let reviewer = widgets.reviewer.text().to_string();
                let reviewer = if reviewer.trim().is_empty() {
                    "未署名".to_string()
                } else {
                    reviewer.trim().to_string()
                };
                let mut s = state.borrow_mut();
                if let Some(run) = s.journal.runs.get_mut(run_idx) {
                    run.reviews.retain(|rv| rv.band_index != band_index);
                    run.reviews.push(HumanReview {
                        band_index,
                        reviewer,
                        decision: decision.clone(),
                        note: String::new(),
                        ts: now_ts(),
                    });
                    let _ = s.journal.save(&journal_path());
                }
                drop(s);
                refresh_detail(&state, &widgets);
            });
            ops.append(&btn);
        }

        grid.attach(&band, 0, r, 1, 1);
        grid.attach(&auto, 1, r, 1, 1);
        grid.attach(&data, 2, r, 1, 1);
        grid.attach(&review, 3, r, 1, 1);
        grid.attach(&ops, 4, r, 1, 1);
    }
    widgets.detail.append(&grid);
}

/// 6. 失败/取消时的可执行后续操作。
fn build_actions_section(widgets: &Rc<Widgets>, run: &RunRecord) {
    if run.next_actions.is_empty() {
        return;
    }
    widgets.detail.append(&section_title("后续操作（可执行）"));
    for (i, action) in run.next_actions.iter().enumerate() {
        let l = gtk::Label::new(Some(&format!("{}. {}", i + 1, action)));
        l.set_xalign(0.0);
        l.set_wrap(true);
        widgets.detail.append(&l);
    }
}
