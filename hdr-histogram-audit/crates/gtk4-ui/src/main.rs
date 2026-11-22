//! GTK4 entry point for the HDR luminance histogram source-check page.

mod state;
mod headless;
mod widgets;

use audit_core::model::{SampleRecord, Verdict};
use audit_core::replay::{capture_truncation, recover_and_replay};
use audit_core::{build_report, run, scenario};
use gstreamer as gst;
use gtk::glib;
use gtk::prelude::*;
use state::AppState;

fn main() -> glib::ExitCode {
    gst::init().expect("GStreamer initializes");
    let application = gtk::Application::new(
        Some("org.audit.hdr.histogram"),
        gtk::gio::ApplicationFlags::FLAGS_NONE,
    );
    application.connect_activate(build_ui);
    application.run()
}

fn build_ui(application: &gtk::Application) {
    let window = gtk::ApplicationWindow::new(application);
    window.set_title(Some("HDR 亮度直方图来源检查"));
    window.set_default_size(1180, 760);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    window.set_child(Some(&root));

    let header = gtk::HeaderBar::new();
    root.append(&header);

    let content = gtk::Paned::new(gtk::Orientation::Horizontal);
    content.set_vexpand(true);
    root.append(&content);

    let app_state = std::rc::Rc::new(std::cell::RefCell::new(AppState::default()));

    let left = build_left_pane(&app_state, &content);
    let right = build_right_pane(&app_state);
    content.set_start_child(Some(&left));
    content.set_end_child(Some(&right));

    load_styles(&window);
    window.present();
}

fn build_left_pane(
    app_state: &std::rc::Rc<std::cell::RefCell<AppState>>,
    content: &gtk::Paned,
) -> gtk::Widget {
    let container = gtk::Box::new(gtk::Orientation::Vertical, 10);
    container.set_width_request(420);
    container.set_margin_top(12);
    container.set_margin_bottom(12);
    container.set_margin_start(12);
    container.set_margin_end(12);

    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let run_button = gtk::Button::with_label("运行审计");
    let truncate_button = gtk::Button::with_label("记录截断");
    let replay_button = gtk::Button::with_label("恢复并重放");
    let gst_button = gtk::Button::with_label("从 GStreamer 采集");
    let export_button = gtk::Button::with_label("导出证据 JSON");
    controls.append(&run_button);
    controls.append(&truncate_button);
    controls.append(&replay_button);
    controls.append(&gst_button);
    controls.append(&export_button);
    container.append(&controls);

    let first_error = gtk::Label::new(Some("首个错误状态：尚未运行"));
    first_error.set_halign(gtk::Align::Start);
    first_error.set_wrap(true);
    first_error.add_css_class("first-error");
    container.append(&first_error);

    let score = gtk::Label::new(None);
    score.set_halign(gtk::Align::Start);
    score.set_wrap(true);
    container.append(&score);

    let record_list = gtk::ListBox::new();
    record_list.add_css_class("record-list");
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_vexpand(true);
    scroller.set_child(Some(&record_list));
    container.append(&scroller);

    wire_run_button(&run_button, app_state, &record_list, &first_error, &score, content);
    wire_truncate_button(&truncate_button, app_state);
    wire_replay_button(&replay_button, app_state, &first_error, &score);
    wire_gst_button(&gst_button, app_state, &record_list, &first_error, &score);
    wire_export_button(&export_button, &first_error);

    container.upcast()
}

fn build_right_pane(app_state: &std::rc::Rc<std::cell::RefCell<AppState>>) -> gtk::Widget {
    let container = gtk::Box::new(gtk::Orientation::Vertical, 10);
    container.set_margin_top(12);
    container.set_margin_bottom(12);
    container.set_margin_start(12);
    container.set_margin_end(12);

    let chart_title = gtk::Label::new(Some("HDR 亮度直方图（按 headroom 归一化分桶）"));
    chart_title.add_css_class("heading");
    container.append(&chart_title);

    let chart_holder = gtk::Box::new(gtk::Orientation::Vertical, 0);
    chart_holder.add_css_class("chart");
    chart_holder.set_vexpand(true);
    container.append(&chart_holder);

    let detail_title = gtk::Label::new(Some("处理步骤与关键证据"));
    detail_title.add_css_class("heading");
    container.append(&detail_title);

    let detail_holder = gtk::Box::new(gtk::Orientation::Vertical, 0);
    detail_holder.set_vexpand(true);
    let detail_scroll = gtk::ScrolledWindow::new();
    detail_scroll.set_child(Some(&detail_holder));
    detail_scroll.set_min_content_height(260);
    container.append(&detail_scroll);

    let mut state = app_state.borrow_mut();
    state.chart_holder = Some(chart_holder.downgrade());
    state.detail_holder = Some(detail_holder.downgrade());
    container.upcast()
}

