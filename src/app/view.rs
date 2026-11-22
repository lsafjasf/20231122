use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk4 as gtk;

use crate::app::{AppState, actions};
use clockbridge::model::{
    AttemptRecord, AttemptStatus, ComparisonRecord, EvidenceEvent, Segment, format_time_ms,
};

pub(crate) fn build(app: &Rc<RefCell<AppState>>, _title: &str) {
    let state = app.borrow_mut();

    let header = gtk::HeaderBar::new();
    header.add_css_class("app-header");
    header.set_show_title_buttons(true);

    let title_box = gtk::Box::new(gtk::Orientation::Vertical, 1);
    state.title_label.set_text("CLOCKBRIDGE / 转码错误重试策略");
    state.title_label.add_css_class("app-title");
    state.title_label.set_xalign(0.0);
    state
        .subtitle_label
        .set_text("双机时钟校准 · 片段级重跑 · 可复核证据");
    state.subtitle_label.add_css_class("app-subtitle");
    state.subtitle_label.set_xalign(0.0);
    title_box.append(&state.title_label);
    title_box.append(&state.subtitle_label);
    header.set_title_widget(Some(&title_box));

    let open_button = gtk::Button::with_label("打开工作区");
    open_button.add_css_class("ghost");
    let open_app = app.clone();
    open_button.connect_clicked(move |_| actions::open_workspace_dialog(&open_app));
    header.pack_start(&open_button);

    let export_button = gtk::Button::with_label("导出复核包");
    export_button.add_css_class("suggested");
    let export_app = app.clone();
    export_button.connect_clicked(move |_| actions::export_bundle_dialog(&export_app));
    header.pack_end(&export_button);
    state.window.set_titlebar(Some(&header));

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("app-root");

    let strip = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    strip.add_css_class("status-strip");
    state.summary_label.set_xalign(0.0);
    state.summary_label.set_hexpand(true);
    state.summary_label.add_css_class("muted");
    state.summary_label.add_css_class("mono");
    strip.append(&state.summary_label);
    state.status_label.set_xalign(1.0);
    state.status_label.add_css_class("pill");
    state.status_label.add_css_class("pill-ok");
    strip.append(&state.status_label);
    root.append(&strip);

    let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
    paned.set_position(390);
    paned.set_wide_handle(true);
    paned.set_vexpand(true);

    let control_panel = gtk::Box::new(gtk::Orientation::Vertical, 14);
    control_panel.add_css_class("control-panel");
    control_panel.set_size_request(372, -1);

    let eyebrow = gtk::Label::new(Some("WORKSPACE"));
    eyebrow.add_css_class("eyebrow");
    eyebrow.set_xalign(0.0);
    control_panel.append(&eyebrow);
    let panel_title = gtk::Label::new(Some("时钟与缺失片段"));
    panel_title.add_css_class("section-title");
    panel_title.set_xalign(0.0);
    control_panel.append(&panel_title);

    let device_a = build_device_card(
        app,
        "设备 A · 主时钟",
        "OFFSET 0 ms",
        &state.device_a_path,
        "device-a",
        None,
    );
    control_panel.append(&device_a);
    let device_b = build_device_card(
        app,
        "设备 B · 跟随",
        "CLOCK OFFSET",
        &state.device_b_path,
        "device-b",
        Some(&state.offset_b),
    );
    control_panel.append(&device_b);

    let separator = gtk::Separator::new(gtk::Orientation::Horizontal);
    separator.set_margin_top(2);
    separator.set_margin_bottom(2);
    control_panel.append(&separator);

    let gap_title = gtk::Label::new(Some("缺失片段 / ALIGN WINDOW"));
    gap_title.add_css_class("eyebrow");
    gap_title.set_xalign(0.0);
    control_panel.append(&gap_title);
    control_panel.append(&form_row("主时间线起点", &state.start_ms));
    control_panel.append(&form_row("缺口时长", &state.duration_ms));
    state
        .reason
        .set_placeholder_text(Some("记录缺失原因、切点和复核提示"));
    state.reason.add_css_class("mono");
    control_panel.append(&state.reason);
    let apply_button = gtk::Button::with_label("应用时钟与缺失范围");
    apply_button.add_css_class("suggested");
    let apply_app = app.clone();
    apply_button.connect_clicked(move |_| actions::apply_missing_range(&apply_app));
    control_panel.append(&apply_button);

    let queue_header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let queue_title = gtk::Label::new(Some("片段队列"));
    queue_title.add_css_class("section-title");
    queue_title.set_xalign(0.0);
    queue_title.set_hexpand(true);
    queue_header.append(&queue_title);
    let queue_hint = gtk::Label::new(Some("ORDERED"));
    queue_hint.add_css_class("eyebrow");
    queue_header.append(&queue_hint);
    control_panel.append(&queue_header);

    state
        .segment_list
        .set_selection_mode(gtk::SelectionMode::Single);
    state.segment_list.set_activate_on_single_click(true);
    control_panel.append(&state.segment_list);

    let spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    spacer.set_vexpand(true);
    control_panel.append(&spacer);
    state.auto_save_label.set_xalign(0.0);
    state.auto_save_label.set_wrap(true);
    state.auto_save_label.add_css_class("faint");
    state.auto_save_label.add_css_class("mono");
    control_panel.append(&state.auto_save_label);

    let control_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .child(&control_panel)
        .build();
    control_scroll.set_size_request(374, -1);
    paned.set_start_child(Some(&control_scroll));

    state.detail_box.add_css_class("detail-panel");
    let detail_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .child(&state.detail_box)
        .build();
    paned.set_end_child(Some(&detail_scroll));
    root.append(&paned);
    state.window.set_child(Some(&root));
}

