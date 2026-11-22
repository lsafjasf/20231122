use std::rc::Rc;
use std::time::Duration;

use bwf_replay_lab::experiment::ExperimentOutput;
use bwf_replay_lab::model::{
    ExperimentReport, FirstError, RunReport, StepPhase, StepRecord, StepStatus,
};
use gtk::prelude::*;
use gtk::{
    Application, ApplicationWindow, Box as GtkBox, Button, CssProvider, Expander, HeaderBar, Label,
    ListBox, ListBoxRow, Orientation, Paned, ScrolledWindow, SelectionMode, Separator,
};

pub fn install_view(app: &Application, output: ExperimentOutput) {
    if let Some(previous) = app.active_window() {
        previous.close();
    }
    install_css();

    let window = ApplicationWindow::builder()
        .application(app)
        .title("BWF 时间标记校验")
        .default_width(1280)
        .default_height(860)
        .build();

    let header = build_header(app.clone());
    window.set_titlebar(Some(&header));
    let report = Rc::new(output.report.clone());
    let content = build_content(report, output);
    window.set_child(Some(&content));
    window.present();
}

pub fn schedule_screenshot(app: &Application, path: std::path::PathBuf) {
    let app = app.clone();
    gtk::glib::timeout_add_local_once(Duration::from_millis(1400), move || {
        let result = (|| -> anyhow::Result<()> {
            let window = app
                .active_window()
                .ok_or_else(|| anyhow::anyhow!("no active window for screenshot"))?;
            let width = window.width().max(1) as f64;
            let height = window.height().max(1) as f64;
            let paintable = gtk::WidgetPaintable::new(Some(&window));
            let snapshot = gtk::Snapshot::new();
            paintable.snapshot(&snapshot, width, height);
            let node = snapshot
                .to_node()
                .ok_or_else(|| anyhow::anyhow!("widget produced no render node"))?;
            let renderer = window
                .renderer()
                .ok_or_else(|| anyhow::anyhow!("window has no renderer"))?;
            let viewport = gtk::graphene::Rect::new(0.0, 0.0, width as f32, height as f32);
            let texture = renderer.render_texture(&node, Some(&viewport));
            texture.save_to_png(&path)?;
            Ok(())
        })();
        if let Err(error) = result {
            eprintln!("screenshot failed: {error:#}");
        }
        app.quit();
    });
}

fn build_header(app: Application) -> HeaderBar {
    let header = HeaderBar::new();
    header.add_css_class("lab-header");
    let titles = GtkBox::new(Orientation::Vertical, 2);
    let title = Label::builder()
        .label("BWF 时间标记校验")
        .xalign(0.0)
        .build();
    title.add_css_class("header-title");
    let subtitle = Label::builder()
        .label("可回放实验 · 截断记录 / 恢复重放 / 顺序变更 / 重复提交")
        .xalign(0.0)
        .build();
    subtitle.add_css_class("header-subtitle");
    titles.append(&title);
    titles.append(&subtitle);
    header.set_title_widget(Some(&titles));

    let rerun = Button::with_label("重新运行实验");
    rerun.add_css_class("suggested-action");
    rerun.add_css_class("rerun-button");
    rerun.connect_clicked(move |_| match bwf_replay_lab::run_experiment(None) {
        Ok(output) => install_view(&app, output),
        Err(error) => eprintln!("rerun failed: {error:#}"),
    });
    header.pack_end(&rerun);
    header
}

