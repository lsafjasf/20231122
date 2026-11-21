//! GTK4 界面：媒体预览 + 安全区叠加 + 片段审查面板。
use subtitle_safe_review::analysis::{self, Analyzer};
use subtitle_safe_review::model::{ExtremeParams, Segment};
use subtitle_safe_review::session::Project;
use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{
    Application, ApplicationWindow, Box as GtkBox, Button, DrawingArea, FileDialog, HeaderBar,
    Label, ListBox, ListBoxRow, Orientation, Overlay, Paned, Picture, Scale, ScrolledWindow,
    SpinButton, TextView,
};
use gstreamer as gst;
use gstreamer::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

const APP_ID: &str = "dev.local.SubtitleSafeReview";

struct AppState {
    project: Option<Project>,
    analyzer: Option<Analyzer>,
    playbin: Option<gst::Element>,
    selected: Option<usize>,
    playing: bool,
}

impl AppState {
    fn new() -> Self {
        Self {
            project: None,
            analyzer: None,
            playbin: None,
            selected: None,
            playing: false,
        }
    }
}

#[derive(Clone)]
struct Widgets {
    window: ApplicationWindow,
    status: Label,
    picture: Picture,
    overlay_area: DrawingArea,
    play_button: Button,
    seek: Scale,
    segment_list: ListBox,
    output: TextView,
    spin_left: SpinButton,
    spin_right: SpinButton,
    spin_top: SpinButton,
    spin_bottom: SpinButton,
    spin_luma: SpinButton,
    spin_violation: SpinButton,
    spin_eps: SpinButton,
}

pub fn run() -> glib::ExitCode {
    let app = Application::new(Some(APP_ID), Default::default());
    app.connect_activate(build_ui);
    app.run()
}

fn build_ui(app: &Application) {
    let state = Rc::new(RefCell::new(AppState::new()));

    let window = ApplicationWindow::new(app);
    window.set_title(Some("字幕安全区审查 · 局部修正"));
    window.set_default_size(1280, 760);

    // 头部：项目与媒体操作
    let header = HeaderBar::new();
    let btn_open_project = Button::with_label("打开/新建项目");
    let btn_open_media = Button::with_label("打开媒体");
    let btn_save = Button::with_label("保存会话");
    header.pack_start(&btn_open_project);
    header.pack_start(&btn_open_media);
    header.pack_end(&btn_save);
    window.set_titlebar(Some(&header));

    // 左侧：视频预览 + 安全区叠加 + 播放控制
    let picture = Picture::new();
    picture.set_content_fit(gtk4::ContentFit::Contain);
    picture.set_hexpand(true);
    picture.set_vexpand(true);
    let overlay_area = DrawingArea::new();
    overlay_area.set_hexpand(true);
    overlay_area.set_vexpand(true);
    let overlay = Overlay::new();
    overlay.set_child(Some(&picture));
    overlay.add_overlay(&overlay_area);

    let play_button = Button::with_label("播放");
    let seek = Scale::with_range(Orientation::Horizontal, 0.0, 1000.0, 1.0);
    seek.set_hexpand(true);
    let transport = GtkBox::new(Orientation::Horizontal, 6);
    transport.append(&play_button);
    transport.append(&seek);

    let left = GtkBox::new(Orientation::Vertical, 6);
    left.append(&overlay);
    left.append(&transport);

    // 右侧：片段列表 + 参数 + 动作 + 输出
    let segment_list = ListBox::new();
    segment_list.set_selection_mode(gtk4::SelectionMode::Single);
    let seg_scroll = ScrolledWindow::new();
    seg_scroll.set_child(Some(&segment_list));
    seg_scroll.set_size_request(-1, 140);

    let output = TextView::new();
    output.set_editable(false);
    output.set_monospace(true);
    let out_scroll = ScrolledWindow::new();
    out_scroll.set_child(Some(&output));
    out_scroll.set_vexpand(true);

    let mk_spin = |min: f64, max: f64, step: f64| {
        SpinButton::with_range(min, max, step)
    };
    let spin_left = mk_spin(0.0, 0.5, 0.01);
    let spin_right = mk_spin(0.0, 0.5, 0.01);
    let spin_top = mk_spin(0.0, 0.5, 0.01);
    let spin_bottom = mk_spin(0.0, 0.5, 0.01);
    let spin_luma = mk_spin(0.0, 255.0, 1.0);
    let spin_violation = mk_spin(0.0, 1.0, 0.001);
    spin_violation.set_digits(3);
    let spin_eps = mk_spin(0.0, 1000.0, 10.0);

    let status = Label::new(Some("就绪：请先打开或新建项目目录"));
    status.set_xalign(0.0);

    let widgets = Widgets {
        window: window.clone(),
        status,
        picture,
        overlay_area,
        play_button,
        seek,
        segment_list,
        output,
        spin_left,
        spin_right,
        spin_top,
        spin_bottom,
        spin_luma,
        spin_violation,
        spin_eps,
    };

    let right = build_side_panel(&widgets, &state);

    let paned = Paned::new(Orientation::Horizontal);
    paned.set_start_child(Some(&left));
    paned.set_end_child(Some(&right));
    paned.set_position(820);
    paned.set_vexpand(true);

    let root = GtkBox::new(Orientation::Vertical, 4);
    root.append(&paned);
    root.append(&widgets.status);
    window.set_child(Some(&root));

    wire_actions(&widgets, &state, &btn_open_project, &btn_open_media, &btn_save);
    window.present();
}

