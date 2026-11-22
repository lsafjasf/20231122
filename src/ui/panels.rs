use gtk::prelude::*;
use gtk::{Align, Button, Orientation};

use crate::model::{Finding, Session};
use crate::ui::chart::{queue_chart, sparkline};
use crate::ui::components::{
    Toast, hbox, hspacer, icon_text, label_with, mono, panel, pill, rule, text_button, vbox,
    wrapped,
};
use crate::ui::dialogs::{show_evidence_dialog, show_message_dialog, show_trace_dialog};

pub fn metrics(session: &Session) -> gtk::Box {
    let row = hbox(10);
    row.set_homogeneous(true);

    let conclusion = metric_card(
        "检查结论",
        "3 : 2",
        "",
        "自动 / 人工 · 1 项待确认",
        Some(pill("需复核", "status-warn")),
        None,
    );
    let peak = metric_card(
        "背压峰值",
        &format!("{:.2}", session.peak_queue()),
        "s",
        "阈值 3.50 s · 超限 +0.87 s",
        Some(pill("OVER", "status-bad")),
        Some(sparkline(
            session.queue_series.clone(),
            (0.30, 0.90, 0.78),
            88,
        )),
    );
    let cancels = metric_card(
        "中途取消",
        "2",
        "次",
        "RUN-01 / RUN-02 · 均无 EOS",
        Some(pill("AUDITED", "status-neutral")),
        None,
    );
    let evidence = metric_card(
        "证据完整性",
        "87",
        "%",
        "缺 RUN-02 尾段 380 ms",
        Some(pill("PARTIAL", "status-warn")),
        None,
    );

    row.append(&conclusion);
    row.append(&peak);
    row.append(&cancels);
    row.append(&evidence);
    row
}

fn metric_card(
    key: &str,
    value: &str,
    unit: &str,
    note: &str,
    badge: Option<gtk::Label>,
    extra: Option<gtk::DrawingArea>,
) -> gtk::Box {
    let card = vbox(6);
    card.add_css_class("metric-card");
    card.set_hexpand(true);

    let top = hbox(6);
    top.append(&label_with(key, &["metric-key"]));
    top.append(&hspacer());
    if let Some(badge) = badge {
        top.append(&badge);
    }
    card.append(&top);

    let value_row = hbox(7);
    value_row.append(&mono(value, &["metric-value"]));
    if !unit.is_empty() {
        let unit_label = mono(unit, &["metric-unit"]);
        unit_label.set_valign(Align::End);
        unit_label.set_margin_bottom(3);
        value_row.append(&unit_label);
    }
    value_row.append(&hspacer());
    if let Some(extra) = extra {
        extra.set_valign(Align::End);
        value_row.append(&extra);
    }
    card.append(&value_row);
    card.append(&label_with(note, &["metric-note"]));
    card
}

pub fn chart_panel(session: &Session) -> gtk::Box {
    let panel = panel(0);
    panel.set_hexpand(true);

    let header = hbox(8);
    header.add_css_class("panel-header");
    let titles = vbox(3);
    titles.append(&label_with("BACKPRESSURE WINDOW", &["panel-kicker"]));
    let title_row = hbox(8);
    title_row.append(&label_with("实时背压窗口", &["panel-title"]));
    title_row.append(&pill("超限", "status-warn"));
    titles.append(&title_row);
    titles.append(&label_with(
        "queue buffered-time · RUN-01 + RUN-02 · 10 Hz",
        &["panel-sub"],
    ));
    header.append(&titles);
    header.append(&hspacer());

    let legend = hbox(12);
    legend.set_valign(Align::End);
    legend.append(&legend_item("#3ee2c4", "队列时长"));
    legend.append(&legend_item("#ff6e75", "取消点"));
    legend.append(&legend_item("#f0b84d", "阈值"));
    header.append(&legend);
    panel.append(&header);

    let chart_wrap = vbox(0);
    chart_wrap.add_css_class("chart-wrap");
    chart_wrap.append(&queue_chart(session.queue_series.clone(), 3.5));
    panel.append(&chart_wrap);
    panel.append(&rule());

    let footer = hbox(16);
    footer.set_margin_top(9);
    footer.set_margin_bottom(9);
    footer.set_margin_start(16);
    footer.set_margin_end(16);
    footer.append(&mono("PEAK 4.37 s", &["xs", "status-bad"]));
    footer.append(&mono("LIMIT 3.50 s", &["xs", "muted"]));
    footer.append(&mono("SAMPLES 622", &["xs", "muted"]));
    footer.append(&mono("CANCELS 2", &["xs", "status-warn"]));
    footer.append(&hspacer());
    let evidence = chip("E-03", session.clone(), None, None);
    if let Some(evidence) = evidence {
        footer.append(&evidence);
    }
    panel.append(&footer);
    panel
}

