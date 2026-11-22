use crate::model::FrameState;
use crate::replay::{FirstError, ReplayStep};
use crate::review::ReviewSummary;
use anyhow::{Context, Result, bail};
use gtk::prelude::*;
use gtk4 as gtk;
use std::cell::RefCell;
use std::fs;
use std::path::Path;
use std::rc::Rc;

#[derive(Debug, Clone)]
struct UiScenario {
    name: String,
    steps: Vec<ReplayStep>,
    first_error: Option<FirstError>,
}

#[derive(Debug, Clone)]
struct UiPoint {
    time_ms: u64,
    luma: f64,
    state: FrameState,
}

struct UiData {
    summary: ReviewSummary,
    scenarios: Vec<UiScenario>,
    current: usize,
}

pub fn run(directory: &Path) -> Result<()> {
    let data = Rc::new(RefCell::new(load_experiment(directory)?));
    let directory = directory.to_path_buf();
    let app = gtk::Application::builder()
        .application_id("io.github.framelossreview.desktop")
        .build();
    app.connect_activate(move |app| {
        if let Err(error) = build_window(app, &directory, data.clone()) {
            eprintln!("GTK4 review window failed: {error:#}");
            app.quit();
        }
    });
    let _ = app.run_with_args::<&str>(&[]);
    Ok(())
}

fn load_experiment(directory: &Path) -> Result<UiData> {
    let summary_path = directory.join("summary.json");
    let summary: ReviewSummary = serde_json::from_slice(
        &fs::read(&summary_path).with_context(|| format!("read {}", summary_path.display()))?,
    )?;
    let mut scenarios = Vec::new();
    for scenario in &summary.scenarios {
        let trace = directory.join(format!("{}.trace.jsonl", scenario.name));
        scenarios.push(UiScenario {
            name: scenario.name.clone(),
            steps: read_trace(&trace)?,
            first_error: scenario.first_error.clone(),
        });
    }
    let recovered_trace = directory.join("recovered.trace.jsonl");
    if !scenarios.iter().any(|item| item.name == "recovered-replay") {
        scenarios.insert(
            0,
            UiScenario {
                name: "recovered-replay".to_string(),
                steps: read_trace(&recovered_trace)?,
                first_error: summary.recovered.first_error.clone(),
            },
        );
    }
    if scenarios.is_empty() {
        bail!("no replay traces found in {}", directory.display());
    }
    Ok(UiData {
        summary,
        scenarios,
        current: 0,
    })
}

fn read_trace(path: &Path) -> Result<Vec<ReplayStep>> {
    let text =
        fs::read_to_string(path).with_context(|| format!("read trace {}", path.display()))?;
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).context("parse trace step"))
        .collect()
}

fn build_window(app: &gtk::Application, directory: &Path, data: Rc<RefCell<UiData>>) -> Result<()> {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("视频帧丢失补偿复核")
        .default_width(1220)
        .default_height(820)
        .build();
    let header = gtk::HeaderBar::new();
    let title = gtk::Label::new(Some("Frame-loss compensation review"));
    header.set_title_widget(Some(&title));
    window.set_titlebar(Some(&header));

    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    root.append(&build_banner(&data.borrow().summary, directory));

    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    controls.set_margin_top(8);
    controls.set_margin_start(12);
    controls.set_margin_end(12);
    controls.append(&gtk::Label::new(Some("场景")));
    let names_owned: Vec<String> = data
        .borrow()
        .scenarios
        .iter()
        .map(|scenario| scenario.name.clone())
        .collect();
    let names: Vec<&str> = names_owned.iter().map(String::as_str).collect();
    let dropdown = gtk::DropDown::from_strings(&names);
    controls.append(&dropdown);
    root.append(&controls);

    let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
    paned.set_vexpand(true);
    paned.set_position(380);
    let step_list = gtk::ListBox::new();
    step_list.set_selection_mode(gtk::SelectionMode::Single);
    let step_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&step_list)
        .build();
    paned.set_start_child(Some(&step_scroll));

    let right = gtk::Box::new(gtk::Orientation::Vertical, 8);
    right.set_margin_top(8);
    right.set_margin_bottom(12);
    right.set_margin_start(12);
    right.set_margin_end(12);
    let first_error = gtk::Label::new(None);
    first_error.set_xalign(0.0);
    first_error.set_wrap(true);
    first_error.set_selectable(true);
    first_error.add_css_class("error");
    right.append(&first_error);
    let drawing = gtk::DrawingArea::new();
    drawing.set_content_height(270);
    drawing.set_hexpand(true);
    drawing.set_draw_func({
        let data = data.clone();
        move |_, context, width, height| {
            let data = data.borrow();
            let steps = &data.scenarios[data.current].steps;
            let points = points_from_steps(steps);
            draw_waveform(context, width as f64, height as f64, &points);
        }
    });
    right.append(&drawing);
    let detail = gtk::Label::new(None);
    detail.set_xalign(0.0);
    detail.set_yalign(0.0);
    detail.set_wrap(true);
    detail.set_selectable(true);
    detail.set_vexpand(true);
    let detail_scroll = gtk::ScrolledWindow::builder()
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .child(&detail)
        .build();
    right.append(&detail_scroll);
    paned.set_end_child(Some(&right));
    root.append(&paned);
    window.set_child(Some(&root));

    render_scenario(&data, 0, &step_list, &detail, &drawing, &first_error);
    dropdown.connect_selected_notify({
        let data = data.clone();
        let step_list = step_list.clone();
        let detail = detail.clone();
        let drawing = drawing.clone();
        let first_error = first_error.clone();
        move |dropdown| {
            render_scenario(
                &data,
                dropdown.selected() as usize,
                &step_list,
                &detail,
                &drawing,
                &first_error,
            );
        }
    });
    step_list.connect_row_activated({
        let data = data.clone();
        let detail = detail.clone();
        move |_, row| {
            let index = row.index().max(0) as usize;
            let data = data.borrow();
            if let Some(step) = data.scenarios[data.current].steps.get(index) {
                detail.set_text(&step_text(step));
            }
        }
    });
    window.present();
    Ok(())
}

