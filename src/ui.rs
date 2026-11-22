use anyhow::Result;
use gtk::gdk::prelude::PaintableExt;
use gtk::gsk::prelude::GskRendererExt;
use gtk::prelude::*;
use gtk4 as gtk;
use pango::prelude::FontMapExt;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;

use crate::model::{
    AutoFinding, ConfirmationState, FindingKind, ManualConfirmation, ScanReport, SourceCheck,
    SourceState, StepState,
};
use crate::util::format_bytes;
use crate::{scanner, util};

pub fn run(root: &Path) -> Result<()> {
    let report = if scanner::report_path(root).is_file() {
        scanner::load_report(root)?
    } else {
        scanner::demo(root)?
    };
    let app = gtk::Application::builder()
        .application_id("io.sourcecheck.ledger")
        .build();
    app.connect_startup(|_| install_css());
    let root = root.to_path_buf();
    let report = Rc::new(RefCell::new(report));
    app.connect_activate(move |app| {
        install_font(&root);
        build_window(app, &root, report.clone());
    });
    app.run_with_args::<&str>(&[]);
    Ok(())
}

fn install_font(root: &Path) {
    let font = root.join("assets/fonts/SourceLedgerSans.otf");
    if !font.is_file() {
        return;
    }
    let probe = gtk::Label::new(None);
    if let Some(map) = probe.font_map() {
        if let Err(error) = map.add_font_file(&font) {
            eprintln!("无法加载界面字体 {}: {error}", font.display());
        }
    }
}

fn install_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_data(include_str!("../assets/style.css"));
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

fn build_window(app: &gtk::Application, root: &Path, report: Rc<RefCell<ScanReport>>) {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("来源检查 · 媒体项目打包清单")
        .default_width(1480)
        .default_height(920)
        .build();
    window.set_size_request(1120, 720);

    let shell = gtk::Box::new(gtk::Orientation::Vertical, 0);
    shell.append(&topbar(root));
    shell.append(&summary_band(&report.borrow()));

    let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    body.set_vexpand(true);
    let (sources, stack) = sources_pane(&report.borrow(), root);
    sources.set_size_request(286, -1);
    body.append(&sources);

    let detail = gtk::Box::new(gtk::Orientation::Vertical, 0);
    detail.add_css_class("detail-pane");
    detail.set_hexpand(true);
    detail.append(&stack);
    body.append(&detail);

    let report_panel = report_pane(&report, root);
    report_panel.set_size_request(398, -1);
    body.append(&report_panel);
    shell.append(&body);
    shell.append(&footer(&report.borrow()));
    window.set_child(Some(&shell));

    let selected_from_env = std::env::var("SOURCE_LEDGER_SELECT").ok();
    let default_id = {
        let report = report.borrow();
        selected_from_env.or_else(|| {
            report
                .sources
                .iter()
                .find(|source| source.state == SourceState::Unknown)
                .or_else(|| report.sources.first())
                .map(|source| source.spec.id.clone())
        })
    };
    if let (Some(id), Some(listbox)) = (default_id, find_listbox(&sources)) {
        select_source(&listbox, &id);
    }
    window.present();

    if let Ok(path) = std::env::var("SOURCE_LEDGER_SCREENSHOT") {
        let window_clone = window.clone();
        gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(900), move || {
            capture_window(&window_clone, Path::new(&path));
            window_clone.close();
        });
    }
}

fn capture_window(window: &gtk::ApplicationWindow, path: &Path) {
    let width = window.width().max(1) as f64;
    let height = window.height().max(1) as f64;
    let paintable = gtk::WidgetPaintable::new(Some(window));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, width, height);
    let Some(node) = snapshot.to_node() else {
        eprintln!("截图失败：窗口没有可渲染节点");
        return;
    };
    let Some(surface) = window.surface() else {
        eprintln!("截图失败：窗口没有 GDK surface");
        return;
    };
    let Some(renderer) = gtk::gsk::Renderer::for_surface(&surface) else {
        eprintln!("截图失败：无法创建 GSK renderer");
        return;
    };
    let rect = gtk::graphene::Rect::new(0.0, 0.0, width as f32, height as f32);
    let texture = renderer.render_texture(&node, Some(&rect));
    match texture.save_to_png(path) {
        Ok(()) => println!("已保存窗口快照：{} ({width}x{height})", path.display()),
        Err(error) => eprintln!("截图保存失败：{error}"),
    }
    renderer.unrealize();
}

fn find_listbox(widget: &gtk::Widget) -> Option<gtk::ListBox> {
    if let Some(list) = widget.downcast_ref::<gtk::ListBox>() {
        return Some(list.clone());
    }
    let mut child = widget.first_child();
    while let Some(current) = child {
        if let Some(found) = find_listbox(&current) {
            return Some(found);
        }
        child = current.next_sibling();
    }
    None
}

fn select_source(listbox: &gtk::ListBox, id: &str) {
    for index in 0.. {
        let Some(row) = listbox.row_at_index(index) else {
            break;
        };
        if row.widget_name() == id {
            listbox.select_row(Some(&row));
            return;
        }
    }
    if let Some(first) = listbox.row_at_index(0) {
        listbox.select_row(Some(&first));
    }
}

