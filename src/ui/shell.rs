use gtk::prelude::*;
use gtk::{Align, Orientation};

use crate::model::Session;
use crate::ui::components::{
    Toast, glyph_button, hbox, hspacer, label_with, mono, pill, rule, text_button, toast, vbox,
    vspacer,
};
use crate::ui::dialogs::show_message_dialog;
use crate::ui::panels::{
    boundary_strip, chart_panel, evidence_panel, export_session, failure_panel, findings_panel,
    manual_panel, metrics, runs_panel, steps_panel,
};

pub fn build_window(app: &gtk::Application, session: &Session) -> gtk::ApplicationWindow {
    let height = std::env::var("BPI_HEIGHT")
        .ok()
        .and_then(|value| value.parse::<i32>().ok())
        .unwrap_or(1000);
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Backpressure Inspector · 来源检查")
        .default_width(1500)
        .default_height(height)
        .decorated(false)
        .build();

    let overlay = gtk::Overlay::new();
    let toast = toast(&overlay);
    let window_ref: &gtk::Window = window.upcast_ref();

    let root = vbox(0);
    root.add_css_class("app-root");
    root.append(&topbar(window_ref, session, &toast));

    let body = hbox(0);
    body.set_vexpand(true);
    body.append(&sidebar(session));
    body.append(&main_content(window_ref, session, &toast));
    root.append(&body);

    overlay.set_child(Some(&root));
    window.set_child(Some(&overlay));
    window
}

fn topbar(window: &gtk::Window, session: &Session, toast: &Toast) -> gtk::Box {
    let bar = hbox(12);
    bar.add_css_class("topbar");

    let mark = gtk::Box::new(Orientation::Vertical, 0);
    mark.add_css_class("brand-mark");
    mark.set_valign(Align::Center);
    let glyph = label_with("≋", &["brand-glyph"]);
    glyph.set_halign(Align::Center);
    glyph.set_valign(Align::Center);
    mark.append(&glyph);
    bar.append(&mark);

    let brand = vbox(0);
    brand.set_valign(Align::Center);
    brand.append(&label_with("RELAY", &["brand-name"]));
    brand.append(&label_with("BACKPRESSURE OBSERVER", &["brand-sub"]));
    bar.append(&brand);

    bar.append(&label_with("/", &["crumb-sep"]));
    bar.append(&label_with("来源检查", &["crumb"]));
    bar.append(&mono(&session.id, &["xs", "dim"]));
    bar.append(&hspacer());

    let live = hbox(6);
    live.add_css_class("env-pill");
    live.set_valign(Align::Center);
    live.append(&label_with("●", &["status-good"]));
    live.append(&mono("GSTREAMER 1.28 · READY", &["xs"]));
    bar.append(&live);

    let search = glyph_button("⌕", "搜索会话");
    let toast_clone = toast.clone();
    search.connect_clicked(move |_| toast_clone.show("会话搜索：按 SRC ID、来源 URI 或操作员筛选"));
    bar.append(&search);

    let help = glyph_button("?", "快捷帮助");
    let toast_clone = toast.clone();
    help.connect_clicked(move |_| toast_clone.show("点击证据编号可查看原始载荷与关联步骤"));
    bar.append(&help);

    let avatar = label_with("LS", &["avatar"]);
    avatar.set_valign(Align::Center);
    avatar.set_halign(Align::Center);
    bar.append(&avatar);

    let close = glyph_button("×", "关闭窗口");
    let window_clone = window.clone();
    close.connect_clicked(move |_| window_clone.close());
    bar.append(&close);
    bar
}

