use gtk::prelude::*;
use gtk4 as gtk;

use crate::model::{AutoFinding, FindingStatus, InspectionReport, StepStatus, Tone};
use crate::ui::dialogs;
use crate::ui::waveform;
use crate::ui::widgets::{
    badge, button, hairline, hbox, label, meta_block, mono, pill, spacer, vbox, wrapped,
};

pub fn build(window: &gtk::ApplicationWindow, report: &InspectionReport) -> gtk::Box {
    let page = vbox(18);
    page.add_css_class("page");
    page.set_hexpand(true);
    page.set_vexpand(true);
    page.append(&intro(report));
    page.append(&kpi_row(report));
    page.append(&source_card(window, report));
    page.append(&waveform_card(window, report));
    page.append(&auto_section(window, report));
    page.append(&processing_chain(window, report));
    page
}

fn intro(report: &InspectionReport) -> gtk::Box {
    let row = hbox(12);
    let titles = vbox(3);
    titles.set_hexpand(true);
    titles.append(&label("SOURCE INSPECTION · RCV-03", "eyebrow"));
    let heading = hbox(12);
    heading.append(&label("来源检查", "page-title"));
    heading.append(&pill("只读输入", "pill-ok"));
    heading.append(&pill("自动 / 人工分离", "pill-info"));
    titles.append(&heading);
    titles.append(&wrapped(
        "多声道监听路由 · 恢复后重放、未知扩展与判定边界。每个结论都能回到原始输入和 GStreamer 处理步骤。",
        "page-subtitle",
    ));
    row.append(&titles);

    let status = vbox(4);
    status.set_halign(gtk::Align::End);
    status.set_valign(gtk::Align::Center);
    status.append(&pill("检查完成", "pill-ok"));
    status.append(&mono(&report.completed_at, "tiny muted"));
    row.append(&status);
    row
}

fn kpi_row(report: &InspectionReport) -> gtk::Box {
    let row = hbox(10);
    row.set_homogeneous(true);
    row.append(&kpi(
        "8 CH",
        &format!(
            "{} kHz · {}-bit",
            report.source.sample_rate / 1000,
            report.source.bits_per_sample
        ),
        "F32LE 检查窗口",
        "cyan",
    ));
    row.append(&kpi(
        &format!(
            "{:.3} s",
            report.recovery.replay_end_s - report.recovery.replay_start_s
        ),
        "恢复后重放",
        "跨窗逐样本同源",
        "amber",
    ));
    row.append(&kpi(
        &format!("{} B", thousands(report.extension.size)),
        "未知扩展 XMTA",
        "已隔离 · 解析失败",
        "red",
    ));
    row.append(&kpi("41 帧", "判定边界", "原始 PCM 已保留", "green"));
    row
}

fn kpi(value: &str, label_text: &str, detail: &str, tone: &str) -> gtk::Box {
    let card = vbox(2);
    card.add_css_class("kpi");
    let value_label = label(value, "kpi-value");
    card.append(&value_label);
    let label_label = label(label_text, "kpi-label");
    card.append(&label_label);
    let detail = label(detail, "kpi-detail");
    detail.add_css_class(tone);
    card.append(&detail);
    card
}