fn build_device_card(
    app: &Rc<RefCell<AppState>>,
    title: &str,
    badge: &str,
    path_label: &gtk::Label,
    device_id: &'static str,
    offset: Option<&gtk::SpinButton>,
) -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 7);
    card.add_css_class("device-card");
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let title_label = gtk::Label::new(Some(title));
    title_label.add_css_class("section-title");
    title_label.set_xalign(0.0);
    title_label.set_hexpand(true);
    row.append(&title_label);
    let badge_label = gtk::Label::new(Some(badge));
    badge_label.add_css_class("eyebrow");
    row.append(&badge_label);
    card.append(&row);

    path_label.set_xalign(0.0);
    path_label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    path_label.add_css_class("mono");
    path_label.add_css_class("muted");
    path_label.set_width_chars(12);
    card.append(path_label);

    let actions_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let choose_button = gtk::Button::with_label("选择媒体");
    choose_button.add_css_class("ghost");
    choose_button.add_css_class("compact");
    let choose_app = app.clone();
    choose_button.connect_clicked(move |_| actions::choose_media(&choose_app, device_id));
    actions_row.append(&choose_button);
    if let Some(offset) = offset {
        offset.set_digits(0);
        offset.set_width_chars(7);
        offset.set_tooltip_text(Some("设备时间 = 主时间线 + 偏移（毫秒）"));
        offset.set_hexpand(true);
        actions_row.append(offset);
        let unit = gtk::Label::new(Some("ms"));
        unit.add_css_class("muted");
        actions_row.append(&unit);
    }
    card.append(&actions_row);
    card
}

fn form_row(label_text: &str, spin: &gtk::SpinButton) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let label = gtk::Label::new(Some(label_text));
    label.add_css_class("muted");
    label.set_xalign(0.0);
    label.set_hexpand(true);
    row.append(&label);
    spin.set_digits(0);
    spin.set_width_chars(8);
    row.append(spin);
    let unit = gtk::Label::new(Some("ms"));
    unit.add_css_class("muted");
    row.append(&unit);
    row
}

