use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::rc::Rc;

use gtk::prelude::*;
use gtk4 as gtk;

use crate::model::{FindingStatus, InspectionReport, ManualStatus};
use crate::ui::dialogs;
use crate::ui::widgets::{button, dot, hbox, label, mono, pill, spacer, vbox, wrapped};

pub fn build(window: &gtk::ApplicationWindow, report: &InspectionReport) -> gtk::Box {
    let rail = vbox(0);
    rail.add_css_class("report-rail");
    rail.set_width_request(410);
    rail.set_vexpand(true);

    let statuses: Rc<RefCell<HashMap<String, ManualStatus>>> = Rc::new(RefCell::new(
        report
            .manual_checks
            .iter()
            .map(|check| (check.id.clone(), check.status))
            .collect(),
    ));

    let scroll = gtk::ScrolledWindow::new();
    scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroll.set_vexpand(true);
    scroll.add_css_class("rail-scroll");
    let body = vbox(14);
    body.add_css_class("rail-body");

    let head = hbox(8);
    let titles = vbox(2);
    titles.set_hexpand(true);
    titles.append(&label("核查报告", "rail-title"));
    titles.append(&label(
        "自动结论与人工确认分开保存，互不覆盖。",
        "small muted",
    ));
    head.append(&titles);
    head.append(&pill("SEALED", "pill-ok"));
    body.append(&head);

    let human_value = label(
        &format!(
            "{} / {}",
            report
                .manual_checks
                .iter()
                .filter(|check| check.status == ManualStatus::Confirmed)
                .count(),
            report.manual_checks.len()
        ),
        "summary-number",
    );
    let pending_count = report.pending_manual();
    let pending_pill = pill(&format!("{pending_count} 待确认"), "pill-warn");
    body.append(&summary_grid(report, &human_value));
    body.append(&auto_report(report));
    body.append(&human_report(
        window,
        report,
        &statuses,
        &human_value,
        &pending_pill,
    ));
    body.append(&evidence_chain(window, report));
    let note = hbox(6);
    note.append(&dot("dot-info"));
    note.append(&wrapped(
        "自动结论仅描述输入与处理结果；人工动作不会改写原始结论。",
        "tiny muted",
    ));
    body.append(&note);
    scroll.set_child(Some(&body));
    rail.append(&scroll);
    rail.append(&footer(window, report, &statuses));
    rail
}

fn summary_grid(report: &InspectionReport, human_value: &gtk::Label) -> gtk::Box {
    let grid = hbox(8);
    grid.set_homogeneous(true);
    grid.append(&summary_stat(
        &report.findings.len().to_string(),
        "自动结论",
        "已生成",
        "cyan",
    ));
    grid.append(&summary_stat_widget(
        human_value,
        "人工确认",
        "待复核",
        "amber",
    ));
    grid.append(&summary_stat(
        &report.evidence.len().to_string(),
        "证据项",
        "已封存",
        "green",
    ));
    grid
}

fn summary_stat(value: &str, title: &str, detail: &str, tone: &str) -> gtk::Box {
    summary_stat_widget(&label(value, "summary-number"), title, detail, tone)
}

fn summary_stat_widget(value: &gtk::Label, title: &str, detail: &str, tone: &str) -> gtk::Box {
    let card = vbox(0);
    card.add_css_class("card");
    card.set_margin_top(4);
    card.set_margin_bottom(4);
    card.set_margin_start(4);
    card.set_margin_end(4);
    card.set_size_request(-1, 76);
    card.set_valign(gtk::Align::Center);
    value.add_css_class(tone);
    card.append(value);
    card.append(&label(title, "summary-caption"));
    let detail_label = label(detail, "tiny");
    detail_label.add_css_class(tone);
    card.append(&detail_label);
    card
}

fn auto_report(report: &InspectionReport) -> gtk::Box {
    let block = vbox(0);
    block.add_css_class("report-block");
    block.add_css_class("auto-block");
    let head = hbox(8);
    head.add_css_class("report-block-head");
    head.append(&label("自动结论", "report-block-title"));
    head.append(&pill("AUTO", "pill-info"));
    head.append(&spacer());
    head.append(&pill(&format!("{} 条", report.findings.len()), "pill-info"));
    block.append(&head);
    for finding in &report.findings {
        let row = hbox(9);
        row.add_css_class("report-row");
        let tone = match finding.status {
            FindingStatus::Confirmed => "dot-info",
            FindingStatus::Isolated | FindingStatus::Undecidable => "dot-warn",
        };
        row.append(&dot(tone));
        let text = vbox(1);
        text.set_hexpand(true);
        let title = label(&finding.title, "report-row-title");
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        text.append(&title);
        text.append(&mono(
            &format!(
                "{} · {}",
                finding.id,
                finding.rule.split('·').next().unwrap_or("").trim()
            ),
            "report-row-meta",
        ));
        row.append(&text);
        let status = match finding.status {
            FindingStatus::Confirmed => label("已确认", "tiny cyan"),
            FindingStatus::Isolated => label("已隔离", "tiny amber"),
            FindingStatus::Undecidable => label("无法判定", "tiny amber"),
        };
        row.append(&status);
        block.append(&row);
    }
    block
}