fn open_path(root: &Path, relative: &str) {
    let path = util::absolute(root, relative);
    let _ = Command::new("xdg-open").arg(path).spawn();
}

fn label(text: &str, class: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class(class);
    label
}

fn flow_label(text: &str, class: &str) -> gtk::Label {
    let label = label(text, class);
    label.set_wrap(true);
    label.set_wrap_mode(pango::WrapMode::WordChar);
    label.set_xalign(0.0);
    label
}

fn hbox(spacing: i32) -> gtk::Box {
    gtk::Box::new(gtk::Orientation::Horizontal, spacing)
}

fn vbox(spacing: i32) -> gtk::Box {
    gtk::Box::new(gtk::Orientation::Vertical, spacing)
}

fn spacer() -> gtk::Box {
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    spacer
}

fn hairline() -> gtk::Box {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    line.add_css_class("hairline");
    line
}

fn state_badge_class(state: SourceState) -> &'static str {
    match state {
        SourceState::Verified | SourceState::Replayed => "badge badge-pass",
        SourceState::Unknown | SourceState::Ambiguous => "badge badge-manual",
        SourceState::Failed => "badge badge-fail",
    }
}

fn dot_class(state: SourceState) -> &'static str {
    match state {
        SourceState::Verified | SourceState::Replayed => "dot-pass",
        SourceState::Unknown | SourceState::Ambiguous => "dot-manual",
        SourceState::Failed => "dot-fail",
    }
}

fn topbar(root: &Path) -> gtk::Widget {
    let bar = hbox(12);
    bar.add_css_class("topbar");
    bar.set_margin_start(22);
    bar.set_margin_end(22);
    bar.set_margin_top(12);
    bar.set_margin_bottom(12);

    let mark = label("S/L", "brand-mark");
    mark.set_halign(gtk::Align::Center);
    mark.set_valign(gtk::Align::Center);
    bar.append(&mark);

    let brand = vbox(1);
    brand.set_valign(gtk::Align::Center);
    brand.append(&label("SOURCE LEDGER  /  来源账本", "brand-kicker"));
    brand.append(&label("媒体项目打包清单", "brand-title"));
    bar.append(&brand);
    bar.append(&spacer());

    let run = vbox(1);
    run.set_valign(gtk::Align::Center);
    let run_label = label("审计运行 · 已落盘", "top-meta");
    run_label.set_halign(gtk::Align::End);
    run.append(&run_label);
    let gst = label("GStreamer 1.28  ·  GTK4  ·  Rust", "top-meta");
    gst.set_halign(gtk::Align::End);
    run.append(&gst);
    bar.append(&run);

    let evidence = gtk::Button::with_label("打开证据目录");
    evidence.add_css_class("ghost-button");
    evidence.set_tooltip_text(Some("打开 artifacts/evidence，查看原始字节与重放记录"));
    let evidence_root = root.to_path_buf();
    evidence.connect_clicked(move |_| {
        open_path(&evidence_root, "artifacts/evidence");
    });
    bar.append(&evidence);
    bar.upcast()
}

fn summary_band(report: &ScanReport) -> gtk::Widget {
    let band = vbox(10);
    band.add_css_class("summary-band");
    band.set_margin_start(24);
    band.set_margin_end(24);
    band.set_margin_top(14);
    band.set_margin_bottom(13);

    let row = hbox(18);
    let title_box = vbox(2);
    title_box.append(&label("SOURCE CHECK / 来源检查", "eyebrow"));
    let title_row = hbox(10);
    title_row.append(&label("交付前来源核对", "page-title"));
    let package = label(&report.project.package_identity, "filter-pill");
    package.set_valign(gtk::Align::Center);
    title_row.append(&package);
    title_box.append(&title_row);
    title_box.append(&label(
        "每一项结论都回指原始输入、处理步骤与保留证据",
        "page-subtitle",
    ));
    row.append(&title_box);
    row.append(&spacer());

    let metrics = [
        (report.counts.total.to_string(), "清单来源"),
        (report.counts.auto_passed.to_string(), "自动通过"),
        (report.counts.needs_manual.to_string(), "待人工"),
        (report.counts.failures.to_string(), "失败"),
        (report.counts.replays.to_string(), "恢复重放"),
    ];
    for (value, name) in metrics {
        let metric = vbox(0);
        metric.add_css_class("metric");
        let value_label = label(&value, "metric-value");
        value_label.set_halign(gtk::Align::End);
        metric.append(&value_label);
        let name_label = label(name, "metric-label");
        name_label.set_halign(gtk::Align::End);
        metric.append(&name_label);
        row.append(&metric);
    }
    band.append(&row);

    let progress = hbox(3);
    progress.set_size_request(-1, 6);
    for source in &report.sources {
        let segment = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        segment.set_hexpand(true);
        segment.set_size_request(20, 6);
        segment.add_css_class(match source.state {
            SourceState::Verified | SourceState::Replayed => "progress-pass",
            SourceState::Unknown | SourceState::Ambiguous => "progress-manual",
            SourceState::Failed => "progress-fail",
        });
        segment.set_tooltip_text(Some(&format!(
            "{} · {}",
            source.spec.label,
            source.state.label()
        )));
        progress.append(&segment);
    }
    band.append(&progress);
    band.upcast()
}

