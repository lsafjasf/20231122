use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk4 as gtk;
use serde_json::{Value, json};

use bframe_addressing_lab::ExperimentRun;
use bframe_addressing_lab::evidence::run_builtin;
use bframe_addressing_lab::journal::JournalEntry;
use bframe_addressing_lab::model::{FrameKind, StateView};

fn main() -> gtk::glib::ExitCode {
    let application = gtk::Application::builder()
        .application_id("io.github.bframe.addressing.lab")
        .build();
    application.connect_activate(build_ui);
    application.run()
}

struct UiModel {
    run: ExperimentRun,
    scenario_index: usize,
    step_index: usize,
    playing: bool,
}

impl UiModel {
    fn scenario(&self) -> &bframe_addressing_lab::ScenarioReport {
        &self.run.bundle.scenarios[self.scenario_index]
    }

    fn entry(&self) -> Option<&JournalEntry> {
        self.scenario().entries.get(self.step_index)
    }

    fn advance(&mut self) -> bool {
        let last = self.scenario().entries.len().saturating_sub(1);
        if self.step_index < last {
            self.step_index += 1;
            true
        } else {
            self.playing = false;
            false
        }
    }
}

fn build_ui(application: &gtk::Application) {
    install_css();
    let run = match run_builtin() {
        Ok(run) => run,
        Err(error) => {
            let window = gtk::ApplicationWindow::builder()
                .application(application)
                .title("B-frame addressing lab - error")
                .default_width(760)
                .default_height(260)
                .build();
            let label = gtk::Label::new(Some(&format!(
                "Failed to build replayable experiment:\n{error:#}"
            )));
            label.set_wrap(true);
            label.set_margin_top(24);
            label.set_margin_bottom(24);
            label.set_margin_start(24);
            label.set_margin_end(24);
            window.set_child(Some(&label));
            window.present();
            return;
        }
    };

    let model = Rc::new(RefCell::new(UiModel {
        run,
        scenario_index: 0,
        step_index: 0,
        playing: false,
    }));

    let window = gtk::ApplicationWindow::builder()
        .application(application)
        .title("B-frame addressing visualization")
        .default_width(1320)
        .default_height(900)
        .build();
    window.add_css_class("main-window");

    let header = gtk::HeaderBar::new();
    let heading = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let title = gtk::Label::new(Some("B 帧寻址可视化"));
    title.add_css_class("app-title");
    let subtitle = gtk::Label::new(Some("Rust · GStreamer · GTK4 replay lab"));
    subtitle.add_css_class("app-subtitle");
    heading.append(&title);
    heading.append(&subtitle);
    header.set_title_widget(Some(&heading));

    let scenario_combo = gtk::ComboBoxText::new();
    for scenario in &model.borrow().run.bundle.scenarios {
        scenario_combo.append(
            Some(&scenario.id),
            &format!("{}  ·  {}", scenario.version, scenario.title),
        );
    }
    scenario_combo.set_active(Some(0));
    scenario_combo.set_tooltip_text(Some("选择回放场景"));
    header.pack_start(&scenario_combo);

    let gst_label = gtk::Label::new(Some(&gstreamer_badge(&model.borrow().run)));
    gst_label.add_css_class("pill");
    gst_label.set_tooltip_text(Some("GStreamer PTS/DTS 往返探测结果"));
    header.pack_end(&gst_label);
    window.set_titlebar(Some(&header));

    let root = gtk::Box::new(gtk::Orientation::Vertical, 12);
    root.set_margin_top(14);
    root.set_margin_bottom(14);
    root.set_margin_start(16);
    root.set_margin_end(16);

    let metrics = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let accepted_metric = metric("accepted", "0", &metrics);
    let duplicate_metric = metric("duplicate", "0", &metrics);
    let conflict_metric = metric("conflict", "0", &metrics);
    let invalid_metric = metric("truncated", "0", &metrics);
    let replay_metric = metric("replay", "—", &metrics);
    let cleanup_metric = metric("temp cleanup", "—", &metrics);
    root.append(&metrics);

    let timeline = gtk::DrawingArea::new();
    timeline.set_content_height(250);
    timeline.set_hexpand(true);
    timeline.set_draw_func({
        let model = Rc::clone(&model);
        move |_, cr, width, height| draw_timeline(cr, width, height, &model.borrow())
    });
    timeline.add_css_class("timeline");
    root.append(&timeline);

    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let reset_button = gtk::Button::with_label("Reset");
    let previous_button = gtk::Button::with_label("‹ 上一步");
    let next_button = gtk::Button::with_label("下一步 ›");
    let play_button = gtk::Button::with_label("▶ 自动回放");
    let step_label = gtk::Label::new(Some("1 / 1"));
    step_label.add_css_class("step-label");
    step_label.set_hexpand(true);
    step_label.set_halign(gtk::Align::End);
    controls.append(&reset_button);
    controls.append(&previous_button);
    controls.append(&next_button);
    controls.append(&play_button);
    controls.append(&step_label);
    root.append(&controls);

    let (input_panel, input_view) = detail_panel("Input", "每步原始输入");
    let (state_panel, state_view) = detail_panel("State", "提交前 / 提交后");
    let (result_panel, result_view) = detail_panel("Result", "结构化结果");
    let details = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    details.set_homogeneous(true);
    details.append(&input_panel);
    details.append(&state_panel);
    details.append(&result_panel);
    let details_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&details)
        .build();
    root.append(&details_scroll);

    let digest_label = gtk::Label::new(None);
    digest_label.add_css_class("digest");
    digest_label.set_selectable(true);
    digest_label.set_xalign(0.0);
    root.append(&digest_label);
    window.set_child(Some(&root));

    let refresh = make_refresh(
        Rc::clone(&model),
        &timeline,
        &input_view,
        &state_view,
        &result_view,
        &step_label,
        &digest_label,
        &accepted_metric,
        &duplicate_metric,
        &conflict_metric,
        &invalid_metric,
        &replay_metric,
        &cleanup_metric,
    );

    connect_controls(
        Rc::clone(&model),
        Rc::clone(&refresh),
        &scenario_combo,
        &reset_button,
        &previous_button,
        &next_button,
        &play_button,
    );
    start_timer(Rc::clone(&model), Rc::clone(&refresh), &play_button);
    refresh();
    window.present();
}