pub(crate) fn refresh(app: &Rc<RefCell<AppState>>) {
    {
        let mut state = app.borrow_mut();
        let workspace = state.store.workspace().clone();
        if state.selected_segment_id.is_none() {
            state.selected_segment_id =
                workspace.segments.first().map(|segment| segment.id.clone());
        }
        if let Some(selected) = &state.selected_segment_id
            && workspace.segment(selected).is_none()
        {
            state.selected_segment_id =
                workspace.segments.first().map(|segment| segment.id.clone());
        }
        state
            .title_label
            .set_text(&format!("CLOCKBRIDGE / {}", workspace.title));
        state.subtitle_label.set_text(&format!(
            "{} · revision {}",
            state.store.workspace_path().display(),
            workspace.revision
        ));
        let attempts: usize = workspace.segments.iter().map(|s| s.attempts.len()).sum();
        let comparisons: usize = workspace.segments.iter().map(|s| s.comparisons.len()).sum();
        state.summary_label.set_text(&format!(
            "{} 台设备   ·   {} 个缺失片段   ·   {} 次尝试   ·   {} 条比较",
            workspace.devices.len(),
            workspace.segments.len(),
            attempts,
            comparisons
        ));
        if state.running_jobs.is_empty() {
            state.status_label.set_text("● 已自动保存");
            state.status_label.remove_css_class("pill-run");
            state.status_label.add_css_class("pill-ok");
        } else {
            state
                .status_label
                .set_text(&format!("● {} 个后台任务", state.running_jobs.len()));
            state.status_label.remove_css_class("pill-ok");
            state.status_label.add_css_class("pill-run");
        }
        state.auto_save_label.set_text(&format!(
            "AUTO-SAVE  rev {}\nEVIDENCE  {} 条（追加式）\nJOBS      {}",
            workspace.revision,
            state.store.events().len(),
            state.running_jobs.len()
        ));
        if let Some(device) = workspace.devices.first() {
            state
                .device_a_path
                .set_text(&display_path(&device.media_path));
            state
                .device_a_path
                .set_tooltip_text(Some(&device.media_path));
        }
        if let Some(device) = workspace.devices.get(1) {
            state
                .device_b_path
                .set_text(&display_path(&device.media_path));
            state
                .device_b_path
                .set_tooltip_text(Some(&device.media_path));
            if !state.offset_b.has_focus() {
                state
                    .offset_b
                    .set_value(device.clock_offset.milliseconds as f64);
            }
        }
        if let Some(missing) = &workspace.missing_segment {
            if !state.start_ms.has_focus() {
                state.start_ms.set_value(missing.timeline_start_ms as f64);
            }
            if !state.duration_ms.has_focus() {
                state.duration_ms.set_value(missing.duration_ms as f64);
            }
            if !state.reason.has_focus() {
                state.reason.set_text(&missing.reason);
            }
        }
        rebuild_segment_list(&mut state);
    }
    render_detail(app);
}

fn rebuild_segment_list(state: &mut AppState) {
    while let Some(child) = state.segment_list.first_child() {
        state.segment_list.remove(&child);
    }
    let selected = state.selected_segment_id.clone();
    let rows: Vec<(String, Segment)> = state
        .store
        .workspace()
        .segments
        .iter()
        .cloned()
        .map(|segment| (segment.id.clone(), segment))
        .collect();
    for (id, segment) in rows {
        let row = gtk::ListBoxRow::new();
        row.set_widget_name(&id);
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        content.set_margin_top(2);
        content.set_margin_bottom(2);
        let ordinal = gtk::Label::new(Some(&format!("{:02}", segment.ordinal)));
        ordinal.add_css_class("mono-large");
        content.append(&ordinal);
        let text = gtk::Box::new(gtk::Orientation::Vertical, 1);
        text.set_hexpand(true);
        let label = gtk::Label::new(Some(&segment.label));
        label.set_xalign(0.0);
        label.add_css_class("section-title");
        text.append(&label);
        let times = gtk::Label::new(Some(&format!(
            "{} · {}",
            format_time_ms(segment.timeline_start_ms),
            format_time_ms(segment.duration_ms)
        )));
        times.set_xalign(0.0);
        times.add_css_class("mono");
        times.add_css_class("faint");
        text.append(&times);
        content.append(&text);
        let latest = segment.attempts.last();
        let chip = status_chip(latest.map(|attempt| attempt.status));
        content.append(&chip);
        row.set_child(Some(&content));
        state.segment_list.append(&row);
        if selected.as_deref() == Some(id.as_str()) {
            state.segment_list.select_row(Some(&row));
        }
    }
}