fn footer(report: &ScanReport) -> gtk::Widget {
    let bar = hbox(10);
    bar.add_css_class("footer");
    bar.set_margin_start(22);
    bar.set_margin_end(22);
    bar.set_margin_top(6);
    bar.set_margin_bottom(6);
    bar.append(&label(
        "证据优先：页面不覆盖原始字节；自动结论与人工确认分别保存。",
        "footer",
    ));
    bar.append(&spacer());
    bar.append(&label(&format!("RUN {}", report.run_id), "mono"));
    bar.append(&label("  ·  ", "footer"));
    bar.append(&label(&format!("生成 {}", report.generated_at), "mono"));
    bar.upcast()
}

fn sources_pane(report: &ScanReport, root: &Path) -> (gtk::Widget, gtk::Stack) {
    let pane = vbox(0);
    pane.add_css_class("sources-pane");

    let head = vbox(7);
    head.set_margin_start(16);
    head.set_margin_end(16);
    head.set_margin_top(18);
    head.set_margin_bottom(10);
    let title_row = hbox(6);
    title_row.append(&label("来源清单", "column-head"));
    title_row.append(&spacer());
    title_row.append(&label(
        &format!("{} 项", report.sources.len()),
        "source-role",
    ));
    head.append(&title_row);

    let filters = hbox(5);
    filters.append(&label("全部", "filter-pill active"));
    filters.append(&label(
        &format!("自动 {}", report.counts.auto_passed),
        "filter-pill",
    ));
    filters.append(&label(
        &format!("人工 {}", report.counts.needs_manual),
        "filter-pill",
    ));
    filters.append(&label(
        &format!("失败 {}", report.counts.failures),
        "filter-pill",
    ));
    head.append(&filters);
    pane.append(&head);

    let list = gtk::ListBox::new();
    list.add_css_class("source-list");
    list.set_selection_mode(gtk::SelectionMode::Single);
    list.set_margin_start(9);
    list.set_margin_end(9);
    list.set_margin_bottom(9);

    let stack = gtk::Stack::new();
    stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    stack.set_transition_duration(120);
    stack.set_vexpand(true);
    stack.set_hexpand(true);

    for source in &report.sources {
        let row = gtk::ListBoxRow::new();
        row.set_widget_name(&source.spec.id);
        row.set_activatable(true);
        row.set_child(Some(&source_row(source)));
        list.append(&row);

        let detail = source_detail(report, source, root);
        stack.add_named(&detail, Some(&source.spec.id));
    }

    let stack_clone = stack.clone();
    list.connect_row_selected(move |_, row| {
        if let Some(row) = row {
            stack_clone.set_visible_child_name(&row.widget_name());
        }
    });

    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .child(&list)
        .build();
    scroll.add_css_class("source-scroll");
    pane.append(&scroll);

    if let Some(first) = list.row_at_index(0) {
        list.select_row(Some(&first));
    }
    (pane.upcast(), stack)
}

fn source_row(source: &SourceCheck) -> gtk::Widget {
    let row = hbox(10);
    row.add_css_class("source-row");

    let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    dot.add_css_class(dot_class(source.state));
    dot.set_valign(gtk::Align::Start);
    dot.set_margin_top(6);
    row.append(&dot);

    let text = vbox(2);
    text.set_hexpand(true);
    let name_row = hbox(6);
    name_row.append(&label(&source.spec.label, "source-name"));
    if source.state == SourceState::Replayed {
        name_row.append(&label("重放", "badge badge-auto"));
    }
    text.append(&name_row);
    let path = label(&source.spec.path, "source-path");
    path.set_ellipsize(pango::EllipsizeMode::Middle);
    text.append(&path);
    let meta = label(
        &format!("{}  ·  {}", source.spec.role, source.state.label()),
        "source-role",
    );
    text.append(&meta);
    row.append(&text);

    let status = label(
        match source.state {
            SourceState::Verified => "PASS",
            SourceState::Replayed => "REPLAY",
            SourceState::Unknown => "REVIEW",
            SourceState::Ambiguous => "EDGE",
            SourceState::Failed => "FAIL",
        },
        "source-role",
    );
    status.set_valign(gtk::Align::Start);
    status.set_margin_top(2);
    row.append(&status);
    row.upcast()
}

fn source_detail(report: &ScanReport, source: &SourceCheck, root: &Path) -> gtk::Widget {
    let content = vbox(18);
    content.add_css_class("detail-content");
    content.set_margin_start(24);
    content.set_margin_end(24);
    content.set_margin_top(21);
    content.set_margin_bottom(24);

    let badges = hbox(7);
    badges.append(&label(
        source.state.label(),
        state_badge_class(source.state),
    ));
    if source.state == SourceState::Replayed {
        badges.append(&label("7 字段一致", "badge badge-auto"));
    }
    if source.spec.edge_case.is_some() {
        badges.append(&label("策略停止点", "badge badge-neutral"));
    }
    if source.state == SourceState::Failed {
        badges.append(&label("需恢复输入", "badge badge-neutral"));
    }
    content.append(&badges);

    let title = flow_label(&source.spec.label, "detail-title");
    title.set_width_chars(12);
    content.append(&title);

    let path_row = hbox(8);
    let path = label(&source.spec.path, "detail-path");
    path.set_selectable(true);
    path_row.append(&path);
    path_row.append(&label("·", "source-role"));
    path_row.append(&label(&source.spec.role, "source-role"));
    content.append(&path_row);

    content.append(&meta_card(source, root));
    content.append(&source_callout(report, source));
    content.append(&section_header("处理步骤", "原始输入到判定", "02"));
    content.append(&steps_section(source));
    content.append(&section_header("关键证据", "可独立复核的落盘文件", "03"));
    content.append(&evidence_section(report, source, root));

    if matches!(source.state, SourceState::Unknown | SourceState::Ambiguous) {
        content.append(&raw_section(source));
    }

    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .child(&content)
        .build();
    scroll.add_css_class("detail-scroll");
    scroll.set_vexpand(true);
    scroll.set_hexpand(true);
    scroll.upcast()
}