fn build_side_panel(w: &Widgets, state: &Rc<RefCell<AppState>>) -> GtkBox {
    let panel = GtkBox::new(Orientation::Vertical, 6);
    panel.set_margin_start(8);
    panel.set_margin_end(8);
    panel.set_margin_top(8);
    panel.set_margin_bottom(8);
    panel.set_size_request(400, -1);

    // 片段工具条
    let seg_bar = GtkBox::new(Orientation::Horizontal, 4);
    let btn_add = Button::with_label("新增缺失片段");
    let btn_del = Button::with_label("删除片段");
    let btn_in = Button::with_label("设为入点");
    let btn_out = Button::with_label("设为出点");
    seg_bar.append(&btn_add);
    seg_bar.append(&btn_del);
    seg_bar.append(&btn_in);
    seg_bar.append(&btn_out);
    panel.append(&Label::new(Some("媒体片段（可单独重跑/比较/导出）")));
    panel.append(&seg_bar);

    let seg_scroll = ScrolledWindow::new();
    seg_scroll.set_child(Some(&w.segment_list));
    seg_scroll.set_size_request(-1, 130);
    panel.append(&seg_scroll);

    // 极限参数表单
    panel.append(&Label::new(Some("极限参数（作用于选中片段）")));
    let grid = gtk4::Grid::new();
    grid.set_row_spacing(4);
    grid.set_column_spacing(6);
    let rows: [(&str, &SpinButton); 7] = [
        ("左内缩", &w.spin_left),
        ("右内缩", &w.spin_right),
        ("上内缩", &w.spin_top),
        ("下内缩", &w.spin_bottom),
        ("亮度阈值", &w.spin_luma),
        ("违规占比", &w.spin_violation),
        ("边界偏移ms", &w.spin_eps),
    ];
    for (i, (label, spin)) in rows.iter().enumerate() {
        let l = Label::new(Some(label));
        l.set_xalign(0.0);
        grid.attach(&l, 0, i as i32, 1, 1);
        grid.attach(*spin, 1, i as i32, 1, 1);
    }
    panel.append(&grid);

    let preset_bar = GtkBox::new(Orientation::Horizontal, 4);
    let btn_tight = Button::with_label("极限收紧");
    let btn_loose = Button::with_label("极限放宽");
    preset_bar.append(&btn_tight);
    preset_bar.append(&btn_loose);
    panel.append(&preset_bar);

    // 动作
    let act_bar = GtkBox::new(Orientation::Horizontal, 4);
    let btn_run = Button::with_label("重跑本段");
    let btn_cmp = Button::with_label("比较上次");
    let btn_export = Button::with_label("导出本段");
    act_bar.append(&btn_run);
    act_bar.append(&btn_cmp);
    act_bar.append(&btn_export);
    panel.append(&act_bar);

    panel.append(&Label::new(Some("结果 / 比较 / 证据")));
    let out_scroll = ScrolledWindow::new();
    out_scroll.set_child(Some(&w.output));
    out_scroll.set_vexpand(true);
    panel.append(&out_scroll);

    wire_segment_actions(state, w, &btn_add, &btn_del, &btn_in, &btn_out);
    wire_param_actions(state, w, &btn_tight, &btn_loose);
    wire_run_actions(state, w, &btn_run, &btn_cmp, &btn_export);
    panel
}

// ---------- 通用辅助 ----------

fn set_status(w: &Widgets, msg: &str) {
    w.status.set_text(msg);
}