fn human_report(
    window: &gtk::ApplicationWindow,
    report: &InspectionReport,
    statuses: &Rc<RefCell<HashMap<String, ManualStatus>>>,
    human_value: &gtk::Label,
    pending_pill: &gtk::Label,
) -> gtk::Box {
    let block = vbox(0);
    block.add_css_class("report-block");
    block.add_css_class("human-block");
    let head = hbox(8);
    head.add_css_class("report-block-head");
    head.append(&label("人工确认", "report-block-title"));
    head.append(&pill("HUMAN", "pill-warn"));
    head.append(&spacer());
    head.append(pending_pill);
    block.append(&head);
    let intro = wrapped(
        "以下结论必须由监听值班员确认；驳回也会作为独立状态导出。",
        "tiny muted",
    );
    intro.set_margin_top(10);
    intro.set_margin_start(14);
    intro.set_margin_end(14);
    block.append(&intro);

    for check in &report.manual_checks {
        block.append(&manual_card(
            window,
            report,
            ManualCardSpec {
                id: check.id.as_str(),
                title: &check.title,
                detail: &check.detail,
                evidence_ids: &check.evidence_ids,
            },
            statuses,
            human_value,
            pending_pill,
        ));
    }
    block
}

struct ManualCardSpec<'a> {
    id: &'a str,
    title: &'a str,
    detail: &'a str,
    evidence_ids: &'a [String],
}

fn manual_card(
    window: &gtk::ApplicationWindow,
    report: &InspectionReport,
    spec: ManualCardSpec<'_>,
    statuses: &Rc<RefCell<HashMap<String, ManualStatus>>>,
    human_value: &gtk::Label,
    pending_pill: &gtk::Label,
) -> gtk::Box {
    let card = vbox(6);
    card.add_css_class("manual-card");
    let head = hbox(7);
    head.append(&mono(spec.id, "tiny amber"));
    let title_label = label(spec.title, "manual-check-title");
    title_label.set_hexpand(true);
    head.append(&title_label);
    let state_label = label("待确认", "tiny amber");
    head.append(&state_label);
    card.append(&head);
    card.append(&wrapped(spec.detail, "manual-detail"));

    let evidence = hbox(4);
    for evidence_id in spec.evidence_ids {
        let link = button(&format!("依据 {evidence_id}"), "link");
        let window_clone = window.clone();
        let evidence_data = report
            .evidence
            .iter()
            .find(|item| &item.id == evidence_id)
            .cloned();
        link.connect_clicked(move |_| {
            if let Some(evidence) = &evidence_data {
                dialogs::show_evidence(&window_clone, evidence);
            }
        });
        evidence.append(&link);
    }
    card.append(&evidence);

    let actions = hbox(6);
    actions.add_css_class("manual-actions");
    let confirm = button("确认", "");
    let reject = button("驳回", "ghost");
    card.append(&actions);
    actions.append(&spacer());
    actions.append(&reject);
    actions.append(&confirm);

    let apply = {
        let card = card.clone();
        let state_label = state_label.clone();
        let id = spec.id.to_string();
        let statuses = statuses.clone();
        let human_value = human_value.clone();
        let pending_pill = pending_pill.clone();
        let total = report.manual_checks.len();
        move |status: ManualStatus| {
            statuses.borrow_mut().insert(id.clone(), status);
            let borrowed = statuses.borrow();
            let confirmed = borrowed
                .values()
                .filter(|value| **value == ManualStatus::Confirmed)
                .count();
            let pending = borrowed
                .values()
                .filter(|value| **value == ManualStatus::Pending)
                .count();
            human_value.set_text(&format!("{confirmed} / {total}"));
            pending_pill.set_text(&format!("{pending} 待确认"));
            card.remove_css_class("confirmed");
            card.remove_css_class("rejected");
            match status {
                ManualStatus::Confirmed => {
                    card.add_css_class("confirmed");
                    state_label.set_text("已确认");
                    state_label.remove_css_class("amber");
                    state_label.add_css_class("green");
                }
                ManualStatus::Rejected => {
                    card.add_css_class("rejected");
                    state_label.set_text("已驳回");
                    state_label.remove_css_class("amber");
                    state_label.add_css_class("red");
                }
                ManualStatus::Pending => {
                    state_label.set_text("待确认");
                }
            }
        }
    };
    let apply_confirm = apply.clone();
    confirm.connect_clicked(move |_| apply_confirm(ManualStatus::Confirmed));
    reject.connect_clicked(move |_| apply(ManualStatus::Rejected));
    card
}