fn source_card(window: &gtk::ApplicationWindow, report: &InspectionReport) -> gtk::Box {
    let card = vbox(10);
    card.add_css_class("card");
    card.add_css_class("card-pad");
    let head = hbox(9);
    head.append(&badge("◉"));
    head.append(&label("原始输入", "card-title"));
    head.append(&pill("Bus A · 8ch Monitor", "pill-info"));
    head.append(&spacer());
    head.append(&pill(
        if report.source.modified {
            "已修改"
        } else {
            "未修改"
        },
        if report.source.modified {
            "pill-danger"
        } else {
            "pill-ok"
        },
    ));
    let evidence = button("依据 E-01", "link");
    let window_clone = window.clone();
    let evidence_data = evidence_by_id(report, "E-01").cloned();
    evidence.connect_clicked(move |_| {
        if let Some(evidence) = &evidence_data {
            dialogs::show_evidence(&window_clone, evidence);
        }
    });
    head.append(&evidence);
    card.append(&head);

    let file_row = hbox(12);
    let file = vbox(2);
    file.set_hexpand(true);
    file.append(&wrapped(&report.source.filename, "source-name"));
    let path = mono(&report.source.path, "tiny muted");
    path.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    path.set_max_width_chars(90);
    file.append(&path);
    file_row.append(&file);
    let copy = button("复制定位", "ghost");
    let copy_text = format!(
        "{}\nsha256:{}\n{}",
        report.source.path, report.source.sha256, report.run_id
    );
    copy.connect_clicked(move |_| crate::ui::widgets::copy_to_clipboard(&copy_text));
    file_row.append(&copy);
    card.append(&file_row);
    card.append(&hairline());

    let metadata = hbox(20);
    metadata.append(&meta_block("恢复会话", &report.source.session, 175));
    metadata.append(&meta_block("监听路由", &report.source.route, 205));
    metadata.append(&meta_block("采集时间", &report.source.captured_at, 195));
    metadata.append(&meta_block(
        "输入摘要",
        &format!("sha256:{}…", report.digest_short()),
        190,
    ));
    metadata.append(&meta_block(
        "来源大小",
        &format!(
            "{:.1} MB / {:.3} s",
            report.source.size_bytes as f64 / 1_048_576.0,
            report.source.duration_secs
        ),
        150,
    ));
    card.append(&metadata);
    card
}

fn waveform_card(window: &gtk::ApplicationWindow, report: &InspectionReport) -> gtk::Box {
    let card = vbox(0);
    card.add_css_class("card");
    card.add_css_class("wave-card");
    let head = hbox(8);
    head.add_css_class("wave-head");
    let titles = vbox(2);
    titles.set_hexpand(true);
    titles.append(&label("8 声道时间线", "card-title"));
    titles.append(&label(
        "重放区间以琥珀色叠加；恢复间隙与判定边界保留在原始样本。",
        "small muted",
    ));
    head.append(&titles);
    head.append(&legend("正常", "cyan"));
    head.append(&legend("重放", "amber"));
    head.append(&legend("间隙", "red"));
    let evidence = button("处理链 E-02", "link");
    let window_clone = window.clone();
    let evidence_data = evidence_by_id(report, "E-02").cloned();
    evidence.connect_clicked(move |_| {
        if let Some(evidence) = &evidence_data {
            dialogs::show_evidence(&window_clone, evidence);
        }
    });
    head.append(&evidence);
    card.append(&head);
    card.append(&waveform::build_waveform(report));

    let foot = hbox(12);
    foot.add_css_class("wave-foot");
    foot.append(&time_fact(
        "恢复点",
        &format_time(report.recovery.recovery_s),
        "amber",
    ));
    foot.append(&time_fact(
        "重放窗口",
        &format!(
            "{} – {}",
            format_time(report.recovery.replay_start_s),
            format_time(report.recovery.replay_end_s)
        ),
        "amber",
    ));
    foot.append(&time_fact(
        "匹配源",
        &format!(
            "{} – {}",
            format_time(report.recovery.source_start_s),
            format_time(
                report.recovery.source_start_s + report.recovery.replay_end_s
                    - report.recovery.replay_start_s
            )
        ),
        "cyan",
    ));
    foot.append(&time_fact(
        "边界样本",
        &format!(
            "{} ± {}",
            report.recovery.boundary_sample, report.recovery.boundary_frames
        ),
        "red",
    ));
    foot.append(&spacer());
    foot.append(&pill(
        &format!("置信 {:.3}", report.recovery.confidence),
        "pill-info",
    ));
    card.append(&foot);
    card
}

fn legend(text: &str, tone: &str) -> gtk::Box {
    let row = hbox(4);
    let dot = gtk::Box::new(gtk::Orientation::Vertical, 0);
    dot.add_css_class("legend-dot");
    dot.add_css_class(match tone {
        "amber" => "dot-warn",
        "red" => "dot-danger",
        _ => "dot-info",
    });
    row.append(&dot);
    row.append(&label(text, "tiny muted"));
    row
}