fn sidebar(session: &Session) -> gtk::Box {
    let side = vbox(5);
    side.add_css_class("sidebar");

    side.append(&label_with("WORKSPACE", &["nav-label"]));
    side.append(&nav_item("◫", "监控总览", None, false));
    side.append(&nav_item("⇄", "实时链路", Some("6"), false));
    side.append(&nav_item("⌁", "来源检查", Some("1"), true));
    side.append(&nav_item("▤", "证据库", Some("8"), false));
    side.append(&nav_item("✓", "人工复核", Some("1"), false));
    side.append(&rule());
    side.append(&label_with("SESSION", &["nav-label"]));
    side.append(&nav_item("◇", "本次运行", None, false));
    side.append(&nav_item("↧", "导出记录", None, false));
    side.append(&vspacer());

    let foot = vbox(6);
    foot.add_css_class("side-foot");
    let title = hbox(6);
    title.append(&label_with("●", &["status-good"]));
    title.append(&label_with("本地检查已保存", &["side-foot-title"]));
    foot.append(&title);
    foot.append(&mono(
        &format!("{} · {}", session.id, session.operator),
        &["side-foot-meta"],
    ));
    foot.append(&mono(
        &format!(
            "{} · 11:52",
            session.checked_at.get(0..10).unwrap_or("2026-11-22")
        ),
        &["side-foot-meta"],
    ));
    let stack = hbox(4);
    stack.append(&label_with("Rust", &["stack-pill"]));
    stack.append(&label_with("GStreamer", &["stack-pill"]));
    stack.append(&label_with("GTK4", &["stack-pill"]));
    foot.append(&stack);
    side.append(&foot);
    side
}

fn nav_item(glyph: &str, text: &str, count: Option<&str>, active: bool) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("nav-item");
    if active {
        button.add_css_class("nav-item-active");
    }
    let row = hbox(8);
    row.append(&label_with(glyph, &["nav-glyph"]));
    row.append(&label_with(text, &["sm"]));
    row.append(&hspacer());
    if let Some(count) = count {
        row.append(&label_with(count, &["nav-count"]));
    }
    button.set_child(Some(&row));
    button
}

fn main_content(window: &gtk::Window, session: &Session, toast: &Toast) -> gtk::Box {
    let main = vbox(0);
    main.set_hexpand(true);
    main.set_vexpand(true);
    main.append(&page_header(window, session, toast));

    let scroll = gtk::ScrolledWindow::new();
    scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroll.set_vexpand(true);
    scroll.add_css_class("content-scroll");

    let content = vbox(12);
    content.add_css_class("content");
    content.set_margin_top(0);
    content.append(&metrics(session));
    content.append(&boundary_strip(session, window, toast));

    let first = hbox(12);
    first.append(&chart_panel(session));
    first.append(&runs_panel(session, window, toast));
    content.append(&first);

    let reports = hbox(12);
    reports.set_homogeneous(true);
    reports.append(&findings_panel(session, window, toast));
    reports.append(&manual_panel(session, window, toast));
    content.append(&reports);

    let trace = hbox(12);
    trace.append(&steps_panel(session, window, toast));
    trace.append(&evidence_panel(session, window, toast));
    content.append(&trace);

    content.append(&failure_panel(session, window, toast));
    let footer = hbox(8);
    footer.set_margin_top(2);
    footer.append(&mono(
        "RUST 1.100 · GSTREAMER 1.28.9 · GTK 4.24.1",
        &["xs", "dim"],
    ));
    footer.append(&hspacer());
    footer.append(&mono(
        "所有结论均链接至原始输入与处理步骤 · 证据只读",
        &["xs", "dim"],
    ));
    content.append(&footer);

    scroll.set_child(Some(&content));
    if let Ok(value) = std::env::var("BPI_SCROLL")
        && let Ok(value) = value.parse::<f64>()
    {
        let adjustment = scroll.vadjustment();
        gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(250), move || {
            adjustment.set_value(value);
        });
    }
    main.append(&scroll);
    main
}