fn meta_card(source: &SourceCheck, root: &Path) -> gtk::Widget {
    let card = vbox(11);
    card.add_css_class("report-card");
    let header = hbox(8);
    header.append(&label("原始输入", "section-title"));
    header.append(&spacer());
    header.append(&label("IMMUTABLE INPUT", "eyebrow"));
    card.append(&header);

    let grid = gtk::Grid::builder()
        .column_spacing(18)
        .row_spacing(8)
        .build();
    grid.set_column_homogeneous(false);
    let probe = source.probe.as_ref();
    meta_row(&grid, 0, "来源", &source.spec.origin, None);
    meta_row(
        &grid,
        1,
        "声明格式",
        source.spec.declared_format.as_deref().unwrap_or("未声明"),
        None,
    );
    meta_row(
        &grid,
        2,
        "大小 / 修改",
        &format!(
            "{}  /  {}",
            probe
                .map(|p| format_bytes(p.byte_size))
                .unwrap_or("文件缺失".into()),
            probe.map(|p| p.modified_at.as_str()).unwrap_or("—")
        ),
        None,
    );
    let detected = probe
        .and_then(|p| p.detected_mime.clone())
        .unwrap_or_else(|| "未形成可判定媒体".to_string());
    let container = probe
        .and_then(|p| p.container.clone())
        .unwrap_or_else(|| "无".to_string());
    meta_row(
        &grid,
        3,
        "GStreamer",
        &format!("{detected}  ·  {container}"),
        None,
    );
    let digest = probe
        .map(|p| p.sha256.clone())
        .unwrap_or_else(|| "没有为缺失文件生成替代哈希".to_string());
    let digest_display = if digest.len() > 24 && digest.chars().all(|ch| ch.is_ascii_hexdigit()) {
        format!("{}…{}", &digest[..16], &digest[digest.len() - 8..])
    } else {
        digest.clone()
    };
    meta_row(&grid, 4, "SHA-256", &digest_display, Some(&digest));
    meta_row(
        &grid,
        5,
        "证据目录",
        &format!("artifacts/evidence/{}", source.spec.id),
        None,
    );
    card.append(&grid);

    let buttons = hbox(7);
    let folder = gtk::Button::with_label("打开来源证据");
    folder.add_css_class("mini-button");
    let evidence_path = format!("artifacts/evidence/{}", source.spec.id);
    let root = root.to_path_buf();
    let root_for_folder = root.clone();
    folder.connect_clicked(move |_| open_path(&root_for_folder, &evidence_path));
    buttons.append(&folder);
    if let Some(probe) = probe {
        let uri = gtk::Button::with_label("在文件管理器中定位");
        uri.add_css_class("mini-button");
        let root_clone = root.clone();
        let relative = probe.uri.trim_start_matches("file://").to_string();
        uri.connect_clicked(move |_| {
            let path = PathBuf::from(&relative);
            let parent = path.parent().unwrap_or(&root_clone);
            let _ = Command::new("xdg-open").arg(parent).spawn();
        });
        buttons.append(&uri);
    }
    card.append(&buttons);
    card.upcast()
}

fn meta_row(grid: &gtk::Grid, row: i32, key: &str, value: &str, tooltip: Option<&str>) {
    let key = label(key, "meta-key");
    key.set_valign(gtk::Align::Start);
    grid.attach(&key, 0, row, 1, 1);
    let value = flow_label(value, "meta-value");
    value.set_width_chars(32);
    value.set_max_width_chars(58);
    value.set_selectable(true);
    if let Some(tooltip) = tooltip {
        value.set_tooltip_text(Some(tooltip));
    }
    grid.attach(&value, 1, row, 1, 1);
}