fn build_content(report: Rc<ExperimentReport>, output: ExperimentOutput) -> GtkBox {
    let root = GtkBox::new(Orientation::Vertical, 0);
    root.add_css_class("app-shell");
    root.append(&build_error_banner(&report));

    let paned = Paned::new(Orientation::Horizontal);
    paned.set_position(300);
    paned.set_hexpand(true);
    paned.set_vexpand(true);

    let detail = GtkBox::new(Orientation::Vertical, 14);
    detail.set_margin_top(18);
    detail.set_margin_bottom(18);
    detail.set_margin_start(20);
    detail.set_margin_end(20);
    detail.set_hexpand(true);
    detail.set_vexpand(true);

    let scenario_list = ListBox::new();
    scenario_list.set_selection_mode(SelectionMode::Single);
    scenario_list.add_css_class("scenario-list");
    for run in &report.runs {
        scenario_list.append(&scenario_row(run));
    }
    let report_for_select = Rc::clone(&report);
    let detail_for_select = detail.clone();
    scenario_list.connect_row_selected(move |_, row| {
        let Some(row) = row else {
            return;
        };
        let index = row.index();
        if index >= 0
            && let Some(run) = report_for_select.runs.get(index as usize)
        {
            show_run(run, &report_for_select, &detail_for_select);
        }
    });

    paned.set_start_child(Some(&build_sidebar(&report, output, &scenario_list)));
    paned.set_end_child(Some(&detail));
    root.append(&paned);

    let selected = std::env::var("BWF_SELECT_RUN")
        .ok()
        .and_then(|id| report.runs.iter().position(|run| run.id == id))
        .unwrap_or(0) as i32;
    if let Some(first) = scenario_list.row_at_index(selected) {
        scenario_list.select_row(Some(&first));
    }
    root
}

fn build_sidebar(
    report: &ExperimentReport,
    output: ExperimentOutput,
    scenario_list: &ListBox,
) -> GtkBox {
    let sidebar = GtkBox::new(Orientation::Vertical, 10);
    sidebar.set_size_request(292, -1);
    sidebar.add_css_class("sidebar");
    sidebar.set_margin_top(18);
    sidebar.set_margin_bottom(18);
    sidebar.set_margin_start(16);

    let heading = Label::builder().label("回放轨迹").xalign(0.0).build();
    heading.add_css_class("section-kicker");
    sidebar.append(&heading);
    let note = Label::builder()
        .label("每条轨迹独立定位首个错误；错误后的输入只观测、不改状态。")
        .xalign(0.0)
        .wrap(true)
        .build();
    note.add_css_class("muted");
    sidebar.append(&note);
    sidebar.append(scenario_list);

    let spacer = GtkBox::new(Orientation::Vertical, 0);
    spacer.set_vexpand(true);
    sidebar.append(&spacer);

    let evidence = GtkBox::new(Orientation::Vertical, 5);
    evidence.add_css_class("side-evidence");
    evidence.append(&small_label("证据落盘"));
    evidence.append(&path_label(&output.report_path));
    evidence.append(&small_label("逐步流水"));
    evidence.append(&path_label(&output.evidence_dir.join("steps.jsonl")));
    evidence.append(&small_label(&format!(
        "{} runs · {} steps",
        report.runs.len(),
        report.runs.iter().map(|run| run.steps.len()).sum::<usize>()
    )));
    sidebar.append(&evidence);
    sidebar
}

fn build_error_banner(report: &ExperimentReport) -> GtkBox {
    let banner = GtkBox::new(Orientation::Horizontal, 18);
    banner.set_margin_top(14);
    banner.set_margin_bottom(14);
    banner.set_margin_start(22);
    banner.set_margin_end(22);
    banner.add_css_class("status-banner");
    if report.first_error.is_some() {
        banner.add_css_class("status-error");
    } else {
        banner.add_css_class("status-ok");
    }

    let copy = GtkBox::new(Orientation::Vertical, 3);
    copy.set_hexpand(true);
    let kicker = Label::builder()
        .label(if report.first_error.is_some() {
            "FIRST ERROR LOCALIZED"
        } else {
            "ALL RUNS PASSED"
        })
        .xalign(0.0)
        .build();
    kicker.add_css_class("banner-kicker");
    copy.append(&kicker);

    if let Some(error) = &report.first_error {
        let run_id = report
            .runs
            .iter()
            .find(|run| {
                run.first_error
                    .as_ref()
                    .map(|item| item.step_index == error.step_index && item.code == error.code)
                    .unwrap_or(false)
            })
            .map(|run| run.id.as_str())
            .unwrap_or("unknown");
        let headline = Label::builder()
            .label(format!(
                "{} · 步骤 {:02} · {}",
                run_id, error.step_index, error.code
            ))
            .xalign(0.0)
            .build();
        headline.add_css_class("banner-headline");
        let detail = Label::builder()
            .label(format!(
                "状态 {} → rejected  ·  expected {}  ·  actual {}",
                error.state_before.name, error.expected, error.actual
            ))
            .xalign(0.0)
            .wrap(true)
            .build();
        detail.add_css_class("banner-detail");
        copy.append(&headline);
        copy.append(&detail);
    }
    banner.append(&copy);

    let principle = Label::builder()
        .label("状态机在首次拒绝处冻结\n不是总分，而是可定位的失败点")
        .justify(gtk::Justification::Right)
        .build();
    principle.add_css_class("banner-side");
    banner.append(&principle);
    banner
}