fn print_out(w: &Widgets, text: &str) {
    let buf = w.output.buffer();
    buf.set_text(text);
}

fn save_project(state: &Rc<RefCell<AppState>>, w: &Widgets) {
    let s = state.borrow();
    if let Some(p) = &s.project {
        match p.save() {
            Ok(()) => set_status(w, "会话已保存（可随时退出，重进自动恢复）"),
            Err(e) => set_status(w, &format!("保存失败: {e:#}")),
        }
    }
}

fn refresh_segments(w: &Widgets, state: &Rc<RefCell<AppState>>) {
    while let Some(child) = w.segment_list.first_child() {
        w.segment_list.remove(&child);
    }
    let s = state.borrow();
    if let Some(p) = &s.project {
        for seg in &p.session.segments {
            let runs = seg.runs.len();
            let row = ListBoxRow::new();
            let label = Label::new(Some(&format!(
                "{}  [{:.2}s - {:.2}s]  运行{}次",
                seg.label,
                seg.start_ms as f64 / 1000.0,
                seg.end_ms as f64 / 1000.0,
                runs
            )));
            label.set_xalign(0.0);
            row.set_child(Some(&label));
            w.segment_list.append(&row);
        }
    }
}

fn current_position_ms(state: &Rc<RefCell<AppState>>) -> u64 {
    state
        .borrow()
        .playbin
        .as_ref()
        .and_then(|p| p.query_position::<gst::ClockTime>())
        .map(|t| t.mseconds())
        .unwrap_or(0)
}

fn read_params(w: &Widgets) -> ExtremeParams {
    ExtremeParams {
        inset_left: w.spin_left.value(),
        inset_right: w.spin_right.value(),
        inset_top: w.spin_top.value(),
        inset_bottom: w.spin_bottom.value(),
        luma_threshold: w.spin_luma.value() as u8,
        violation_ratio: w.spin_violation.value(),
        boundary_epsilon_ms: w.spin_eps.value() as u64,
    }
}

fn show_params(w: &Widgets, p: &ExtremeParams) {
    w.spin_left.set_value(p.inset_left);
    w.spin_right.set_value(p.inset_right);
    w.spin_top.set_value(p.inset_top);
    w.spin_bottom.set_value(p.inset_bottom);
    w.spin_luma.set_value(p.luma_threshold as f64);
    w.spin_violation.set_value(p.violation_ratio);
    w.spin_eps.set_value(p.boundary_epsilon_ms as f64);
}

fn selected_segment(state: &Rc<RefCell<AppState>>) -> Option<Segment> {
    let s = state.borrow();
    let idx = s.selected?;
    s.project
        .as_ref()?
        .session
        .segments
        .get(idx)
        .cloned()
}

fn mutate_selected<R>(
    state: &Rc<RefCell<AppState>>,
    f: impl FnOnce(&mut Segment) -> R,
) -> Option<R> {
    let mut s = state.borrow_mut();
    let idx = s.selected?;
    let project = s.project.as_mut()?;
    let seg = project.session.segments.get_mut(idx)?;
    Some(f(seg))
}

// ---------- 头部与播放 ----------

fn open_media_into(w: &Widgets, state: &Rc<RefCell<AppState>>, uri: &str) {
    // 预览管线
    let playbin = gst::ElementFactory::make("playbin")
        .property("uri", uri)
        .build()
        .expect("无法创建 playbin");
    if let Ok(sink) = gst::ElementFactory::make("gtk4paintablesink").build() {
        let paintable: gdk::Paintable = sink.property("paintable");
        w.picture.set_paintable(Some(&paintable));
        playbin.set_property("video-sink", &sink);
    } else {
        set_status(w, "警告: 缺少 gtk4paintablesink，仅分析可用");
    }
    let _ = playbin.set_state(gst::State::Paused);

    // 分析器
    match Analyzer::new(uri) {
        Ok(an) => {
            let dur = an.duration_ms();
            w.seek.set_range(0.0, dur as f64);
            state.borrow_mut().analyzer = Some(an);
            set_status(w, &format!("媒体已加载，时长 {:.1}s", dur as f64 / 1000.0));
        }
        Err(e) => set_status(w, &format!("分析器初始化失败: {e:#}")),
    }
    state.borrow_mut().playbin = Some(playbin);
}