fn legend_item(color: &str, text: &str) -> gtk::Box {
    let row = hbox(5);
    let dot = gtk::DrawingArea::new();
    dot.set_content_width(6);
    dot.set_content_height(6);
    dot.set_valign(Align::Center);
    let rgba = parse_hex(color);
    dot.set_draw_func(move |_, cr, width, height| {
        cr.set_source_rgba(rgba.0, rgba.1, rgba.2, 1.0);
        cr.arc(
            f64::from(width) / 2.0,
            f64::from(height) / 2.0,
            f64::from(width) / 2.0,
            0.0,
            std::f64::consts::TAU,
        );
        cr.fill().ok();
    });
    row.append(&dot);
    row.append(&label_with(text, &["xs", "muted"]));
    row
}

fn parse_hex(value: &str) -> (f64, f64, f64) {
    let value = value.trim_start_matches('#');
    let red = u8::from_str_radix(&value[0..2], 16).unwrap_or(255) as f64 / 255.0;
    let green = u8::from_str_radix(&value[2..4], 16).unwrap_or(255) as f64 / 255.0;
    let blue = u8::from_str_radix(&value[4..6], 16).unwrap_or(255) as f64 / 255.0;
    (red, green, blue)
}

pub fn runs_panel(session: &Session, window: &gtk::Window, toast: &Toast) -> gtk::Box {
    let panel = panel(0);
    panel.set_width_request(370);

    let header = hbox(8);
    header.add_css_class("panel-header");
    let titles = vbox(3);
    titles.append(&label_with("CANCELLATION AUDIT", &["panel-kicker"]));
    let title_row = hbox(8);
    title_row.append(&label_with("取消记录", &["panel-title"]));
    title_row.append(&pill("2 次中途取消", "status-bad"));
    titles.append(&title_row);
    titles.append(&label_with("每次停止都绑定最后处理步骤", &["panel-sub"]));
    header.append(&titles);
    header.append(&hspacer());
    panel.append(&header);

    let body = vbox(8);
    body.set_margin_start(12);
    body.set_margin_end(12);
    body.set_margin_bottom(10);
    for (index, run) in session.runs.iter().enumerate() {
        body.append(&run_card(index, run, session, window, toast));
    }
    let note = icon_text(
        "●",
        "两段运行均终止于 PAUSED，未产生 EOS / ERROR。",
        "status-good",
        "xs",
    );
    note.set_margin_start(2);
    body.append(&note);
    panel.append(&body);
    panel
}

fn run_card(
    index: usize,
    run: &crate::model::Run,
    session: &Session,
    window: &gtk::Window,
    toast: &Toast,
) -> gtk::Button {
    let card = gtk::Button::new();
    card.add_css_class("cancel-row");
    card.set_hexpand(true);

    let content = vbox(6);
    let top = hbox(7);
    top.append(&mono(
        &format!("#{}  {}", index + 1, run.cancel_kind),
        &["cancel-number"],
    ));
    top.append(&hspacer());
    top.append(&pill(&format!("PEAK {}", run.queue_peak), "status-warn"));
    content.append(&top);

    let time_row = hbox(8);
    time_row.append(&mono(&run.cancel_at, &["cancel-time"]));
    time_row.append(&label_with(
        &format!("{} · {}", run.trigger, run.actor),
        &["xs", "muted"],
    ));
    time_row.append(&hspacer());
    time_row.append(&label_with("›", &["muted"]));
    content.append(&time_row);

    let bottom = hbox(6);
    let step = session.step(&run.last_step);
    let last_step = step
        .map(|step| format!("{} · {} / {}", step.id, step.name, step.element))
        .unwrap_or_else(|| run.last_step.clone());
    bottom.append(&mono(&last_step, &["xs", "dim"]));
    bottom.append(&hspacer());
    for evidence in &run.evidence {
        if let Some(chip) = chip(
            evidence,
            session.clone(),
            Some(window.clone()),
            Some(toast.clone()),
        ) {
            bottom.append(&chip);
        }
    }
    content.append(&bottom);
    card.set_child(Some(&content));
    card
}

