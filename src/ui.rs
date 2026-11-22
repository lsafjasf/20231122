use std::path::PathBuf;

use gtk::prelude::*;
use gtk4 as gtk;

use crate::experiment::Experiment;
use crate::media::probe_media;
use crate::model::{ReplayRun, StepOutcome};

const CSS: &str = r#"
window { background: #f3f0e8; color: #1e2a32; }
.header-title { font-size: 18px; font-weight: 800; }
.header-sub { color: #66727b; font-size: 12px; }
.summary-strip { background: #fffdf8; border-bottom: 1px solid #d9d3c7; padding: 14px 18px; }
.summary-card { background: #ffffff; border: 1px solid #ddd6c8; border-radius: 10px; padding: 12px; }
.summary-card.error { background: #fff1ed; border-color: #c94c36; }
.eyebrow { color: #66727b; font-size: 11px; font-weight: 800; }
.mono { font-family: "JetBrains Mono", "DejaVu Sans Mono", monospace; font-size: 11px; }
.sidebar { background: #e9e4d9; border-right: 1px solid #d1cabd; padding: 14px; }
.sidebar-title { font-weight: 800; font-size: 13px; }
.scenario-row { padding: 10px 12px; border-radius: 8px; }
.scenario-row:selected { background: #1e2a32; color: white; }
.scenario-row.error:selected { background: #9f3324; color: white; }
.scenario-name { font-weight: 700; }
.scenario-note { font-size: 11px; color: #6e787f; }
.scenario-row:selected .scenario-note { color: #d7dee3; }
.content { padding: 18px; }
.page-title { font-size: 22px; font-weight: 800; }
.page-note { color: #66727b; }
.step-row { padding: 9px 10px; border-bottom: 1px solid #e7e1d5; }
.step-row.first-error { background: #ffe4dc; border-left: 4px solid #b83b28; }
.step-index { font-weight: 800; color: #52616b; }
.step-title { font-weight: 700; }
.step-meta { font-size: 11px; color: #68757d; }
.accepted { color: #216e4e; font-weight: 700; }
.rejected { color: #a32c1b; font-weight: 700; }
.detail-panel { background: #172026; color: #e8edf0; border-radius: 8px; padding: 12px; }
.detail-panel text { background: #172026; color: #e8edf0; }
.probe-panel { background: #fffdf8; border-top: 1px solid #d9d3c7; padding: 10px 18px; }
"#;

pub fn run(experiment: Experiment) -> gtk::glib::ExitCode {
    let app = gtk::Application::builder()
        .application_id("dev.media-meta.replay")
        .build();
    app.connect_activate(move |app| build_window(app, experiment.clone()));
    app.run()
}

fn build_window(app: &gtk::Application, experiment: Experiment) {
    install_css();
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("媒体元数据对照 · 可回放实验")
        .default_width(1280)
        .default_height(820)
        .build();

    let header = gtk::HeaderBar::new();
    let title_box = gtk::Box::new(gtk::Orientation::Vertical, 1);
    let title = gtk::Label::new(Some("媒体元数据对照"));
    title.add_css_class("header-title");
    let subtitle = gtk::Label::new(Some("截断记录 · 恢复重放 · 首错定位"));
    subtitle.add_css_class("header-sub");
    title_box.append(&title);
    title_box.append(&subtitle);
    header.set_title_widget(Some(&title_box));

    let probe_button = gtk::Button::with_label("探测示例媒体");
    probe_button.add_css_class("suggested-action");
    header.pack_end(&probe_button);
    window.set_titlebar(Some(&header));

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let probe_buffer = gtk::TextBuffer::new(None);
    probe_buffer
        .set_text("尚未运行 GStreamer 探针。\n点击右上角按钮探测 fixtures/media/take-001.wav。");
    let probe_label = gtk::Label::new(Some("GStreamer 探针未运行"));
    probe_label.set_xalign(0.0);
    probe_label.add_css_class("eyebrow");
    let probe_version = gtk::Label::new(Some(""));
    probe_version.set_xalign(0.0);
    probe_version.add_css_class("mono");

    root.append(&summary_strip(&experiment));

    let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
    let stack = content_stack(&experiment);
    paned.set_start_child(Some(&sidebar(&experiment, &stack)));
    paned.set_end_child(Some(&stack));
    paned.set_position(250);
    paned.set_vexpand(true);
    root.append(&paned);
    root.append(&probe_panel(&probe_label, &probe_version, &probe_buffer));
    window.set_child(Some(&root));

    {
        let window = window.clone();
        let probe_buffer = probe_buffer.clone();
        let probe_label = probe_label.clone();
        let probe_version = probe_version.clone();
        probe_button.connect_clicked(move |_| {
            choose_media(&window, &probe_buffer, &probe_label, &probe_version);
        });
    }

    window.present();
}

fn install_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_data(CSS);
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

fn summary_strip(experiment: &Experiment) -> gtk::Box {
    let strip = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    strip.add_css_class("summary-strip");
    strip.set_homogeneous(true);

    let control = card("恢复控制", "recovered-control");
    let control_text = gtk::Label::new(Some(&format!(
        "{} 步 · 最终 {:?} · 全部接受 {}",
        experiment.control.traces.len(),
        experiment.control.final_state.phase,
        if experiment.control.first_error.is_none() {
            "是"
        } else {
            "否"
        }
    )));
    control_text.set_xalign(0.0);
    control_text.set_wrap(true);
    control.append(&control_text);
    strip.append(&control);

    let truncation = card("截断证据", "tail cut");
    let truncation_text = gtk::Label::new(Some(&format!(
        "完整前缀 {} 行 / {} bytes\n缺失尾部 {} bytes",
        experiment.truncated.stats.complete_lines,
        experiment.truncated.stats.valid_prefix_bytes,
        experiment.truncated.stats.incomplete_tail_bytes
    )));
    truncation_text.set_xalign(0.0);
    truncation_text.add_css_class("mono");
    truncation.append(&truncation_text);
    strip.append(&truncation);

    let error_card = card("跨场景首个错误状态", "first failure");
    if let Some(error) = experiment.first_error_across_scenarios() {
        error_card.add_css_class("error");
        let text = gtk::Label::new(Some(&format!(
            "{} · 步骤 #{} · {}\n最后有效 {:?} c{} r{} · 首错 {:?} c{} r{}",
            error.scenario,
            error.step_index,
            error.rule,
            error.state_before.phase,
            error.state_before.cursor,
            error.state_before.revision,
            error.state_after.phase,
            error.state_after.cursor,
            error.state_after.revision
        )));
        text.set_xalign(0.0);
        text.set_wrap(true);
        error_card.append(&text);
    } else {
        let text = gtk::Label::new(Some("没有规则错误"));
        text.set_xalign(0.0);
        error_card.append(&text);
    }
    strip.append(&error_card);
    strip
}

fn card(eyebrow: &str, heading: &str) -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 4);
    card.add_css_class("summary-card");
    let eyebrow = gtk::Label::new(Some(eyebrow));
    eyebrow.set_xalign(0.0);
    eyebrow.add_css_class("eyebrow");
    let heading = gtk::Label::new(Some(heading));
    heading.set_xalign(0.0);
    heading.add_css_class("scenario-name");
    card.append(&eyebrow);
    card.append(&heading);
    card
}

fn sidebar(experiment: &Experiment, stack: &gtk::Stack) -> gtk::Box {
    let outer = gtk::Box::new(gtk::Orientation::Vertical, 8);
    outer.add_css_class("sidebar");
    outer.set_width_request(250);
    let title = gtk::Label::new(Some("回放场景"));
    title.set_xalign(0.0);
    title.add_css_class("sidebar-title");
    outer.append(&title);
    let note = gtk::Label::new(Some("选择场景查看逐步输入、状态与结果"));
    note.set_xalign(0.0);
    note.set_wrap(true);
    note.add_css_class("scenario-note");
    outer.append(&note);

    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    list.add_css_class("sidebar-list");
    append_scenario_row(
        &list,
        "recovered-control",
        "恢复后的重放",
        "对照组 · 全部步骤",
        false,
    );
    for scenario in &experiment.scenarios {
        let note = if let Some(error) = &scenario.run.first_error {
            format!("首错 #{} · {}", error.step_index, error.rule)
        } else {
            format!("{} 步 · 无规则错误", scenario.run.traces.len())
        };
        append_scenario_row(
            &list,
            &scenario.name,
            &scenario.name,
            &note,
            scenario.run.first_error.is_some(),
        );
    }
    outer.append(&list);
    let stack = stack.clone();
    list.connect_row_selected(move |_, row| {
        if let Some(row) = row {
            stack.set_visible_child_name(row.widget_name().as_str());
        }
    });
    if let Some(row) = list.row_at_index(0) {
        list.select_row(Some(&row));
    }
    outer
}

fn append_scenario_row(list: &gtk::ListBox, id: &str, name: &str, note: &str, error: bool) {
    let row = gtk::ListBoxRow::new();
    row.set_widget_name(id);
    row.add_css_class("scenario-row");
    if error {
        row.add_css_class("error");
    }
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let name_label = gtk::Label::new(Some(name));
    name_label.set_xalign(0.0);
    name_label.add_css_class("scenario-name");
    let note_label = gtk::Label::new(Some(note));
    note_label.set_xalign(0.0);
    note_label.set_wrap(true);
    note_label.add_css_class("scenario-note");
    text.append(&name_label);
    text.append(&note_label);
    row.set_child(Some(&text));
    list.append(&row);
}

fn content_stack(experiment: &Experiment) -> gtk::Stack {
    let stack = gtk::Stack::new();
    stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    stack.set_transition_duration(140);
    stack.set_hexpand(true);
    stack.add_named(
        &run_page(
            "恢复后的重放对照",
            "从恢复记录重新执行全部输入；每一步都保留输入、进入前状态、结果和证据哈希。",
            "对照组不使用截断尾部；最终状态应为 Verified。",
            &experiment.control,
        ),
        Some("recovered-control"),
    );
    for scenario in &experiment.scenarios {
        let comparison = if let Some(divergence) = &scenario.comparison.first_divergence {
            format!(
                "与恢复对照首次分歧 #{} · {}",
                divergence.index, divergence.reason
            )
        } else {
            "与恢复对照一致".to_owned()
        };
        stack.add_named(
            &run_page(
                &scenario.name,
                &scenario.description,
                &comparison,
                &scenario.run,
            ),
            Some(&scenario.name),
        );
    }
    stack.set_visible_child_name("recovered-control");
    stack
}

fn run_page(title: &str, note: &str, comparison: &str, run: &ReplayRun) -> gtk::Box {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 10);
    page.add_css_class("content");
    page.set_vexpand(true);
    let heading = gtk::Label::new(Some(title));
    heading.set_xalign(0.0);
    heading.add_css_class("page-title");
    let note = gtk::Label::new(Some(note));
    note.set_xalign(0.0);
    note.set_wrap(true);
    note.add_css_class("page-note");
    let comparison = gtk::Label::new(Some(comparison));
    comparison.set_xalign(0.0);
    comparison.add_css_class("mono");
    page.append(&heading);
    page.append(&note);
    page.append(&comparison);

    let first_error = run.first_error.as_ref().map(|error| error.step_index);
    let detail = gtk::TextView::builder()
        .editable(false)
        .monospace(true)
        .wrap_mode(gtk::WrapMode::WordChar)
        .build();
    detail.add_css_class("mono");
    let detail_buffer = detail.buffer();
    let detail_panel = gtk::Box::new(gtk::Orientation::Vertical, 0);
    detail_panel.add_css_class("detail-panel");
    let detail_title = gtk::Label::new(Some("步骤详情（输入 / 状态 / 结果 / 证据）"));
    detail_title.set_xalign(0.0);
    detail_title.add_css_class("eyebrow");
    let detail_scroll = gtk::ScrolledWindow::builder()
        .child(&detail)
        .height_request(260)
        .build();
    detail_panel.append(&detail_title);
    detail_panel.append(&detail_scroll);

    let rows = gtk::ListBox::new();
    rows.set_selection_mode(gtk::SelectionMode::Single);
    let mut details = Vec::new();
    for trace in &run.traces {
        let json = serde_json::to_string_pretty(trace).unwrap_or_else(|_| "{}".to_owned());
        details.push(json.clone());
        let row = gtk::ListBoxRow::new();
        row.set_widget_name(&trace.index.to_string());
        row.add_css_class("step-row");
        if Some(trace.index) == first_error {
            row.add_css_class("first-error");
        }
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let index = gtk::Label::new(Some(&format!("#{:02}", trace.index)));
        index.add_css_class("step-index");
        let body = gtk::Box::new(gtk::Orientation::Vertical, 2);
        body.set_hexpand(true);
        let action = gtk::Label::new(Some(&format!(
            "{}  ·  {}",
            trace.input.action.name(),
            trace.input.event_id
        )));
        action.set_xalign(0.0);
        action.add_css_class("step-title");
        let status = match &trace.outcome {
            StepOutcome::Accepted { effect } => {
                format!(
                    "accepted · {effect} · {:?} → {:?}",
                    trace.state_before.phase, trace.state_after.phase
                )
            }
            StepOutcome::Rejected { rule, expected, .. } => {
                format!(
                    "rejected · {rule} · expected {expected} · {:?} → rejected",
                    trace.state_before.phase
                )
            }
        };
        let meta = gtk::Label::new(Some(&status));
        meta.set_xalign(0.0);
        meta.set_wrap(true);
        meta.add_css_class("step-meta");
        if matches!(trace.outcome, StepOutcome::Rejected { .. }) {
            meta.add_css_class("rejected");
        } else {
            meta.add_css_class("accepted");
        }
        body.append(&action);
        body.append(&meta);
        line.append(&index);
        line.append(&body);
        row.set_child(Some(&line));
        rows.append(&row);
    }
    if let Some(first) = details.first() {
        detail_buffer.set_text(first);
    }
    let details = std::rc::Rc::new(details);
    let detail_buffer_for_rows = detail_buffer.clone();
    rows.connect_row_selected(move |_, row| {
        if let Some(row) = row
            && let Ok(index) = row.widget_name().parse::<usize>()
            && let Some(json) = details.get(index)
        {
            detail_buffer_for_rows.set_text(json);
        }
    });
    if let Some(row) = rows.row_at_index(0) {
        rows.select_row(Some(&row));
    }
    let list_scroll = gtk::ScrolledWindow::builder()
        .child(&rows)
        .height_request(250)
        .vexpand(true)
        .build();
    page.append(&list_scroll);
    page.append(&detail_panel);
    page
}

fn probe_panel(label: &gtk::Label, version: &gtk::Label, buffer: &gtk::TextBuffer) -> gtk::Box {
    let panel = gtk::Box::new(gtk::Orientation::Vertical, 5);
    panel.add_css_class("probe-panel");
    let heading = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    heading.append(label);
    heading.append(version);
    let text = gtk::TextView::with_buffer(buffer);
    text.set_editable(false);
    text.set_monospace(true);
    text.add_css_class("mono");
    let scroll = gtk::ScrolledWindow::builder()
        .child(&text)
        .height_request(120)
        .build();
    panel.append(&heading);
    panel.append(&scroll);
    panel
}

fn choose_media(
    window: &gtk::ApplicationWindow,
    buffer: &gtk::TextBuffer,
    label: &gtk::Label,
    version: &gtk::Label,
) {
    let dialog = gtk::FileDialog::builder()
        .title("选择媒体文件并运行 GStreamer 探针")
        .build();
    let fixture = PathBuf::from("fixtures/media/take-001.wav");
    if fixture.exists() {
        dialog.set_initial_file(Some(&gtk::gio::File::for_path(&fixture)));
    }
    let buffer = buffer.clone();
    let label = label.clone();
    let version = version.clone();
    dialog.open(
        Some(window),
        gtk::gio::Cancellable::NONE,
        move |result| match result {
            Ok(file) => {
                let Some(path) = file.path() else {
                    label.set_label("所选文件没有本地路径");
                    return;
                };
                match probe_media(&path) {
                    Ok(probe) => {
                        label.set_label(&format!(
                            "GStreamer 探针 · {} bytes · {}",
                            probe.facts.size_bytes, probe.facts.discoverer_result
                        ));
                        version.set_label(&format!(
                            "{} · sha256 {}…",
                            probe.gstreamer_version,
                            &probe.facts.sha256[..12]
                        ));
                        let json = serde_json::to_string_pretty(&probe)
                            .unwrap_or_else(|error| error.to_string());
                        buffer.set_text(&json);
                    }
                    Err(error) => {
                        label.set_label("GStreamer 探针失败");
                        version.set_label("");
                        buffer.set_text(&error.to_string());
                    }
                }
            }
            Err(error) => {
                label.set_label("未选择媒体");
                version.set_label("");
                buffer.set_text(&error.to_string());
            }
        },
    );
}