fn time_fact(key: &str, value: &str, tone: &str) -> gtk::Box {
    let fact = hbox(6);
    fact.append(&label(key, "tiny muted"));
    let value = mono(value, "small");
    value.add_css_class(tone);
    fact.append(&value);
    fact
}

fn format_time(seconds: f64) -> String {
    let minutes = (seconds / 60.0).floor() as u32;
    let remainder = seconds - minutes as f64 * 60.0;
    format!("{minutes:02}:{remainder:06.3}")
}

fn thousands(value: u64) -> String {
    let digits = value.to_string();
    let mut output = String::new();
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            output.push(',');
        }
        output.push(character);
    }
    output
}

fn evidence_by_id<'a>(
    report: &'a InspectionReport,
    id: &str,
) -> Option<&'a crate::model::Evidence> {
    report.evidence.iter().find(|evidence| evidence.id == id)
}

fn auto_section(window: &gtk::ApplicationWindow, report: &InspectionReport) -> gtk::Box {
    let section = vbox(11);
    section.set_margin_top(4);
    let head = hbox(9);
    head.append(&label("自动结论", "section-heading"));
    head.append(&pill(&format!("{} 条", report.findings.len()), "pill-info"));
    head.append(&label(
        "由检查器生成 · 可逐条回到原始输入与处理步骤",
        "section-note",
    ));
    head.append(&spacer());
    head.append(&pill("2 可判定 / 1 无法判定", "pill-warn"));
    section.append(&head);
    for finding in &report.findings {
        section.append(&finding_card(window, report, finding));
    }
    section
}

fn finding_card(
    window: &gtk::ApplicationWindow,
    report: &InspectionReport,
    finding: &AutoFinding,
) -> gtk::Box {
    let card = vbox(9);
    card.add_css_class("finding");
    let head = hbox(10);
    let (symbol, pill_class, status_text) = match finding.status {
        FindingStatus::Confirmed => ("✓", "pill-info", "已确认"),
        FindingStatus::Isolated => ("!", "pill-warn", "已隔离"),
        FindingStatus::Undecidable => ("?", "pill-warn", "无法判定"),
    };
    let symbol_badge = badge(symbol);
    if finding.tone == Tone::Warn {
        symbol_badge.add_css_class("amber");
    }
    head.append(&symbol_badge);
    let titles = vbox(1);
    titles.set_hexpand(true);
    let title_row = hbox(8);
    title_row.append(&label(&finding.id, "finding-id"));
    title_row.append(&label(&finding.title, "finding-title"));
    titles.append(&title_row);
    titles.append(&label(&finding.rule, "tiny muted"));
    head.append(&titles);
    head.append(&pill(status_text, pill_class));
    let confidence = finding
        .confidence
        .map(|value| format!("置信 {value:.3}"))
        .unwrap_or_else(|| "无置信值".to_string());
    head.append(&label(&confidence, "tiny muted"));
    card.append(&head);
    card.append(&wrapped(&finding.summary, "finding-summary"));

    let trace = vbox(3);
    trace.add_css_class("trace");
    let input = hbox(7);
    input.append(&label("INPUT", "trace-key"));
    let source_ref = mono(&finding.source_ref, "");
    source_ref.set_ellipsize(gtk::pango::EllipsizeMode::End);
    input.append(&source_ref);
    trace.append(&input);
    let processing = hbox(7);
    processing.append(&label("CHAIN", "trace-key"));
    let processing_ref = mono(&finding.processing_ref, "");
    processing_ref.set_ellipsize(gtk::pango::EllipsizeMode::End);
    processing.append(&processing_ref);
    trace.append(&processing);
    card.append(&trace);

    if finding.id == "A-02" {
        card.append(&failure_panel(window, report));
    }
    if finding.id == "A-03" {
        card.append(&boundary_panel(window, report));
    }

    let footer = hbox(5);
    for evidence_id in &finding.evidence_ids {
        let evidence_button = button(&format!("打开 {evidence_id} 依据"), "link");
        let window_clone = window.clone();
        let evidence_data = evidence_by_id(report, evidence_id).cloned();
        evidence_button.connect_clicked(move |_| {
            if let Some(evidence) = &evidence_data {
                dialogs::show_evidence(&window_clone, evidence);
            }
        });
        footer.append(&evidence_button);
    }
    footer.append(&spacer());
    card.append(&footer);
    card
}