pub fn boundary_strip(session: &Session, window: &gtk::Window, toast: &Toast) -> gtk::Box {
    let card = hbox(14);
    card.add_css_class("boundary-card");
    card.set_margin_top(2);
    card.set_margin_bottom(2);

    let mark = label_with("!", &["metric-value", "status-warn"]);
    mark.set_width_chars(1);
    mark.set_halign(Align::Center);
    card.append(&mark);

    let copy = vbox(4);
    copy.set_hexpand(true);
    let title_row = hbox(8);
    title_row.append(&label_with(
        "无法判断边界 · 保留原始证据",
        &["boundary-title"],
    ));
    title_row.append(&mono(&session.boundary.window, &["boundary-window"]));
    title_row.append(&pill("INDETERMINATE", "status-warn"));
    copy.append(&title_row);
    copy.append(&wrapped(&session.boundary.summary, &["boundary-body"]));
    let evidence = hbox(6);
    evidence.append(&label_with("关键证据", &["xs", "dim"]));
    for id in &session.boundary.evidence {
        if let Some(chip) = chip(
            id,
            session.clone(),
            Some(window.clone()),
            Some(toast.clone()),
        ) {
            evidence.append(&chip);
        }
    }
    evidence.append(&label_with(
        "阈值 3.50 s · 取消 #2 后 380 ms 观察中断",
        &["xs", "dim"],
    ));
    copy.append(&evidence);
    card.append(&copy);

    let action_column = vbox(4);
    action_column.set_valign(Align::Center);
    let action = text_button("转人工 · 锁定证据", "btn-ghost");
    let toast_clone = toast.clone();
    action.connect_clicked(move |_| {
        toast_clone.show("边界 B-01 已锁定；自动结论不会升级为故障");
    });
    action_column.append(&action);
    action_column.append(&mono("H-02 · PENDING", &["xs", "status-warn"]));
    card.append(&action_column);
    card
}

pub fn findings_panel(session: &Session, window: &gtk::Window, toast: &Toast) -> gtk::Box {
    let panel = panel(0);
    panel.set_hexpand(true);

    let header = hbox(8);
    header.add_css_class("panel-header");
    let titles = vbox(3);
    let kicker = hbox(7);
    kicker.append(&label_with("AUTOMATED REPORT", &["panel-kicker"]));
    kicker.append(&pill("AUTO", "status-good"));
    titles.append(&kicker);
    titles.append(&label_with("自动结论", &["panel-title"]));
    titles.append(&label_with(
        "仅由 GStreamer 采样、原始输入与处理步骤推导",
        &["panel-sub"],
    ));
    header.append(&titles);
    header.append(&hspacer());
    header.append(&mono("3 FINDINGS", &["xs", "dim"]));
    panel.append(&header);

    let body = vbox(7);
    body.set_margin_start(12);
    body.set_margin_end(12);
    body.set_margin_bottom(11);
    for finding in &session.auto_findings {
        body.append(&finding_card(finding, session, window, toast));
    }
    let note = wrapped(
        "自动结论不采纳人工判断；任何人工补充只进入右侧独立报告。",
        &["xs", "dim"],
    );
    note.set_margin_start(3);
    body.append(&note);
    panel.append(&body);
    panel
}