fn scenario_row(run: &RunReport) -> ListBoxRow {
    let row = ListBoxRow::new();
    row.add_css_class("scenario-row");
    let box_ = GtkBox::new(Orientation::Horizontal, 10);
    box_.set_margin_top(9);
    box_.set_margin_bottom(9);
    box_.set_margin_start(9);
    box_.set_margin_end(9);

    let status = Label::new(Some(if run.passed { "✓" } else { "!" }));
    status.add_css_class("scenario-status");
    status.add_css_class(if run.passed { "ok" } else { "error" });
    status.set_size_request(24, 24);
    box_.append(&status);

    let copy = GtkBox::new(Orientation::Vertical, 2);
    copy.set_hexpand(true);
    let title = Label::builder().label(&run.title).xalign(0.0).build();
    title.add_css_class("scenario-title");
    let meta = Label::builder()
        .label(if let Some(error) = &run.first_error {
            format!("step {:02} · {}", error.step_index, error.code)
        } else {
            format!("{} steps · PASS", run.steps.len())
        })
        .xalign(0.0)
        .build();
    meta.add_css_class("scenario-meta");
    copy.append(&title);
    copy.append(&meta);
    box_.append(&copy);
    row.set_child(Some(&box_));
    row
}

fn install_css() {
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };
    let provider = CssProvider::new();
    provider.load_from_string(CSS);
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

fn small_label(text: &str) -> Label {
    let label = Label::builder().label(text).xalign(0.0).build();
    label.add_css_class("small-caps");
    label
}

fn path_label(path: &std::path::Path) -> Label {
    let label = Label::builder()
        .label(path.display().to_string())
        .xalign(0.0)
        .wrap(true)
        .selectable(true)
        .build();
    label.add_css_class("path");
    label
}

fn mono_label(text: &str) -> Label {
    let label = Label::builder()
        .label(text)
        .xalign(0.0)
        .yalign(0.0)
        .wrap(true)
        .selectable(true)
        .build();
    label.add_css_class("mono");
    label
}

fn arrow() -> Label {
    let label = Label::new(Some("→"));
    label.add_css_class("arrow");
    label
}

fn clear(container: &GtkBox) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

fn compact_json(value: &serde_json::Value) -> String {
    let text = serde_json::to_string(value).unwrap_or_else(|_| "{}".to_owned());
    if text.chars().count() > 150 {
        let short: String = text.chars().take(147).collect();
        format!("{short}…")
    } else {
        text
    }
}

fn human_phase(phase: &StepPhase) -> &'static str {
    match phase {
        StepPhase::Ingest => "载入",
        StepPhase::Parse => "结构校验",
        StepPhase::Recover => "恢复",
        StepPhase::GstReplay => "GST 重放",
        StepPhase::SubmitMarker => "提交标记",
        StepPhase::Complete => "完成",
    }
}

fn status_text(status: &StepStatus) -> &'static str {
    match status {
        StepStatus::Accepted => "ACCEPTED",
        StepStatus::Warning => "WARNING",
        StepStatus::Rejected => "REJECTED",
        StepStatus::ObservedAfterError => "OBSERVED",
    }
}

fn status_class(status: &StepStatus) -> &'static str {
    match status {
        StepStatus::Accepted => "accepted",
        StepStatus::Warning => "warning",
        StepStatus::Rejected => "rejected",
        StepStatus::ObservedAfterError => "observed",
    }
}

