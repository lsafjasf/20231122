use gtk::prelude::*;
use gtk4 as gtk;

use crate::analyzer;
use crate::model::InspectionReport;
use crate::ui::dialogs;
use crate::ui::install_report;
use crate::ui::widgets::{button, hbox, label, mono, pill, spacer, vbox};

pub fn build_header(window: &gtk::ApplicationWindow, report: &InspectionReport) -> gtk::HeaderBar {
    let header = gtk::HeaderBar::new();
    header.add_css_class("app-header");
    header.set_show_title_buttons(true);
    header.set_title_widget(Some(&spacer()));
    header.pack_start(&brand());

    let actions = hbox(8);
    let run = mono(&report.run_id, "pill mono");
    run.add_css_class("small");
    actions.append(&run);
    actions.append(&pill("自动检查完成", "pill-ok"));
    actions.append(&pill("证据已封存", "pill-info"));

    let recheck = button("重新检查", "");
    let window_clone = window.clone();
    recheck.connect_clicked(move |_| match analyzer::run_demo_inspection() {
        Ok(report) => install_report(&window_clone, &report),
        Err(failure) => crate::ui::install_failure(&window_clone, failure),
    });
    actions.append(&recheck);

    let export = button("导出证据包", "primary");
    let report_dir = report.report_dir.clone();
    let run_id = report.run_id.clone();
    let window_clone = window.clone();
    export.connect_clicked(move |_| {
        dialogs::show_text(
            &window_clone,
            "证据包已写入",
            "EVIDENCE SEALED",
            "原始块、F32LE 边界、GStreamer 处理链和自动/人工分离报告均已落盘。",
            Some(&format!("{run_id}\n{report_dir}")),
        );
    });
    actions.append(&export);
    header.pack_end(&actions);
    header
}

pub fn build_simple_header(window: &gtk::ApplicationWindow) -> gtk::HeaderBar {
    let header = gtk::HeaderBar::new();
    header.add_css_class("app-header");
    header.set_show_title_buttons(true);
    header.set_title_widget(Some(&spacer()));
    header.pack_start(&brand());
    let actions = hbox(8);
    actions.append(&pill("检查中止", "pill-danger"));
    let retry = button("重试", "");
    let window_clone = window.clone();
    retry.connect_clicked(move |_| match analyzer::run_demo_inspection() {
        Ok(report) => install_report(&window_clone, &report),
        Err(failure) => crate::ui::install_failure(&window_clone, failure),
    });
    actions.append(&retry);
    header.pack_end(&actions);
    header
}

fn brand() -> gtk::Box {
    let brand = hbox(10);
    let mark = label("≋", "app-mark");
    mark.set_halign(gtk::Align::Center);
    mark.set_valign(gtk::Align::Center);
    brand.append(&mark);
    let names = vbox(0);
    names.append(&label("SIGNAL DESK", "app-name"));
    names.append(&label("MONITOR ROUTING", "app-context"));
    brand.append(&names);
    let separator = hbox(0);
    separator.add_css_class("top-separator");
    brand.append(&separator);
    let page = vbox(0);
    page.append(&label("来源检查", "app-name"));
    page.append(&label("多声道监听路由", "small muted"));
    brand.append(&page);
    brand
}