fn finding_card(
    finding: &Finding,
    session: &Session,
    window: &gtk::Window,
    toast: &Toast,
) -> gtk::Box {
    let card = vbox(5);
    card.add_css_class("finding");
    card.add_css_class(match finding.severity.as_str() {
        "high" => "finding-high",
        "medium" => "finding-medium",
        _ => "finding-boundary",
    });

    let top = hbox(7);
    let severity = match finding.severity.as_str() {
        "high" => ("高置信", "status-bad"),
        "medium" => ("已观测", "status-warn"),
        _ => ("边界", "status-warn"),
    };
    top.append(&pill(severity.0, severity.1));
    top.append(&label_with(&finding.title, &["finding-title"]));
    top.append(&hspacer());
    top.append(&mono(&format!("{}%", finding.confidence), &["confidence"]));
    card.append(&top);
    card.append(&wrapped(&finding.summary, &["finding-summary"]));

    let refs = hbox(6);
    refs.append(&label_with("回到", &["xs", "dim"]));
    refs.append(&source_chip(session, window));
    for step_id in &finding.step_refs {
        if let Some(step) = session.step(step_id) {
            let chip = Button::with_label(&format!("{} {}", step.id, step.time));
            chip.add_css_class("evidence-chip");
            let parent = window.clone();
            let session_clone = session.clone();
            let reference = step.id.clone();
            chip.set_tooltip_text(Some(&format!("{} · {}", step.name, step.element)));
            chip.connect_clicked(move |_| {
                crate::ui::dialogs::show_trace_dialog(&parent, &session_clone, &reference);
            });
            refs.append(&chip);
        }
    }
    for evidence in &finding.evidence {
        if let Some(chip) = chip(
            evidence,
            session.clone(),
            Some(window.clone()),
            Some(toast.clone()),
        ) {
            refs.append(&chip);
        }
    }
    card.append(&refs);
    card
}

pub fn manual_panel(session: &Session, window: &gtk::Window, toast: &Toast) -> gtk::Box {
    let panel = panel(0);
    panel.set_hexpand(true);

    let header = hbox(8);
    header.add_css_class("panel-header");
    let titles = vbox(3);
    let kicker = hbox(7);
    kicker.append(&label_with("HUMAN CONFIRMATION", &["panel-kicker"]));
    kicker.append(&pill("HUMAN", "status-human"));
    titles.append(&kicker);
    titles.append(&label_with("人工确认", &["panel-title"]));
    titles.append(&label_with(
        "与自动结论分栏保存，记录确认人和时间",
        &["panel-sub"],
    ));
    header.append(&titles);
    header.append(&hspacer());
    let pending = session.pending_manual();
    let confirmed = session.manual_confirmations.len().saturating_sub(pending);
    header.append(&mono(
        &format!("{confirmed} 已确认 · {pending} 待确认"),
        &["xs", "status-human"],
    ));
    panel.append(&header);

    let body = vbox(7);
    body.set_margin_start(12);
    body.set_margin_end(12);
    body.set_margin_bottom(11);
    for manual in &session.manual_confirmations {
        body.append(&human_card(manual, session, window, toast));
    }

    let guard = vbox(3);
    guard.add_css_class("source-box");
    guard.append(&label_with("人工判读护栏", &["source-val"]));
    guard.append(&wrapped(
        "未确认的边界保持 INDETERMINATE；人工补录必须同时引用输入、步骤与原始证据，不能覆盖自动报告。",
        &["xs", "dim"],
    ));
    body.append(&guard);

    let footer = hbox(8);
    footer.append(&wrapped(
        &format!(
            "边界 {} 必须由人工补录；当前自动判定保持降级。",
            session.boundary.id
        ),
        &["xs", "dim"],
    ));
    footer.append(&hspacer());
    let assign = text_button("指派复核", "btn-ghost");
    let toast_clone = toast.clone();
    assign.connect_clicked(move |_| {
        toast_clone.show("复核任务已创建 · 指派给 l.shen · 关联 B-01");
    });
    footer.append(&assign);
    body.append(&footer);
    panel.append(&body);
    panel
}

fn human_card(
    manual: &crate::model::ManualConfirmation,
    session: &Session,
    window: &gtk::Window,
    toast: &Toast,
) -> gtk::Box {
    let card = vbox(5);
    card.add_css_class("human-card");
    let top = hbox(7);
    let (status, class) = if manual.status == "confirmed" {
        ("已确认", "status-good")
    } else {
        ("待确认", "status-warn")
    };
    top.append(&pill(status, class));
    top.append(&label_with(&manual.title, &["human-title"]));
    top.append(&hspacer());
    top.append(&mono(
        &format!("{} · {}", manual.author, manual.time),
        &["xs", "dim"],
    ));
    card.append(&top);
    card.append(&wrapped(&manual.note, &["human-note"]));
    let refs = hbox(6);
    refs.append(&label_with("引用", &["xs", "dim"]));
    refs.append(&source_chip(session, window));
    for id in &manual.evidence {
        if let Some(chip) = chip(
            id,
            session.clone(),
            Some(window.clone()),
            Some(toast.clone()),
        ) {
            refs.append(&chip);
        }
    }
    card.append(&refs);
    card
}