fn metric(name: &str, value: &str, parent: &gtk::Box) -> gtk::Label {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 1);
    card.add_css_class("metric");
    let name_label = gtk::Label::new(Some(name));
    name_label.add_css_class("metric-name");
    name_label.set_xalign(0.0);
    let value_label = gtk::Label::new(Some(value));
    value_label.add_css_class("metric-value");
    value_label.set_xalign(0.0);
    card.append(&name_label);
    card.append(&value_label);
    parent.append(&card);
    value_label
}

fn detail_panel(title: &str, subtitle: &str) -> (gtk::Box, gtk::TextView) {
    let panel = gtk::Box::new(gtk::Orientation::Vertical, 4);
    panel.add_css_class("detail-panel");
    panel.set_hexpand(true);
    let heading = gtk::Label::new(Some(title));
    heading.add_css_class("detail-title");
    heading.set_xalign(0.0);
    let caption = gtk::Label::new(Some(subtitle));
    caption.add_css_class("detail-subtitle");
    caption.set_xalign(0.0);
    let view = gtk::TextView::new();
    view.set_editable(false);
    view.set_cursor_visible(false);
    view.set_monospace(true);
    view.set_wrap_mode(gtk::WrapMode::Char);
    view.set_top_margin(8);
    view.set_bottom_margin(8);
    view.set_left_margin(8);
    view.set_right_margin(8);
    let scroll = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&view)
        .build();
    scroll.add_css_class("code-scroll");
    panel.append(&heading);
    panel.append(&caption);
    panel.append(&scroll);
    (panel, view)
}

