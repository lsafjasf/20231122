use std::cell::{Cell, RefCell};
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::Result;
use gtk4 as gtk;
use gtk::prelude::*;
use gtk::{
    Application, ApplicationWindow, Box as GtkBox, Button, DrawingArea, DropDown,
    HeaderBar, Label, ListBox, Orientation, Paned, Scale, ScrolledWindow, Switch, TextView,
};
use serde_json::json;

use crate::gst::{self, Transport};
use crate::layout::{self, CreditsLayout};
use crate::model::{
    BlockStyle, CreditsBlock, CreditsSample, LayoutConfig, ASPECTS,
};
use crate::render;
use crate::report;
use crate::samples;

struct UiState {
    sample_index: usize,
    custom_sample: Option<CreditsSample>,
    config: LayoutConfig,
    layout: CreditsLayout,
    playhead: f64,
    playing: bool,
    debug: bool,
    anchor_note: String,
}

impl UiState {
    fn new() -> Result<Self> {
        let config = LayoutConfig::default();
        let sample = samples::all_samples()[0].clone();
        let layout = layout::build(&sample, &config)?;
        let playhead = showcase_time(&layout);
        Ok(Self {
            sample_index: 0,
            custom_sample: None,
            config,
            layout,
            playhead,
            playing: false,
            debug: true,
            anchor_note: "初始定位在首行基线 · 可播放或回到开头".into(),
        })
    }

    fn sample(&self) -> CreditsSample {
        self.custom_sample
            .clone()
            .unwrap_or_else(|| samples::all_samples()[self.sample_index].clone())
    }

    fn rebuild_preserving_anchor(&mut self) -> Result<()> {
        let old = &self.layout;
        let focus = old.safe_area.y + old.safe_area.height * 0.5;
        let anchor = old.anchor_at(self.playhead, focus);
        let sample = self.sample();
        let changed = layout::build(&sample, &self.config)?;
        if let Some(anchor) = anchor {
            if let Some((new_time, line_id)) = changed.time_for_anchor(
                &anchor.block_id,
                anchor.hard_line_index,
                anchor.screen_baseline_y,
            ) {
                self.playhead = new_time.clamp(0.0, changed.duration);
                let jump = changed
                    .anchor_at(self.playhead, anchor.screen_baseline_y)
                    .map(|value| (value.screen_baseline_y - anchor.screen_baseline_y).abs())
                    .unwrap_or(0.0);
                self.anchor_note = format!(
                    "锚点 {} → {} · 基线跳变 {:.1} px · 时间轴未重置",
                    anchor.line_id, line_id, jump
                );
                self.layout = changed;
                return Ok(());
            }
        }
        self.playhead = self.playhead.min(changed.duration);
        self.anchor_note = "无可锚定行，保留时间比例".into();
        self.layout = changed;
        Ok(())
    }

    fn reload_sample(&mut self) -> Result<()> {
        let sample = self.sample();
        let changed = layout::build(&sample, &self.config)?;
        self.playhead = self.playhead.min(changed.duration);
        self.layout = changed;
        self.anchor_note = "已切换样例，时间轴按当前秒数保留".into();
        Ok(())
    }

    fn apply_text(&mut self, text: &str) -> Result<()> {
        let hard_lines: Vec<String> = text.lines().map(ToString::to_string).collect();
        self.custom_sample = Some(CreditsSample {
            id: "custom-copy".into(),
            label: "自定义字幕文本".into(),
            blocks: vec![CreditsBlock {
                id: "B00".into(),
                style: BlockStyle::Body,
                hard_lines,
            }],
        });
        let sample = self.sample();
        let changed = layout::build(&sample, &self.config)?;
        self.playhead = showcase_time(&changed);
        self.layout = changed;
        self.anchor_note = "已应用编辑文本 · 每个换行均作为硬换行".into();
        Ok(())
    }
}

pub fn run() -> Result<()> {
    gst::init().ok();
    let app = Application::builder()
        .application_id("io.creditslayout.studio")
        .build();
    app.connect_activate(build_window);
    app.run_with_args(&["credits-layout"]);
    Ok(())
}

