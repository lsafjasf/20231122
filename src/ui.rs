use gtk::prelude::*;
use gtk4 as gtk;

use crate::model::ExtremeParams;

pub fn label(text: impl AsRef<str>, class: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text.as_ref()));
    label.set_xalign(0.0);
    if !class.is_empty() {
        label.add_css_class(class);
    }
    label
}

pub fn pill(text: impl AsRef<str>, class: &str) -> gtk::Label {
    let label = label(text, "pill");
    label.add_css_class(class);
    label.set_halign(gtk::Align::Start);
    label
}

fn spacer() -> gtk::Box {
    let spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    spacer.set_vexpand(true);
    spacer
}

fn vbox(spacing: i32) -> gtk::Box {
    gtk::Box::new(gtk::Orientation::Vertical, spacing)
}

fn hbox(spacing: i32) -> gtk::Box {
    gtk::Box::new(gtk::Orientation::Horizontal, spacing)
}

fn icon_mark(text: &str, class: &str) -> gtk::Label {
    let mark = label(text, class);
    mark.set_halign(gtk::Align::Center);
    mark.set_valign(gtk::Align::Center);
    mark
}

pub struct ParamWidgets {
    pub threshold: gtk::SpinButton,
    pub ceiling: gtk::SpinButton,
    pub makeup: gtk::SpinButton,
    pub attack: gtk::SpinButton,
    pub release: gtk::SpinButton,
    pub boundary: gtk::SpinButton,
    pub extreme: gtk::ToggleButton,
    pub stable: gtk::ToggleButton,
    pub custom: gtk::ToggleButton,
}

impl ParamWidgets {
    pub fn values(&self) -> ExtremeParams {
        let preset = if self.extreme.is_active() {
            "极限"
        } else if self.stable.is_active() {
            "稳健"
        } else {
            "自定义"
        };
        ExtremeParams {
            preset: preset.into(),
            threshold_dbfs: self.threshold.value() as f32,
            ceiling_dbfs: self.ceiling.value() as f32,
            makeup_db: self.makeup.value() as f32,
            attack_ms: self.attack.value() as f32,
            release_ms: self.release.value() as f32,
            boundary_window_ms: self.boundary.value().round() as u32,
        }
    }

    pub fn set_values(&self, params: &ExtremeParams) {
        self.threshold.set_value(params.threshold_dbfs as f64);
        self.ceiling.set_value(params.ceiling_dbfs as f64);
        self.makeup.set_value(params.makeup_db as f64);
        self.attack.set_value(params.attack_ms as f64);
        self.release.set_value(params.release_ms as f64);
        self.boundary.set_value(params.boundary_window_ms as f64);
        match params.preset.as_str() {
            "极限" => self.extreme.set_active(true),
            "稳健" => self.stable.set_active(true),
            _ => self.custom.set_active(true),
        }
    }

    pub fn mark_custom(&self) {
        self.custom.set_active(true);
    }
}

pub struct Ui {
    pub window: gtk::ApplicationWindow,
    pub project_title: gtk::Label,
    pub project_subtitle: gtk::Label,
    pub media_meta: gtk::Label,
    pub session_id: gtk::Label,
    pub autosave_pill: gtk::Label,
    pub saved_time: gtk::Label,
    pub storage_label: gtk::Label,
    pub timeline: gtk::DrawingArea,
    pub timeline_caption: gtk::Label,
    pub missing_pill: gtk::Label,
    pub threshold_pill: gtk::Label,
    pub params: ParamWidgets,
    pub segment_list: gtk::Box,
    pub segment_count: gtk::Label,
    pub evidence_box: gtk::Box,
    pub run_all: gtk::Button,
    pub export_all: gtk::Button,
    pub import: gtk::Button,
    pub status_text: gtk::Label,
    pub status_dot: gtk::Box,
    pub spinner: gtk::Spinner,
}

fn nav_button(text: &str, active: bool) -> gtk::Button {
    let button = gtk::Button::with_label(text);
    button.add_css_class("nav-item");
    button.set_halign(gtk::Align::Fill);
    if active {
        button.add_css_class("active");
    }
    button
}