fn source_callout(report: &ScanReport, source: &SourceCheck) -> gtk::Widget {
    if let Some(replay) = report
        .replay
        .as_ref()
        .filter(|replay| replay.source_id == source.spec.id)
    {
        return replay_callout(replay);
    }
    let callout = vbox(5);
    callout.add_css_class("callout");
    match source.state {
        SourceState::Unknown => {
            callout.append(&label("自动规则在这里停止", "callout-title"));
            callout.append(&flow_label(
                "扩展名不在来源策略中，GStreamer 也未形成完整媒体流。页面不会把文本外观、文件名或同类扩展当作格式证据；需要人工确认是否保留及采用哪种权威 MIME。",
                "callout-body",
            ));
            callout.append(&label(
                "已保留：original.bin · head.hex · tail.hex · probe.json",
                "step-seq",
            ));
        }
        SourceState::Ambiguous => {
            callout.append(&label("边界不可判定，已保留原始窗口", "callout-title"));
            callout.append(&flow_label(
                "头部出现 ISO BMFF 的 ftyp 标记，但没有完整 moov；无法区分“截断媒体”“恢复残片”或“伪装的头部”。自动结论只确认签名字节，不把它扩大为可交付媒体。",
                "callout-body",
            ));
            callout.append(&label(
                "已保留：original.bin · boundary-window.hex · head.hex · tail.hex",
                "step-seq",
            ));
        }
        SourceState::Failed => {
            callout.add_css_class("callout-fail");
            callout.append(&label("检查失败：输入不存在", "callout-title"));
            callout.append(&flow_label(
                "清单声明了来源与格式，但目标路径没有文件。没有生成占位哈希，也没有用同名文件替代。右侧“失败处置”给出可执行恢复步骤。",
                "callout-body",
            ));
        }
        SourceState::Verified | SourceState::Replayed => {
            let finding = report
                .auto_conclusions
                .iter()
                .find(|finding| finding.source_id == source.spec.id);
            callout.append(&label(
                finding
                    .map(|item| item.headline.as_str())
                    .unwrap_or("自动检查已通过"),
                "callout-title",
            ));
            callout.append(&flow_label(
                finding
                    .map(|item| item.reasoning.as_str())
                    .unwrap_or("GStreamer 已解析输入，原始 SHA-256 已固定。"),
                "callout-body",
            ));
        }
    }
    callout.upcast()
}

fn replay_callout(replay: &crate::model::ReplayRecord) -> gtk::Widget {
    let callout = vbox(8);
    callout.add_css_class("callout");
    callout.add_css_class("callout-pass");
    let header = hbox(8);
    header.append(&label(
        if replay.identical {
            "恢复后的重放已记录，字段一致"
        } else {
            "恢复后的重放存在差异"
        },
        "callout-title",
    ));
    header.append(&spacer());
    header.append(&label(
        if replay.identical {
            "REPLAY MATCH"
        } else {
            "REPLAY DIFF"
        },
        "eyebrow",
    ));
    callout.append(&header);
    callout.append(&flow_label(
        &format!(
            "检查点 {} 的原始快照被重新载入，同一路径再次交给 GStreamer；比较 {}，未覆盖旧证据。",
            replay.checkpoint_id,
            replay.compared_fields.join("、")
        ),
        "callout-body",
    ));
    let hashes = hbox(18);
    hashes.append(&hash_block(
        "检查点原始 SHA-256",
        &replay.original_sha256,
        "重放前",
    ));
    hashes.append(&hash_block(
        "恢复后重放 SHA-256",
        &replay.replay_sha256,
        "重放后",
    ));
    callout.append(&hashes);
    callout.upcast()
}

fn hash_block(title: &str, hash: &str, note: &str) -> gtk::Widget {
    let block = vbox(2);
    block.set_hexpand(true);
    block.append(&label(title, "meta-key"));
    let value = label(
        &format!("{}…{}", &hash[..14], &hash[hash.len() - 8..]),
        "mono",
    );
    value.set_selectable(true);
    value.set_tooltip_text(Some(hash));
    block.append(&value);
    block.append(&label(note, "source-role"));
    block.upcast()
}

fn section_header(title: &str, subtitle: &str, number: &str) -> gtk::Widget {
    let row = hbox(9);
    row.append(&label(number, "section-num"));
    row.append(&label(title, "section-title"));
    row.append(&label(subtitle, "source-role"));
    row.append(&spacer());
    row.upcast()
}

fn steps_section(source: &SourceCheck) -> gtk::Widget {
    let list = vbox(6);
    for step in &source.steps {
        let row = hbox(10);
        row.add_css_class("step");
        row.add_css_class(match step.state {
            StepState::Ok => "step-ok",
            StepState::Warning | StepState::Skipped => "step-warn",
            StepState::Failed => "step-fail",
        });
        let seq = label(&format!("{:02}", step.seq), "step-seq");
        seq.set_valign(gtk::Align::Start);
        seq.set_margin_top(1);
        row.append(&seq);
        let body = vbox(3);
        body.set_hexpand(true);
        let top = hbox(7);
        top.append(&label(&step.action, "step-action"));
        top.append(&spacer());
        top.append(&label(
            match step.state {
                StepState::Ok => "OK",
                StepState::Warning => "CHECK",
                StepState::Failed => "STOP",
                StepState::Skipped => "SKIP",
            },
            "step-seq",
        ));
        body.append(&top);
        body.append(&flow_label(&step.input, "step-flow"));
        let output = flow_label(&format!("→ {}", step.output), "step-flow");
        output.set_tooltip_text(Some(&step.evidence_ref));
        body.append(&output);
        row.append(&body);
        list.append(&row);
    }
    list.upcast()
}

