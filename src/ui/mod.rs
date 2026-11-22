mod dialogs;
mod header;
mod page;
mod sidebar;
mod waveform;
mod widgets;

use gtk::prelude::*;
use gtk4 as gtk;

use crate::analyzer;
use crate::model::{FailureState, InspectionReport};
use widgets::{button, hbox, label, mono, spacer, vbox, wrapped};

pub fn run() -> gtk::glib::ExitCode {
    let app = gtk::Application::builder()
        .application_id("io.signaldesk.MonitorRouteInspector")
        .build();
    app.connect_activate(|app| {
        install_css();
        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .title("多声道监听路由 · 来源检查")
            .default_width(1450)
            .default_height(980)
            .build();
        match analyzer::run_demo_inspection() {
            Ok(report) => install_report(&window, &report),
            Err(failure) => install_failure(&window, failure),
        }
        window.present();
    });
    app.run()
}

fn install_css() {
    let provider = gtk::CssProvider::new();
    #[allow(deprecated)]
    provider.load_from_data(include_str!("../../assets/style.css"));
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

pub fn install_report(window: &gtk::ApplicationWindow, report: &InspectionReport) {
    window.set_titlebar(Some(&header::build_header(window, report)));
    let body = hbox(0);
    body.add_css_class("page-bg");

    let main_scroll = gtk::ScrolledWindow::new();
    main_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    main_scroll.set_hexpand(true);
    main_scroll.set_vexpand(true);
    main_scroll.add_css_class("main-scroll");
    main_scroll.set_child(Some(&page::build(window, report)));
    body.append(&main_scroll);
    body.append(&sidebar::build(window, report));
    window.set_child(Some(&body));
}

pub(crate) fn install_failure(window: &gtk::ApplicationWindow, failure: FailureState) {
    window.set_titlebar(Some(&header::build_simple_header(window)));
    let outer = vbox(0);
    outer.add_css_class("page-bg");
    outer.set_vexpand(true);
    let card = vbox(16);
    card.add_css_class("card");
    card.add_css_class("card-pad");
    card.set_width_request(720);
    card.set_halign(gtk::Align::Center);
    card.set_valign(gtk::Align::Center);
    card.set_margin_top(80);
    card.set_margin_bottom(80);

    card.append(&label("INSPECTION FAILED", "eyebrow"));
    card.append(&wrapped(&failure.title, "page-title"));
    card.append(&wrapped(
        "自动检查已停止，原始输入没有被重写。下面是仍然保留的证据和可执行后续操作。",
        "page-subtitle",
    ));
    let code_row = hbox(8);
    code_row.append(&label("ERROR", "meta-key"));
    code_row.append(&mono(&failure.code, "small red"));
    card.append(&code_row);
    card.append(&wrapped(&failure.detail, "small soft"));

    card.append(&label("已保留", "meta-key"));
    let preserved = vbox(5);
    for item in &failure.preserved {
        let row = hbox(8);
        row.append(&label("•", "cyan"));
        row.append(&wrapped(item, "small muted"));
        preserved.append(&row);
    }
    card.append(&preserved);
    card.append(&widgets::hairline());

    let actions = hbox(8);
    let copy = button("复制诊断信息", "");
    let diagnostic = format!(
        "{}\ncode={}\nsource={}\n{}\npreserved={}",
        failure.title,
        failure.code,
        failure.source_path,
        failure.detail,
        failure.preserved.join(" | ")
    );
    copy.connect_clicked(move |_| widgets::copy_to_clipboard(&diagnostic));
    actions.append(&copy);
    let open_source = button("查看来源路径", "");
    let source_path = failure.source_path.clone();
    let window_for_source = window.clone();
    open_source.connect_clicked(move |_| {
        widgets::copy_to_clipboard(&source_path);
        dialogs::show_text(
            &window_for_source,
            "来源路径已复制",
            "SOURCE LOCATION",
            "可在文件管理器中定位原始输入。自动检查不会修改此文件。",
            Some(&source_path),
        );
    });
    actions.append(&open_source);
    actions.append(&spacer());
    let retry = button("重新执行检查", "primary");
    let window_clone = window.clone();
    retry.connect_clicked(move |_| match analyzer::run_demo_inspection() {
        Ok(report) => install_report(&window_clone, &report),
        Err(failure) => install_failure(&window_clone, failure),
    });
    actions.append(&retry);
    card.append(&actions);
    outer.append(&card);
    window.set_child(Some(&outer));
}