fn wire_actions(
    w: &Widgets,
    state: &Rc<RefCell<AppState>>,
    btn_open_project: &Button,
    btn_open_media: &Button,
    btn_save: &Button,
) {
    // 打开/新建项目
    {
        let (w, state) = (w.clone(), state.clone());
        btn_open_project.connect_clicked(move |_| {
            let dialog = FileDialog::new();
            dialog.set_title("选择项目目录（不存在可先创建）");
            let (w, state) = (w.clone(), state.clone());
            let win = w.window.clone();
            dialog.select_folder(Some(&win), None::<&gtk4::gio::Cancellable>, move |res| {
                if let Ok(folder) = res {
                    if let Some(path) = folder.path() {
                        match Project::open_or_create(&path) {
                            Ok(p) => {
                                let media = p.session.media_uri.clone();
                                let n = p.session.segments.len();
                                state.borrow_mut().project = Some(p);
                                refresh_segments(&w, &state);
                                set_status(&w, &format!("项目已载入：{n} 个片段（重进恢复成功）"));
                                if let Some(uri) = media {
                                    open_media_into(&w, &state, &uri);
                                }
                            }
                            Err(e) => set_status(&w, &format!("打开项目失败: {e:#}")),
                        }
                    }
                }
            });
        });
    }

    // 打开媒体文件
    {
        let (w, state) = (w.clone(), state.clone());
        btn_open_media.connect_clicked(move |_| {
            let dialog = FileDialog::new();
            dialog.set_title("选择媒体文件");
            let (w, state) = (w.clone(), state.clone());
            let win = w.window.clone();
            dialog.open(Some(&win), None::<&gtk4::gio::Cancellable>, move |res| {
                if let Ok(file) = res {
                    let uri = file.uri().to_string();
                    {
                        let mut s = state.borrow_mut();
                        if let Some(p) = &mut s.project {
                            p.session.media_uri = Some(uri.clone());
                        }
                    }
                    open_media_into(&w, &state, &uri);
                    save_project(&state, &w);
                }
            });
        });
    }

    // 手动保存
    {
        let (w, state) = (w.clone(), state.clone());
        btn_save.connect_clicked(move |_| save_project(&state, &w));
    }

    // 打开/新建项目目录
    {
        let (w, state) = (w.clone(), state.clone());
        // 通过窗口标题栏按钮重建：从 header 查找不便，直接绑定在 window 上
        w.window.connect_close_request(move |win| {
            // 中途退出：尽力保存会话
            let s = state.borrow();
            if let Some(p) = &s.project {
                let _ = p.save();
            }
            if let Some(pb) = &s.playbin {
                let _ = pb.set_state(gst::State::Null);
            }
            drop(s);
            win.set_visible(false);
            glib::Propagation::Proceed
        });
    }

    // 片段选择
    {
        let (w, state) = (w.clone(), state.clone());
        let wl = w.clone();
        w.segment_list.connect_row_selected(move |_, row| {
            if let Some(row) = row {
                let idx = row.index() as usize;
                state.borrow_mut().selected = Some(idx);
                if let Some(seg) = selected_segment(&state) {
                    show_params(&wl, &seg.params);
                    wl.overlay_area.queue_draw();
                }
            }
        });
    }

    // 安全区叠加绘制
    {
        let state = state.clone();
        w.overlay_area.set_draw_func(move |_area, cr, width, height| {
            let Some(seg) = selected_segment(&state) else {
                return;
            };
            let p = &seg.params;
            let (wdt, hgt) = (width as f64, height as f64);
            let x = p.inset_left * wdt;
            let y = p.inset_top * hgt;
            let sw = wdt * (1.0 - p.inset_left - p.inset_right);
            let sh = hgt * (1.0 - p.inset_top - p.inset_bottom);
            cr.set_source_rgba(0.1, 0.9, 0.3, 0.9);
            cr.set_line_width(2.0);
            cr.rectangle(x, y, sw, sh);
            let _ = cr.stroke();
            cr.set_source_rgba(1.0, 0.2, 0.2, 0.25);
            cr.rectangle(0.0, 0.0, wdt, y);
            let _ = cr.fill();
            cr.rectangle(0.0, y + sh, wdt, hgt - y - sh);
            let _ = cr.fill();
        });
    }

    // 播放/暂停
    {
        let (w, state) = (w.clone(), state.clone());
        let _ = &w;
        w.play_button.connect_clicked(move |btn| {
            let mut s = state.borrow_mut();
            if let Some(pb) = &s.playbin {
                if s.playing {
                    let _ = pb.set_state(gst::State::Paused);
                    btn.set_label("播放");
                } else {
                    let _ = pb.set_state(gst::State::Playing);
                    btn.set_label("暂停");
                }
                s.playing = !s.playing;
            }
        });
    }

    // 拖动进度
    {
        let state = state.clone();
        w.seek.connect_change_value(move |_, _, value| {
            if let Some(pb) = &state.borrow().playbin {
                let _ = pb.seek_simple(
                    gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT,
                    gst::ClockTime::from_mseconds(value as u64),
                );
            }
            glib::Propagation::Proceed
        });
    }

    // 位置刷新
    {
        let w = w.clone();
        let state = state.clone();
        glib::timeout_add_local(Duration::from_millis(250), move || {
            let pos = current_position_ms(&state);
            if !w.seek.has_focus() {
                w.seek.set_value(pos as f64);
            }
            glib::ControlFlow::Continue
        });
    }
}

