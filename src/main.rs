use gtk4::prelude::*;
use gtk4::{
    Application, ApplicationWindow, Box as GtkBox, Button, FileChooserAction,
    FileChooserDialog, Label, ListBox,
    ListBoxRow, Orientation, ResponseType, ScrolledWindow, Separator,
};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use syncwall::analyze::{analyze, Report};
use syncwall::model::{Finding, Verdict};

const CSS: &str = "
.verdict-ok { color: #1a7f37; font-weight: bold; }
.verdict-indeterminate { color: #9a6700; font-weight: bold; }
.verdict-error { color: #cf222e; font-weight: bold; }
.first-error-card { background: #ffebe9; border: 1px solid #cf222e; border-radius: 8px; padding: 12px; }
.boundary-card { background: #fff8c5; border: 1px solid #9a6700; border-radius: 8px; padding: 8px; }
.evidence { font-family: monospace; font-size: 11px; background: #f6f8fa; padding: 4px; }
.section-title { font-weight: bold; font-size: 15px; }
";

fn main() {
    let app = Application::builder()
        .application_id("dev.syncwall.inspector")
        .build();
    app.connect_activate(build_ui);
    app.run();
}

struct State {
    inputs: Vec<PathBuf>,
    report: Option<Report>,
}

fn verdict_label(v: &Verdict) -> Label {
    let l = Label::new(Some(v.label()));
    let cls = match v {
        Verdict::Ok => "verdict-ok",
        Verdict::Indeterminate => "verdict-indeterminate",
        Verdict::Error => "verdict-error",
    };
    l.add_css_class(cls);
    l
}

fn set_margins<W: gtk4::prelude::WidgetExt>(w: &W, m: i32) {
    w.set_margin_top(m);
    w.set_margin_bottom(m);
    w.set_margin_start(m);
    w.set_margin_end(m);
}

fn build_ui(app: &Application) {
    let provider = gtk4::CssProvider::new();
    provider.load_from_data(CSS);
    gtk4::style_context_add_provider_for_display(
        &gtk4::gdk::Display::default().unwrap(),
        &provider,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    let state = Rc::new(RefCell::new(State {
        inputs: vec![
            PathBuf::from("artifacts/session-truncated.jsonl"),
            PathBuf::from("artifacts/session-replay.jsonl"),
        ],
        report: None,
    }));

    let root = GtkBox::new(Orientation::Vertical, 8);
    set_margins(&root, 12);

    // 顶部：输入来源 + 操作
    let header = GtkBox::new(Orientation::Horizontal, 8);
    let files_label = Label::new(None);
    files_label.set_hexpand(true);
    files_label.set_xalign(0.0);
    let btn_add = Button::with_label("添加日志…");
    let btn_run = Button::with_label("重新分析");
    header.append(&files_label);
    header.append(&btn_add);
    header.append(&btn_run);
    root.append(&header);
    root.append(&Separator::new(Orientation::Horizontal));

    // 首个错误状态卡片
    let first_card = GtkBox::new(Orientation::Vertical, 4);
    first_card.add_css_class("first-error-card");
    root.append(&first_card);

    // 摘要行
    let summary = Label::new(None);
    summary.set_xalign(0.0);
    root.append(&summary);

    // 主区：左结论列表，右证据详情
    let paned = GtkBox::new(Orientation::Horizontal, 8);
    paned.set_vexpand(true);
    let findings_list = ListBox::new();
    findings_list.set_selection_mode(gtk4::SelectionMode::Single);
    let findings_scroll = ScrolledWindow::builder()
        .child(&findings_list)
        .min_content_width(430)
        .build();
    paned.append(&findings_scroll);

    let detail = GtkBox::new(Orientation::Vertical, 6);
    set_margins(&detail, 8);
    let detail_scroll = ScrolledWindow::builder().child(&detail).hexpand(true).build();
    paned.append(&detail_scroll);
    root.append(&paned);

    let win = ApplicationWindow::builder()
        .application(app)
        .title("多路画面同步墙 · 来源检查页")
        .default_width(1100)
        .default_height(720)
        .child(&root)
        .build();

    // 渲染：摘要 + 首个错误 + 结论列表
    let render = {
        let state = state.clone();
        let files_label = files_label.clone();
        let first_card = first_card.clone();
        let summary = summary.clone();
        let findings_list = findings_list.clone();
        let detail = detail.clone();
        move || {
            let mut st = state.borrow_mut();
            let names: Vec<String> = st
                .inputs
                .iter()
                .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
                .collect();
            files_label.set_text(&format!("输入来源: {}", names.join("  +  ")));
            let paths: Vec<PathBuf> = st.inputs.clone();
            let refs: Vec<&std::path::Path> = paths.iter().map(|p| p.as_path()).collect();
            match analyze(&refs) {
                Ok(report) => {
                    summary.set_text(&format!(
                        "通道 {} 路 · 错误 {} 处 · 无法判断 {} 处 · 处理步骤 {} 步",
                        report.channels.len(),
                        report.error_count,
                        report.indeterminate_count,
                        report.steps.len()
                    ));
                    render_first_error(&first_card, &report);
                    render_findings(&findings_list, &detail, &report);
                    st.report = Some(report);
                }
                Err(e) => {
                    summary.set_text(&format!("分析失败: {}", e));
                }
            }
        }
    };

    btn_run.connect_clicked({
        let render = render.clone();
        move |_| render()
    });
    btn_add.connect_clicked({
        let state = state.clone();
        let render = render.clone();
        let win = win.clone();
        move |_| {
            let dialog = FileChooserDialog::new(
                Some("选择日志文件"),
                Some(&win),
                FileChooserAction::Open,
                &[("打开", ResponseType::Accept), ("取消", ResponseType::Cancel)],
            );
            let state = state.clone();
            let render = render.clone();
            dialog.connect_response(move |d, resp| {
                if resp == ResponseType::Accept {
                    if let Some(f) = d.file() {
                        if let Some(p) = f.path() {
                            state.borrow_mut().inputs.push(p);
                            render();
                        }
                    }
                }
                d.close();
            });
            dialog.present();
        }
    });

    render();
    win.present();
}

fn render_first_error(card: &GtkBox, report: &Report) {
    while let Some(c) = card.first_child() {
        card.remove(&c);
    }
    let title = Label::new(Some("第一个错误状态"));
    title.add_css_class("section-title");
    title.set_xalign(0.0);
    card.append(&title);
    match report.first_error {
        Some(id) => {
            let f = &report.findings[id];
            let text = format!(
                "[{}] 通道 {} · {}ms · {}",
                f.kind,
                f.channel.map(|c| c.to_string()).unwrap_or("-".into()),
                f.at_ms.map(|t| t.to_string()).unwrap_or("-".into()),
                f.summary
            );
            let l = Label::new(Some(&text));
            l.set_xalign(0.0);
            l.set_wrap(true);
            card.append(&l);
        }
        None => {
            card.append(&Label::new(Some("未发现错误状态")));
        }
    }
}

fn render_findings(list: &ListBox, detail: &GtkBox, report: &Report) {
    while let Some(c) = list.first_child() {
        list.remove(&c);
    }
    let findings: Vec<Finding> = report.findings.clone();
    let steps = report.steps.clone();
    for f in &findings {
        let row_box = GtkBox::new(Orientation::Horizontal, 8);
        set_margins(&row_box, 4);
        row_box.append(&verdict_label(&f.verdict));
        let text = format!(
            "#{} [{}] ch{} @{}\n{}",
            f.id,
            f.kind,
            f.channel.map(|c| c.to_string()).unwrap_or("-".into()),
            f.at_ms.map(|t| format!("{}ms", t)).unwrap_or("-".into()),
            f.summary
        );
        let l = Label::new(Some(&text));
        l.set_xalign(0.0);
        l.set_wrap(true);
        row_box.append(&l);
        if f.verdict == Verdict::Indeterminate {
            row_box.add_css_class("boundary-card");
        }
        let row = ListBoxRow::new();
        row.set_child(Some(&row_box));
        list.append(&row);
    }

    let findings_c = findings.clone();
    let detail = detail.clone();
    list.connect_row_selected(move |_, row| {
        let Some(row) = row else { return };
        let f = &findings_c[row.index() as usize];
        render_detail(&detail, f, &steps);
    });
}

fn render_detail(detail: &GtkBox, f: &Finding, steps: &[syncwall::model::ProcessingStep]) {
    while let Some(c) = detail.first_child() {
        detail.remove(&c);
    }
    let head = GtkBox::new(Orientation::Horizontal, 8);
    head.append(&verdict_label(&f.verdict));
    head.append(&Label::new(Some(&format!("结论 #{} [{}]", f.id, f.kind))));
    detail.append(&head);

    let sum = Label::new(Some(&f.summary));
    sum.set_xalign(0.0);
    sum.set_wrap(true);
    detail.append(&sum);

    // 处理步骤：结论可回溯到管线的哪一步
    if let Some(step) = steps.get(f.step_index) {
        let s = Label::new(Some(&format!(
            "处理步骤 {} · {} — {}",
            step.index, step.name, step.detail
        )));
        s.set_xalign(0.0);
        detail.append(&s);
    }

    // 证据链：每条结论回到原始输入行
    let ev_title = Label::new(Some("证据链（原始输入）"));
    ev_title.add_css_class("section-title");
    ev_title.set_xalign(0.0);
    detail.append(&ev_title);
    for e in &f.evidence {
        let ev = Label::new(Some(&format!(
            "{}:{}\n  {}\n  → {}",
            e.source_file, e.line_no, e.raw_excerpt, e.note
        )));
        ev.add_css_class("evidence");
        ev.set_xalign(0.0);
        ev.set_selectable(true);
        detail.append(&ev);
    }
}