fn failure_panel(window: &gtk::ApplicationWindow, report: &InspectionReport) -> gtk::Box {
    let panel = vbox(8);
    panel.add_css_class("failure-card");
    panel.set_tooltip_text(Some(&format!(
        "{}\nraw_preview={}",
        report.extension.parser_error, report.extension.preview_hex
    )));
    let head = hbox(8);
    head.append(&label(&report.failure.title, "failure-title"));
    head.append(&pill(&report.failure.code, "pill-danger"));
    head.append(&spacer());
    head.append(&pill(
        if report.failure.recovered {
            "已自动降级"
        } else {
            "等待人工处置"
        },
        "pill-warn",
    ));
    panel.append(&head);
    panel.append(&wrapped(&report.failure.detail, "failure-detail"));
    panel.append(&mono(
        &format!(
            "{} @ 0x{:08X} · sha256:{}…",
            report.extension.fourcc,
            report.extension.offset,
            &report.extension.sha256[..12]
        ),
        "tiny muted",
    ));
    panel.append(&label(
        "以下操作可分别执行；原始扩展不会被重写。",
        "tiny muted",
    ));
    let actions = hbox(6);
    let open = button(
        &report
            .failure
            .actions
            .first()
            .cloned()
            .unwrap_or_else(|| "导出原始块".to_string()),
        "",
    );
    let window_clone = window.clone();
    let extension = evidence_by_id(report, "E-05").cloned();
    open.connect_clicked(move |_| {
        if let Some(evidence) = &extension {
            dialogs::show_evidence(&window_clone, evidence);
        }
    });
    actions.append(&open);
    let retry = button("仅重试扩展解析", "");
    let window_clone = window.clone();
    let detail = report.failure.detail.clone();
    retry.connect_clicked(move |_| {
        dialogs::show_text(
            &window_clone,
            "只读重试已完成，仍无法安全解析",
            "XMTA RETRY",
            "重试只读取已隔离字节，不重新解码来源、不修改输入。越界条件仍成立，因此继续保持隔离。",
            Some(&detail),
        );
    });
    actions.append(&retry);
    let keep = button("保持隔离继续监听", "primary");
    let window_clone = window.clone();
    keep.connect_clicked(move |_| {
        dialogs::show_text(
            &window_clone,
            "隔离策略已激活",
            "ROUTING CONTINUES",
            "8 声道 7.1 兼容路由继续；XMTA 不参与 Lb/Rb 映射。该动作仍需要人工复核 H-02。",
            Some("routing=7.1-compatible\nxmta=isolated\ninput=read-only"),
        );
    });
    actions.append(&keep);
    panel.append(&actions);
    panel
}