fn evidence_section(report: &ScanReport, source: &SourceCheck, root: &Path) -> gtk::Widget {
    let list = vbox(6);
    for evidence in &source.evidence {
        let row = hbox(10);
        row.add_css_class("evidence");
        let marker = label("•", "section-num");
        marker.set_valign(gtk::Align::Start);
        row.append(&marker);
        let body = vbox(2);
        body.set_hexpand(true);
        let top = hbox(7);
        top.append(&label(&evidence.label, "evidence-title"));
        top.append(&spacer());
        top.append(&label(&evidence.id, "step-seq"));
        body.append(&top);
        body.append(&flow_label(&evidence.note, "evidence-note"));
        let path = label(&evidence.path, "source-path");
        path.set_ellipsize(pango::EllipsizeMode::Middle);
        body.append(&path);
        row.append(&body);
        let open = gtk::Button::with_label("打开");
        open.add_css_class("mini-button");
        let root = root.to_path_buf();
        let path = evidence.path.clone();
        open.connect_clicked(move |_| open_path(&root, &path));
        open.set_valign(gtk::Align::Center);
        row.append(&open);
        list.append(&row);
    }

    if let Some(manual) = report
        .manual_confirmations
        .iter()
        .find(|item| item.source_id == source.spec.id && item.state != ConfirmationState::Pending)
    {
        let note = vbox(2);
        note.add_css_class("report-card");
        note.add_css_class("report-card-manual");
        note.append(&label("人工确认已记录", "evidence-title"));
        note.append(&flow_label(
            &format!(
                "{} · {}{}",
                manual.reviewer.as_deref().unwrap_or("未署名"),
                manual.note.as_deref().unwrap_or("未填写说明").to_string(),
                manual
                    .decided_at
                    .as_deref()
                    .map(|value| format!(" · {value}"))
                    .unwrap_or_default()
            ),
            "evidence-note",
        ));
        list.append(&note);
    }
    list.upcast()
}

fn raw_section(source: &SourceCheck) -> gtk::Widget {
    let section = vbox(7);
    section.append(&section_header("字节窗口", "边界原始输入，不做美化", "04"));
    if let Some(probe) = &source.probe {
        section.append(&label(
            &format!(
                "HEAD · offset 0 · {} bytes",
                probe.head_hex.split_whitespace().count()
            ),
            "meta-key",
        ));
        let head = label(&probe.head_hex, "hexbox");
        head.set_wrap(true);
        head.set_wrap_mode(pango::WrapMode::WordChar);
        head.set_xalign(0.0);
        head.set_selectable(true);
        section.append(&head);
        section.append(&label(
            &format!(
                "TAIL · last {} bytes",
                probe.tail_hex.split_whitespace().count()
            ),
            "meta-key",
        ));
        let tail = label(&probe.tail_hex, "hexbox");
        tail.set_wrap(true);
        tail.set_wrap_mode(pango::WrapMode::WordChar);
        tail.set_xalign(0.0);
        tail.set_selectable(true);
        section.append(&tail);
    }
    section.upcast()
}

fn report_pane(report_cell: &Rc<RefCell<ScanReport>>, root: &Path) -> gtk::Widget {
    let pane = vbox(0);
    pane.add_css_class("report-pane");
    let report = report_cell.borrow();

    let head = vbox(8);
    head.set_margin_start(18);
    head.set_margin_end(18);
    head.set_margin_top(18);
    head.set_margin_bottom(11);
    let top = hbox(8);
    top.append(&label("检查报告", "section-title"));
    top.append(&spacer());
    top.append(&label("READ-ONLY FACTS", "eyebrow"));
    head.append(&top);
    head.append(&flow_label(
        "自动结论只陈述机器可验证事实；人工确认单独记录，并保留决定人与时间。",
        "page-subtitle",
    ));
    head.append(&report_identity(&report));
    pane.append(&head);

    let auto_head = report_subsection(
        "AUTO / 自动结论",
        &format!("{} 条 · 不改写原始输入", report.auto_conclusions.len()),
        "auto",
    );
    auto_head.set_margin_start(14);
    auto_head.set_margin_end(14);
    pane.append(&auto_head);

    let auto_content = vbox(6);
    auto_content.set_margin_start(14);
    auto_content.set_margin_end(14);
    auto_content.set_margin_top(8);
    auto_content.set_margin_bottom(8);
    for finding in &report.auto_conclusions {
        auto_content.append(&auto_card(&report, finding));
    }
    let auto_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .min_content_height(286)
        .max_content_height(320)
        .child(&auto_content)
        .build();
    auto_scroll.add_css_class("report-scroll");
    pane.append(&auto_scroll);

    let divider = hairline();
    divider.set_margin_start(14);
    divider.set_margin_end(14);
    divider.set_margin_top(5);
    divider.set_margin_bottom(5);
    pane.append(&divider);

    let human_head = report_subsection(
        "HUMAN / 人工确认",
        &format!("{} 条 · 决策单独落盘", report.manual_confirmations.len()),
        "manual",
    );
    human_head.set_margin_start(14);
    human_head.set_margin_end(14);
    pane.append(&human_head);

    let human_content = vbox(8);
    human_content.set_margin_start(14);
    human_content.set_margin_end(14);
    human_content.set_margin_top(8);
    human_content.set_margin_bottom(16);
    for manual in &report.manual_confirmations {
        human_content.append(&manual_card(report_cell, root, manual));
    }
    if !report.failures.is_empty() {
        human_content.append(&hairline());
        human_content.append(&report_subsection(
            "FAILURE / 失败处置",
            &format!("{} 条 · 可直接执行", report.failures.len()),
            "fail",
        ));
        for failure in &report.failures {
            human_content.append(&failure_card(failure, root));
        }
    }
    human_content.append(&flow_label(
        "自动结论与人工确认不互相替代；人工选择不会修改源文件或已生成的探测快照。",
        "report-footnote",
    ));

    let human_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .child(&human_content)
        .build();
    human_scroll.add_css_class("report-scroll");
    pane.append(&human_scroll);
    pane.upcast()
}