// ---------- 片段管理 ----------

fn wire_segment_actions(
    state: &Rc<RefCell<AppState>>,
    w: &Widgets,
    btn_add: &Button,
    btn_del: &Button,
    btn_in: &Button,
    btn_out: &Button,
) {
    {
        let (w, state) = (w.clone(), state.clone());
        btn_add.connect_clicked(move |_| {
            let pos = current_position_ms(&state);
            let mut s = state.borrow_mut();
            let Some(p) = &mut s.project else {
                set_status(&w, "请先打开项目");
                return;
            };
            let n = p.session.segments.len() + 1;
            let seg = Segment::new(
                format!("seg{n:03}"),
                format!("缺失片段{n}"),
                pos,
                pos + 2000,
            );
            p.session.segments.push(seg);
            drop(s);
            refresh_segments(&w, &state);
            save_project(&state, &w);
        });
    }
    {
        let (w, state) = (w.clone(), state.clone());
        btn_del.connect_clicked(move |_| {
            let mut s = state.borrow_mut();
            let Some(idx) = s.selected else { return };
            if let Some(p) = &mut s.project {
                if idx < p.session.segments.len() {
                    p.session.segments.remove(idx);
                }
            }
            s.selected = None;
            drop(s);
            refresh_segments(&w, &state);
            save_project(&state, &w);
        });
    }
    for (btn, is_in) in [(btn_in, true), (btn_out, false)] {
        let (w, state) = (w.clone(), state.clone());
        btn.connect_clicked(move |_| {
            let pos = current_position_ms(&state);
            mutate_selected(&state, |seg| {
                if is_in {
                    seg.start_ms = pos.min(seg.end_ms);
                } else {
                    seg.end_ms = pos.max(seg.start_ms);
                }
            });
            refresh_segments(&w, &state);
            save_project(&state, &w);
        });
    }
}

// ---------- 极限参数 ----------

fn wire_param_actions(
    state: &Rc<RefCell<AppState>>,
    w: &Widgets,
    btn_tight: &Button,
    btn_loose: &Button,
) {
    for (btn, params) in [
        (btn_tight, ExtremeParams::tightened()),
        (btn_loose, ExtremeParams::loosened()),
    ] {
        let (w, state) = (w.clone(), state.clone());
        btn.connect_clicked(move |_| {
            show_params(&w, &params);
            mutate_selected(&state, |seg| seg.params = params.clone());
            w.overlay_area.queue_draw();
            save_project(&state, &w);
        });
    }
    // 参数编辑实时写回选中片段
    let spins = [
        &w.spin_left,
        &w.spin_right,
        &w.spin_top,
        &w.spin_bottom,
        &w.spin_luma,
        &w.spin_violation,
        &w.spin_eps,
    ];
    for spin in spins {
        let (w, state) = (w.clone(), state.clone());
        spin.connect_value_changed(move |_| {
            let p = read_params(&w);
            mutate_selected(&state, |seg| seg.params = p);
            w.overlay_area.queue_draw();
        });
    }
}

// ---------- 重跑 / 比较 / 导出 ----------

fn format_run(seg: &Segment) -> String {
    let mut out = String::new();
    let Some(run) = seg.latest_run() else {
        return "尚无运行结果".to_string();
    };
    out.push_str(&format!(
        "片段 {}  最新运行\n  扫描帧数: {}  违规帧: {} ({:.2}%)\n  最大墨迹占比: {:.4}  平均: {:.4}\n",
        seg.label,
        run.frames_scanned,
        run.violating_frames,
        run.violation_ratio() * 100.0,
        run.max_ink_ratio,
        run.mean_ink_ratio,
    ));
    for (tag, pair) in [("入点", &run.entry_boundary), ("出点", &run.exit_boundary)] {
        if let Some(bp) = pair {
            out.push_str(&format!(
                "  {tag}边界 {}ms: 前 {:.4} → 后 {:.4}  (Δ{:+.4})\n    证据: {} / {}\n",
                bp.boundary_ms,
                bp.before.ink_ratio,
                bp.after.ink_ratio,
                bp.ink_ratio_delta,
                bp.before.snapshot,
                bp.after.snapshot,
            ));
        }
    }
    out
}