fn param_spin(
    title: &str,
    unit: &str,
    range: (f64, f64),
    step: f64,
    digits: u32,
) -> (gtk::Box, gtk::SpinButton) {
    let tile = vbox(4);
    tile.add_css_class("param-tile");
    tile.set_hexpand(true);
    let title = label(title, "param-name");
    let controls = hbox(5);
    let adjustment = gtk::Adjustment::new(range.0, range.0, range.1, step, step * 10.0, 0.0);
    let spin = gtk::SpinButton::new(Some(&adjustment), step, digits);
    spin.set_numeric(true);
    spin.set_hexpand(true);
    let unit = label(unit, "tiny");
    controls.append(&spin);
    controls.append(&unit);
    tile.append(&title);
    tile.append(&controls);
    (tile, spin)
}

fn page_header() -> (gtk::Box, gtk::Label, gtk::Label, gtk::Label) {
    let row = hbox(18);
    row.set_valign(gtk::Align::Center);
    let text = vbox(3);
    let title = label("现场录音 C-07 · 局部修正", "page-title");
    let subtitle = hbox(8);
    let media = label("", "page-subtitle");
    let dot = icon_mark("·", "muted");
    let meta = label("", "page-subtitle");
    subtitle.append(&media);
    subtitle.append(&dot);
    subtitle.append(&meta);
    text.append(&title);
    text.append(&subtitle);
    row.append(&text);
    let fill = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    fill.set_hexpand(true);
    row.append(&fill);
    (row, title, media, meta)
}