pub fn failure_panel(session: &Session, window: &gtk::Window, toast: &Toast) -> gtk::Box {
    let card = hbox(16);
    card.add_css_class("failure-card");

    let copy = vbox(4);
    copy.set_hexpand(true);
    let title = hbox(8);
    title.append(&label_with(&session.failure.title, &["failure-title"]));
    title.append(&mono(&session.failure.code, &["failure-code"]));
    copy.append(&title);
    copy.append(&wrapped(&session.failure.detail, &["failure-text"]));
    copy.append(&mono(&session.failure.command, &["xs", "failure-code"]));
    card.append(&copy);

    let actions = gtk::Grid::new();
    actions.set_column_spacing(6);
    actions.set_row_spacing(6);
    actions.set_valign(Align::Center);

    let retry = action_button("↻  重试来源探测");
    let parent = window.clone();
    let toast_clone = toast.clone();
    let uri = session.source.uri.clone();
    let command = session.failure.command.clone();
    retry.connect_clicked(move |_| {
        toast_clone.show("正在启动 GStreamer 来源探针…");
        match crate::gst_probe::validate_source(&uri) {
            Ok(report) => show_message_dialog(
                &parent,
                "来源探针已完成",
                &format!(
                    "GStreamer 在 {} ms 内完成 preroll（{}），来源可访问。",
                    report.preroll_ms, report.state
                ),
                Some("注意：本次成功不代表 RUN-02 缺失的 380 ms 日志已经恢复。"),
                None,
                &toast_clone,
            ),
            Err(error) => show_message_dialog(
                &parent,
                "来源探测失败",
                &error,
                Some("保持证据包为 PARTIAL，并先执行下方诊断命令或联系边缘节点值班。"),
                Some(&command),
                &toast_clone,
            ),
        }
    });
    actions.attach(&retry, 0, 0, 1, 1);

    let copy_command = action_button("⌘  复制诊断命令");
    let toast_clone = toast.clone();
    let command = session.failure.command.clone();
    copy_command.connect_clicked(move |_| {
        crate::ui::components::copy_to_clipboard(&command);
        toast_clone.show("诊断命令已复制，可在 GStreamer 环境复现");
    });
    actions.attach(&copy_command, 1, 0, 1, 1);

    let export = action_button("⇩  导出证据包");
    let parent = window.clone();
    let toast_clone = toast.clone();
    let session_clone = session.clone();
    let session_id = session.id.clone();
    export.connect_clicked(move |_| match export_session(&session_clone, &session_id) {
        Ok(path) => show_message_dialog(
            &parent,
            "证据包已导出",
            "原始输入、处理步骤、取消审计、自动结论和人工确认已一并写入。",
            Some(&path),
            None,
            &toast_clone,
        ),
        Err(error) => show_message_dialog(
            &parent,
            "导出失败",
            &error,
            Some("请检查 artifacts/exports 目录的写入权限。"),
            None,
            &toast_clone,
        ),
    });
    actions.attach(&export, 2, 0, 1, 1);

    let review = action_button("＋  创建复核任务");
    let toast_clone = toast.clone();
    review.connect_clicked(move |_| {
        toast_clone.show("L1 复核任务已创建 · 关联 SRC-20261122-042 / B-01");
    });
    actions.attach(&review, 3, 0, 1, 1);
    card.append(&actions);
    card
}

fn action_button(text: &str) -> gtk::Button {
    let button = Button::with_label(text);
    button.add_css_class("action-button");
    button
}

pub fn export_session(session: &Session, session_id: &str) -> Result<String, String> {
    let directory = std::path::Path::new("artifacts/exports");
    std::fs::create_dir_all(directory).map_err(|error| format!("创建导出目录失败: {error}"))?;
    let path = directory.join(format!("{session_id}.json"));
    let pretty = serde_json::to_string_pretty(session)
        .map_err(|error| format!("序列化证据包失败: {error}"))?;
    std::fs::write(&path, pretty).map_err(|error| format!("写入证据包失败: {error}"))?;
    Ok(path.display().to_string())
}