fn build_banner(summary: &ReviewSummary, directory: &Path) -> gtk::Box {
    let banner = gtk::Box::new(gtk::Orientation::Vertical, 3);
    banner.set_margin_top(10);
    banner.set_margin_start(12);
    banner.set_margin_end(12);
    let title = gtk::Label::new(Some("逐帧证据链 · 不输出总分"));
    title.set_xalign(0.0);
    title.add_css_class("title-3");
    banner.append(&title);
    let text = format!(
        "Rust + GStreamer + GTK4 | has_total_score={} | 最早首错={} | evidence={}",
        summary.has_total_score,
        summary
            .first_error_across_scenarios
            .as_ref()
            .map_or("none".to_string(), |error| format!(
                "{} step {} {}",
                error.scenario, error.step_index, error.rule
            )),
        directory.display()
    );
    let subtitle = gtk::Label::new(Some(&text));
    subtitle.set_xalign(0.0);
    subtitle.set_selectable(true);
    banner.append(&subtitle);
    banner
}

fn render_scenario(
    data: &Rc<RefCell<UiData>>,
    index: usize,
    list: &gtk::ListBox,
    detail: &gtk::Label,
    drawing: &gtk::DrawingArea,
    first_error_label: &gtk::Label,
) {
    let mut data = data.borrow_mut();
    if index >= data.scenarios.len() {
        return;
    }
    data.current = index;
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
    let scenario = &data.scenarios[index];
    for step in &scenario.steps {
        let row = gtk::ListBoxRow::new();
        let label = gtk::Label::new(Some(&step_row_text(step)));
        label.set_xalign(0.0);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.set_margin_top(5);
        label.set_margin_bottom(5);
        label.set_margin_start(8);
        label.set_margin_end(8);
        row.set_child(Some(&label));
        if step.outcome.status == "rejected" {
            row.add_css_class("error");
        }
        list.append(&row);
    }
    let error_text = scenario.first_error.as_ref().map_or_else(
        || "首个错误：none。右侧列出每一步的进入前状态、结果、拒绝后状态与证据哈希。".to_string(),
        |error| {
            format!(
                "首个错误：step={} rule={}\n期望：{}\n实际：{}\n进入前：{}    拒绝后：{}\n证据：{}",
                error.step_index,
                error.rule,
                error.expected,
                error.observed,
                state_text(&error.state_before),
                state_text(&error.state_after),
                error.evidence_sha256
            )
        },
    );
    first_error_label.set_text(&error_text);
    if let Some(first) = scenario.steps.first() {
        detail.set_text(&step_text(first));
    } else {
        detail.set_text("该场景没有可显示的回放步骤。");
    }
    if let Some(row) = list.row_at_index(0) {
        list.select_row(Some(&row));
    }
    drawing.queue_draw();
}

fn step_row_text(step: &ReplayStep) -> String {
    let record = step.input.get("record_id").and_then(|value| value.as_str());
    let frame = step
        .input
        .get("frame")
        .and_then(|value| value.get("frame"))
        .and_then(|value| value.as_u64());
    let time = step
        .input
        .get("frame")
        .and_then(|value| value.get("time_ms"))
        .and_then(|value| value.as_u64());
    let tail = step
        .outcome
        .effect
        .clone()
        .or_else(|| {
            step.outcome
                .rule
                .as_ref()
                .map(|rule| format!("REJECT {rule}"))
        })
        .unwrap_or_else(|| step.outcome.status.clone());
    format!(
        "#{:02} {} {} {}",
        step.index,
        record.unwrap_or("(truncated)"),
        frame.map_or("-".to_string(), |frame| format!("frame={frame}")),
        time.map_or(tail.clone(), |time| format!("t={time}ms  {tail}"))
    )
}