fn report_identity(report: &ScanReport) -> gtk::Widget {
    let card = vbox(6);
    card.add_css_class("report-card");
    let row = hbox(7);
    row.append(&label(&report.project.package_identity, "source-name"));
    row.append(&spacer());
    row.append(&label(&report.run_id, "mono"));
    card.append(&row);
    card.append(&flow_label(&report.project.request, "report-reason"));
    let scope = flow_label(&report.project.audit_scope, "source-path");
    scope.set_tooltip_text(Some(&format!(
        "清单 SHA-256: {}",
        report.project.manifest_sha256
    )));
    card.append(&scope);
    card.upcast()
}

fn report_subsection(title: &str, meta: &str, kind: &str) -> gtk::Widget {
    let row = hbox(7);
    row.set_margin_top(2);
    let key = label(
        match kind {
            "auto" => "A",
            "manual" => "H",
            _ => "!",
        },
        "section-num",
    );
    row.append(&key);
    row.append(&label(title, "column-head"));
    row.append(&spacer());
    let meta = label(meta, "source-role");
    meta.set_halign(gtk::Align::End);
    row.append(&meta);
    row.upcast()
}

fn auto_card(report: &ScanReport, finding: &AutoFinding) -> gtk::Widget {
    let source = report
        .sources
        .iter()
        .find(|source| source.spec.id == finding.source_id);
    let card = vbox(4);
    card.add_css_class("report-card");
    card.add_css_class(match finding.kind {
        FindingKind::Pass | FindingKind::ReplayConsistent => "report-card-auto",
        FindingKind::Failure => "report-card-fail",
        FindingKind::Unresolved | FindingKind::Ambiguous => "report-card-manual",
    });
    let top = hbox(6);
    let mark = label(
        match finding.kind {
            FindingKind::Pass => "PASS",
            FindingKind::ReplayConsistent => "REPLAY",
            FindingKind::Unresolved => "STOP",
            FindingKind::Ambiguous => "EDGE",
            FindingKind::Failure => "FAIL",
        },
        match finding.kind {
            FindingKind::Pass | FindingKind::ReplayConsistent => "badge badge-pass",
            FindingKind::Failure => "badge badge-fail",
            _ => "badge badge-manual",
        },
    );
    top.append(&mark);
    top.append(&label(
        source
            .map(|item| item.spec.label.as_str())
            .unwrap_or("未知来源"),
        "report-headline",
    ));
    top.append(&spacer());
    top.append(&label(
        &format!("{} refs", finding.evidence_refs.len()),
        "step-seq",
    ));
    card.append(&top);
    card.append(&flow_label(&finding.headline, "report-headline"));
    card.append(&flow_label(&finding.reasoning, "report-reason"));
    let refs = flow_label(
        &format!(
            "步骤 {} · 证据 {}",
            finding
                .step_refs
                .iter()
                .map(|seq| format!("S{seq:02}"))
                .collect::<Vec<_>>()
                .join(" / "),
            finding
                .evidence_refs
                .first()
                .cloned()
                .unwrap_or_else(|| "none".to_string())
        ),
        "source-path",
    );
    refs.set_tooltip_text(Some(&finding.policy));
    card.append(&refs);
    card.upcast()
}