#[allow(clippy::too_many_arguments)]
fn make_refresh(
    model: Rc<RefCell<UiModel>>,
    timeline: &gtk::DrawingArea,
    input_view: &gtk::TextView,
    state_view: &gtk::TextView,
    result_view: &gtk::TextView,
    step_label: &gtk::Label,
    digest_label: &gtk::Label,
    accepted_metric: &gtk::Label,
    duplicate_metric: &gtk::Label,
    conflict_metric: &gtk::Label,
    invalid_metric: &gtk::Label,
    replay_metric: &gtk::Label,
    cleanup_metric: &gtk::Label,
) -> Rc<dyn Fn()> {
    let timeline = timeline.clone();
    let input_view = input_view.clone();
    let state_view = state_view.clone();
    let result_view = result_view.clone();
    let step_label = step_label.clone();
    let digest_label = digest_label.clone();
    let accepted_metric = accepted_metric.clone();
    let duplicate_metric = duplicate_metric.clone();
    let conflict_metric = conflict_metric.clone();
    let invalid_metric = invalid_metric.clone();
    let replay_metric = replay_metric.clone();
    let cleanup_metric = cleanup_metric.clone();

    Rc::new(move || {
        let snapshot = {
            let model = model.borrow();
            let scenario = model.scenario();
            let entry = model.entry().cloned();
            (
                scenario.summary.clone(),
                scenario.replay.matched,
                scenario.replay.checked_entries,
                model.step_index,
                scenario.entries.len(),
                model.run.bundle.cleanup.clone(),
                entry,
            )
        };
        let (summary, replay_match, replayed, step, total, cleanup, entry) = snapshot;
        accepted_metric.set_text(&summary.accepted.to_string());
        duplicate_metric.set_text(&summary.duplicates.to_string());
        conflict_metric.set_text(&summary.conflicts.to_string());
        invalid_metric.set_text(&summary.invalid.to_string());
        let replay_text = if replay_match {
            format!("{replayed} ✓")
        } else {
            "mismatch".to_string()
        };
        replay_metric.set_text(&replay_text);
        cleanup_metric.set_text(if cleanup.temporary_directory_removed {
            "removed ✓"
        } else {
            "residual"
        });

        if let Some(entry) = entry {
            step_label.set_text(&format!(
                "step {} / {}  ·  {}",
                step + 1,
                total,
                entry.label
            ));
            set_pretty(&input_view, &json_value(&entry.input));
            set_pretty(
                &state_view,
                &json!({
                    "before": state_json(&entry.state_before),
                    "after": state_json(&entry.state_after),
                }),
            );
            set_pretty(&result_view, &json_value(&entry.result));
            digest_label.set_text(&format!(
                "input {}   before {}   after {}",
                short_digest(&entry.input_digest),
                short_digest(&entry.state_before_digest),
                short_digest(&entry.state_after_digest),
            ));
        } else {
            step_label.set_text("no journal entry");
            set_pretty(&input_view, &Value::Null);
            set_pretty(&state_view, &Value::Null);
            set_pretty(&result_view, &Value::Null);
            digest_label.set_text("");
        }
        timeline.queue_draw();
    })
}

fn connect_controls(
    model: Rc<RefCell<UiModel>>,
    refresh: Rc<dyn Fn()>,
    combo: &gtk::ComboBoxText,
    reset: &gtk::Button,
    previous: &gtk::Button,
    next: &gtk::Button,
    play: &gtk::Button,
) {
    {
        let model = Rc::clone(&model);
        let refresh = Rc::clone(&refresh);
        combo.connect_changed(move |combo| {
            let Some(active_id) = combo.active_id() else {
                return;
            };
            let mut model = model.borrow_mut();
            if let Some(index) = model
                .run
                .bundle
                .scenarios
                .iter()
                .position(|scenario| scenario.id == active_id.as_str())
            {
                model.scenario_index = index;
                model.step_index = 0;
                model.playing = false;
                drop(model);
                refresh();
            }
        });
    }
    {
        let model = Rc::clone(&model);
        let refresh = Rc::clone(&refresh);
        reset.connect_clicked(move |_| {
            let mut model = model.borrow_mut();
            model.step_index = 0;
            model.playing = false;
            drop(model);
            refresh();
        });
    }
    {
        let model = Rc::clone(&model);
        let refresh = Rc::clone(&refresh);
        previous.connect_clicked(move |_| {
            let mut model = model.borrow_mut();
            model.step_index = model.step_index.saturating_sub(1);
            model.playing = false;
            drop(model);
            refresh();
        });
    }
    {
        let model = Rc::clone(&model);
        let refresh = Rc::clone(&refresh);
        next.connect_clicked(move |_| {
            let mut model = model.borrow_mut();
            model.advance();
            model.playing = false;
            drop(model);
            refresh();
        });
    }
    {
        let model = Rc::clone(&model);
        let refresh = Rc::clone(&refresh);
        let play = play.clone();
        let play_handle = play.clone();
        play.connect_clicked(move |_| {
            let mut model = model.borrow_mut();
            if !model.playing && model.step_index + 1 >= model.scenario().entries.len() {
                model.step_index = 0;
            }
            model.playing = !model.playing;
            play_handle.set_label(if model.playing {
                "❚❚ 暂停"
            } else {
                "▶ 自动回放"
            });
            drop(model);
            refresh();
        });
    }
}

fn start_timer(model: Rc<RefCell<UiModel>>, refresh: Rc<dyn Fn()>, play: &gtk::Button) {
    let play = play.clone();
    gtk::glib::timeout_add_local(Duration::from_millis(700), move || {
        let should_refresh = {
            let mut model = model.borrow_mut();
            if !model.playing {
                false
            } else {
                model.advance();
                if !model.playing {
                    play.set_label("▶ 自动回放");
                }
                true
            }
        };
        if should_refresh {
            refresh();
        }
        gtk::glib::ControlFlow::Continue
    });
}