fn install_css() {
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };
    let provider = gtk::CssProvider::new();
    provider.load_from_data(
        "
        window { background: #0b0f15; }
        .preview-shell { background: #05070b; border-radius: 12px; }
        .sidebar { background: #111720; }
        .panel { background: #151d28; border-radius: 10px; padding: 12px; }
        .section-title { color: #7fd6a0; font-weight: 700; font-size: 13px; }
        .metric { color: #aebbd0; font-family: monospace; font-size: 12px; }
        .metric-strong { color: #f4f6fa; font-family: monospace; font-size: 12px; }
        .hint { color: #778499; font-size: 11px; }
        .export { background: #1d6b46; color: white; }
        ",
    );
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

fn build_window(app: &Application) {
    install_css();
    let state = match UiState::new() {
        Ok(state) => Rc::new(RefCell::new(state)),
        Err(error) => {
            eprintln!("cannot initialise layout: {error:#}");
            app.quit();
            return;
        }
    };
    let transport = match Transport::new() {
        Ok(transport) => Rc::new(RefCell::new(transport)),
        Err(error) => {
            eprintln!("cannot initialise GStreamer clock: {error:#}");
            app.quit();
            return;
        }
    };

    let window = ApplicationWindow::builder()
        .application(app)
        .title("滚动字幕安全区工作台")
        .default_width(1520)
        .default_height(920)
        .build();
    let header = HeaderBar::new();
    let title = Label::new(Some("CREDITS SAFE-AREA STUDIO"));
    title.add_css_class("section-title");
    header.set_title_widget(Some(&title));
    let stack_label = Label::new(Some("Rust · GTK4 · GStreamer"));
    stack_label.add_css_class("hint");
    header.pack_end(&stack_label);
    window.set_titlebar(Some(&header));

    let area = DrawingArea::new();
    area.set_hexpand(true);
    area.set_vexpand(true);
    area.set_content_width(920);
    area.set_content_height(620);
    area.add_css_class("preview-shell");

    let preview_shell = GtkBox::new(Orientation::Vertical, 0);
    preview_shell.set_margin_top(18);
    preview_shell.set_margin_bottom(18);
    preview_shell.set_margin_start(18);
    preview_shell.set_margin_end(10);
    preview_shell.append(&area);

    let sidebar = build_sidebar();
    sidebar.root.add_css_class("sidebar");
    let sidebar_scroll = ScrolledWindow::new();
    sidebar_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    sidebar_scroll.set_width_request(408);
    sidebar_scroll.set_vexpand(true);
    sidebar_scroll.set_child(Some(&sidebar.root));
    let paned = Paned::new(Orientation::Horizontal);
    paned.set_start_child(Some(&preview_shell));
    paned.set_end_child(Some(&sidebar_scroll));
    paned.set_position(1080);
    window.set_child(Some(&paned));
    wire_draw(&area, state.clone());
    let inspector = sidebar.inspector.clone();
    wire_controls(sidebar, state.clone(), transport.clone(), area.clone());
    start_tick_loop(state, transport, area, inspector);
    window.present();
    schedule_snapshot(&window, app.clone());
}

fn wire_draw(area: &DrawingArea, state: Rc<RefCell<UiState>>) {
    area.set_draw_func(move |_, context, width, height| {
        let state = state.borrow();
        let layout = &state.layout;
        let scale = (width as f64 / layout.canvas.width)
            .min(height as f64 / layout.canvas.height)
            .max(0.001);
        let offset_x = (width as f64 - layout.canvas.width * scale) * 0.5;
        let offset_y = (height as f64 - layout.canvas.height * scale) * 0.5;
        let _ = context.save();
        context.translate(offset_x, offset_y);
        context.scale(scale, scale);
        let _ = render::draw_frame(context, layout, state.playhead, state.debug);
        let _ = context.restore();
    });
}

#[derive(Clone)]
struct Inspector {
    metrics: Label,
    anchor: Label,
    visible: Label,
    overflow: Label,
    status: Label,
    timeline: Scale,
    play_button: Button,
    line_list: ListBox,
    timeline_guard: Rc<Cell<bool>>,
}

struct Sidebar {
    root: GtkBox,
    sample_dropdown: DropDown,
    aspect_dropdown: DropDown,
    font_dropdown: DropDown,
    size_scale: Scale,
    gap_scale: Scale,
    speed_scale: Scale,
    copy_view: TextView,
    apply_text_button: Button,
    debug_switch: Switch,
    play_button: Button,
    reset_button: Button,
    export_button: Button,
    inspector: Inspector,
}

fn build_sidebar() -> Sidebar {
    let root = GtkBox::new(Orientation::Vertical, 12);
    root.set_width_request(390);
    root.set_margin_top(18);
    root.set_margin_bottom(18);
    root.set_margin_start(10);
    root.set_margin_end(18);

    let playback = panel("播放 / TRANSPORT");
    let transport_row = GtkBox::new(Orientation::Horizontal, 8);
    let play_button = Button::with_label("▶ 播放");
    play_button.add_css_class("suggested-action");
    let reset_button = Button::with_label("↺ 回到开头");
    transport_row.append(&play_button);
    transport_row.append(&reset_button);
    playback.append(&transport_row);
    let timeline = Scale::with_range(Orientation::Horizontal, 0.0, 1000.0, 1.0);
    timeline.set_draw_value(false);
    playback.append(&timeline);
    root.append(&playback);

    let layout_panel = panel("排版 / LAYOUT");
    let sample_dropdown =
        DropDown::from_strings(&["中英文混排", "表情符号", "超长单行"]);
    layout_panel.append(&control_row("样例", &sample_dropdown));
    let aspects: Vec<&str> = ASPECTS.iter().map(|aspect| aspect.label).collect();
    let aspect_dropdown = DropDown::from_strings(&aspects);
    layout_panel.append(&control_row("画幅", &aspect_dropdown));
    let font_dropdown = DropDown::from_strings(&[
        "Noto Sans CJK SC",
        "Droid Sans Fallback",
        "DejaVu Sans",
        "Noto Sans Mono",
    ]);
    layout_panel.append(&control_row("字体", &font_dropdown));
    let size_scale = slider(0.6, 1.8, 0.05, 1.0);
    layout_panel.append(&control_row("字号倍率", &size_scale));
    let gap_scale = slider(0.0, 0.9, 0.05, 0.35);
    layout_panel.append(&control_row("行距因子", &gap_scale));
    let speed_scale = slider(20.0, 360.0, 10.0, 120.0);
    layout_panel.append(&control_row("滚动速度", &speed_scale));
    root.append(&layout_panel);

    let copy_panel = panel("文本 / COPY");
    let copy_view = TextView::new();
    copy_view.set_wrap_mode(gtk::WrapMode::WordChar);
    copy_view.set_top_margin(6);
    copy_view.set_bottom_margin(6);
    copy_view.set_left_margin(6);
    copy_view.set_right_margin(6);
    copy_view
        .buffer()
        .set_text(&sample_text(&samples::all_samples()[0]));
    let copy_scroll = ScrolledWindow::new();
    copy_scroll.set_min_content_height(108);
    copy_scroll.set_child(Some(&copy_view));
    copy_panel.append(&copy_scroll);
    let apply_text_button = Button::with_label("应用文本并重排");
    copy_panel.append(&apply_text_button);
    let copy_hint = Label::new(Some("每个换行保留为硬换行；粘贴超长单行会自动字符级折行"));
    copy_hint.add_css_class("hint");
    copy_hint.set_xalign(0.0);
    copy_hint.set_wrap(true);
    copy_panel.append(&copy_hint);
    root.append(&copy_panel);

    let inspect = panel("实时检查 / INSPECTOR");
    let metrics = Label::new(None);
    metrics.add_css_class("metric");
    metrics.set_xalign(0.0);
    inspect.append(&metrics);
    let anchor = Label::new(None);
    anchor.add_css_class("metric-strong");
    anchor.set_xalign(0.0);
    anchor.set_wrap(true);
    inspect.append(&anchor);
    let visible = Label::new(None);
    visible.add_css_class("metric");
    visible.set_xalign(0.0);
    visible.set_wrap(true);
    inspect.append(&visible);
    let overflow = Label::new(None);
    overflow.add_css_class("metric");
    overflow.set_xalign(0.0);
    overflow.set_wrap(true);
    inspect.append(&overflow);
    let line_list = ListBox::new();
    line_list.set_selection_mode(gtk::SelectionMode::None);
    let list_scroll = ScrolledWindow::new();
    list_scroll.set_min_content_height(132);
    list_scroll.set_child(Some(&line_list));
    inspect.append(&list_scroll);
    root.append(&inspect);

    let export_panel = panel("导出 / EVIDENCE");
    let export_button = Button::with_label("导出参数 + 越界记录 + 预览");
    export_button.add_css_class("export");
    export_panel.append(&export_button);
    let hint = Label::new(Some("JSON 含每行基线、时间窗、来源换行与越界区间"));
    hint.add_css_class("hint");
    hint.set_xalign(0.0);
    hint.set_wrap(true);
    export_panel.append(&hint);
    let debug_switch = Switch::new();
    debug_switch.set_active(true);
    debug_switch.set_halign(gtk::Align::Start);
    export_panel.append(&control_row("显示基线与安全框", &debug_switch));
    let status = Label::new(Some("就绪 · GStreamer 时钟已就绪"));
    status.add_css_class("hint");
    status.set_xalign(0.0);
    status.set_wrap(true);
    export_panel.append(&status);
    root.append(&export_panel);

    let inspector = Inspector {
        metrics,
        anchor,
        visible,
        overflow,
        status,
        timeline: timeline.clone(),
        play_button: play_button.clone(),
        line_list,
        timeline_guard: Rc::new(Cell::new(false)),
    };
    Sidebar {
        root,
        sample_dropdown,
        aspect_dropdown,
        font_dropdown,
        size_scale,
        gap_scale,
        speed_scale,
        copy_view,
        apply_text_button,
        debug_switch,
        play_button,
        reset_button,
        export_button,
        inspector,
    }
}

fn panel(title: &str) -> GtkBox {
    let container = GtkBox::new(Orientation::Vertical, 8);
    container.add_css_class("panel");
    let label = Label::new(Some(title));
    label.add_css_class("section-title");
    label.set_xalign(0.0);
    container.append(&label);
    container
}

fn sample_text(sample: &CreditsSample) -> String {
    sample
        .blocks
        .iter()
        .map(|block| block.hard_lines.join("\n"))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn control_row(label: &str, widget: &impl IsA<gtk::Widget>) -> GtkBox {
    let row = GtkBox::new(Orientation::Vertical, 3);
    let caption = Label::new(Some(label));
    caption.add_css_class("hint");
    caption.set_xalign(0.0);
    row.append(&caption);
    row.append(widget);
    row
}

fn slider(min: f64, max: f64, step: f64, value: f64) -> Scale {
    let scale = Scale::with_range(Orientation::Horizontal, min, max, step);
    scale.set_value(value);
    scale.set_digits(if step < 1.0 { 2 } else { 0 });
    scale.set_draw_value(true);
    scale.set_value_pos(gtk::PositionType::Right);
    scale.set_hexpand(true);
    scale
}

fn wire_controls(
    sidebar: Sidebar,
    state: Rc<RefCell<UiState>>,
    transport: Rc<RefCell<Transport>>,
    area: DrawingArea,
) {
    let inspector = sidebar.inspector.clone();

    {
        let state = state.clone();
        let transport = transport.clone();
        let area = area.clone();
        let inspector = inspector.clone();
        let copy_view = sidebar.copy_view.clone();
        sidebar.sample_dropdown.connect_selected_notify(move |dropdown| {
            let index = (dropdown.selected() as usize).min(2);
            let text = {
                let mut state = state.borrow_mut();
                state.sample_index = index;
                state.custom_sample = None;
                state.playhead = 0.0;
                state.playing = false;
                if let Err(error) = state.reload_sample() {
                    inspector.status.set_text(&format!("布局失败: {error}"));
                    return;
                }
                sample_text(&state.sample())
            };
            copy_view.buffer().set_text(&text);
            let mut transport = transport.borrow_mut();
            transport.pause();
            transport.seek(0.0);
            drop(transport);
            sync_after_change(&state, &area, &inspector);
        });
    }

    {
        let state = state.clone();
        let transport = transport.clone();
        let area = area.clone();
        let inspector = inspector.clone();
        let copy_view = sidebar.copy_view.clone();
        sidebar.apply_text_button.connect_clicked(move |_| {
            let buffer = copy_view.buffer();
            let text = buffer
                .text(&buffer.start_iter(), &buffer.end_iter(), true)
                .to_string();
            let result = state.borrow_mut().apply_text(&text);
            if let Err(error) = result {
                inspector.status.set_text(&format!("文本排版失败: {error:#}"));
                return;
            }
            let position = state.borrow().playhead;
            {
                let mut transport = transport.borrow_mut();
                transport.pause();
                transport.seek(position);
            }
            sync_after_change(&state, &area, &inspector);
        });
    }

    {
        let state = state.clone();
        let transport = transport.clone();
        let area = area.clone();
        let inspector = inspector.clone();
        sidebar.aspect_dropdown.connect_selected_notify(move |dropdown| {
            let index = (dropdown.selected() as usize).min(ASPECTS.len() - 1);
            let result = {
                let mut state = state.borrow_mut();
                state.config.aspect = ASPECTS[index];
                state.rebuild_preserving_anchor()
            };
            finish_layout_change(result, &state, &transport, &area, &inspector);
        });
    }

    {
        let state = state.clone();
        let transport = transport.clone();
        let area = area.clone();
        let inspector = inspector.clone();
        sidebar.font_dropdown.connect_selected_notify(move |dropdown| {
            const FONTS: [&str; 4] = [
                "Noto Sans CJK SC",
                "Droid Sans Fallback",
                "DejaVu Sans",
                "Noto Sans Mono",
            ];
            let font = FONTS[(dropdown.selected() as usize).min(FONTS.len() - 1)];
            let result = {
                let mut state = state.borrow_mut();
                state.config.font_family = font.to_string();
                state.rebuild_preserving_anchor()
            };
            finish_layout_change(result, &state, &transport, &area, &inspector);
        });
    }

    {
        let state = state.clone();
        let transport = transport.clone();
        let area = area.clone();
        let inspector = inspector.clone();
        sidebar.size_scale.connect_value_changed(move |scale| {
            let result = {
                let mut state = state.borrow_mut();
                state.config.scaled_fonts(scale.value());
                state.rebuild_preserving_anchor()
            };
            finish_layout_change(result, &state, &transport, &area, &inspector);
        });
    }

    {
        let state = state.clone();
        let transport = transport.clone();
        let area = area.clone();
        let inspector = inspector.clone();
        sidebar.gap_scale.connect_value_changed(move |scale| {
            let result = {
                let mut state = state.borrow_mut();
                state.config.line_gap_factor = scale.value();
                state.rebuild_preserving_anchor()
            };
            finish_layout_change(result, &state, &transport, &area, &inspector);
        });
    }

    {
        let state = state.clone();
        let transport = transport.clone();
        let area = area.clone();
        let inspector = inspector.clone();
        sidebar.speed_scale.connect_value_changed(move |scale| {
            let result = {
                let mut state = state.borrow_mut();
                state.config.speed_px_per_second = scale.value();
                state.rebuild_preserving_anchor()
            };
            finish_layout_change(result, &state, &transport, &area, &inspector);
        });
    }

    {
        let state = state.clone();
        let area = area.clone();
        sidebar.debug_switch.connect_active_notify(move |switch| {
            state.borrow_mut().debug = switch.is_active();
            area.queue_draw();
        });
    }

    {
        let state = state.clone();
        let transport = transport.clone();
        let inspector = inspector.clone();
        sidebar.play_button.connect_clicked(move |_| {
            let (playing, duration, position) = {
                let state = state.borrow();
                (state.playing, state.layout.duration, state.playhead)
            };
            if playing {
                state.borrow_mut().playing = false;
                transport.borrow_mut().pause();
            } else {
                let position = if position >= duration - 0.001 {
                    transport.borrow_mut().seek(0.0);
                    0.0
                } else {
                    position
                };
                transport.borrow_mut().seek(position);
                {
                    let mut state = state.borrow_mut();
                    state.playhead = position;
                    state.playing = true;
                }
                transport.borrow_mut().play();
            }
            inspector.play_button.set_label(if playing {
                "▶ 播放"
            } else {
                "⏸ 暂停"
            });
        });
    }

    {
        let state = state.clone();
        let transport = transport.clone();
        let area = area.clone();
        let inspector = inspector.clone();
        sidebar.reset_button.connect_clicked(move |_| {
            {
                let mut state = state.borrow_mut();
                state.playhead = 0.0;
                state.playing = false;
            }
            let mut transport = transport.borrow_mut();
            transport.pause();
            transport.seek(0.0);
            drop(transport);
            sync_after_change(&state, &area, &inspector);
        });
    }

    {
        let state = state.clone();
        let transport = transport.clone();
        let guard = inspector.timeline_guard.clone();
        let timeline = inspector.timeline.clone();
        timeline.connect_value_changed(move |scale| {
            if guard.get() {
                return;
            }
            let mut state = state.borrow_mut();
            state.playhead = scale.value() / 1000.0 * state.layout.duration;
            let position = state.playhead;
            drop(state);
            transport.borrow_mut().seek(position);
        });
    }

    {
        let state = state.clone();
        let inspector = inspector.clone();
        sidebar.export_button.connect_clicked(move |_| {
            let (sample, config, time) = {
                let state = state.borrow();
                (state.sample(), state.config.clone(), state.playhead)
            };
            inspector.status.set_text("正在渲染 GStreamer H.264 预览…");
            match export_bundle(&sample, &config, time) {
                Ok(paths) => inspector.status.set_text(&format!(
                    "已导出 {}\n{}",
                    paths.0.display(),
                    paths.1.join(" · ")
                )),
                Err(error) => inspector.status.set_text(&format!("导出失败: {error:#}")),
            }
        });
    }

    refresh_inspector(&state.borrow(), &inspector);
}

fn finish_layout_change(
    result: Result<()>,
    state: &Rc<RefCell<UiState>>,
    transport: &Rc<RefCell<Transport>>,
    area: &DrawingArea,
    inspector: &Inspector,
) {
    if let Err(error) = result {
        inspector.status.set_text(&format!("重排失败: {error:#}"));
        return;
    }
    let position = state.borrow().playhead;
    transport.borrow_mut().seek(position);
    sync_after_change(state, area, inspector);
}

fn sync_after_change(
    state: &Rc<RefCell<UiState>>,
    area: &DrawingArea,
    inspector: &Inspector,
) {
    area.queue_draw();
    refresh_inspector(&state.borrow(), inspector);
}

fn refresh_inspector(state: &UiState, inspector: &Inspector) {
    let layout = &state.layout;
    let visible_lines: Vec<_> = layout
        .lines
        .iter()
        .filter_map(|line| {
            let rect = layout.screen_rect(line, state.playhead);
            if rect.bottom() < layout.safe_area.y || rect.y > layout.safe_area.bottom() {
                None
            } else {
                Some((line, rect, layout.is_fully_safe(line, state.playhead)))
            }
        })
        .collect();
    let fully_safe = visible_lines.iter().filter(|(_, _, safe)| *safe).count();
    inspector.metrics.set_text(&format!(
        "画幅 {}×{} · 安全区 {:.0}×{:.0}\n{} 视觉行 · 内容 {:.0} px · {:.2} s",
        layout.canvas.width,
        layout.canvas.height,
        layout.safe_area.width,
        layout.safe_area.height,
        layout.lines.len(),
        layout.content_height,
        layout.duration
    ));
    inspector.anchor.set_text(&state.anchor_note);
    inspector.visible.set_text(&format!(
        "可见 {} 行 · 完全在安全区 {} 行 · 播放 {:.2}s",
        visible_lines.len(),
        fully_safe,
        state.playhead
    ));
    let violations = report::layout_violations(layout);
    let horizontal = violations
        .iter()
        .filter(|record| record.kind == "HorizontalOverflow")
        .count();
    let source_wrap = violations
        .iter()
        .filter(|record| record.kind == "SourceWrapOverflow")
        .count();
    inspector.overflow.set_text(&format!(
        "越界记录 {} · 横向 {} · 来源折行 {}\n速度 {:.0} px/s · GStreamer 时钟",
        violations.len(),
        horizontal,
        source_wrap,
        layout.config.speed_px_per_second
    ));

    while let Some(child) = inspector.line_list.first_child() {
        inspector.line_list.remove(&child);
    }
    for (line, rect, safe) in visible_lines.iter().take(9) {
        let baseline = layout.screen_baseline(line, state.playhead);
        let state_text = if *safe { "SAFE" } else { "CLIP" };
        let label = Label::new(Some(&format!(
            "{:<8} baseline {:7.1}  {}  w{:5.0}",
            line.line_id, baseline, state_text, rect.width
        )));
        label.add_css_class(if *safe { "metric-strong" } else { "metric" });
        label.set_xalign(0.0);
        let row = gtk::ListBoxRow::new();
        row.set_activatable(false);
        row.set_child(Some(&label));
        inspector.line_list.append(&row);
    }
    inspector.play_button.set_label(if state.playing {
        "⏸ 暂停"
    } else {
        "▶ 播放"
    });
    inspector.timeline_guard.set(true);
    let ratio = if layout.duration > 0.0 {
        (state.playhead / layout.duration).clamp(0.0, 1.0)
    } else {
        0.0
    };
    inspector.timeline.set_value(ratio * 1000.0);
    inspector.timeline_guard.set(false);
}

fn showcase_time(layout: &CreditsLayout) -> f64 {
    let Some(first) = layout.lines.first() else {
        return 0.0;
    };
    let target = layout.safe_area.y + layout.safe_area.height * 0.5;
    ((layout.content_origin_y() + first.baseline_y - target)
        / layout.config.speed_px_per_second.max(1.0))
    .clamp(0.0, layout.duration)
}

fn export_bundle(
    sample: &CreditsSample,
    config: &LayoutConfig,
    time: f64,
) -> Result<(PathBuf, Vec<String>)> {
    let dir = PathBuf::from("exports");
    fs::create_dir_all(&dir)?;
    let tag = format!(
        "{}-{}-font{:.2}",
        sample.id,
        config.aspect.label.replace(':', "x"),
        config.font_scale()
    );
    let layout = layout::build(sample, config)?;
    let report_path = dir.join(format!("{tag}.report.json"));
    report::write_report(&report_path, sample, config)?;
    let params_path = dir.join(format!("{tag}.params.json"));
    let params = json!({
        "sample": sample.id,
        "config": config,
        "canvas": layout.canvas,
        "safe_area": layout.safe_area,
        "content_height": layout.content_height,
        "line_count": layout.lines.len(),
        "duration_seconds": layout.duration,
        "exported_playhead": time,
        "engine": {"layout": "Pango/GTK4", "clock": "GStreamer", "language": "Rust"}
    });
    fs::write(&params_path, serde_json::to_vec_pretty(&params)?)?;
    let png_path = dir.join(format!("{tag}.png"));
    render::export_png(&png_path, &layout, time, true)?;
    let video_path = dir.join(format!("{tag}.mp4"));
    gst::export_video(&video_path, &layout, 1280, 720, 30)?;
    Ok((
        report_path,
        vec![
            params_path.display().to_string(),
            png_path.display().to_string(),
            video_path.display().to_string(),
        ],
    ))
}

fn start_tick_loop(
    state: Rc<RefCell<UiState>>,
    transport: Rc<RefCell<Transport>>,
    area: DrawingArea,
    inspector: Inspector,
) {
    let last_refresh = Rc::new(Cell::new(Instant::now() - Duration::from_secs(1)));
    gtk::glib::timeout_add_local(Duration::from_millis(16), move || {
        let (playing, duration) = {
            let state = state.borrow();
            (state.playing, state.layout.duration)
        };
        if playing {
            let position = transport.borrow().position();
            if position >= duration {
                {
                    let mut state = state.borrow_mut();
                    state.playhead = duration;
                    state.playing = false;
                }
                transport.borrow_mut().pause();
            } else {
                state.borrow_mut().playhead = position;
            }
        }
        area.queue_draw();
        if last_refresh.get().elapsed() >= Duration::from_millis(100) {
            last_refresh.set(Instant::now());
            refresh_inspector(&state.borrow(), &inspector);
        }
        gtk::glib::ControlFlow::Continue
    });
}

fn schedule_snapshot(window: &ApplicationWindow, app: Application) {
    let Ok(path) = std::env::var("CREDITS_SNAPSHOT") else {
        return;
    };
    let window = window.clone();
    gtk::glib::timeout_add_local_once(Duration::from_millis(1200), move || {
        let width = window.width();
        let height = window.height();
        let paintable = gtk::WidgetPaintable::new(Some(&window));
        let snapshot = gtk::Snapshot::new();
        paintable.snapshot(&snapshot, width as f64, height as f64);
        let result = snapshot
            .to_node()
            .and_then(|node| {
                window.renderer().map(|renderer| {
                    let viewport = gtk::graphene::Rect::new(
                        0.0,
                        0.0,
                        width as f32,
                        height as f32,
                    );
                    renderer.render_texture(&node, Some(&viewport))
                })
            })
            .ok_or_else(|| "window has no renderer".to_string())
            .and_then(|texture| {
                texture
                    .save_to_png(&path)
                    .map_err(|error| error.to_string())
            });
        match result {
            Ok(()) => eprintln!("snapshot written to {path}"),
            Err(error) => eprintln!("snapshot failed: {error}"),
        }
        app.quit();
    });
}