fn wire_run_button(
    button: &gtk::Button,
    app_state: &std::rc::Rc<std::cell::RefCell<AppState>>,
    record_list: &gtk::ListBox,
    first_error: &gtk::Label,
    score: &gtk::Label,
    content: &gtk::Paned,
) {
    let record_list = record_list.clone();
    let first_error = first_error.clone();
    let score = score.clone();
    let app_state = app_state.clone();
    let content = content.clone();
    button.connect_clicked(move |_| {
        let config = scenario::default_config();
        let pipeline_run = run(&scenario::fixture_samples(), &config);
        let report = build_report(&pipeline_run);

        {
            let mut state = app_state.borrow_mut();
            state.run = Some(pipeline_run.clone());
            state.report = Some(report.clone());
            state.selected_record = 0;
            state.truncation = None;
            state.replay = None;
        }

        refresh_record_list(&app_state, &record_list);
        refresh_summary(&app_state, &first_error, &score);
        refresh_chart_and_detail(&app_state);
        content.set_position(440);
    });
}

fn wire_truncate_button(button: &gtk::Button, app_state: &std::rc::Rc<std::cell::RefCell<AppState>>) {
    let app_state = app_state.clone();
    button.connect_clicked(move |_| {
        let config = scenario::default_config();
        let samples = scenario::fixture_samples();
        let completed = app_state
            .borrow()
            .run
            .as_ref()
            .map(|run| run.records[..3].to_vec())
            .unwrap_or_else(|| run(&samples, &config).records[..3].to_vec());
        let record = capture_truncation("caps 变更触发 flush", &samples, completed, &config);
        app_state.borrow_mut().truncation = Some(record);
    });
}

fn wire_replay_button(
    button: &gtk::Button,
    app_state: &std::rc::Rc<std::cell::RefCell<AppState>>,
    first_error: &gtk::Label,
    score: &gtk::Label,
) {
    let app_state = app_state.clone();
    let first_error = first_error.clone();
    let score = score.clone();
    button.connect_clicked(move |_| {
        let truncation = match app_state.borrow().truncation.clone() {
            Some(record) => record,
            None => {
                first_error.set_text("请先记录一次截断，再执行恢复重放");
                return;
            }
        };
        let replay = recover_and_replay(&truncation);
        app_state.borrow_mut().replay = Some(replay);
        refresh_summary(&app_state, &first_error, &score);
    });
}

fn wire_gst_button(
    button: &gtk::Button,
    app_state: &std::rc::Rc<std::cell::RefCell<AppState>>,
    record_list: &gtk::ListBox,
    first_error: &gtk::Label,
    score: &gtk::Label,
) {
    let app_state = app_state.clone();
    let record_list = record_list.clone();
    let first_error = first_error.clone();
    let score = score.clone();
    button.connect_clicked(move |_| {
        let source_config = gst_source::SourceConfig::default();
        let samples = match gst_source::collect_samples(&source_config) {
            Ok(samples) => samples,
            Err(error) => {
                first_error.set_text(&format!("GStreamer 采集失败：{error}"));
                return;
            }
        };
        let config = audit_core::PipelineConfig {
            headroom_nits: source_config.mastering_peak_nits,
            ..scenario::default_config()
        };
        let pipeline_run = run(&samples, &config);
        let report = build_report(&pipeline_run);
        {
            let mut state = app_state.borrow_mut();
            state.run = Some(pipeline_run);
            state.report = Some(report);
            state.selected_record = 0;
            state.truncation = None;
            state.replay = None;
        }
        refresh_record_list(&app_state, &record_list);
        refresh_summary(&app_state, &first_error, &score);
        refresh_chart_and_detail(&app_state);
    });
}

fn wire_export_button(button: &gtk::Button, first_error: &gtk::Label) {
    let first_error = first_error.clone();
    button.connect_clicked(move |_| {
        let config = scenario::default_config();
        let artifacts = headless::assemble(scenario::fixture_samples(), config);
        let path = std::env::current_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("."))
            .join("hdr_audit_evidence.json");
        match serde_json::to_string_pretty(&artifacts) {
            Ok(json) => match std::fs::write(&path, json) {
                Ok(()) => first_error.set_text(&format!(
                    "证据已导出：{}（重放匹配={}）",
                    path.display(),
                    artifacts.replay.matches
                )),
                Err(error) => first_error.set_text(&format!("写入证据失败：{error}")),
            },
            Err(error) => first_error.set_text(&format!("序列化证据失败：{error}")),
        }
    });
}