fn evidence_chain(window: &gtk::ApplicationWindow, report: &InspectionReport) -> gtk::Box {
    let section = vbox(5);
    let head = hbox(8);
    head.append(&label("证据链", "section-heading"));
    head.append(&pill(&format!("{} 项", report.evidence.len()), "pill-info"));
    head.append(&spacer());
    head.append(&label("按处理顺序", "tiny muted"));
    section.append(&head);
    for evidence in &report.evidence {
        let row = button("", "evidence-row");
        let content = vbox(2);
        let top = hbox(7);
        top.append(&label(&evidence.id, "evidence-id"));
        let title = label(&evidence.title, "evidence-title");
        title.set_hexpand(true);
        title.set_halign(gtk::Align::Start);
        top.append(&title);
        top.append(&label("›", "micro muted"));
        content.append(&top);
        let digest = label(&short_digest(&evidence.digest), "evidence-digest mono");
        digest.set_ellipsize(gtk::pango::EllipsizeMode::End);
        digest.set_halign(gtk::Align::Start);
        content.append(&digest);
        row.set_child(Some(&content));
        let window_clone = window.clone();
        let evidence = evidence.clone();
        row.connect_clicked(move |_| dialogs::show_evidence(&window_clone, &evidence));
        section.append(&row);
    }
    section
}

fn short_digest(digest: &str) -> String {
    let clean = digest.replace('\n', " ");
    if clean.chars().count() > 64 {
        format!("{}…", clean.chars().take(64).collect::<String>())
    } else {
        clean
    }
}

fn footer(
    window: &gtk::ApplicationWindow,
    report: &InspectionReport,
    statuses: &Rc<RefCell<HashMap<String, ManualStatus>>>,
) -> gtk::Box {
    let footer = vbox(7);
    footer.add_css_class("rail-footer");
    let export = button("导出证据包", "primary");
    export.set_hexpand(true);
    let window_clone = window.clone();
    let report_for_export = Rc::new(RefCell::new(report.clone()));
    let statuses = statuses.clone();
    export.connect_clicked(move |_| {
        let borrowed = statuses.borrow();
        let mut export_report = report_for_export.borrow().clone();
        for check in &mut export_report.manual_checks {
            if let Some(status) = borrowed.get(&check.id) {
                check.status = *status;
            }
        }
        let path = format!("{}/report.json", export_report.report_dir);
        let result = serde_json::to_string_pretty(&export_report.export_summary())
            .map_err(anyhow::Error::from)
            .and_then(|json| fs::write(&path, json).map_err(anyhow::Error::from));
        match result {
            Ok(()) => dialogs::show_text(
                &window_clone,
                "自动与人工报告已分别写入",
                "EVIDENCE PACKAGE",
                "report.json 保留自动结论，同时记录当前人工确认状态。原始块与处理链保持不变。",
                Some(&format!(
                    "{}\n{} 项证据 · {} 条人工记录",
                    path,
                    export_report.evidence.len(),
                    export_report.manual_checks.len()
                )),
            ),
            Err(error) => dialogs::show_text(
                &window_clone,
                "证据包写入失败",
                "EXPORT FAILED",
                "自动结论仍在界面中。请检查报告目录写权限后重试，不会触碰原始输入。",
                Some(&format!("{error:#}")),
            ),
        }
    });
    footer.append(&export);

    let open = button("打开报告目录", "ghost");
    open.set_hexpand(true);
    let report_dir = report.report_dir.clone();
    let window_clone = window.clone();
    open.connect_clicked(move |_| {
        crate::ui::widgets::copy_to_clipboard(&report_dir);
        dialogs::show_text(
            &window_clone,
            "报告目录已复制",
            "LOCAL EVIDENCE",
            "报告目录路径已复制到剪贴板。该目录包含 report.json、处理链、边界样本和原始扩展。",
            Some(&report_dir),
        );
    });
    footer.append(&open);
    footer.append(&label("原始输入始终只读", "tiny muted"));
    footer
}