fn status_chip(status: Option<AttemptStatus>) -> gtk::Label {
    let (text, class) = match status {
        Some(AttemptStatus::Succeeded) => ("OK", "pill-ok"),
        Some(AttemptStatus::Running) => ("RUN", "pill-run"),
        Some(AttemptStatus::Failed) => ("FAIL", "pill-fail"),
        Some(AttemptStatus::Interrupted) => ("INT", "pill-fail"),
        None => ("NEW", "pill-muted"),
    };
    let chip = gtk::Label::new(Some(text));
    chip.add_css_class("pill");
    chip.add_css_class(class);
    chip
}

fn display_path(path: &str) -> String {
    if path.is_empty() {
        "未选择媒体文件".to_string()
    } else {
        path.to_string()
    }
}

fn render_detail(app: &Rc<RefCell<AppState>>) {
    let detail = { app.borrow().detail_box.clone() };
    while let Some(child) = detail.first_child() {
        detail.remove(&child);
    }
    let segment = {
        let state = app.borrow();
        state
            .selected_segment_id
            .as_deref()
            .and_then(|id| state.store.workspace().segment(id))
            .cloned()
    };
    let events = { app.borrow().store.events().to_vec() };
    let Some(segment) = segment else {
        let empty = gtk::Label::new(Some("没有片段。请先应用一个缺失范围。"));
        empty.add_css_class("muted");
        empty.set_margin_top(60);
        detail.append(&empty);
        return;
    };

    let top = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let eyebrow = gtk::Label::new(Some(&format!("SEGMENT {:02} / TIMELINE", segment.ordinal)));
    eyebrow.add_css_class("eyebrow");
    eyebrow.set_xalign(0.0);
    eyebrow.set_hexpand(true);
    top.append(&eyebrow);
    let latest_status = segment.attempts.last().map(|attempt| attempt.status);
    top.append(&status_chip(latest_status));
    detail.append(&top);

    let title = gtk::Label::new(Some(&segment.label));
    title.add_css_class("detail-title");
    title.set_xalign(0.0);
    detail.append(&title);

    let metrics = gtk::Box::new(gtk::Orientation::Horizontal, 24);
    metrics.append(&metric_block(
        "主时间线起点",
        &format_time_ms(segment.timeline_start_ms),
        false,
    ));
    metrics.append(&metric_block(
        "缺口时长",
        &format_time_ms(segment.duration_ms),
        true,
    ));
    metrics.append(&metric_block(
        "尝试 / 比较",
        &format!("{} / {}", segment.attempts.len(), segment.comparisons.len()),
        false,
    ));
    detail.append(&metrics);

    if !segment.reason.is_empty() {
        let reason = gtk::Label::new(Some(&segment.reason));
        reason.set_wrap(true);
        reason.set_xalign(0.0);
        reason.add_css_class("card");
        reason.add_css_class("muted");
        detail.append(&reason);
    }

    let source_heading = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let source_title = gtk::Label::new(Some("双机解析范围"));
    source_title.add_css_class("section-title");
    source_title.set_xalign(0.0);
    source_title.set_hexpand(true);
    source_heading.append(&source_title);
    let align_hint = gtk::Label::new(Some("DEVICE TIME = TIMELINE + OFFSET"));
    align_hint.add_css_class("eyebrow");
    source_heading.append(&align_hint);
    detail.append(&source_heading);

    let source_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    source_row.set_homogeneous(true);
    for source in &segment.sources {
        let latest = segment
            .attempts
            .iter()
            .rev()
            .find(|attempt| attempt.device_id == source.device_id);
        source_row.append(&source_card(app, &segment, source, latest));
    }
    detail.append(&source_row);

    let actions_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let rerun_both = gtk::Button::with_label("重跑双机");
    rerun_both.add_css_class("suggested");
    let rerun_app = app.clone();
    let rerun_segment_id = segment.id.clone();
    rerun_both.connect_clicked(move |_| actions::rerun_segment(&rerun_app, &rerun_segment_id));
    actions_row.append(&rerun_both);

    let compare_latest = gtk::Button::with_label("比较最新两次");
    compare_latest.add_css_class("ghost");
    compare_latest.set_sensitive(segment.attempts.len() >= 2);
    let compare_app = app.clone();
    let compare_segment_id = segment.id.clone();
    compare_latest
        .connect_clicked(move |_| actions::compare_latest(&compare_app, &compare_segment_id));
    actions_row.append(&compare_latest);

    let export_segment = gtk::Button::with_label("导出本段证据");
    export_segment.add_css_class("ghost");
    let export_app = app.clone();
    let export_segment_id = segment.id.clone();
    export_segment
        .connect_clicked(move |_| actions::export_segment_dialog(&export_app, &export_segment_id));
    actions_row.append(&export_segment);
    detail.append(&actions_row);

    append_attempts(app, &detail, &segment);
    append_comparison(app, &detail, &segment);
    append_evidence(&detail, &events);
}