fn refresh_record_list(
    app_state: &std::rc::Rc<std::cell::RefCell<AppState>>,
    record_list: &gtk::ListBox,
) {
    while let Some(child) = record_list.first_child() {
        record_list.remove(&child);
    }
    let records = app_state.borrow().run.clone().map(|run| run.records).unwrap_or_default();
    for (index, record) in records.iter().enumerate() {
        let row = gtk::ListBoxRow::new();
        let label = gtk::Label::new(Some(&record_row_label(index, record)));
        label.set_halign(gtk::Align::Start);
        label.set_margin_top(6);
        label.set_margin_bottom(6);
        row.set_child(Some(&label));
        if record.first_problem_step().is_some() {
            row.add_css_class("problem-row");
        }
        record_list.append(&row);
    }

    let state_for_selection = app_state.clone();
    record_list.connect_row_selected(move |_list, row| {
        if let Some(row) = row {
            state_for_selection.borrow_mut().selected_record = row.index() as usize;
            refresh_chart_and_detail(&state_for_selection);
        }
    });
}

fn record_row_label(index: usize, record: &SampleRecord) -> String {
    let verdict = match record.verdict {
        Verdict::Binned { bin } => format!("分桶 {bin}"),
        Verdict::Indeterminate => "无法判断的边界".to_string(),
        Verdict::Truncated => "已截断".to_string(),
        Verdict::Error => "错误".to_string(),
    };
    format!("#{index} {} — {verdict}", record.raw.sample_id)
}

fn refresh_summary(
    app_state: &std::rc::Rc<std::cell::RefCell<AppState>>,
    first_error: &gtk::Label,
    score: &gtk::Label,
) {
    let state = app_state.borrow();
    if let Some(report) = &state.report {
        match &report.first_error {
            Some(error) => first_error.set_text(&format!(
                "首个错误状态：记录 #{}（{}）步骤 #{} {} → {:?}",
                error.record_index,
                error.sample_id,
                error.step_index,
                error.step_name,
                error.state
            )),
            None => first_error.set_text("首个错误状态：无"),
        }
        score.set_text(&format!(
            "计分（仅作上下文）：总计 {}，分桶 {}，截断 {}，边界待定 {}，错误 {}",
            report.score.total,
            report.score.binned,
            report.score.truncated,
            report.score.indeterminate,
            report.score.errors
        ));
    }
    if let Some(replay) = &state.replay {
        let text = if replay.matches {
            format!("重放完成：{} 条待处理样本与参考运行完全一致", replay.replayed_sample_count)
        } else {
            format!("重放发现 {} 处不一致", replay.mismatches.len())
        };
        first_error.set_text(&text);
    }
}

fn refresh_chart_and_detail(app_state: &std::rc::Rc<std::cell::RefCell<AppState>>) {
    let state = app_state.borrow();
    if let (Some(run), Some(chart_weak), Some(detail_weak)) =
        (&state.run, &state.chart_holder, &state.detail_holder)
    {
        if let (Some(chart), Some(detail)) = (chart_weak.upgrade(), detail_weak.upgrade()) {
            while let Some(child) = chart.first_child() {
                chart.remove(&child);
            }
            chart.append(&widgets::histogram_drawing_area(run));

            while let Some(child) = detail.first_child() {
                detail.remove(&child);
            }
            if let Some(record) = state.selected() {
                detail.append(&widgets::evidence_panel(record));
            }
        }
    }
}

fn load_styles(window: &gtk::ApplicationWindow) {
    let css = r#"
    .heading { font-weight: 700; font-size: 15px; }
    .caption { color: #777; font-size: 12px; }
    .first-error { color: #ff6b6b; font-weight: 700; }
    .problem-row { background: #3a2326; }
    .badge-ok { color: #2ecc71; font-weight: 700; }
    .badge-indeterminate { color: #f5a623; font-weight: 700; }
    .badge-truncated { color: #ff6b6b; font-weight: 700; }
    .badge-error { color: #ff3b30; font-weight: 700; }
    .chart { background: #171a1e; border-radius: 8px; padding: 8px; }
    "#;
    let provider = gtk::CssProvider::new();
    provider.load_from_data(css);
    let display = gtk::prelude::WidgetExt::display(window);
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}