fn step_text(step: &ReplayStep) -> String {
    format!(
        "步骤 {}\n输入：{}\n结果：{}\n期望：{}\n实际：{}\n\n进入前状态\n{}\n\n拒绝后状态\n{}\n\n证据\ninput={}\nstate_before={}\nstate_after={}\nresult={}\nprevious={}\nevidence={}",
        step.index,
        serde_json::to_string(&step.input).unwrap_or_default(),
        step.outcome.status,
        step.outcome.expected.as_deref().unwrap_or("none"),
        step.outcome
            .effect
            .as_deref()
            .or(step.outcome.observed.as_deref())
            .unwrap_or("none"),
        state_text(&step.state_before),
        state_text(&step.state_after),
        step.evidence.input_sha256,
        step.evidence.state_before_sha256,
        step.evidence.state_after_sha256,
        step.evidence.result_sha256,
        step.evidence.previous_evidence_sha256,
        step.evidence.evidence_sha256
    )
}

fn state_text(state: &crate::replay::ReplayState) -> String {
    format!(
        "phase={:?} cursor={} accepted={} last_record={} missing={} compensated={} params={} waveform={} first_error={}",
        state.phase,
        state.cursor,
        state.accepted,
        state.last_record_id.as_deref().unwrap_or("none"),
        state.missing_frames,
        state.compensated_frames,
        state.params_fingerprint.as_deref().unwrap_or("none"),
        &state.waveform_digest[..12],
        state
            .first_error
            .as_ref()
            .map_or("none", |error| error.rule.as_str())
    )
}

fn points_from_steps(steps: &[ReplayStep]) -> Vec<UiPoint> {
    steps
        .iter()
        .filter_map(|step| {
            let luma = step.outcome.output_luma?;
            let frame = step.input.get("frame")?;
            let time_ms = frame.get("time_ms")?.as_u64()?;
            let state = match frame.get("state").and_then(|value| value.as_str()) {
                Some("missing") => FrameState::Missing,
                _ => FrameState::Present,
            };
            Some(UiPoint {
                time_ms,
                luma,
                state,
            })
        })
        .collect()
}

fn draw_waveform(context: &gtk::cairo::Context, width: f64, height: f64, points: &[UiPoint]) {
    let left = 52.0;
    let right = width - 28.0;
    let top = 20.0;
    let bottom = height - 34.0;
    context.set_source_rgb(0.063, 0.094, 0.125);
    context.rectangle(0.0, 0.0, width, height);
    let _ = context.fill();
    context.set_source_rgb(0.706, 0.769, 0.824);
    context.set_font_size(12.0);
    context.move_to(left, 15.0);
    let _ = context.show_text("output luma waveform · cyan=present orange=compensated");
    for tick in 0..=4 {
        let y = bottom - (bottom - top) * (tick as f64 / 4.0);
        context.set_source_rgb(0.15, 0.216, 0.275);
        context.set_line_width(1.0);
        context.move_to(left, y);
        context.line_to(right, y);
        let _ = context.stroke();
        context.set_source_rgb(0.561, 0.643, 0.702);
        context.move_to(8.0, y + 4.0);
        let _ = context.show_text(&format!("{:.2}", tick as f64 / 4.0));
    }
    if points.is_empty() {
        context.set_source_rgb(0.561, 0.643, 0.702);
        context.move_to(left, top + 30.0);
        let _ = context.show_text("no accepted waveform samples");
        return;
    }
    let min_time = points.first().map_or(0, |point| point.time_ms);
    let max_time = points
        .iter()
        .map(|point| point.time_ms)
        .max()
        .unwrap_or(min_time + 1)
        .max(min_time + 1);
    let x = |time: u64| {
        left + (time.saturating_sub(min_time) as f64 / (max_time - min_time) as f64)
            * (right - left)
    };
    let y = |luma: f64| bottom - luma.clamp(0.0, 1.0) * (bottom - top);
    context.set_source_rgb(0.357, 0.753, 0.922);
    context.set_line_width(2.2);
    for (index, point) in points.iter().enumerate() {
        let px = x(point.time_ms);
        let py = y(point.luma);
        if index == 0 {
            context.move_to(px, py);
        } else {
            context.line_to(px, py);
        }
    }
    let _ = context.stroke();
    for point in points {
        let color = if point.state == FrameState::Missing {
            (0.957, 0.635, 0.380)
        } else {
            (0.341, 0.839, 0.553)
        };
        context.set_source_rgb(color.0, color.1, color.2);
        context.arc(
            x(point.time_ms),
            y(point.luma),
            3.5,
            0.0,
            std::f64::consts::TAU,
        );
        let _ = context.fill();
    }
    context.set_source_rgb(0.561, 0.643, 0.702);
    context.set_font_size(10.0);
    context.move_to(left, height - 12.0);
    let _ = context.show_text(&format!("t={}ms", min_time));
    context.move_to(right - 58.0, height - 12.0);
    let _ = context.show_text(&format!("t={}ms", max_time));
}