fn metric_block(label_text: &str, value: &str, teal: bool) -> gtk::Box {
    let block = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let label = gtk::Label::new(Some(label_text));
    label.set_xalign(0.0);
    label.add_css_class("faint");
    block.append(&label);
    let value = gtk::Label::new(Some(value));
    value.set_xalign(0.0);
    value.add_css_class("mono-large");
    if !teal {
        value.add_css_class("accent");
        value.remove_css_class("mono-large");
        value.add_css_class("metric");
    }
    block.append(&value);
    block
}

fn source_card(
    app: &Rc<RefCell<AppState>>,
    segment: &Segment,
    source: &clockbridge::model::SourceBinding,
    latest: Option<&AttemptRecord>,
) -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 8);
    card.add_css_class("source-card");
    card.set_hexpand(true);

    let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let name = gtk::Label::new(Some(&source.device_name));
    name.set_xalign(0.0);
    name.add_css_class("section-title");
    name.set_hexpand(true);
    top.append(&name);
    top.append(&status_chip(latest.map(|attempt| attempt.status)));
    card.append(&top);

    let range = gtk::Label::new(Some(&format!(
        "{}  →  {}",
        format_time_ms(source.resolved_start_ms),
        format_time_ms(source.resolved_end_ms)
    )));
    range.set_xalign(0.0);
    range.add_css_class("mono-large");
    card.append(&range);

    let offset = gtk::Label::new(Some(&format!(
        "clock offset  {:+.0} ms   ·   request {} ms",
        source.clock_offset_ms,
        source.clamped_duration_ms()
    )));
    offset.set_xalign(0.0);
    offset.add_css_class("mono");
    offset.add_css_class("muted");
    card.append(&offset);

    let path = gtk::Label::new(Some(&display_path(&source.media_path)));
    path.set_xalign(0.0);
    path.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    path.set_tooltip_text(Some(&source.media_path));
    path.add_css_class("mono");
    path.add_css_class("faint");
    card.append(&path);

    if let Some(attempt) = latest {
        let digest = gtk::Label::new(Some(&format!(
            "#{:03}  {}  {}",
            attempt.attempt_no,
            attempt.status.label(),
            attempt
                .output_sha256
                .as_deref()
                .map(|hash| format!("sha256:{}", short_hash(hash)))
                .unwrap_or_else(|| "无输出指纹".to_string())
        )));
        digest.set_xalign(0.0);
        digest.add_css_class("mono");
        digest.add_css_class("muted");
        card.append(&digest);
    }

    let run_button = gtk::Button::with_label("单独重跑此机");
    run_button.add_css_class("ghost");
    run_button.add_css_class("compact");
    let run_app = app.clone();
    let segment_id = segment.id.clone();
    let device_id = source.device_id.clone();
    run_button.connect_clicked(move |_| actions::rerun_source(&run_app, &segment_id, &device_id));
    card.append(&run_button);
    card
}