const CSS: &str = r#"
window, .app-shell { background: #f3efe6; color: #20262d; }
.lab-header { background: #16252b; color: #f7f2e9; border: none; box-shadow: none; }
.lab-header windowcontrols button { color: #f7f2e9; }
.header-title { font-size: 21px; font-weight: 800; letter-spacing: 0.2px; }
.header-subtitle { font-size: 12px; color: #a9bcc0; }
.rerun-button { border-radius: 8px; padding: 8px 14px; font-weight: 700; }
.status-banner { border-radius: 14px; }
.status-error { background: #321f1b; color: #ffe7df; border: 1px solid #6e3c32; }
.status-ok { background: #17312b; color: #def7ed; border: 1px solid #2e6457; }
.banner-kicker { font-size: 10px; font-weight: 800; letter-spacing: 1.6px; color: #ff9f83; }
.status-ok .banner-kicker { color: #7ee0bd; }
.banner-headline { font-size: 19px; font-weight: 800; }
.banner-detail { font-size: 12px; color: #dfb9ae; }
.status-ok .banner-detail { color: #b5d6cc; }
.banner-side { font-size: 11px; color: #c99c90; }
.sidebar { border-right: 1px solid #d8d0c3; }
.section-kicker { font-size: 11px; font-weight: 800; letter-spacing: 1.4px; color: #6e6558; }
.muted { color: #776f63; font-size: 12px; }
.scenario-list { background: transparent; }
.scenario-row { background: transparent; border-radius: 10px; margin-top: 3px; margin-bottom: 3px; }
.scenario-row:hover { background: #ebe4d8; }
.scenario-row:selected { background: #dce9e4; }
.scenario-row:selected .scenario-title { color: #102d31; }
.scenario-row:selected .scenario-meta { color: #49645f; }
.scenario-status { border-radius: 12px; font-weight: 900; }
.scenario-status.ok { background: #c8eadc; color: #12634c; }
.scenario-status.error { background: #ffd9cc; color: #94351d; }
.scenario-title { font-size: 13px; font-weight: 700; }
.scenario-meta { font-size: 10px; color: #756b5e; }
.side-evidence { background: #e9e2d6; border-radius: 10px; padding: 10px; }
.small-caps { font-size: 9px; font-weight: 800; letter-spacing: 1.1px; color: #766d60; }
.path { font-size: 10px; color: #4d585c; }
.mono { font-family: "Noto Sans Mono CJK SC", monospace; font-size: 11px; }
.card { background: #fffdf8; border: 1px solid #d8d0c3; border-radius: 11px; padding: 12px; }
.card-kicker { font-size: 9px; font-weight: 800; letter-spacing: 1.2px; color: #81776a; }
.card-value { font-size: 20px; font-weight: 800; color: #1a3437; }
.card-foot { font-size: 10px; color: #776e62; }
.run-title { font-size: 21px; font-weight: 800; }
.run-subtitle { font-size: 12px; color: #756c60; }
.pill { border-radius: 99px; padding: 3px 9px; font-size: 9px; font-weight: 800; letter-spacing: 0.6px; }
.pill.pass { background: #c8eadc; color: #12634c; }
.pill.fail { background: #ffd9cc; color: #94351d; }
.first-error-card { background: #fff0ea; border: 1px solid #e3a18c; border-radius: 11px; padding: 12px; }
.first-error-title { color: #8d2f19; font-weight: 800; font-size: 13px; }
.first-error-detail { color: #743a2b; font-size: 11px; }
.table-head { padding: 5px 14px; border-bottom: 1px solid #d8d0c3; }
.table-head label { font-size: 9px; font-weight: 800; letter-spacing: 1px; color: #81776a; }
.step-list { background: transparent; }
.step-card { background: #fffdf8; border: 1px solid #d8d0c3; border-radius: 9px; padding: 2px 8px; margin-bottom: 6px; }
.step-card.error-step { border: 2px solid #b94729; background: #fff8f4; }
.step-card.observed-step { background: #eee9df; border-color: #d5cdc0; }
.step-index { font-family: "Noto Sans Mono CJK SC", monospace; font-size: 12px; font-weight: 800; color: #5d6e71; }
.phase-chip { background: #e5ecea; color: #294b4e; border-radius: 6px; padding: 1px 7px; font-size: 9px; font-weight: 800; }
.step-code { font-family: "Noto Sans Mono CJK SC", monospace; font-size: 11px; font-weight: 700; }
.step-detail { color: #746b5f; font-size: 10px; }
.state-name { font-family: "Noto Sans Mono CJK SC", monospace; font-size: 10px; color: #506164; }
.arrow { color: #9a8f7f; }
.badge { border-radius: 99px; padding: 1px 7px; font-size: 8px; font-weight: 900; letter-spacing: 0.5px; }
.badge.accepted { background: #c8eadc; color: #12634c; }
.badge.warning { background: #f6dfa6; color: #6e5513; }
.badge.rejected { background: #ffd3c4; color: #8d2f19; }
.badge.observed { background: #ddd8cd; color: #655e54; }
.detail-grid { padding: 8px 6px 10px 26px; }
.detail-label { font-size: 9px; font-weight: 800; letter-spacing: 1px; color: #81776a; }
.scrollbox { background: transparent; }
scrolledwindow { background: transparent; }
separator { background: #d8d0c3; }
"#;

fn show_run(run: &RunReport, report: &ExperimentReport, detail: &GtkBox) {
    clear(detail);
    detail.append(&build_summary_cards(report));
    detail.append(&build_run_heading(run));

    if let Some(error) = &run.first_error {
        detail.append(&build_first_error(error));
    } else {
        detail.append(&build_pass_note());
    }

    detail.append(&build_table_head());
    let steps_box = GtkBox::new(Orientation::Vertical, 0);
    steps_box.add_css_class("step-list");
    steps_box.set_margin_top(6);
    for step in &run.steps {
        steps_box.append(&build_step(step, run.first_error.as_ref()));
    }

    let scroll = ScrolledWindow::new();
    scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroll.set_vexpand(true);
    scroll.set_child(Some(&steps_box));
    scroll.add_css_class("scrollbox");
    detail.append(&scroll);
    detail.append(&build_evidence_footer(report));
}

fn build_summary_cards(report: &ExperimentReport) -> GtkBox {
    let row = GtkBox::new(Orientation::Horizontal, 10);
    row.set_homogeneous(true);

    let structure_code = report
        .first_error
        .as_ref()
        .map(|error| error.code.clone())
        .unwrap_or_else(|| "PASS".to_owned());
    let structure_step = report
        .first_error
        .as_ref()
        .map(|error| format!("first error @ step {:02}", error.step_index))
        .unwrap_or_else(|| "no structural error".to_owned());
    row.append(&summary_card("STRUCTURE", &structure_code, &structure_step));

    row.append(&summary_card(
        "RECOVERY",
        &format_bytes(report.evidence.recovery.appended_zero_bytes),
        "zero-padded PCM tail",
    ));

    let control = report.runs.iter().find(|run| run.id == "recovered_control");
    let (frames, duration) = control
        .and_then(|run| {
            run.steps
                .iter()
                .find(|step| step.phase == StepPhase::GstReplay)
        })
        .map(|step| {
            let frames = step.evidence["decoded_frames"].as_u64().unwrap_or_default();
            (
                format_number(frames),
                format!("{:.1} s · EOS", frames as f64 / 48_000.0),
            )
        })
        .unwrap_or_else(|| ("n/a".to_owned(), "replay unavailable".to_owned()));
    row.append(&summary_card("GStreamer CONTROL", &frames, &duration));

    let failed_runs = report
        .runs
        .iter()
        .filter(|run| run.first_error.is_some())
        .count();
    row.append(&summary_card(
        "STATE MACHINE",
        &format!("{failed_runs} 个首错"),
        "顺序 / 重复 / 截断",
    ));
    row
}

fn summary_card(kicker: &str, value: &str, foot: &str) -> GtkBox {
    let card = GtkBox::new(Orientation::Vertical, 3);
    card.add_css_class("card");
    card.set_hexpand(true);
    let kicker = Label::builder().label(kicker).xalign(0.0).build();
    kicker.add_css_class("card-kicker");
    let value = Label::builder()
        .label(value)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    value.add_css_class("card-value");
    let foot = Label::builder()
        .label(foot)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    foot.add_css_class("card-foot");
    card.append(&kicker);
    card.append(&value);
    card.append(&foot);
    card
}

fn build_run_heading(run: &RunReport) -> GtkBox {
    let row = GtkBox::new(Orientation::Horizontal, 12);
    let copy = GtkBox::new(Orientation::Vertical, 2);
    copy.set_hexpand(true);
    let title = Label::builder().label(&run.title).xalign(0.0).build();
    title.add_css_class("run-title");
    let subtitle = Label::builder()
        .label(format!(
            "{} · {} steps · {}",
            run.id,
            run.steps.len(),
            run.asset
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| run.asset.display().to_string())
        ))
        .xalign(0.0)
        .build();
    subtitle.add_css_class("run-subtitle");
    copy.append(&title);
    copy.append(&subtitle);
    row.append(&copy);

    let pill = Label::new(Some(if run.passed { "PASS" } else { "FAIL" }));
    pill.add_css_class("pill");
    pill.add_css_class(if run.passed { "pass" } else { "fail" });
    row.append(&pill);
    row
}

fn build_first_error(error: &FirstError) -> GtkBox {
    let card = GtkBox::new(Orientation::Vertical, 4);
    card.add_css_class("first-error-card");
    let title = Label::builder()
        .label(format!(
            "首个错误状态 · STEP {:02} · {}",
            error.step_index, error.code
        ))
        .xalign(0.0)
        .build();
    title.add_css_class("first-error-title");
    let state = Label::builder()
        .label(format!(
            "{} → {}    {}",
            error.state_before.name, error.error_state.name, error.message
        ))
        .xalign(0.0)
        .wrap(true)
        .build();
    state.add_css_class("first-error-detail");
    let values = Label::builder()
        .label(format!(
            "expected: {}    actual: {}",
            error.expected, error.actual
        ))
        .xalign(0.0)
        .wrap(true)
        .selectable(true)
        .build();
    values.add_css_class("mono");
    values.add_css_class("first-error-detail");
    card.append(&title);
    card.append(&state);
    card.append(&values);
    card
}

fn build_pass_note() -> GtkBox {
    let card = GtkBox::new(Orientation::Horizontal, 8);
    card.add_css_class("card");
    let check = Label::new(Some("✓"));
    check.add_css_class("scenario-status");
    check.add_css_class("ok");
    let text = Label::builder()
        .label("该轨迹没有错误状态；每一步状态迁移与结果均已保留")
        .xalign(0.0)
        .build();
    text.add_css_class("run-subtitle");
    card.append(&check);
    card.append(&text);
    card
}

fn build_table_head() -> GtkBox {
    let row = GtkBox::new(Orientation::Horizontal, 10);
    row.add_css_class("table-head");
    row.append(&fixed_label("#", 32));
    row.append(&fixed_label("阶段", 82));
    row.append(&fixed_label("结果", 76));
    row.append(&fixed_label("状态迁移 / 输入摘要", 0));
    row
}

fn fixed_label(text: &str, width: i32) -> Label {
    let label = Label::builder().label(text).xalign(0.0).build();
    if width > 0 {
        label.set_size_request(width, -1);
    } else {
        label.set_hexpand(true);
    }
    label
}

fn build_step(step: &StepRecord, first_error: Option<&FirstError>) -> Expander {
    let expander = Expander::new(None);
    expander.add_css_class("step-card");
    let is_first = first_error
        .map(|error| error.step_index == step.index)
        .unwrap_or(false);
    if is_first {
        expander.add_css_class("error-step");
    }
    if step.result.status == StepStatus::ObservedAfterError {
        expander.add_css_class("observed-step");
    }

    let label = GtkBox::new(Orientation::Vertical, 2);
    label.set_margin_top(5);
    label.set_margin_bottom(5);
    let line = GtkBox::new(Orientation::Horizontal, 9);
    let index = Label::new(Some(&format!("{:02}", step.index)));
    index.add_css_class("step-index");
    index.set_size_request(30, -1);
    line.append(&index);

    let phase = Label::new(Some(human_phase(&step.phase)));
    phase.add_css_class("phase-chip");
    phase.set_size_request(78, -1);
    line.append(&phase);

    let status = Label::new(Some(status_text(&step.result.status)));
    status.add_css_class("badge");
    status.add_css_class(status_class(&step.result.status));
    status.set_size_request(72, -1);
    line.append(&status);

    let code = Label::builder()
        .label(&step.result.code)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    code.add_css_class("step-code");
    code.set_size_request(215, -1);
    line.append(&code);

    let summary = Label::builder()
        .label(compact_json(&step.input))
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::Middle)
        .build();
    summary.add_css_class("step-detail");
    summary.set_hexpand(true);
    line.append(&summary);
    label.append(&line);

    let second = GtkBox::new(Orientation::Horizontal, 7);
    second.set_margin_start(39);
    let before = Label::new(Some(&step.state_before.name));
    before.add_css_class("state-name");
    let after = Label::new(Some(&step.state_after.name));
    after.add_css_class("state-name");
    let detail = Label::builder()
        .label(&step.result.detail)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    detail.add_css_class("step-detail");
    detail.set_hexpand(true);
    second.append(&before);
    second.append(&arrow());
    second.append(&after);
    second.append(&Separator::new(Orientation::Vertical));
    second.append(&detail);
    label.append(&second);
    expander.set_label_widget(Some(&label));
    expander.set_child(Some(&build_step_details(step)));
    expander.set_expanded(is_first);
    expander
}

fn build_step_details(step: &StepRecord) -> gtk::Grid {
    let grid = gtk::Grid::builder()
        .column_spacing(12)
        .row_spacing(7)
        .build();
    grid.add_css_class("detail-grid");

    let input = serde_json::to_string_pretty(&step.input).unwrap_or_else(|_| "{}".to_owned());
    let state = format!(
        "before: {}\nafter:  {}",
        serde_json::to_string_pretty(&step.state_before).unwrap_or_default(),
        serde_json::to_string_pretty(&step.state_after).unwrap_or_default()
    );
    let result = format!(
        "{} · {}\n{}",
        status_text(&step.result.status),
        step.result.code,
        step.result.detail
    );
    let evidence = serde_json::to_string_pretty(&step.evidence).unwrap_or_else(|_| "{}".to_owned());

    attach_detail(&grid, 0, "输入", &input);
    attach_detail(&grid, 1, "状态", &state);
    attach_detail(&grid, 2, "结果", &result);
    attach_detail(&grid, 3, "证据", &evidence);
    grid
}

fn attach_detail(grid: &gtk::Grid, row: i32, title: &str, body: &str) {
    let title = Label::builder()
        .label(title)
        .xalign(0.0)
        .yalign(0.0)
        .build();
    title.add_css_class("detail-label");
    let body = mono_label(body);
    body.set_hexpand(true);
    grid.attach(&title, 0, row, 1, 1);
    grid.attach(&body, 1, row, 1, 1);
}

fn build_evidence_footer(report: &ExperimentReport) -> GtkBox {
    let box_ = GtkBox::new(Orientation::Vertical, 2);
    box_.add_css_class("side-evidence");
    let row = GtkBox::new(Orientation::Horizontal, 8);
    let title = small_label("KEY EVIDENCE");
    title.set_size_request(96, -1);
    row.append(&title);
    let path = Label::builder()
        .label(report.evidence.report_path.display().to_string())
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::Middle)
        .selectable(true)
        .hexpand(true)
        .build();
    path.add_css_class("path");
    row.append(&path);
    box_.append(&row);
    let hashes = Label::builder()
        .label(format!(
            "truncated {}    recovered {}",
            &report.evidence.truncated.sha256[..20],
            &report.evidence.recovery.recovered_sha256[..20]
        ))
        .xalign(0.0)
        .selectable(true)
        .build();
    hashes.add_css_class("mono");
    box_.append(&hashes);
    box_
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

fn format_number(number: u64) -> String {
    let text = number.to_string();
    let mut out = String::new();
    for (index, ch) in text.chars().enumerate() {
        if index > 0 && (text.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}