pub fn steps_panel(session: &Session, window: &gtk::Window, toast: &Toast) -> gtk::Box {
    let panel = panel(0);
    panel.set_hexpand(true);

    let header = hbox(8);
    header.add_css_class("panel-header");
    let titles = vbox(3);
    titles.append(&label_with("PROCESS TRACE", &["panel-kicker"]));
    let title_row = hbox(8);
    title_row.append(&label_with("来源输入与处理步骤", &["panel-title"]));
    title_row.append(&pill("9 STEPS", "status-neutral"));
    titles.append(&title_row);
    titles.append(&label_with(
        "所有结论均可回跳到原始输入和对应管线步骤",
        &["panel-sub"],
    ));
    header.append(&titles);
    header.append(&hspacer());
    let trace = text_button("查看来源上下文", "btn-ghost");
    let parent = window.clone();
    let session_clone = session.clone();
    trace.connect_clicked(move |_| {
        crate::ui::dialogs::show_trace_dialog(&parent, &session_clone, "IN-01");
    });
    header.append(&trace);
    panel.append(&header);

    let body = vbox(0);
    body.set_margin_start(12);
    body.set_margin_end(12);
    body.set_margin_bottom(11);

    let source = vbox(5);
    source.add_css_class("source-box");
    let source_top = hbox(8);
    source_top.append(&pill("RAW INPUT · IN-01", "status-good"));
    source_top.append(&mono(&session.source.uri, &["source-uri"]));
    source_top.append(&hspacer());
    if let Some(chip) = chip(
        "E-01",
        session.clone(),
        Some(window.clone()),
        Some(toast.clone()),
    ) {
        source_top.append(&chip);
    }
    source.append(&source_top);
    let source_meta = hbox(14);
    source_meta.append(&mono(&session.source.format, &["xs", "dim"]));
    source_meta.append(&mono(&session.source.video, &["xs", "dim"]));
    source_meta.append(&mono(&session.source.audio, &["xs", "dim"]));
    source_meta.append(&hspacer());
    source_meta.append(&mono(&session.source.fingerprint, &["xs", "dim"]));
    source.append(&source_meta);
    body.append(&source);

    let columns = hbox(10);
    columns.set_margin_top(8);
    columns.set_margin_bottom(2);
    columns.set_margin_start(4);
    let time_col = label_with("TIME", &["xs", "dim"]);
    time_col.set_width_chars(8);
    columns.append(&time_col);
    columns.append(&label_with("STAGE / ELEMENT", &["xs", "dim"]));
    columns.append(&hspacer());
    columns.append(&label_with("EVIDENCE", &["xs", "dim"]));
    body.append(&columns);
    body.append(&rule());

    for (index, step) in session.steps.iter().enumerate() {
        let row = hbox(10);
        row.add_css_class("step-row");
        row.set_margin_top(3);
        row.set_margin_bottom(3);
        let time = mono(&step.time, &["step-time"]);
        time.set_width_chars(8);
        time.set_valign(Align::Center);
        row.append(&time);

        let dot = gtk::Box::new(Orientation::Horizontal, 0);
        dot.add_css_class("step-dot");
        match step.state.as_str() {
            "warn" => dot.add_css_class("step-dot-warn"),
            "cancel" => dot.add_css_class("step-dot-cancel"),
            _ => {}
        }
        dot.set_valign(Align::Center);
        row.append(&dot);

        let stage = vbox(1);
        stage.set_width_request(155);
        let name = label_with(&step.name, &["step-name"]);
        let element = mono(&step.element, &["step-element"]);
        stage.append(&name);
        stage.append(&element);
        stage.set_valign(Align::Center);
        row.append(&stage);

        let detail = label_with(&step.detail, &["step-detail"]);
        detail.set_ellipsize(gtk::pango::EllipsizeMode::End);
        detail.set_hexpand(true);
        detail.set_valign(Align::Center);
        detail.set_tooltip_text(Some(&step.detail));
        row.append(&detail);

        let refs = hbox(4);
        refs.set_valign(Align::Center);
        for id in &step.evidence {
            if let Some(chip) = chip(
                id,
                session.clone(),
                Some(window.clone()),
                Some(toast.clone()),
            ) {
                refs.append(&chip);
            }
        }
        refs.append(&mono(&step.id, &["xs", "dim"]));
        row.append(&refs);
        body.append(&row);

        if index + 1 < session.steps.len() {
            body.append(&rule());
        }
    }
    panel.append(&body);
    panel
}