fn append_attempts(app: &Rc<RefCell<AppState>>, detail: &gtk::Box, segment: &Segment) {
    let heading = gtk::Label::new(Some(&format!(
        "重跑记录 / RETRY LOG  ({})",
        segment.attempts.len()
    )));
    heading.set_xalign(0.0);
    heading.add_css_class("section-title");
    heading.set_margin_top(8);
    detail.append(&heading);
    if segment.attempts.is_empty() {
        let empty = gtk::Label::new(Some(
            "尚未重跑。每次运行都会先落盘尝试记录，再启动 GStreamer，并保留输入/输出指纹。",
        ));
        empty.set_xalign(0.0);
        empty.set_wrap(true);
        empty.add_css_class("muted");
        empty.add_css_class("card");
        detail.append(&empty);
        return;
    }
    for attempt in &segment.attempts {
        detail.append(&attempt_card(app, segment, attempt));
    }
}

fn attempt_card(
    app: &Rc<RefCell<AppState>>,
    segment: &Segment,
    attempt: &AttemptRecord,
) -> gtk::Box {
    let selected = segment.selected_attempt_id.as_deref() == Some(attempt.id.as_str());
    let card = gtk::Box::new(gtk::Orientation::Vertical, 5);
    card.add_css_class("attempt-card");
    if selected {
        card.add_css_class("attempt-card-selected");
    }

    let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let title = gtk::Label::new(Some(&format!(
        "#{:03}  {}  ·  {}",
        attempt.attempt_no,
        attempt.device_name,
        attempt.status.label()
    )));
    title.set_xalign(0.0);
    title.add_css_class("section-title");
    title.set_hexpand(true);
    top.append(&title);
    if selected {
        let baseline = gtk::Label::new(Some("BASELINE"));
        baseline.add_css_class("pill");
        baseline.add_css_class("pill-run");
        top.append(&baseline);
    }
    top.append(&status_chip(Some(attempt.status)));

    let rerun = gtk::Button::with_label("重跑");
    rerun.add_css_class("compact");
    let rerun_app = app.clone();
    let segment_id = segment.id.clone();
    let device_id = attempt.device_id.clone();
    rerun.connect_clicked(move |_| actions::rerun_source(&rerun_app, &segment_id, &device_id));
    top.append(&rerun);

    let baseline = gtk::Button::with_label("设为基准");
    baseline.add_css_class("compact");
    baseline.set_sensitive(attempt.status == AttemptStatus::Succeeded && !selected);
    let baseline_app = app.clone();
    let baseline_segment_id = segment.id.clone();
    let baseline_attempt_id = attempt.id.clone();
    baseline.connect_clicked(move |_| {
        actions::select_baseline(&baseline_app, &baseline_segment_id, &baseline_attempt_id)
    });
    top.append(&baseline);
    card.append(&top);

    let range = gtk::Label::new(Some(&format!(
        "source {} + {} ms   ·   wall {} ms   ·   {}",
        format_time_ms(attempt.resolved_start_ms),
        attempt.duration_ms,
        attempt.wall_time_ms,
        attempt
            .output_size_bytes
            .map(format_bytes)
            .unwrap_or_else(|| "无产物大小".to_string())
    )));
    range.set_xalign(0.0);
    range.add_css_class("mono");
    range.add_css_class("muted");
    card.append(&range);

    let fingerprints = gtk::Label::new(Some(&format!(
        "IN  {}\nOUT {}",
        attempt
            .source_sha256
            .as_deref()
            .map(|hash| format!("sha256:{}", short_hash(hash)))
            .unwrap_or_else(|| "未取得".to_string()),
        attempt
            .output_sha256
            .as_deref()
            .map(|hash| format!("sha256:{}", short_hash(hash)))
            .unwrap_or_else(|| "未取得".to_string())
    )));
    fingerprints.set_xalign(0.0);
    fingerprints.add_css_class("mono");
    fingerprints.add_css_class("faint");
    card.append(&fingerprints);

    if !attempt.output_path.is_empty() {
        let output = gtk::Label::new(Some(&attempt.output_path));
        output.set_xalign(0.0);
        output.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        output.set_tooltip_text(Some(&attempt.output_path));
        output.add_css_class("mono");
        output.add_css_class("faint");
        card.append(&output);
    }
    if let Some(error) = &attempt.error
        && !error.is_empty()
    {
        let error_label = gtk::Label::new(Some(error));
        error_label.set_xalign(0.0);
        error_label.set_wrap(true);
        error_label.add_css_class("danger");
        card.append(&error_label);
    }
    card
}