fn format_compare(seg: &Segment) -> String {
    let (Some(prev), Some(latest)) = (seg.previous_run(), seg.latest_run()) else {
        return "需要至少两次运行才能比较".to_string();
    };
    let mut out = format!("片段 {}  本次 vs 上次\n", seg.label);
    out.push_str(&format!(
        "  违规帧率: {:.2}% → {:.2}%  (Δ{:+.2}%)\n",
        prev.violation_ratio() * 100.0,
        latest.violation_ratio() * 100.0,
        (latest.violation_ratio() - prev.violation_ratio()) * 100.0,
    ));
    out.push_str(&format!(
        "  最大墨迹: {:.4} → {:.4}  (Δ{:+.4})\n",
        prev.max_ink_ratio,
        latest.max_ink_ratio,
        latest.max_ink_ratio - prev.max_ink_ratio,
    ));
    for (tag, a, b) in [
        ("入点", &prev.entry_boundary, &latest.entry_boundary),
        ("出点", &prev.exit_boundary, &latest.exit_boundary),
    ] {
        if let (Some(pa), Some(pb)) = (a, b) {
            out.push_str(&format!(
                "  {tag}边界Δ: {:+.4} → {:+.4}\n",
                pa.ink_ratio_delta, pb.ink_ratio_delta,
            ));
        }
    }
    out
}

fn wire_run_actions(
    state: &Rc<RefCell<AppState>>,
    w: &Widgets,
    btn_run: &Button,
    btn_cmp: &Button,
    btn_export: &Button,
) {
    // 重跑本段（阻塞式，单片段范围小可接受）
    {
        let (w, state) = (w.clone(), state.clone());
        btn_run.connect_clicked(move |_| {
            let Some(seg) = selected_segment(&state) else {
                set_status(&w, "请先选择片段");
                return;
            };
            set_status(&w, &format!("正在重跑 {} …", seg.label));
            let result = {
                let s = state.borrow();
                match (&s.project, &s.analyzer) {
                    (Some(p), Some(an)) => Some(analysis::run_segment(p, an, &seg).map(|r| {
                        let _ = p.append_run_log(&seg.id, &r);
                        r
                    })),
                    _ => None,
                }
            };
            match result {
                Some(Ok(run)) => {
                    mutate_selected(&state, |s| s.push_run(run));
                    save_project(&state, &w);
                    if let Some(seg) = selected_segment(&state) {
                        print_out(&w, &format_run(&seg));
                    }
                    set_status(&w, "重跑完成，证据已写入 evidence/");
                }
                Some(Err(e)) => set_status(&w, &format!("重跑失败: {e:#}")),
                None => set_status(&w, "请先打开项目与媒体"),
            }
        });
    }
    // 比较上次
    {
        let (w, state) = (w.clone(), state.clone());
        btn_cmp.connect_clicked(move |_| {
            if let Some(seg) = selected_segment(&state) {
                print_out(&w, &format_compare(&seg));
            }
        });
    }
    // 导出本段
    {
        let (w, state) = (w.clone(), state.clone());
        btn_export.connect_clicked(move |_| {
            let Some(seg) = selected_segment(&state) else {
                set_status(&w, "请先选择片段");
                return;
            };
            let dialog = FileDialog::new();
            dialog.set_title("选择导出目录");
            let (w, state) = (w.clone(), state.clone());
            let win = w.window.clone();
            dialog.select_folder(Some(&win), None::<&gtk4::gio::Cancellable>, move |res| {
                if let Ok(folder) = res {
                    if let Some(path) = folder.path() {
                        let s = state.borrow();
                        if let Some(p) = &s.project {
                            match p.export_segment(&seg.id, &path) {
                                Ok(out) => {
                                    set_status(&w, &format!("已导出到 {}", out.display()))
                                }
                                Err(e) => set_status(&w, &format!("导出失败: {e:#}")),
                            }
                        }
                    }
                }
            });
        });
    }
}