pub fn build_window(app: &gtk::Application) -> Ui {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("PEAKGUARD · 音频峰值预警")
        .default_width(1440)
        .default_height(900)
        .build();

    let header = gtk::HeaderBar::new();
    header.set_show_title_buttons(true);
    let header_left = hbox(10);
    let mini_mark = icon_mark("◭", "brand-mark");
    mini_mark.set_width_request(24);
    mini_mark.set_height_request(24);
    header_left.append(&mini_mark);
    header_left.append(&label("PEAKGUARD  /  修正工作台", "breadcrumb"));
    header.pack_start(&header_left);
    let autosave_pill = pill("自动保存 · 就绪", "pill-ready");
    header.pack_end(&autosave_pill);
    window.set_titlebar(Some(&header));

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let body = hbox(0);
    body.set_vexpand(true);

    let sidebar = vbox(0);
    sidebar.add_css_class("sidebar");
    sidebar.set_width_request(236);
    let brand = hbox(10);
    brand.append(&icon_mark("◭", "brand-mark"));
    let brand_text = vbox(0);
    brand_text.append(&label("PEAKGUARD", "brand-name"));
    brand_text.append(&label("AUDIO PEAK GUARD", "brand-sub"));
    brand.append(&brand_text);
    sidebar.append(&brand);

    sidebar.append(&label("工作区", "nav-label"));
    sidebar.append(&nav_button("◉   修正工作台", true));
    sidebar.append(&nav_button("▤   证据档案", false));
    sidebar.append(&nav_button("⇩   导出记录", false));
    sidebar.append(&label("项目", "nav-label"));

    let session_card = vbox(7);
    session_card.add_css_class("recovery-card");
    let session_top = hbox(8);
    let green = gtk::Box::new(gtk::Orientation::Vertical, 0);
    green.add_css_class("dot-green");
    green.set_valign(gtk::Align::Center);
    session_top.append(&green);
    session_top.append(&label("当前会话", "section-kicker"));
    let session_id = label("PG-C07", "pill pill-ready");
    session_id.set_hexpand(true);
    session_id.set_halign(gtk::Align::End);
    session_top.append(&session_id);
    session_card.append(&session_top);
    let session_name = label("现场录音 C-07 · 局部修正", "metric-small");
    session_name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    session_card.append(&session_name);
    session_card.append(&label("退出后可从中断处继续", "muted"));
    sidebar.append(&session_card);
    sidebar.append(&spacer());

    let recovery = vbox(7);
    recovery.add_css_class("recovery-card");
    let recovery_top = hbox(8);
    let recovery_dot = gtk::Box::new(gtk::Orientation::Vertical, 0);
    recovery_dot.add_css_class("dot-green");
    recovery_dot.set_valign(gtk::Align::Center);
    recovery_top.append(&recovery_dot);
    recovery_top.append(&label("恢复保护已开启", "section-kicker"));
    recovery.append(&recovery_top);
    recovery.append(&label("每次出入任务都写入关键证据", "muted"));
    let saved_time = label("最近保存 · --:--:--", "tiny");
    recovery.append(&saved_time);
    sidebar.append(&recovery);
    sidebar.append(&label("Rust  ·  GStreamer 1.28  ·  GTK4", "tiny"));
    body.append(&sidebar);

    let main = vbox(0);
    main.add_css_class("main-area");
    main.set_hexpand(true);

    let content = vbox(12);
    content.set_margin_top(15);
    content.set_margin_bottom(15);
    content.set_margin_start(22);
    content.set_margin_end(22);

    let (header_row, project_title, media_meta, file_meta) = page_header();
    let actions = hbox(8);
    let export_all = gtk::Button::with_label("⇩  导出全部");
    export_all.add_css_class("secondary");
    let import = gtk::Button::with_label("＋  导入媒体");
    import.add_css_class("secondary");
    let run_all = gtk::Button::with_label("▶  运行全部");
    run_all.add_css_class("primary");
    actions.append(&export_all);
    actions.append(&import);
    actions.append(&run_all);
    header_row.append(&actions);
    content.append(&header_row);

    let timeline_card = vbox(10);
    timeline_card.add_css_class("card");
    timeline_card.set_margin_top(2);
    let timeline_pad = vbox(9);
    timeline_pad.set_margin_top(10);
    timeline_pad.set_margin_bottom(10);
    timeline_pad.set_margin_start(14);
    timeline_pad.set_margin_end(14);
    let timeline_header = hbox(10);
    let timeline_titles = vbox(2);
    timeline_titles.append(&label("媒体时间线", "section-title"));
    timeline_titles.append(&label(
        "缺失片段定位 · 边界前后双采样 · 预警区间高亮",
        "section-kicker",
    ));
    timeline_header.append(&timeline_titles);
    let timeline_fill = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    timeline_fill.set_hexpand(true);
    timeline_header.append(&timeline_fill);
    let missing_pill = pill("缺失片段 08:00.00 — 12:00.00", "pill-missing");
    let threshold_pill = pill("阈值 -6.0 dBFS", "pill-warning");
    timeline_header.append(&missing_pill);
    timeline_header.append(&threshold_pill);
    timeline_pad.append(&timeline_header);
    let wave_wrap = vbox(0);
    wave_wrap.add_css_class("wave-wrap");
    let timeline = gtk::DrawingArea::new();
    timeline.set_content_height(110);
    timeline.set_draw_func(|_, _, _, _| {});
    wave_wrap.append(&timeline);
    timeline_pad.append(&wave_wrap);
    let timeline_caption = label(
        "青绿：安全峰值   珊瑚：超过预警阈值   红色区：缺失片段   虚线：边界采样点",
        "tiny",
    );
    timeline_pad.append(&timeline_caption);
    timeline_card.append(&timeline_pad);
    content.append(&timeline_card);

    let params_card = vbox(9);
    params_card.add_css_class("card");
    let params_pad = vbox(9);
    params_pad.set_margin_top(10);
    params_pad.set_margin_bottom(10);
    params_pad.set_margin_start(14);
    params_pad.set_margin_end(14);
    let params_header = hbox(10);
    let params_titles = vbox(2);
    params_titles.append(&label("极限参数组", "section-title"));
    params_titles.append(&label(
        "每次运行都冻结一组参数，写入独立证据记录",
        "section-kicker",
    ));
    params_header.append(&params_titles);
    let params_fill = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    params_fill.set_hexpand(true);
    params_header.append(&params_fill);
    let extreme = gtk::ToggleButton::with_label("极限组");
    extreme.add_css_class("preset");
    extreme.set_active(true);
    let stable = gtk::ToggleButton::with_label("稳健组");
    stable.add_css_class("preset");
    stable.set_group(Some(&extreme));
    let custom = gtk::ToggleButton::with_label("自定义");
    custom.add_css_class("preset");
    custom.set_group(Some(&extreme));
    let all_pill = pill("运行快照 · 不回溯", "pill-param");
    params_header.append(&all_pill);
    params_header.append(&extreme);
    params_header.append(&stable);
    params_header.append(&custom);
    params_pad.append(&params_header);

    let param_row = hbox(8);
    let (threshold_tile, threshold) = param_spin("预警阈值", "dBFS", (-80.0, 0.0), 0.5, 1);
    let (ceiling_tile, ceiling) = param_spin("峰值上限", "dBFS", (-24.0, 0.0), 0.1, 1);
    let (makeup_tile, makeup) = param_spin("补偿增益", "dB", (0.0, 24.0), 0.5, 1);
    let (attack_tile, attack) = param_spin("启动时间", "ms", (0.1, 100.0), 0.1, 1);
    let (release_tile, release) = param_spin("释放时间", "ms", (10.0, 1000.0), 5.0, 0);
    let (boundary_tile, boundary) = param_spin("边界窗口", "ms", (20.0, 500.0), 10.0, 0);
    for tile in [
        threshold_tile,
        ceiling_tile,
        makeup_tile,
        attack_tile,
        release_tile,
        boundary_tile,
    ] {
        param_row.append(&tile);
    }
    params_pad.append(&param_row);
    params_card.append(&params_pad);
    content.append(&params_card);

    let lower = hbox(14);
    let segments_card = vbox(8);
    segments_card.add_css_class("card");
    segments_card.set_hexpand(true);
    segments_card.set_vexpand(true);
    let segments_pad = vbox(6);
    segments_pad.set_margin_top(12);
    segments_pad.set_margin_bottom(8);
    segments_pad.set_margin_start(12);
    segments_pad.set_margin_end(12);
    let segments_header = hbox(10);
    let segment_titles = vbox(2);
    segment_titles.append(&label("媒体片段", "section-title"));
    segment_titles.append(&label(
        "每段单独运行、比较和导出；退出后保留最近一次证据",
        "section-kicker",
    ));
    segments_header.append(&segment_titles);
    let segment_fill = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    segment_fill.set_hexpand(true);
    segments_header.append(&segment_fill);
    let segment_count = pill("0 / 0 已完成", "pill-idle");
    segments_header.append(&segment_count);
    segments_pad.append(&segments_header);
    let segment_list = vbox(0);
    segments_pad.append(&segment_list);
    segments_card.append(&segments_pad);
    lower.append(&segments_card);

    let evidence_panel = vbox(0);
    evidence_panel.add_css_class("evidence-panel");
    evidence_panel.set_width_request(356);
    evidence_panel.set_vexpand(true);
    let evidence_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .build();
    let evidence_box = vbox(10);
    evidence_box.set_margin_top(12);
    evidence_box.set_margin_bottom(12);
    evidence_box.set_margin_start(12);
    evidence_box.set_margin_end(12);
    evidence_scroll.set_child(Some(&evidence_box));
    evidence_panel.append(&evidence_scroll);
    lower.append(&evidence_panel);
    content.append(&lower);

    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .child(&content)
        .build();
    main.append(&scroll);

    let statusbar = hbox(8);
    statusbar.add_css_class("statusbar");
    let status_dot = gtk::Box::new(gtk::Orientation::Vertical, 0);
    status_dot.add_css_class("status-dot");
    status_dot.set_valign(gtk::Align::Center);
    let spinner = gtk::Spinner::new();
    spinner.set_visible(false);
    let status_text = label("就绪 · 自动保存已开启", "muted");
    statusbar.append(&status_dot);
    statusbar.append(&spinner);
    statusbar.append(&status_text);
    let status_fill = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    status_fill.set_hexpand(true);
    statusbar.append(&status_fill);
    let storage_label = label("证据目录加载中…", "tiny");
    let status_right = label("GStreamer 1.28  ·  GTK4 4.22", "tiny");
    statusbar.append(&storage_label);
    statusbar.append(&label("  ·  ", "tiny"));
    statusbar.append(&status_right);
    main.append(&statusbar);

    body.append(&main);
    root.append(&body);
    window.set_child(Some(&root));

    Ui {
        window,
        project_title,
        project_subtitle: media_meta,
        media_meta: file_meta,
        session_id,
        autosave_pill,
        saved_time,
        storage_label,
        timeline,
        timeline_caption,
        missing_pill,
        threshold_pill,
        params: ParamWidgets {
            threshold,
            ceiling,
            makeup,
            attack,
            release,
            boundary,
            extreme,
            stable,
            custom,
        },
        segment_list,
        segment_count,
        evidence_box,
        run_all,
        export_all,
        import,
        status_text,
        status_dot,
        spinner,
    }
}