fn append_comparison(app: &Rc<RefCell<AppState>>, detail: &gtk::Box, segment: &Segment) {
    let heading = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let title = gtk::Label::new(Some("人工比较 / COMPARE"));
    title.set_xalign(0.0);
    title.add_css_class("section-title");
    title.set_hexpand(true);
    heading.append(&title);
    let history = gtk::Label::new(Some(&format!("{} RECORDS", segment.comparisons.len())));
    history.add_css_class("eyebrow");
    heading.append(&history);
    detail.append(&heading);

    if segment.attempts.len() >= 2 {
        let labels: Vec<String> = segment
            .attempts
            .iter()
            .map(|attempt| {
                format!(
                    "#{:03} {} · {} · {}",
                    attempt.attempt_no,
                    attempt.device_name,
                    attempt.status.label(),
                    attempt
                        .output_sha256
                        .as_deref()
                        .map(short_hash)
                        .unwrap_or("-")
                )
            })
            .collect();
        let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let model = gtk::StringList::new(&label_refs);
        let left = gtk::DropDown::new(Some(model.clone()), None::<gtk::Expression>);
        let right = gtk::DropDown::new(Some(model), None::<gtk::Expression>);
        let (default_left, default_right) = default_comparison_pair(segment);
        left.set_selected(default_left as u32);
        right.set_selected(default_right as u32);
        left.set_hexpand(true);
        right.set_hexpand(true);

        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.add_css_class("card");
        let left_label = gtk::Label::new(Some("LEFT"));
        left_label.add_css_class("eyebrow");
        row.append(&left_label);
        row.append(&left);
        let arrow = gtk::Label::new(Some("↔"));
        arrow.add_css_class("accent");
        row.append(&arrow);
        let right_label = gtk::Label::new(Some("RIGHT"));
        right_label.add_css_class("eyebrow");
        row.append(&right_label);
        row.append(&right);
        let record = gtk::Button::with_label("记录比较");
        record.add_css_class("suggested");
        let ids: Rc<Vec<String>> = Rc::new(
            segment
                .attempts
                .iter()
                .map(|attempt| attempt.id.clone())
                .collect(),
        );
        let compare_app = app.clone();
        let compare_segment_id = segment.id.clone();
        record.connect_clicked(move |_| {
            let left_index = left.selected() as usize;
            let right_index = right.selected() as usize;
            if left_index == right_index {
                actions::show_error(&compare_app, "无法比较", "左右两侧必须选择不同的尝试记录。");
                return;
            }
            let Some(left_id) = ids.get(left_index) else {
                return;
            };
            let Some(right_id) = ids.get(right_index) else {
                return;
            };
            actions::compare_pair(&compare_app, &compare_segment_id, left_id, right_id);
        });
        row.append(&record);
        detail.append(&row);
    } else {
        let hint = gtk::Label::new(Some(
            "至少需要两次尝试才能比较。可分别单独重跑 A、B 两机，再记录结论。",
        ));
        hint.set_xalign(0.0);
        hint.set_wrap(true);
        hint.add_css_class("muted");
        hint.add_css_class("card");
        detail.append(&hint);
    }

    if let Some(comparison) = segment.comparisons.last() {
        detail.append(&comparison_card(comparison));
    }
}