fn manual_card(
    report_cell: &Rc<RefCell<ScanReport>>,
    root: &Path,
    manual: &ManualConfirmation,
) -> gtk::Widget {
    let card = vbox(6);
    card.add_css_class("report-card");
    card.add_css_class(match manual.state {
        ConfirmationState::Pending => "report-card-manual",
        ConfirmationState::Confirmed => "report-card-auto",
        ConfirmationState::Rejected => "report-card-fail",
    });

    let top = hbox(6);
    let state_label = label(
        manual_state_label(manual.state),
        manual_state_badge(manual.state),
    );
    top.append(&state_label);
    top.append(&label(&manual.title, "report-headline"));
    top.append(&spacer());
    top.append(&label(
        &format!("{} refs", manual.evidence_refs.len()),
        "step-seq",
    ));
    card.append(&top);
    card.append(&flow_label(&manual.question, "report-headline"));
    card.append(&flow_label(&manual.blocker, "report-reason"));

    let detail = vbox(2);
    if manual.state != ConfirmationState::Pending {
        detail.append(&flow_label(
            &format!(
                "{} · {}{}",
                manual.reviewer.as_deref().unwrap_or("未署名"),
                manual.note.as_deref().unwrap_or("未填写说明"),
                manual
                    .decided_at
                    .as_deref()
                    .map(|value| format!(" · {value}"))
                    .unwrap_or_default()
            ),
            "report-reason",
        ));
    }
    card.append(&detail);

    let actions = hbox(6);
    let confirm = gtk::Button::with_label("确认保留");
    confirm.add_css_class("mini-button");
    confirm.add_css_class("mini-button-confirm");
    let reject = gtk::Button::with_label("记录排除");
    reject.add_css_class("mini-button");
    reject.add_css_class("mini-button-reject");
    actions.append(&confirm);
    actions.append(&reject);
    actions.append(&spacer());
    actions.append(&label(
        manual
            .evidence_refs
            .first()
            .map(String::as_str)
            .unwrap_or("无证据引用"),
        "source-path",
    ));

    if manual.state != ConfirmationState::Pending {
        confirm.set_sensitive(false);
        reject.set_sensitive(false);
    }
    card.append(&actions);

    let report_cell_confirm = report_cell.clone();
    let root_confirm = root.to_path_buf();
    let id_confirm = manual.id.clone();
    let state_confirm = state_label.clone();
    let detail_confirm = detail.clone();
    let actions_confirm = actions.clone();
    confirm.connect_clicked(move |_| {
        apply_manual_decision(
            &report_cell_confirm,
            &root_confirm,
            &id_confirm,
            ConfirmationState::Confirmed,
            "确认保留",
            &state_confirm,
            &detail_confirm,
            &actions_confirm,
        );
    });

    let report_cell_reject = report_cell.clone();
    let root_reject = root.to_path_buf();
    let id_reject = manual.id.clone();
    let state_reject = state_label.clone();
    let detail_reject = detail.clone();
    let actions_reject = actions.clone();
    reject.connect_clicked(move |_| {
        apply_manual_decision(
            &report_cell_reject,
            &root_reject,
            &id_reject,
            ConfirmationState::Rejected,
            "记录排除",
            &state_reject,
            &detail_reject,
            &actions_reject,
        );
    });
    card.upcast()
}

fn apply_manual_decision(
    report_cell: &Rc<RefCell<ScanReport>>,
    root: &Path,
    id: &str,
    state: ConfirmationState,
    note: &str,
    state_label: &gtk::Label,
    detail: &gtk::Box,
    actions: &gtk::Box,
) {
    match scanner::set_manual_decision(
        root,
        id,
        state,
        Some("界面审阅人".to_string()),
        Some(note.to_string()),
    ) {
        Ok(updated) => {
            *report_cell.borrow_mut() = updated;
            state_label.set_text(manual_state_label(state));
            for class in ["badge-manual", "badge-pass", "badge-fail"] {
                state_label.remove_css_class(class);
            }
            state_label.add_css_class(manual_state_badge(state));
            while let Some(child) = detail.first_child() {
                detail.remove(&child);
            }
            detail.append(&flow_label(
                &format!("界面审阅人 · {note} · {}", crate::util::now_iso()),
                "report-reason",
            ));
            actions.set_sensitive(false);
        }
        Err(error) => {
            state_label.set_text("保存失败");
            state_label.remove_css_class("badge-manual");
            state_label.add_css_class("badge-fail");
            while let Some(child) = detail.first_child() {
                detail.remove(&child);
            }
            detail.append(&flow_label(
                &format!("无法写入人工确认：{error}"),
                "report-reason",
            ));
        }
    }
}

fn manual_state_label(state: ConfirmationState) -> &'static str {
    match state {
        ConfirmationState::Pending => "待人工",
        ConfirmationState::Confirmed => "已确认",
        ConfirmationState::Rejected => "已排除",
    }
}

fn manual_state_badge(state: ConfirmationState) -> &'static str {
    match state {
        ConfirmationState::Pending => "badge badge-manual",
        ConfirmationState::Confirmed => "badge badge-pass",
        ConfirmationState::Rejected => "badge badge-fail",
    }
}

fn failure_card(failure: &crate::model::FailureAction, root: &Path) -> gtk::Widget {
    let card = vbox(7);
    card.add_css_class("report-card");
    card.add_css_class("report-card-fail");
    let top = hbox(7);
    top.append(&label("FAIL", "badge badge-fail"));
    top.append(&label(&failure.code, "mono"));
    top.append(&spacer());
    let open = gtk::Button::with_label("证据");
    open.add_css_class("mini-button");
    let root = root.to_path_buf();
    let evidence_path = format!("artifacts/evidence/{}", failure.source_id);
    open.connect_clicked(move |_| open_path(&root, &evidence_path));
    top.append(&open);
    card.append(&top);
    card.append(&flow_label(&failure.summary, "report-headline"));
    for (index, action) in failure.next_actions.iter().enumerate() {
        let row = vbox(3);
        row.add_css_class("action-row");
        let head = hbox(6);
        head.append(&label(&format!("{}.", index + 1), "step-seq"));
        head.append(&label(&action.label, "action-label"));
        row.append(&head);
        row.append(&flow_label(&action.detail, "action-detail"));
        if let Some(command) = &action.command {
            let command = label(command, "command");
            command.set_wrap(true);
            command.set_wrap_mode(pango::WrapMode::Char);
            command.set_xalign(0.0);
            command.set_selectable(true);
            row.append(&command);
        }
        card.append(&row);
    }
    card.upcast()
}