fn boundary_panel(window: &gtk::ApplicationWindow, report: &InspectionReport) -> gtk::Box {
    let panel = vbox(9);
    panel.add_css_class("boundary-panel");
    let head = hbox(8);
    head.append(&label("判定边界 · 无法唯一归因", "boundary-heading"));
    head.append(&spacer());
    head.append(&pill("待人工 H-03", "pill-warn"));
    panel.append(&head);
    panel.append(&wrapped(
        &format!(
            "样本 {} 附近的 {} 帧处于交叉淡化区。首个重复样本与 GStreamer 恢复 PTS 给出不同边界，现有输入不足以裁决。",
            report.recovery.boundary_sample, report.recovery.boundary_frames
        ),
        "hypothesis-detail",
    ));

    let hypotheses = hbox(8);
    hypotheses.set_homogeneous(true);
    let h1 = vbox(2);
    h1.add_css_class("hypothesis");
    h1.append(&label("H1 · 首个重复样本", "hypothesis-title"));
    h1.append(&label(
        &format!(
            "重放起点 = {}",
            report.recovery.boundary_sample + report.recovery.boundary_frames
        ),
        "hypothesis-detail mono",
    ));
    hypotheses.append(&h1);
    let h2 = vbox(2);
    h2.add_css_class("hypothesis");
    h2.append(&label("H2 · 恢复 buffer PTS", "hypothesis-title"));
    h2.append(&label(
        &format!("恢复时刻 = {:.3} s", report.recovery.recovery_s),
        "hypothesis-detail mono",
    ));
    hypotheses.append(&h2);
    panel.append(&hypotheses);

    if let Some(evidence) = evidence_by_id(report, "E-04") {
        let hex_line = evidence
            .payload
            .lines()
            .find(|line| line.starts_with("hex="))
            .unwrap_or(&evidence.payload);
        let hex = label(hex_line, "hex");
        hex.set_ellipsize(gtk::pango::EllipsizeMode::End);
        panel.append(&hex);
    }
    let actions = hbox(6);
    let open = button("打开 41 帧原始证据", "");
    let window_clone = window.clone();
    let boundary = evidence_by_id(report, "E-04").cloned();
    open.connect_clicked(move |_| {
        if let Some(evidence) = &boundary {
            dialogs::show_evidence(&window_clone, evidence);
        }
    });
    actions.append(&open);
    let decide = button("记录人工裁定", "primary");
    let window_clone = window.clone();
    let body = format!(
        "请在报告的人工确认区处理 H-03。当前自动结论保持“无法判定”，不会用重放检测结果替代边界裁定。\n\n样本: {}\n宽度: {} 帧\n原始 PCM24: {} B\n解码 F32LE: {} B",
        report.recovery.boundary_sample,
        report.recovery.boundary_frames,
        report.recovery.boundary_frames * 8 * 3,
        report.recovery.boundary_frames * 8 * 4
    );
    decide.connect_clicked(move |_| {
        dialogs::show_text(
            &window_clone,
            "待人工确认已保留",
            "BOUNDARY HELD",
            &body,
            Some("H-03 · pending\nrule=R-BOUNDARY-UNKNOWN"),
        );
    });
    actions.append(&decide);
    panel.append(&actions);
    panel
}

fn processing_chain(window: &gtk::ApplicationWindow, report: &InspectionReport) -> gtk::Box {
    let card = vbox(0);
    card.add_css_class("card");
    let head = hbox(9);
    head.add_css_class("card-pad");
    let titles = vbox(2);
    titles.set_hexpand(true);
    titles.append(&label("处理链与证据绑定", "card-title"));
    titles.append(&label(
        "从原始字节到自动结论的每一步都保留独立证据编号。",
        "small muted",
    ));
    head.append(&titles);
    head.append(&pill(&format!("{} 步", report.steps.len()), "pill-info"));
    head.append(&pill("GStreamer 1.28", "pill-ok"));
    card.append(&head);
    card.append(&hairline());

    for step in &report.steps {
        let row = hbox(9);
        row.add_css_class("report-row");
        let tone = match step.status {
            StepStatus::Ok => "dot-info",
            StepStatus::Warn => "dot-warn",
            StepStatus::Failed => "dot-danger",
        };
        row.append(&widgets_dot(tone));
        row.append(&mono(&step.id, "tiny muted"));
        let title = label(&step.title, "report-row-title");
        title.set_width_request(142);
        row.append(&title);
        let detail = label(&step.detail, "small muted");
        detail.set_hexpand(true);
        detail.set_ellipsize(gtk::pango::EllipsizeMode::End);
        row.append(&detail);
        row.append(&mono(&format!("{} ms", step.elapsed_ms), "tiny muted"));
        let evidence = button(&format!("{} · 查看", step.evidence_id), "link");
        let window_clone = window.clone();
        let evidence_data = evidence_by_id(report, &step.evidence_id).cloned();
        evidence.connect_clicked(move |_| {
            if let Some(evidence) = &evidence_data {
                dialogs::show_evidence(&window_clone, evidence);
            }
        });
        row.append(&evidence);
        card.append(&row);
    }
    card
}

fn widgets_dot(tone: &str) -> gtk::Box {
    let dot = gtk::Box::new(gtk::Orientation::Vertical, 0);
    dot.add_css_class("status-dot");
    dot.add_css_class(tone);
    dot
}