fn page_header(window: &gtk::Window, session: &Session, toast: &Toast) -> gtk::Box {
    let header = hbox(18);
    header.add_css_class("page-header");

    let left = vbox(4);
    left.set_hexpand(true);
    left.append(&label_with(
        "SOURCE INSPECTION / TRACEABLE REPORT",
        &["eyebrow"],
    ));
    let title_row = hbox(10);
    title_row.append(&label_with("来源检查", &["page-title"]));
    title_row.append(&pill("证据不足 · 待人工复核", "status-warn"));
    left.append(&title_row);
    left.append(&label_with(
        "核验实时转码入口、背压峰值与中途取消，确保每个结论可回到原始输入和处理步骤。",
        &["page-subtitle"],
    ));

    let meta = hbox(14);
    meta.set_margin_top(7);
    meta.append(&mono(&session.id, &["meta-val"]));
    let uri = mono(&session.source.uri, &["meta-val"]);
    uri.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    uri.set_max_width_chars(58);
    uri.set_tooltip_text(Some(&session.source.uri));
    meta.append(&uri);
    meta.append(&label_with("·", &["meta-key"]));
    meta.append(&mono(&session.source.fingerprint, &["meta-key"]));
    meta.append(&label_with("·", &["meta-key"]));
    meta.append(&mono(
        &format!("OPERATOR {}", session.operator),
        &["meta-key"],
    ));
    left.append(&meta);
    header.append(&left);

    let actions = vbox(10);
    actions.set_valign(Align::Center);
    let button_row = hbox(8);
    button_row.set_halign(Align::End);

    let export = text_button("⇩  导出证据包", "btn");
    let parent = window.clone();
    let toast_clone = toast.clone();
    let session_clone = session.clone();
    let session_id = session.id.clone();
    export.connect_clicked(move |_| match export_session(&session_clone, &session_id) {
        Ok(path) => show_message_dialog(
            &parent,
            "证据包已导出",
            "自动结论、人工确认、取消审计与原始证据已分栏保存。",
            Some(&path),
            None,
            &toast_clone,
        ),
        Err(error) => show_message_dialog(
            &parent,
            "导出失败",
            &error,
            Some("请确认 artifacts/exports 目录可写。"),
            None,
            &toast_clone,
        ),
    });
    button_row.append(&export);

    let rerun = text_button("↻  重新检查", "btn-primary");
    let parent = window.clone();
    let toast_clone = toast.clone();
    rerun.connect_clicked(move |_| {
        toast_clone.show("正在运行 GStreamer 背压探针（约 2.6 s）…");
        match crate::gst_probe::run_backpressure_probe() {
            Ok(report) => {
                let status = if report.peak_ms >= report.threshold_ms {
                    "检测到可复现背压"
                } else {
                    "未检测到背压"
                };
                show_message_dialog(
                    &parent,
                    "本地背压探针完成",
                    &format!(
                        "{status}：{} 个样本，峰值 {} ms，阈值 {} ms，耗时 {} ms。",
                        report.samples,
                        report.peak_ms,
                        report.threshold_ms,
                        report.duration_ms
                    ),
                    Some("这是新的本地探针结果；历史证据链未被覆盖，需人工决定是否合并。"),
                    None,
                    &toast_clone,
                );
            }
            Err(error) => show_message_dialog(
                &parent,
                "背压探针失败",
                &error,
                Some("检查 GStreamer 插件是否完整，再执行下方命令。"),
                Some(
                    "gst-launch-1.0 videotestsrc is-live=true ! queue ! identity sleep-time=55000 ! fakesink sync=false",
                ),
                &toast_clone,
            ),
        }
    });
    button_row.append(&rerun);
    actions.append(&button_row);

    let stamps = hbox(8);
    stamps.set_halign(Align::End);
    stamps.append(&mono(
        &format!("CHECKED {}", session.checked_at),
        &["xs", "dim"],
    ));
    stamps.append(&pill("SESSION OPEN", "status-good"));
    actions.append(&stamps);
    header.append(&actions);
    header
}