fn comparison_card(comparison: &ComparisonRecord) -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 7);
    card.add_css_class("compare-card");
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let checksum = gtk::Label::new(Some(if comparison.checksum_equal {
        "校验和一致"
    } else {
        "校验和不同"
    }));
    checksum.add_css_class("pill");
    checksum.add_css_class(if comparison.checksum_equal {
        "pill-ok"
    } else {
        "pill-run"
    });
    top.append(&checksum);
    let size_delta = gtk::Label::new(Some(&format!(
        "大小差 {}",
        comparison
            .size_delta_bytes
            .map(|value| format!("{value:+} B"))
            .unwrap_or_else(|| "-".to_string())
    )));
    size_delta.add_css_class("mono");
    size_delta.add_css_class("muted");
    top.append(&size_delta);
    let duration = gtk::Label::new(Some(&format!(
        "时长 {} / {} ms",
        comparison.left_duration_ms, comparison.right_duration_ms
    )));
    duration.add_css_class("mono");
    duration.add_css_class("muted");
    top.append(&duration);
    let source = gtk::Label::new(Some(if comparison.same_source {
        "同源重跑"
    } else {
        "跨设备"
    }));
    source.add_css_class("eyebrow");
    top.append(&source);
    card.append(&top);
    let verdict = gtk::Label::new(Some(&comparison.verdict));
    verdict.set_xalign(0.0);
    verdict.set_wrap(true);
    verdict.add_css_class("teal");
    card.append(&verdict);
    let meta = gtk::Label::new(Some(&format!(
        "{} · left {} · right {}",
        comparison.created_at,
        short_hash(&comparison.left_attempt_id),
        short_hash(&comparison.right_attempt_id)
    )));
    meta.set_xalign(0.0);
    meta.add_css_class("mono");
    meta.add_css_class("faint");
    card.append(&meta);
    card
}

fn append_evidence(detail: &gtk::Box, events: &[EvidenceEvent]) {
    let heading = gtk::Label::new(Some("证据链 / EVIDENCE"));
    heading.set_xalign(0.0);
    heading.add_css_class("section-title");
    heading.set_margin_top(8);
    detail.append(&heading);
    if events.is_empty() {
        let hint = gtk::Label::new(Some("尚无证据事件。"));
        hint.set_xalign(0.0);
        hint.add_css_class("muted");
        detail.append(&hint);
        return;
    }
    let hint = gtk::Label::new(Some(
        "最近事件倒序显示；seq、时间和原始摘要均随导出包保留。",
    ));
    hint.set_xalign(0.0);
    hint.add_css_class("faint");
    detail.append(&hint);
    for event in events.iter().rev().take(7) {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        row.add_css_class("attempt-card");
        let seq = gtk::Label::new(Some(&format!("#{:04}", event.seq)));
        seq.add_css_class("mono");
        seq.add_css_class("accent");
        row.append(&seq);
        let text = gtk::Box::new(gtk::Orientation::Vertical, 1);
        text.set_hexpand(true);
        let summary = gtk::Label::new(Some(&event.summary));
        summary.set_xalign(0.0);
        summary.add_css_class("muted");
        summary.set_ellipsize(gtk::pango::EllipsizeMode::End);
        text.append(&summary);
        let meta = gtk::Label::new(Some(&format!("{} · {}", event.kind, event.at)));
        meta.set_xalign(0.0);
        meta.add_css_class("faint");
        meta.add_css_class("mono");
        text.append(&meta);
        row.append(&text);
        detail.append(&row);
    }
}

fn default_comparison_pair(segment: &Segment) -> (usize, usize) {
    let left = segment
        .attempts
        .iter()
        .enumerate()
        .rev()
        .find(|(_, attempt)| {
            attempt.status == AttemptStatus::Succeeded
                && segment
                    .sources
                    .first()
                    .map(|source| source.device_id == attempt.device_id)
                    .unwrap_or(false)
        })
        .map(|(index, _)| index)
        .unwrap_or(0);
    let right = segment
        .attempts
        .iter()
        .enumerate()
        .rev()
        .find(|(index, attempt)| *index != left && attempt.status == AttemptStatus::Succeeded)
        .map(|(index, _)| index)
        .unwrap_or_else(|| segment.attempts.len().saturating_sub(1));
    (left, right)
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.2} MiB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

fn short_hash(value: &str) -> &str {
    value.get(..12).unwrap_or(value)
}