fn chip(
    id: &str,
    session: Session,
    window: Option<gtk::Window>,
    toast: Option<Toast>,
) -> Option<gtk::Button> {
    let evidence = session.evidence(id)?.clone();
    let button = Button::with_label(id);
    button.add_css_class("evidence-chip");
    button.set_tooltip_text(Some(&format!("{} · {}", evidence.title, evidence.subtitle)));
    if let (Some(window), Some(toast)) = (window, toast) {
        let id = id.to_string();
        button.connect_clicked(move |_| {
            show_evidence_dialog(&window, &session, &toast, &id);
        });
    }
    Some(button)
}

pub fn evidence_panel(session: &Session, window: &gtk::Window, toast: &Toast) -> gtk::Box {
    let panel = panel(0);
    panel.set_width_request(350);

    let header = hbox(8);
    header.add_css_class("panel-header");
    let titles = vbox(3);
    titles.append(&label_with("EVIDENCE PACK", &["panel-kicker"]));
    let title_row = hbox(8);
    title_row.append(&label_with("关键证据", &["panel-title"]));
    title_row.append(&pill("8 ITEMS", "status-neutral"));
    titles.append(&title_row);
    titles.append(&label_with("原始输入、处理步骤与判读依据", &["panel-sub"]));
    header.append(&titles);
    header.append(&hspacer());
    panel.append(&header);

    let body = vbox(6);
    body.set_margin_start(12);
    body.set_margin_end(12);
    body.set_margin_bottom(12);

    let package_row = hbox(7);
    package_row.set_margin_top(2);
    package_row.append(&mono("PACKAGE 87%", &["xs", "status-warn"]));
    package_row.append(&hspacer());
    package_row.append(&mono("7 / 8 REPLAYABLE", &["xs", "dim"]));
    body.append(&package_row);

    let completeness = gtk::Box::new(Orientation::Horizontal, 0);
    completeness.set_height_request(3);
    completeness.add_css_class("metric-track");
    let filled = gtk::Box::new(Orientation::Horizontal, 0);
    filled.set_width_request(285);
    filled.add_css_class("metric-fill");
    completeness.append(&filled);
    body.append(&completeness);

    for evidence in &session.evidence {
        let row = gtk::Button::new();
        row.add_css_class("source-box");
        let content = hbox(8);
        content.append(&mono(&evidence.id, &["xs", "status-good", "status-pill"]));
        let title = vbox(1);
        title.append(&label_with(&evidence.title, &["source-val"]));
        title.append(&mono(&evidence.kind, &["xs", "dim"]));
        content.append(&title);
        content.append(&hspacer());
        content.append(&label_with("›", &["dim"]));
        row.set_child(Some(&content));
        let parent = window.clone();
        let session_clone = session.clone();
        let toast_clone = toast.clone();
        let id = evidence.id.clone();
        row.connect_clicked(move |_| {
            show_evidence_dialog(&parent, &session_clone, &toast_clone, &id);
        });
        body.append(&row);
    }

    let footer = hbox(8);
    footer.append(&wrapped(
        "E-07 是边界判定的关键证据；缺失的 380 ms 不会被补齐或推测。",
        &["xs", "dim"],
    ));
    footer.append(&hspacer());
    let export = text_button("导出", "btn-ghost");
    let parent = window.clone();
    let toast_clone = toast.clone();
    let session_clone = session.clone();
    let session_id = session.id.clone();
    export.connect_clicked(move |_| match export_session(&session_clone, &session_id) {
        Ok(path) => show_message_dialog(
            &parent,
            "证据包已导出",
            "包含原始输入快照、处理步骤与两组取消审计。",
            Some(&path),
            None,
            &toast_clone,
        ),
        Err(error) => show_message_dialog(
            &parent,
            "导出失败",
            &error,
            Some("检查 artifacts/exports 写入权限。"),
            None,
            &toast_clone,
        ),
    });
    footer.append(&export);
    body.append(&footer);
    panel.append(&body);
    panel
}

fn source_chip(session: &Session, window: &gtk::Window) -> gtk::Button {
    let button = Button::with_label("IN-01");
    button.add_css_class("evidence-chip");
    button.set_tooltip_text(Some(&format!("原始输入 · {}", session.source.uri)));
    let parent = window.clone();
    let session = session.clone();
    button.connect_clicked(move |_| {
        show_trace_dialog(&parent, &session, "IN-01");
    });
    button
}