fn gstreamer_badge(run: &ExperimentRun) -> String {
    let Some(report) = &run.bundle.gstreamer else {
        return "GStreamer unavailable".to_string();
    };
    let version = report["version"].as_str().unwrap_or("?");
    let observed = report["observed_frames"].as_u64().unwrap_or(0);
    let expected = report["expected_frames"].as_u64().unwrap_or(0);
    let preserved = report["timestamps_preserved"].as_bool().unwrap_or(false);
    format!(
        "GStreamer {version} · {observed}/{expected} {}",
        if preserved {
            "PTS/DTS ✓"
        } else {
            "core only"
        }
    )
}

fn state_json(state: &StateView) -> Value {
    let entries: Vec<Value> = state
        .entries
        .values()
        .map(|entry| {
            json!({
                "object_id": entry.object_id,
                "address": entry.address,
                "kind": entry.frame_kind.as_str(),
                "pts_ms": entry.pts_ns / 1_000_000,
                "dts_ms": entry.dts_ns / 1_000_000,
                "submission_index": entry.submission_index,
            })
        })
        .collect();
    json!({
        "version": state.version,
        "policy": serde_json::to_value(state.policy).unwrap_or(Value::Null),
        "submission_seq": state.submission_seq,
        "accepted": state.accepted,
        "duplicates": state.duplicates,
        "conflicts": state.conflicts,
        "invalid": state.invalid,
        "entries": entries,
    })
}

fn json_value<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

fn set_pretty(view: &gtk::TextView, value: &Value) {
    let text = serde_json::to_string_pretty(value).unwrap_or_else(|_| "null".to_string());
    view.buffer().set_text(&text);
}

fn short_digest(digest: &str) -> &str {
    digest.get(..12).unwrap_or(digest)
}

fn draw_timeline(cr: &gtk::cairo::Context, width: i32, height: i32, model: &UiModel) {
    let width = width as f64;
    let height = height as f64;
    cr.set_source_rgb(1.0, 0.997, 0.98);
    cr.paint().ok();

    cr.set_source_rgb(0.13, 0.15, 0.18);
    cr.select_font_face(
        "Sans",
        gtk::cairo::FontSlant::Normal,
        gtk::cairo::FontWeight::Bold,
    );
    cr.set_font_size(14.0);
    cr.move_to(20.0, 24.0);
    let _ = cr.show_text("Decode order (DTS lanes) vs presentation order (PTS axis)");
    cr.set_source_rgb(0.42, 0.44, 0.46);
    cr.select_font_face(
        "Sans",
        gtk::cairo::FontSlant::Normal,
        gtk::cairo::FontWeight::Normal,
    );
    cr.set_font_size(10.5);
    cr.move_to(20.0, 40.0);
    let _ =
        cr.show_text("Node label: frame kind, object id, resolved address. B frames are orange.");

    let Some(entry) = model.entry() else {
        return;
    };
    let state = &entry.state_after;
    if state.entries.is_empty() {
        cr.set_source_rgb(0.48, 0.48, 0.45);
        cr.set_font_size(13.0);
        cr.move_to(20.0, 80.0);
        let _ = cr.show_text("Snapshot loaded. Advance to submit the first record.");
        return;
    }

    let left = 92.0;
    let right = width - 24.0;
    let top = 64.0;
    let bottom = height - 35.0;
    let plot_width = (right - left).max(1.0);
    let plot_height = (bottom - top).max(1.0);
    let max_pts = state
        .entries
        .values()
        .map(|entry| entry.pts_ns)
        .max()
        .unwrap_or(1)
        .max(1);

    let mut ranked: Vec<_> = state.entries.values().collect();
    ranked.sort_by_key(|entry| (entry.dts_ns, entry.object_id.clone()));
    let row_count = ranked.len().max(1);
    let row_height = (plot_height / row_count as f64).clamp(18.0, 31.0);
    let mut positions = std::collections::BTreeMap::new();
    for (rank, entry) in ranked.iter().enumerate() {
        let x = left + entry.pts_ns as f64 / max_pts as f64 * plot_width;
        let y = top + (rank as f64 + 0.5) * row_height;
        positions.insert(entry.object_id.as_str(), (x, y));
    }

    let seconds = max_pts.div_ceil(1_000_000_000).max(1);
    for second in 0..=seconds {
        let x = left + second as f64 / seconds as f64 * plot_width;
        cr.set_source_rgba(0.55, 0.57, 0.58, 0.18);
        cr.set_line_width(1.0);
        cr.move_to(x, top);
        cr.line_to(x, bottom);
        cr.stroke().ok();
        cr.set_source_rgb(0.45, 0.47, 0.49);
        cr.set_font_size(9.0);
        cr.move_to(x - 5.0, bottom + 16.0);
        let _ = cr.show_text(&format!("{second}s"));
    }

    cr.set_source_rgba(0.20, 0.22, 0.24, 0.35);
    cr.set_line_width(1.0);
    cr.move_to(left, bottom);
    cr.line_to(right, bottom);
    cr.stroke().ok();

    for entry in state.entries.values() {
        let Some((to_x, to_y)) = positions.get(entry.object_id.as_str()) else {
            continue;
        };
        for reference in &entry.references {
            let Some((from_x, from_y)) = positions.get(reference.as_str()) else {
                continue;
            };
            cr.set_source_rgba(0.32, 0.45, 0.58, 0.22);
            cr.set_line_width(1.2);
            cr.move_to(*from_x, *from_y);
            cr.line_to(*to_x, *to_y);
            cr.stroke().ok();
        }
    }

    let focus = focused_object(entry);
    for entry in state.entries.values() {
        let Some((x, y)) = positions.get(entry.object_id.as_str()) else {
            continue;
        };
        let color = match entry.frame_kind {
            FrameKind::I => (0.18, 0.54, 0.34),
            FrameKind::P => (0.17, 0.41, 0.70),
            FrameKind::B => (0.91, 0.48, 0.11),
        };
        let node_width = 166.0;
        let node_height = 18.0;
        let x = (*x - node_width / 2.0).clamp(left, right - node_width);
        let y = *y - node_height / 2.0;
        cr.set_source_rgb(color.0, color.1, color.2);
        cr.rectangle(x, y, node_width, node_height);
        cr.fill().ok();
        if focus == Some(entry.object_id.as_str()) {
            cr.set_source_rgb(0.10, 0.10, 0.10);
            cr.set_line_width(2.2);
            cr.rectangle(x - 1.0, y - 1.0, node_width + 2.0, node_height + 2.0);
            cr.stroke().ok();
        }
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.set_font_size(9.3);
        cr.move_to(x + 5.0, y + 12.5);
        let _ = cr.show_text(&format!(
            "{} {}  {}",
            entry.frame_kind.as_str(),
            entry.object_id,
            entry.address
        ));
    }

    cr.set_source_rgb(0.28, 0.30, 0.32);
    cr.set_font_size(9.5);
    cr.move_to(18.0, top + 4.0);
    let _ = cr.show_text("DTS ↓");
}

fn focused_object(entry: &JournalEntry) -> Option<&str> {
    use bframe_addressing_lab::SubmitOutcome;
    use bframe_addressing_lab::journal::StepResult;
    match &entry.result {
        StepResult::Record { outcome } => match outcome {
            SubmitOutcome::Accepted { object_id, .. }
            | SubmitOutcome::Duplicate { object_id, .. }
            | SubmitOutcome::Conflict { object_id, .. } => Some(object_id),
            SubmitOutcome::InvalidRecord { .. } => None,
        },
        _ => None,
    }
}

fn install_css() {
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };
    let provider = gtk::CssProvider::new();
    provider.load_from_data(
        r#"
        .main-window { background-color: #f2eee5; }
        .app-title { font-size: 16px; font-weight: 800; color: #1d2326; }
        .app-subtitle { font-size: 10px; color: #6b6e68; }
        .pill {
            background-color: #1f3a34;
            color: #edf4e8;
            border-radius: 999px;
            padding: 5px 11px;
            font-size: 11px;
            margin: 5px;
        }
        .metric {
            background-color: #fffdf8;
            border: 1px solid #d9d3c7;
            border-radius: 9px;
            padding: 7px 11px;
        }
        .metric-name { font-size: 9px; color: #77786f; letter-spacing: 1px; }
        .metric-value { font-size: 17px; font-weight: 800; color: #202629; }
        .timeline { border: 1px solid #d9d3c7; border-radius: 12px; }
        button { border-radius: 7px; padding: 5px 12px; }
        .step-label { color: #525852; font-size: 11px; }
        .detail-panel {
            background-color: #fffdf8;
            border: 1px solid #d9d3c7;
            border-radius: 10px;
            padding: 8px;
        }
        .detail-title { font-weight: 800; color: #202629; }
        .detail-subtitle { font-size: 9px; color: #7a7c75; }
        .code-scroll { background-color: #f8f5ee; border-radius: 6px; }
        textview { font-size: 10px; color: #30383a; background-color: #f8f5ee; }
        .digest { font-family: monospace; font-size: 9px; color: #666b67; }
        "#,
    );
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}
