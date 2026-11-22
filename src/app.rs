use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::Local;
use gtk::prelude::*;
use gtk4 as gtk;

use crate::audio;
use crate::demo;
use crate::model::{
    CorrectionRun, EvidenceSampleChange, PeakSample, Project, ProjectStore, Segment, SegmentStatus,
    format_time_ms,
};
use crate::ui::{self, Ui};

struct AppState {
    store: ProjectStore,
    project: RefCell<Project>,
    ui: Ui,
    waveform: RefCell<Vec<PeakSample>>,
    processing: Cell<bool>,
    syncing_params: Cell<bool>,
}

enum WorkerEvent {
    Started(String),
    Finished(String, Box<CorrectionRun>),
    Failed(String, String),
    AllDone,
}

struct RunSpec {
    id: String,
    index: usize,
    start_ms: u64,
    end_ms: u64,
    output: PathBuf,
}

pub fn activate(app: &gtk::Application) -> Result<()> {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(include_str!("../style.css"));
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }

    let store = ProjectStore::default();
    store.ensure_dirs()?;
    demo::ensure_demo_media(&store.demo_media_path())?;

    let project = match store.load() {
        Ok(Some(project)) => project,
        Ok(None) => Project::new_demo(store.demo_media_path()),
        Err(error) => {
            eprintln!("恢复会话失败，已创建新会话: {error:#}");
            Project::new_demo(store.demo_media_path())
        }
    };
    let mut project = project;
    if !Path::new(&project.media_path).exists() {
        project.media_path = store.demo_media_path().to_string_lossy().into_owned();
        project.add_evidence(
            "recovery",
            None,
            "原始媒体路径失效，已回退到内置演示素材；历史证据未删除。",
        );
    }
    let waveform = audio::capture_levels(Path::new(&project.media_path)).unwrap_or_default();
    store.save(&project)?;

    let ui = ui::build_window(app);
    ui.params.set_values(&project.params);
    let state = Rc::new(AppState {
        store,
        project: RefCell::new(project),
        ui,
        waveform: RefCell::new(waveform),
        processing: Cell::new(false),
        syncing_params: Cell::new(false),
    });

    let weak = Rc::downgrade(&state);
    state
        .ui
        .timeline
        .set_draw_func(move |area, context, width, height| {
            if let Some(state) = weak.upgrade() {
                state.draw_timeline(area, context, width, height);
            }
        });
    state.bind_events();
    state.save();
    state.refresh();
    state.ui.window.present();
    if std::env::var_os("PEAKGUARD_AUTORUN").is_some() {
        let state = state.clone();
        gtk::glib::timeout_add_local_once(Duration::from_millis(650), move || {
            state.run_all_segments();
        });
    }
    if let Ok(delay) = std::env::var("PEAKGUARD_AUTOEXPORT_AFTER_MS")
        && let Ok(delay) = delay.parse::<u64>()
    {
        let state = state.clone();
        gtk::glib::timeout_add_local_once(Duration::from_millis(delay), move || {
            state.export_all_segments();
        });
    }
    if let Ok(delay) = std::env::var("PEAKGUARD_OPEN_COMPARE_AFTER_MS")
        && let Ok(delay) = delay.parse::<u64>()
    {
        let state = state.clone();
        gtk::glib::timeout_add_local_once(Duration::from_millis(delay), move || {
            let selected = state.project.borrow().selected_segment.clone();
            if let Some(segment_id) = selected {
                state.show_compare(&segment_id);
            }
        });
    }
    if let Some(path) = std::env::var_os("PEAKGUARD_SCREENSHOT") {
        let window = state.ui.window.clone();
        let delay_ms = std::env::var("PEAKGUARD_SCREENSHOT_DELAY_MS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(1200);
        gtk::glib::timeout_add_local_once(Duration::from_millis(delay_ms), move || {
            let active = gtk::Window::list_toplevels()
                .into_iter()
                .filter_map(|widget| widget.downcast::<gtk::Window>().ok())
                .last()
                .unwrap_or_else(|| window.clone().upcast());
            if let Err(error) = save_window_png(&active, Path::new(&path)) {
                eprintln!("截图失败: {error:#}");
            }
        });
    }
    Ok(())
}

impl AppState {
    fn set_status(self: &Rc<Self>, message: impl AsRef<str>, busy: bool) {
        self.ui.status_text.set_text(message.as_ref());
        if busy {
            self.ui.status_dot.add_css_class("busy");
        } else {
            self.ui.status_dot.remove_css_class("busy");
        }
    }

    fn save(self: &Rc<Self>) {
        let result = {
            let project = self.project.borrow();
            self.store.save(&project)
        };
        match result {
            Ok(()) => {
                let now = Local::now().format("%H:%M:%S");
                self.ui.autosave_pill.set_text(&format!("自动保存 · {now}"));
                self.ui.saved_time.set_text(&format!("最近保存 · {now}"));
                self.ui.autosave_pill.remove_css_class("pill-warning");
                self.ui.autosave_pill.add_css_class("pill-ready");
            }
            Err(error) => {
                self.ui.autosave_pill.set_text("自动保存失败");
                self.ui.autosave_pill.remove_css_class("pill-ready");
                self.ui.autosave_pill.add_css_class("pill-warning");
                self.set_status(format!("保存失败: {error:#}"), false);
            }
        }
    }

    fn sync_params(self: &Rc<Self>, params: &crate::model::ExtremeParams) {
        self.syncing_params.set(true);
        self.ui.params.set_values(params);
        self.syncing_params.set(false);
        self.update_threshold_pill();
    }

    fn update_threshold_pill(self: &Rc<Self>) {
        let params = self.ui.params.values();
        self.ui.threshold_pill.set_text(&format!(
            "阈值 {:.1} dBFS · 上限 {:.1} dBFS",
            params.threshold_dbfs, params.ceiling_dbfs
        ));
    }

    fn bind_events(self: &Rc<Self>) {
        let state = self.clone();
        self.ui
            .run_all
            .connect_clicked(move |_| state.run_all_segments());
        let state = self.clone();
        self.ui
            .export_all
            .connect_clicked(move |_| state.export_all_segments());
        let state = self.clone();
        self.ui
            .import
            .connect_clicked(move |_| state.open_import_dialog());

        for spin in [
            &self.ui.params.threshold,
            &self.ui.params.ceiling,
            &self.ui.params.makeup,
            &self.ui.params.attack,
            &self.ui.params.release,
            &self.ui.params.boundary,
        ] {
            let state = self.clone();
            spin.connect_value_changed(move |_| {
                if !state.syncing_params.get() {
                    state.ui.params.mark_custom();
                    state.update_threshold_pill();
                    state.set_status("参数已修改 · 下次运行将冻结为新证据", false);
                }
            });
        }

        let state = self.clone();
        self.ui.params.extreme.connect_toggled(move |button| {
            if button.is_active() && !state.syncing_params.get() {
                state.sync_params(&crate::model::ExtremeParams::extreme());
                state.set_status("已载入极限参数组", false);
            }
        });
        let state = self.clone();
        self.ui.params.stable.connect_toggled(move |button| {
            if button.is_active() && !state.syncing_params.get() {
                state.sync_params(&crate::model::ExtremeParams::restrained());
                state.set_status("已载入稳健参数组", false);
            }
        });

        let state = self.clone();
        self.ui.timeline.add_controller({
            let gesture = gtk::GestureClick::new();
            let state_for_click = state.clone();
            gesture.connect_pressed(move |_, _, x, _| {
                state_for_click.select_at_timeline_x(x);
            });
            gesture
        });
    }

    fn select_at_timeline_x(self: &Rc<Self>, x: f64) {
        let width = self.ui.timeline.width().max(1) as f64;
        let fraction = ((x - 14.0) / (width - 28.0)).clamp(0.0, 1.0);
        let project = self.project.borrow();
        let time_ms = (project.media_duration_ms as f64 * fraction) as u64;
        let segment_id = project
            .segments
            .iter()
            .find(|segment| time_ms >= segment.start_ms && time_ms < segment.end_ms)
            .map(|segment| segment.id.clone());
        drop(project);
        if let Some(segment_id) = segment_id {
            self.select_segment(&segment_id);
        }
    }

    fn draw_timeline(
        &self,
        _area: &gtk::DrawingArea,
        cr: &gtk::cairo::Context,
        width: i32,
        height: i32,
    ) {
        let project = self.project.borrow();
        let waveform = self.waveform.borrow();
        let params = project.params.clone();
        let width = width.max(1) as f64;
        let height = height.max(1) as f64;
        let left = 16.0;
        let right = width - 16.0;
        let top = 16.0;
        let bottom = height - 17.0;
        let plot_width = (right - left).max(1.0);
        let baseline = (top + bottom) * 0.52;
        let half_height = (bottom - top) * 0.48;
        let duration = project.media_duration_ms.max(1) as f64;
        let map_x = |ms: f64| left + (ms / duration).clamp(0.0, 1.0) * plot_width;
        let map_db = |db: f32| {
            let normalized = ((db + 60.0) / 60.0).clamp(0.0, 1.0) as f64;
            baseline - normalized * half_height
        };

        cr.set_line_width(1.0);
        cr.set_source_rgba(0.11, 0.16, 0.20, 0.7);
        let mut tick = 0u64;
        while tick <= project.media_duration_ms {
            let x = map_x(tick as f64);
            cr.move_to(x.round() + 0.5, top);
            cr.line_to(x.round() + 0.5, bottom);
            cr.stroke().ok();
            tick += 4_000;
        }
        for db in [-48.0f32, -36.0, -24.0, -12.0] {
            let y = map_db(db);
            cr.move_to(left, y.round() + 0.5);
            cr.line_to(right, y.round() + 0.5);
            cr.stroke().ok();
        }

        for segment in &project.segments {
            if segment.missing {
                let x1 = map_x(segment.start_ms as f64);
                let x2 = map_x(segment.end_ms as f64);
                cr.set_source_rgba(0.52, 0.16, 0.17, 0.24);
                cr.rectangle(x1, top, (x2 - x1).max(1.0), bottom - top);
                cr.fill().ok();
                cr.set_source_rgba(0.75, 0.26, 0.24, 0.15);
                cr.set_line_width(1.0);
                let mut hatch = x1 - (bottom - top);
                while hatch < x2 {
                    cr.move_to(hatch, bottom);
                    cr.line_to(hatch + (bottom - top), top);
                    cr.stroke().ok();
                    hatch += 16.0;
                }
            }
            let x = map_x(segment.start_ms as f64);
            cr.set_source_rgba(0.24, 0.31, 0.36, 0.7);
            cr.set_dash(&[3.0, 5.0], 0.0);
            cr.move_to(x.round() + 0.5, top);
            cr.line_to(x.round() + 0.5, bottom);
            cr.stroke().ok();
            cr.set_dash(&[], 0.0);
        }

        let columns = (plot_width as usize).max(1);
        let mut peaks = vec![-120.0f32; columns];
        for sample in waveform.iter() {
            let fraction = (sample.at_ms as f64 / duration).clamp(0.0, 0.999_999);
            let index = (fraction * columns as f64) as usize;
            if let Some(value) = peaks.get_mut(index) {
                *value = value.max(sample.peak_dbfs);
            }
        }
        for (index, peak) in peaks.iter().enumerate() {
            if *peak <= -119.0 {
                continue;
            }
            let amplitude = ((*peak + 60.0) / 60.0).clamp(0.02, 1.0) as f64;
            let x = left + index as f64;
            let half = amplitude * half_height * 0.86;
            if *peak > params.threshold_dbfs {
                cr.set_source_rgba(1.0, 0.43, 0.35, 0.86);
            } else {
                cr.set_source_rgba(0.30, 0.88, 0.76, 0.64);
            }
            cr.rectangle(x, baseline - half, 1.3, (half * 2.0).max(1.0));
            cr.fill().ok();
        }

        let threshold_y = map_db(params.threshold_dbfs);
        cr.set_source_rgba(1.0, 0.72, 0.36, 0.9);
        cr.set_line_width(1.0);
        cr.set_dash(&[6.0, 5.0], 0.0);
        cr.move_to(left, threshold_y.round() + 0.5);
        cr.line_to(right, threshold_y.round() + 0.5);
        cr.stroke().ok();
        cr.set_dash(&[], 0.0);
        cr.select_font_face(
            "Sans",
            gtk::cairo::FontSlant::Normal,
            gtk::cairo::FontWeight::Bold,
        );
        cr.set_font_size(9.0);
        cr.set_source_rgba(1.0, 0.72, 0.36, 0.95);
        cr.move_to(right - 86.0, threshold_y - 4.0);
        cr.show_text(&format!("预警 {:.1} dB", params.threshold_dbfs))
            .ok();

        if let Some(segment) = project.selected()
            && let Some(run) = segment.latest()
        {
            for (_, sample) in run.original.boundaries.ordered() {
                let Some(sample) = sample else {
                    continue;
                };
                let x = map_x(sample.at_ms as f64);
                let y = map_db(sample.peak_dbfs);
                cr.set_source_rgba(0.79, 1.0, 0.28, 0.9);
                cr.set_line_width(1.0);
                cr.set_dash(&[2.0, 3.0], 0.0);
                cr.move_to(x.round() + 0.5, top);
                cr.line_to(x.round() + 0.5, bottom);
                cr.stroke().ok();
                cr.set_dash(&[], 0.0);
                cr.arc(x, y, 3.0, 0.0, std::f64::consts::TAU);
                cr.fill().ok();
            }
        }

        cr.set_source_rgba(0.46, 0.54, 0.60, 0.9);
        cr.set_font_size(9.0);
        tick = 0;
        while tick <= project.media_duration_ms {
            let x = map_x(tick as f64);
            cr.move_to(x + 3.0, height - 3.0);
            cr.show_text(&format_time_ms(tick)).ok();
            tick += 4_000;
        }
        for segment in &project.segments {
            if segment.missing {
                let center = map_x((segment.start_ms + segment.end_ms) as f64 / 2.0);
                cr.set_source_rgba(1.0, 0.58, 0.50, 1.0);
                cr.set_font_size(10.0);
                let text = "缺失片段 · 无可恢复采样";
                cr.move_to(center - 58.0, top + 15.0);
                cr.show_text(text).ok();
            }
        }
    }

    fn refresh(self: &Rc<Self>) {
        let project = self.project.borrow();
        let media_name = Path::new(&project.media_path)
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("未知媒体");
        self.ui.project_title.set_text(&project.name);
        self.ui.project_subtitle.set_text(media_name);
        self.ui.media_meta.set_text(&format!(
            "{} · {} Hz · {} 声道 · 原始文件只读",
            format_time_ms(project.media_duration_ms),
            project.sample_rate,
            project.channels
        ));
        self.ui.session_id.set_text(&project.id);
        self.ui
            .storage_label
            .set_text(&format!("证据目录 {}", self.store.root().display()));
        let missing = project.segments.iter().find(|segment| segment.missing);
        self.ui.missing_pill.set_text(&match missing {
            Some(segment) => format!(
                "缺失片段 {} — {}",
                format_time_ms(segment.start_ms),
                format_time_ms(segment.end_ms)
            ),
            None => "未标记缺失片段".into(),
        });
        let completed = project
            .segments
            .iter()
            .filter(|segment| {
                matches!(
                    segment.status,
                    SegmentStatus::Ready | SegmentStatus::Warning
                )
            })
            .count();
        self.ui
            .segment_count
            .set_text(&format!("{completed} / {} 已完成", project.segments.len()));
        self.ui.segment_count.remove_css_class("pill-ready");
        self.ui.segment_count.remove_css_class("pill-idle");
        self.ui
            .segment_count
            .add_css_class(if completed == project.segments.len() {
                "pill-ready"
            } else {
                "pill-idle"
            });
        self.ui.timeline_caption.set_text(&format!(
            "青绿：安全峰值   珊瑚：超过预警阈值   红色区：缺失片段   虚线：边界采样点   ·   {} 个 level 样本",
            self.waveform.borrow().len()
        ));
        self.rebuild_segment_list(&project);
        self.rebuild_evidence(&project);
        drop(project);
        self.update_threshold_pill();
        self.ui.timeline.queue_draw();
    }

    fn rebuild_segment_list(self: &Rc<Self>, project: &Project) {
        clear_children(&self.ui.segment_list);
        for segment in &project.segments {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 9);
            row.add_css_class("segment-row");
            if project.selected_segment.as_deref() == Some(segment.id.as_str()) {
                row.add_css_class("selected");
            }
            let index = ui::label(format!("{:02}", segment.index), "segment-index");
            index.set_valign(gtk::Align::Center);
            if segment.missing {
                index.add_css_class("missing");
            }
            row.append(&index);

            let name_box = gtk::Box::new(gtk::Orientation::Vertical, 1);
            name_box.set_valign(gtk::Align::Center);
            name_box.set_width_request(176);
            let name_row = gtk::Box::new(gtk::Orientation::Horizontal, 5);
            name_row.append(&ui::label(&segment.label, "segment-name"));
            if segment.missing {
                name_row.append(&ui::pill("缺失", "pill-missing"));
            }
            name_box.append(&name_row);
            name_box.append(&ui::label(
                format!(
                    "{} — {}  ·  {:.1}s",
                    format_time_ms(segment.start_ms),
                    format_time_ms(segment.end_ms),
                    segment.duration_ms() as f64 / 1000.0
                ),
                "segment-time",
            ));
            row.append(&name_box);

            let status = ui::pill(
                segment.status.label(),
                &format!("pill-{}", segment.status.css()),
            );
            status.set_valign(gtk::Align::Center);
            row.append(&status);

            let metrics = gtk::Box::new(gtk::Orientation::Vertical, 1);
            metrics.set_valign(gtk::Align::Center);
            metrics.set_width_request(100);
            if let Some(run) = segment.latest() {
                metrics.append(&ui::label(
                    format!(
                        "{:.1} → {:.1} dBFS",
                        run.original.peak_dbfs, run.corrected.peak_dbfs
                    ),
                    "metric-small",
                ));
                metrics.append(&ui::label(
                    format!(
                        "预警 {} · 截断 {}",
                        run.original.warning_count, run.clipped_samples
                    ),
                    "tiny",
                ));
            } else {
                metrics.append(&ui::label("尚未运行", "muted"));
                metrics.append(&ui::label("参数待应用", "tiny"));
            }
            row.append(&metrics);

            let fill = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            fill.set_hexpand(true);
            row.append(&fill);

            let compare = gtk::Button::with_label("≋ 对比");
            compare.add_css_class("ghost");
            compare.set_tooltip_text(Some("比较原始与修正证据"));
            compare.set_sensitive(segment.latest().is_some() && !self.processing.get());
            let export = gtk::Button::with_label("⇩ 导出");
            export.add_css_class("ghost");
            export.set_tooltip_text(Some("导出此片段与证据"));
            export.set_sensitive(segment.latest().is_some() && !self.processing.get());
            let rerun = gtk::Button::with_label("↺  重跑");
            rerun.add_css_class("ghost");
            rerun.set_sensitive(!self.processing.get());
            row.append(&compare);
            row.append(&export);
            row.append(&rerun);

            let state = self.clone();
            let segment_id = segment.id.clone();
            let gesture = gtk::GestureClick::new();
            gesture.connect_pressed(move |_, _, _, _| {
                state.select_segment(&segment_id);
            });
            row.add_controller(gesture);

            let state = self.clone();
            let segment_id = segment.id.clone();
            rerun.connect_clicked(move |_| {
                state.run_segments(vec![segment_id.clone()]);
            });
            let state = self.clone();
            let segment_id = segment.id.clone();
            compare.connect_clicked(move |_| {
                state.show_compare(&segment_id);
            });
            let state = self.clone();
            let segment_id = segment.id.clone();
            export.connect_clicked(move |_| {
                state.export_segments(vec![segment_id.clone()]);
            });
            self.ui.segment_list.append(&row);
        }
    }
}

fn clear_children(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

fn save_window_png(window: &gtk::Window, path: &Path) -> Result<()> {
    use gtk::prelude::*;
    let width = window.width().max(640);
    let height = window.height().max(480);
    let paintable = gtk::WidgetPaintable::new(Some(window));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, width as f64, height as f64);
    let node = snapshot.to_node().context("无法创建窗口渲染节点")?;
    let renderer = gtk::gsk::CairoRenderer::new();
    if !renderer.is_realized() {
        renderer.realize(None).context("无法初始化 Cairo 渲染器")?;
    }
    let viewport = gtk::graphene::Rect::new(0.0, 0.0, width as f32, height as f32);
    let texture = renderer.render_texture(&node, Some(&viewport));
    let result = texture
        .save_to_png(path)
        .with_context(|| format!("写入截图失败: {}", path.display()));
    renderer.unrealize();
    result
}

impl AppState {
    fn rebuild_evidence(self: &Rc<Self>, project: &Project) {
        clear_children(&self.ui.evidence_box);
        let Some(segment) = project.selected().cloned() else {
            self.ui
                .evidence_box
                .append(&ui::label("请选择一个媒体片段", "muted"));
            return;
        };
        let latest = segment.latest().cloned();

        let title_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let titles = gtk::Box::new(gtk::Orientation::Vertical, 1);
        titles.append(&ui::label("证据面板", "section-title"));
        titles.append(&ui::label("边界双样本与运行留痕", "section-kicker"));
        title_row.append(&titles);
        let fill = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        fill.set_hexpand(true);
        title_row.append(&fill);
        title_row.append(&ui::pill(
            format!("片段 {:02}", segment.index),
            "pill-param",
        ));
        self.ui.evidence_box.append(&title_row);

        let head = gtk::Box::new(gtk::Orientation::Horizontal, 7);
        let head_text = gtk::Box::new(gtk::Orientation::Vertical, 1);
        head_text.append(&ui::label(&segment.label, "metric-small"));
        head_text.append(&ui::label(
            format!(
                "{} — {}",
                format_time_ms(segment.start_ms),
                format_time_ms(segment.end_ms)
            ),
            "tiny",
        ));
        head.append(&head_text);
        let head_fill = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        head_fill.set_hexpand(true);
        head.append(&head_fill);
        let missing_toggle = gtk::ToggleButton::with_label("缺失");
        missing_toggle.add_css_class("preset");
        missing_toggle.set_active(segment.missing);
        missing_toggle.set_tooltip_text(Some("将本段标记为缺失片段，作为局部修正范围"));
        head.append(&missing_toggle);
        self.ui.evidence_box.append(&head);
        let state = self.clone();
        let segment_id = segment.id.clone();
        missing_toggle.connect_toggled(move |button| {
            let active = button.is_active();
            {
                let mut project = state.project.borrow_mut();
                if let Some(segment) = project
                    .segments
                    .iter_mut()
                    .find(|segment| segment.id == segment_id)
                {
                    segment.missing = active;
                    if active && segment.label == "媒体片段" {
                        segment.label = "缺失片段".into();
                    }
                }
                project.add_evidence(
                    "mark",
                    Some(segment_id.clone()),
                    if active {
                        "标记为缺失片段；后续运行将保留该区间边界采样。"
                    } else {
                        "已取消缺失片段标记。"
                    },
                );
            }
            state.save();
            state.refresh();
            state.set_status("缺失标记已更新并自动保存", false);
        });

        let quick_actions = gtk::Box::new(gtk::Orientation::Horizontal, 5);
        let rerun = gtk::Button::with_label("↺  重跑");
        rerun.add_css_class("primary");
        rerun.set_hexpand(true);
        rerun.set_sensitive(!self.processing.get());
        let compare = gtk::Button::with_label("≋  对比");
        compare.add_css_class("secondary");
        compare.set_sensitive(latest.is_some() && !self.processing.get());
        let export = gtk::Button::with_label("⇩  导出");
        export.add_css_class("secondary");
        export.set_sensitive(latest.is_some() && !self.processing.get());
        quick_actions.append(&rerun);
        quick_actions.append(&compare);
        quick_actions.append(&export);
        self.ui.evidence_box.append(&quick_actions);
        let state = self.clone();
        let segment_id = segment.id.clone();
        rerun.connect_clicked(move |_| {
            state.run_segments(vec![segment_id.clone()]);
        });
        let state = self.clone();
        let segment_id = segment.id.clone();
        compare.connect_clicked(move |_| state.show_compare(&segment_id));
        let state = self.clone();
        let segment_id = segment.id.clone();
        export.connect_clicked(move |_| state.export_segments(vec![segment_id.clone()]));

        if let Some(run) = &latest {
            self.ui
                .evidence_box
                .append(&self.boundary_section(&run.boundary_changes));
            self.ui
                .evidence_box
                .append(&self.run_summary_card(&segment, run));
        } else {
            let empty = gtk::Box::new(gtk::Orientation::Vertical, 7);
            empty.add_css_class("evidence-card");
            empty.append(&ui::label("尚无运行证据", "metric-small"));
            empty.append(&ui::label(
                "单独运行本段后，将生成修正 WAV、前后峰值、截断样本和四个边界样本。",
                "muted",
            ));
            let run_button = gtk::Button::with_label("↺  运行本段并采集边界样本");
            run_button.add_css_class("secondary");
            run_button.set_halign(gtk::Align::Start);
            run_button.set_sensitive(!self.processing.get());
            let state = self.clone();
            let segment_id = segment.id.clone();
            run_button.connect_clicked(move |_| {
                state.run_segments(vec![segment_id.clone()]);
            });
            empty.append(&run_button);
            self.ui.evidence_box.append(&empty);
        }

        let trail_header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        trail_header.append(&ui::label("证据轨迹", "section-title"));
        let trail_fill = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        trail_fill.set_hexpand(true);
        trail_header.append(&trail_fill);
        trail_header.append(&ui::label(
            format!("保留 {} 条", project.evidence.len()),
            "tiny",
        ));
        self.ui.evidence_box.append(&trail_header);
        let trail = gtk::Box::new(gtk::Orientation::Vertical, 6);
        for event in project.evidence.iter().rev().take(5) {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 7);
            let dot = gtk::Box::new(gtk::Orientation::Vertical, 0);
            dot.add_css_class("trail-dot");
            dot.set_valign(gtk::Align::Start);
            dot.set_margin_top(4);
            row.append(&dot);
            let texts = gtk::Box::new(gtk::Orientation::Vertical, 1);
            texts.set_hexpand(true);
            let message = ui::label(&event.message, "muted");
            message.set_wrap(true);
            message.set_xalign(0.0);
            texts.append(&message);
            texts.append(&ui::label(
                format!(
                    "{} · {}",
                    event.at.with_timezone(&Local).format("%H:%M:%S"),
                    event.kind
                ),
                "tiny",
            ));
            row.append(&texts);
            trail.append(&row);
        }
        self.ui.evidence_box.append(&trail);
    }

    fn run_summary_card(&self, segment: &Segment, run: &CorrectionRun) -> gtk::Box {
        let card = gtk::Box::new(gtk::Orientation::Vertical, 7);
        card.add_css_class("evidence-card");
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 7);
        top.append(&ui::label("最近运行", "section-title"));
        let fill = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        fill.set_hexpand(true);
        top.append(&fill);
        top.append(&ui::label(
            run.created_at
                .with_timezone(&Local)
                .format("%m-%d %H:%M:%S")
                .to_string(),
            "tiny",
        ));
        card.append(&top);
        let metrics = gtk::Box::new(gtk::Orientation::Horizontal, 5);
        metrics.append(&evidence_metric(
            "原始峰值",
            format!("{:.1}", run.original.peak_dbfs),
            "danger-text",
        ));
        metrics.append(&evidence_metric(
            "修正峰值",
            format!("{:.1}", run.corrected.peak_dbfs),
            "teal-text",
        ));
        metrics.append(&evidence_metric(
            "截断样本",
            compact_number(run.clipped_samples),
            "accent-text",
        ));
        card.append(&metrics);
        let headroom = if run.corrected.peak_dbfs < -90.0 {
            "无有效信号".to_string()
        } else {
            format!("余量 +{:.1} dB", run.headroom_recovered_db)
        };
        let detail = format!(
            "预警 {} → {}  ·  {}  ·  手柄 ±{} ms  ·  {}",
            run.original.warning_count,
            run.corrected.warning_count,
            headroom,
            run.params.boundary_window_ms,
            Path::new(&run.output_path)
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("输出.wav")
        );
        let detail_label = ui::label(detail, "tiny");
        detail_label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        card.append(&detail_label);
        let footer = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        footer.append(&ui::label(run.params.summary(), "tiny"));
        let fill = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        fill.set_hexpand(true);
        footer.append(&fill);
        footer.append(&ui::pill(
            if segment.status == SegmentStatus::Warning {
                "需复核"
            } else {
                "已修正"
            },
            if segment.status == SegmentStatus::Warning {
                "pill-warning"
            } else {
                "pill-ready"
            },
        ));
        card.append(&footer);
        card
    }

    fn boundary_section(&self, changes: &[EvidenceSampleChange]) -> gtk::Box {
        let section = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        header.append(&ui::label("边界双样本", "section-title"));
        let fill = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        fill.set_hexpand(true);
        header.append(&fill);
        header.append(&ui::label("每侧取 1 个样本", "tiny"));
        section.append(&header);
        let grid = gtk::Grid::new();
        grid.set_row_spacing(5);
        grid.set_column_spacing(5);
        grid.set_column_homogeneous(true);
        for (index, change) in changes.iter().take(4).enumerate() {
            let cell = gtk::Box::new(gtk::Orientation::Vertical, 3);
            cell.add_css_class("boundary-cell");
            cell.append(&ui::label(&change.boundary, "boundary-label"));
            let values = gtk::Box::new(gtk::Orientation::Horizontal, 4);
            values.append(&ui::label(
                format!(
                    "原 {}",
                    change
                        .before
                        .as_ref()
                        .map(|sample| format!("{:.1}", sample.peak_dbfs))
                        .unwrap_or_else(|| "--".into())
                ),
                "tiny",
            ));
            values.append(&ui::label("→", "tiny"));
            values.append(&ui::label(
                format!(
                    "修 {}",
                    change
                        .after
                        .as_ref()
                        .map(|sample| format!("{:.1}", sample.peak_dbfs))
                        .unwrap_or_else(|| "--".into())
                ),
                "tiny",
            ));
            cell.append(&values);
            let delta = change.delta_db.unwrap_or(0.0);
            cell.append(&ui::label(
                format!("Δ {delta:+.1} dB"),
                if delta <= 0.0 {
                    "tiny delta-positive"
                } else {
                    "tiny delta-negative"
                },
            ));
            grid.attach(&cell, (index % 2) as i32, (index / 2) as i32, 1, 1);
        }
        section.append(&grid);
        section
    }
}

fn evidence_metric(title: &str, value: String, class: &str) -> gtk::Box {
    let metric = gtk::Box::new(gtk::Orientation::Vertical, 0);
    metric.set_hexpand(true);
    metric.append(&ui::label(title, "boundary-label"));
    metric.append(&ui::label(value, &format!("metric-small {class}")));
    metric
}

fn compact_number(value: u64) -> String {
    if value >= 1_000_000 {
        format!("{:.1}M", value as f64 / 1_000_000.0)
    } else if value >= 1_000 {
        format!("{:.1}k", value as f64 / 1_000.0)
    } else {
        value.to_string()
    }
}

fn compare_column(title: &str, prefix: &str, run: &CorrectionRun, original: bool) -> gtk::Box {
    let analysis = if original {
        &run.original
    } else {
        &run.corrected
    };
    let card = gtk::Box::new(gtk::Orientation::Vertical, 7);
    card.add_css_class("dialog-card");
    card.set_hexpand(true);
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    top.append(&ui::label(title, "section-title"));
    let fill = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    fill.set_hexpand(true);
    top.append(&fill);
    top.append(&ui::pill(
        if original { "只读" } else { "可导出" },
        if original { "pill-idle" } else { "pill-ready" },
    ));
    card.append(&top);
    card.append(&metric_row(
        &format!("{prefix}峰值"),
        format!("{:.2} dBFS", analysis.peak_dbfs),
    ));
    card.append(&metric_row(
        &format!("{prefix} RMS"),
        format!("{:.2} dBFS", analysis.rms_dbfs),
    ));
    card.append(&metric_row(
        if original {
            "预警区间"
        } else {
            "残余超线"
        },
        analysis.warning_count.to_string(),
    ));
    card.append(&metric_row("采样窗口", analysis.sample_count.to_string()));
    if !original {
        card.append(&metric_row(
            "硬截断样本",
            compact_number(run.clipped_samples),
        ));
        card.append(&metric_row(
            "恢复余量",
            format!("+{:.2} dB", run.headroom_recovered_db),
        ));
    }
    card.append(&ui::label(
        if original {
            "原始媒体不被覆盖，任何运行都可追溯回此基线。"
        } else {
            "输出为独立 WAV，边界样本在渲染后重新测量。"
        },
        "tiny",
    ));
    card
}

fn metric_row(title: &str, value: String) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    row.append(&ui::label(title, "muted"));
    let fill = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    fill.set_hexpand(true);
    row.append(&fill);
    row.append(&ui::label(value, "metric-small"));
    row
}

#[allow(clippy::too_many_arguments)]
fn export_report(
    project_name: &str,
    project_id: &str,
    source_media: &str,
    index: usize,
    label: &str,
    start_ms: u64,
    end_ms: u64,
    run: &CorrectionRun,
    wave_name: &str,
) -> String {
    let mut report = String::new();
    report.push_str("# 音频峰值预警 · 局部修正证据\n\n");
    report.push_str(&format!("- 项目：{project_name} (`{project_id}`)\n"));
    report.push_str(&format!("- 源媒体（只读）：`{source_media}`\n"));
    report.push_str(&format!(
        "- 片段：{:02} · {} · {} — {}\n",
        index,
        label,
        format_time_ms(start_ms),
        format_time_ms(end_ms)
    ));
    report.push_str(&format!("- 运行：`{}`\n", run.id));
    report.push_str(&format!(
        "- 时间：{}\n",
        run.created_at
            .with_timezone(&Local)
            .format("%Y-%m-%d %H:%M:%S")
    ));
    report.push_str(&format!("- 修正文件：`{wave_name}`\n"));
    report.push_str(&format!(
        "- 参数组：{} · {} · 启动 {:.1} ms · 释放 {:.1} ms · 边界窗口/手柄 ±{} ms\n",
        run.params.preset,
        run.params.summary(),
        run.params.attack_ms,
        run.params.release_ms,
        run.params.boundary_window_ms
    ));
    report.push_str("\n## 峰值结果\n\n");
    report.push_str("| 指标 | 原始 | 修正 |\n|---|---:|---:|\n");
    report.push_str(&format!(
        "| 峰值 | {:.2} dBFS | {:.2} dBFS |\n",
        run.original.peak_dbfs, run.corrected.peak_dbfs
    ));
    report.push_str(&format!(
        "| RMS | {:.2} dBFS | {:.2} dBFS |\n",
        run.original.rms_dbfs, run.corrected.rms_dbfs
    ));
    report.push_str(&format!(
        "| 预警窗口 | {} | {} |\n",
        run.original.warning_count, run.corrected.warning_count
    ));
    report.push_str(&format!("| 硬截断样本 | — | {} |\n", run.clipped_samples));
    report.push_str(&format!(
        "| 恢复余量 | — | +{:.2} dB |\n",
        run.headroom_recovered_db
    ));
    report.push_str("\n## 边界前后样本\n\n");
    report.push_str("| 边界 | 原始峰值 | 修正峰值 | 变化 |\n|---|---:|---:|---:|\n");
    for change in &run.boundary_changes {
        report.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            change.boundary,
            change
                .before
                .as_ref()
                .map(|sample| format!("{:.2} dBFS", sample.peak_dbfs))
                .unwrap_or_else(|| "--".into()),
            change
                .after
                .as_ref()
                .map(|sample| format!("{:.2} dBFS", sample.peak_dbfs))
                .unwrap_or_else(|| "--".into()),
            change
                .delta_db
                .map(|value| format!("{value:+.2} dB"))
                .unwrap_or_else(|| "--".into())
        ));
    }
    report.push_str(
        "\n> 输出 WAV 在片段核心区前后各保留一个边界手柄，用于比较边界前后样本；原始文件不被写入。\n",
    );
    report
}

impl AppState {
    fn select_segment(self: &Rc<Self>, segment_id: &str) {
        let params = {
            let mut project = self.project.borrow_mut();
            project.select(segment_id);
            project
                .selected()
                .and_then(|segment| segment.latest())
                .map(|run| run.params.clone())
                .unwrap_or_else(|| project.params.clone())
        };
        self.sync_params(&params);
        self.save();
        self.refresh();
        self.set_status("已载入该片段最近一次参数快照；可修改后单独重跑", false);
    }

    fn run_all_segments(self: &Rc<Self>) {
        let ids = {
            let project = self.project.borrow();
            project
                .segments
                .iter()
                .map(|segment| segment.id.clone())
                .collect()
        };
        self.run_segments(ids);
    }

    fn run_segments(self: &Rc<Self>, ids: Vec<String>) {
        if self.processing.get() || ids.is_empty() {
            return;
        }
        let params = self.ui.params.values();
        let source = {
            let mut project = self.project.borrow_mut();
            project.params = params.clone();
            project.add_evidence(
                "params",
                None,
                format!(
                    "冻结参数组「{}」：{}；边界窗口 {} ms。",
                    params.preset,
                    params.summary(),
                    params.boundary_window_ms
                ),
            );
            PathBuf::from(&project.media_path)
        };
        let specs: Vec<RunSpec> = {
            let project = self.project.borrow();
            project
                .segments
                .iter()
                .filter(|segment| ids.iter().any(|id| id == &segment.id))
                .map(|segment| RunSpec {
                    id: segment.id.clone(),
                    index: segment.index,
                    start_ms: segment.start_ms,
                    end_ms: segment.end_ms,
                    output: self.store.segment_output_path(segment),
                })
                .collect()
        };
        if specs.is_empty() {
            return;
        }
        self.save();
        self.processing.set(true);
        self.set_processing_ui(true);
        self.set_status(
            format!(
                "已提交 {} 个独立片段任务 · 参数与证据将逐段落盘",
                specs.len()
            ),
            true,
        );
        let (sender, receiver) = mpsc::channel();
        let params_for_worker = params.clone();
        std::thread::spawn(move || {
            let _ = audio::init();
            for spec in specs {
                if sender.send(WorkerEvent::Started(spec.id.clone())).is_err() {
                    return;
                }
                let result = audio::run_segment(
                    &spec.id,
                    spec.index,
                    &source,
                    &spec.output,
                    spec.start_ms,
                    spec.end_ms,
                    &params_for_worker,
                );
                match result {
                    Ok(run) => {
                        if sender
                            .send(WorkerEvent::Finished(spec.id, Box::new(run)))
                            .is_err()
                        {
                            return;
                        }
                    }
                    Err(error) => {
                        if sender
                            .send(WorkerEvent::Failed(spec.id, format!("{error:#}")))
                            .is_err()
                        {
                            return;
                        }
                    }
                }
            }
            let _ = sender.send(WorkerEvent::AllDone);
        });
        self.poll_worker(receiver);
    }

    fn poll_worker(self: &Rc<Self>, receiver: Receiver<WorkerEvent>) {
        let state = self.clone();
        gtk::glib::timeout_add_local(Duration::from_millis(100), move || {
            let mut all_done = false;
            loop {
                match receiver.try_recv() {
                    Ok(WorkerEvent::Started(id)) => {
                        {
                            let mut project = state.project.borrow_mut();
                            if let Some(segment) =
                                project.segments.iter_mut().find(|segment| segment.id == id)
                            {
                                segment.status = SegmentStatus::Running;
                                segment.last_error = None;
                                let index = segment.index;
                                project.add_evidence(
                                    "start",
                                    Some(id.clone()),
                                    format!("片段 {index:02} 开始运行；原始文件保持只读。"),
                                );
                            }
                        }
                        state.save();
                        state.refresh();
                        state.set_status("GStreamer 正在采集峰值并渲染局部音频…", true);
                    }
                    Ok(WorkerEvent::Finished(id, run)) => {
                        {
                            let mut project = state.project.borrow_mut();
                            project.apply_run(&id, *run);
                        }
                        state.save();
                        state.refresh();
                        state.set_status("片段完成 · 修正 WAV 与边界证据已保存", true);
                    }
                    Ok(WorkerEvent::Failed(id, error)) => {
                        {
                            let mut project = state.project.borrow_mut();
                            if let Some(segment) =
                                project.segments.iter_mut().find(|segment| segment.id == id)
                            {
                                segment.status = SegmentStatus::Failed;
                                segment.last_error = Some(error.clone());
                                let index = segment.index;
                                project.add_evidence(
                                    "error",
                                    Some(id.clone()),
                                    format!("片段 {index:02} 失败：{error}"),
                                );
                            }
                        }
                        state.save();
                        state.refresh();
                        state.set_status(format!("片段处理失败 · {error}"), true);
                    }
                    Ok(WorkerEvent::AllDone) => {
                        all_done = true;
                        break;
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        all_done = true;
                        break;
                    }
                }
            }
            if all_done {
                state.processing.set(false);
                state.set_processing_ui(false);
                state.refresh();
                state.set_status("任务结束 · 所有关键证据已自动保存，可逐段比较或导出", false);
                gtk::glib::ControlFlow::Break
            } else {
                gtk::glib::ControlFlow::Continue
            }
        });
    }

    fn set_processing_ui(&self, busy: bool) {
        self.ui.run_all.set_sensitive(!busy);
        self.ui.export_all.set_sensitive(!busy);
        self.ui.import.set_sensitive(!busy);
        self.ui.spinner.set_visible(busy);
        if busy {
            self.ui.spinner.start();
            self.ui.status_dot.add_css_class("busy");
        } else {
            self.ui.spinner.stop();
            self.ui.status_dot.remove_css_class("busy");
        }
        for spin in [
            &self.ui.params.threshold,
            &self.ui.params.ceiling,
            &self.ui.params.makeup,
            &self.ui.params.attack,
            &self.ui.params.release,
            &self.ui.params.boundary,
        ] {
            spin.set_sensitive(!busy);
        }
    }

    fn show_compare(self: &Rc<Self>, segment_id: &str) {
        let Some(segment) = self
            .project
            .borrow()
            .segments
            .iter()
            .find(|segment| segment.id == segment_id)
            .cloned()
        else {
            return;
        };
        let Some(run) = segment.latest().cloned() else {
            self.set_status("该片段还没有可比较的运行证据，请先运行", false);
            return;
        };
        let dialog = gtk::Window::builder()
            .transient_for(&self.ui.window)
            .modal(true)
            .title(format!("片段 {:02} · 修正对比", segment.index))
            .default_width(780)
            .default_height(680)
            .build();
        dialog.add_css_class("dialog-window");
        let header = gtk::HeaderBar::new();
        header.set_title_widget(Some(&ui::label(
            format!("片段 {:02} · 原始 / 修正对比", segment.index),
            "section-title",
        )));
        dialog.set_titlebar(Some(&header));
        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        content.set_margin_top(16);
        content.set_margin_bottom(16);
        content.set_margin_start(18);
        content.set_margin_end(18);

        let top = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let titles = gtk::Box::new(gtk::Orientation::Vertical, 2);
        titles.append(&ui::label(&segment.label, "page-title"));
        titles.append(&ui::label(
            format!(
                "{} — {}  ·  {}  ·  {}",
                format_time_ms(segment.start_ms),
                format_time_ms(segment.end_ms),
                run.params.preset,
                run.created_at
                    .with_timezone(&Local)
                    .format("%Y-%m-%d %H:%M:%S")
            ),
            "muted",
        ));
        top.append(&titles);
        let fill = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        fill.set_hexpand(true);
        top.append(&fill);
        top.append(&ui::pill(
            if segment.status == SegmentStatus::Warning {
                "需复核"
            } else {
                "已修正"
            },
            if segment.status == SegmentStatus::Warning {
                "pill-warning"
            } else {
                "pill-ready"
            },
        ));
        content.append(&top);

        let columns = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        columns.append(&compare_column("原始音频", "原始", &run, true));
        columns.append(&compare_column("修正音频", "修正", &run, false));
        content.append(&columns);

        let boundary_card = gtk::Box::new(gtk::Orientation::Vertical, 6);
        boundary_card.add_css_class("dialog-card");
        boundary_card.append(&ui::label("边界前后样本变化", "section-title"));
        let grid = gtk::Grid::new();
        grid.set_row_spacing(0);
        grid.set_column_spacing(8);
        grid.set_column_homogeneous(true);
        for (column, title) in ["边界", "原始峰值", "修正峰值", "变化"].iter().enumerate()
        {
            grid.attach(&ui::label(*title, "tiny"), column as i32, 0, 1, 1);
        }
        for (row, change) in run.boundary_changes.iter().enumerate() {
            let before = change
                .before
                .as_ref()
                .map(|sample| format!("{:.1} dBFS", sample.peak_dbfs))
                .unwrap_or_else(|| "--".into());
            let after = change
                .after
                .as_ref()
                .map(|sample| format!("{:.1} dBFS", sample.peak_dbfs))
                .unwrap_or_else(|| "--".into());
            let delta = change
                .delta_db
                .map(|value| format!("{value:+.1} dB"))
                .unwrap_or_else(|| "--".into());
            for (column, text) in [change.boundary.clone(), before, after, delta]
                .iter()
                .enumerate()
            {
                grid.attach(
                    &ui::label(text, if row == 0 { "muted" } else { "metric-small" }),
                    column as i32,
                    row as i32 + 1,
                    1,
                    1,
                );
            }
        }
        boundary_card.append(&grid);
        content.append(&boundary_card);

        let history = gtk::Box::new(gtk::Orientation::Vertical, 5);
        history.add_css_class("dialog-card");
        history.append(&ui::label(
            format!("运行历史 · {} 次", segment.runs.len()),
            "section-title",
        ));
        for (index, item) in segment.runs.iter().rev().take(5).enumerate() {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            row.append(&ui::label(if index == 0 { "最新" } else { "历史" }, "tiny"));
            row.append(&ui::label(
                item.created_at
                    .with_timezone(&Local)
                    .format("%H:%M:%S")
                    .to_string(),
                "tiny",
            ));
            row.append(&ui::label(item.params.summary(), "muted"));
            let fill = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            fill.set_hexpand(true);
            row.append(&fill);
            row.append(&ui::label(
                format!(
                    "{:.1} → {:.1} dBFS",
                    item.original.peak_dbfs, item.corrected.peak_dbfs
                ),
                "metric-small teal-text",
            ));
            history.append(&row);
        }
        content.append(&history);

        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 7);
        let fill = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        fill.set_hexpand(true);
        actions.append(&fill);
        let close = gtk::Button::with_label("关闭");
        close.add_css_class("secondary");
        let export = gtk::Button::with_label("⇩  导出此段");
        export.add_css_class("secondary");
        let rerun = gtk::Button::with_label("↺  重跑");
        rerun.add_css_class("primary");
        actions.append(&close);
        actions.append(&export);
        actions.append(&rerun);
        content.append(&actions);

        let scroll = gtk::ScrolledWindow::builder()
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&content)
            .build();
        dialog.set_child(Some(&scroll));
        let dialog_for_close = dialog.clone();
        close.connect_clicked(move |_| dialog_for_close.close());
        let state = self.clone();
        let dialog_for_export = dialog.clone();
        let id = segment.id.clone();
        export.connect_clicked(move |_| {
            state.export_segments(vec![id.clone()]);
            dialog_for_export.close();
        });
        let state = self.clone();
        let dialog_for_run = dialog.clone();
        let id = segment.id.clone();
        rerun.connect_clicked(move |_| {
            dialog_for_run.close();
            state.run_segments(vec![id.clone()]);
        });
        dialog.present();
        if let Some(path) = std::env::var_os("PEAKGUARD_COMPARE_SCREENSHOT") {
            let dialog = dialog.clone();
            gtk::glib::timeout_add_local_once(Duration::from_millis(450), move || {
                if let Err(error) = save_window_png(&dialog, Path::new(&path)) {
                    eprintln!("对比窗口截图失败: {error:#}");
                }
            });
        }
    }

    fn export_segments(self: &Rc<Self>, ids: Vec<String>) {
        let project = self.project.borrow();
        let mut items = Vec::new();
        for segment in &project.segments {
            if !ids.iter().any(|id| id == &segment.id) {
                continue;
            }
            let Some(run) = segment.latest() else {
                continue;
            };
            items.push((
                segment.id.clone(),
                segment.index,
                segment.label.clone(),
                segment.start_ms,
                segment.end_ms,
                run.clone(),
            ));
        }
        if items.is_empty() {
            drop(project);
            self.set_status("没有可导出的修正结果，请先运行片段", false);
            return;
        }
        let project_id = project.id.clone();
        let project_name = project.name.clone();
        let source_media = project.media_path.clone();
        drop(project);
        let export_dir = self.store.exports_dir();
        if let Err(error) = std::fs::create_dir_all(&export_dir) {
            self.set_status(format!("创建导出目录失败: {error}"), false);
            return;
        }
        let mut exported = Vec::new();
        for (segment_id, index, label, start_ms, end_ms, run) in items {
            let wave_name = format!(
                "{}-seg-{index:02}-{}.wav",
                project_id,
                run.created_at.format("%Y%m%d-%H%M%S")
            );
            let wave_path = export_dir.join(&wave_name);
            if let Err(error) = std::fs::copy(&run.output_path, &wave_path) {
                self.set_status(format!("复制 {wave_name} 失败: {error}"), false);
                continue;
            }
            let evidence_name = format!(
                "{}-seg-{index:02}-{}.evidence.json",
                project_id,
                run.created_at.format("%Y%m%d-%H%M%S")
            );
            let evidence_path = export_dir.join(&evidence_name);
            if let Ok(json) = serde_json::to_string_pretty(&run) {
                let _ = std::fs::write(&evidence_path, json);
            }
            let report_name = format!(
                "{}-seg-{index:02}-{}.report.md",
                project_id,
                run.created_at.format("%Y%m%d-%H%M%S")
            );
            let report_path = export_dir.join(&report_name);
            let report = export_report(
                &project_name,
                &project_id,
                &source_media,
                index,
                &label,
                start_ms,
                end_ms,
                &run,
                &wave_name,
            );
            let _ = std::fs::write(&report_path, report);
            exported.push((segment_id, wave_name));
        }
        {
            let mut project = self.project.borrow_mut();
            for (segment_id, wave_name) in &exported {
                project.add_evidence(
                    "export",
                    Some(segment_id.clone()),
                    format!("已导出 {wave_name}，并附带 JSON 证据与 Markdown 报告。"),
                );
            }
        }
        self.save();
        self.refresh();
        self.set_status(
            format!(
                "已导出 {} 个片段到 {}",
                exported.len(),
                export_dir.display()
            ),
            false,
        );
    }

    fn export_all_segments(self: &Rc<Self>) {
        let ids = {
            let project = self.project.borrow();
            project
                .segments
                .iter()
                .filter(|segment| segment.latest().is_some())
                .map(|segment| segment.id.clone())
                .collect()
        };
        self.export_segments(ids);
    }

    fn open_import_dialog(self: &Rc<Self>) {
        if self.processing.get() {
            return;
        }
        let dialog = gtk::FileDialog::new();
        dialog.set_title("导入音频或视频媒体");
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("音频 / 视频媒体"));
        for pattern in [
            "*.wav", "*.mp3", "*.flac", "*.ogg", "*.m4a", "*.aac", "*.mp4", "*.mkv", "*.mov",
            "*.webm",
        ] {
            filter.add_pattern(pattern);
        }
        let filters = gtk::gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        dialog.set_filters(Some(&filters));
        dialog.set_default_filter(Some(&filter));
        let state = self.clone();
        dialog.open(
            Some(&self.ui.window),
            gtk::gio::Cancellable::NONE,
            move |result| match result {
                Ok(file) => {
                    if let Some(path) = file.path() {
                        state.import_media(path);
                    }
                }
                Err(error) => {
                    if !error.matches(gtk::gio::IOErrorEnum::Cancelled) {
                        state.set_status(format!("选择媒体失败: {error}"), false);
                    }
                }
            },
        );
    }

    fn import_media(self: &Rc<Self>, path: PathBuf) {
        self.set_status("正在探测媒体并建立分段…", true);
        let info = match audio::discover(&path) {
            Ok(info) => info,
            Err(error) => {
                self.set_status(format!("导入失败: {error:#}"), false);
                return;
            }
        };
        let levels = audio::capture_levels(&path).unwrap_or_default();
        let mut project =
            Project::from_media(&path, info.duration_ms, info.sample_rate, info.channels, 6);
        let params = project.params.clone();
        let mut quietest: Option<(usize, f32)> = None;
        for (index, segment) in project.segments.iter().enumerate() {
            let analysis =
                audio::analyze_segment(&levels, segment.start_ms, segment.end_ms, 0, &params);
            if quietest
                .as_ref()
                .map(|(_, peak)| analysis.peak_dbfs < *peak)
                .unwrap_or(true)
            {
                quietest = Some((index, analysis.peak_dbfs));
            }
        }
        if let Some((index, peak)) = quietest
            && peak < -55.0
        {
            if let Some(segment) = project.segments.get_mut(index) {
                segment.missing = true;
                segment.label = "疑似缺失片段".into();
            }
            project.selected_segment = Some(format!("seg-{:02}", index + 1));
            project.add_evidence(
                "analyze",
                project.selected_segment.clone(),
                format!(
                    "导入分析发现低能量区间（{:.1} dBFS），已标记为疑似缺失片段。",
                    peak
                ),
            );
        }
        if project.selected_segment.is_none() {
            project.selected_segment = project.segments.first().map(|segment| segment.id.clone());
        }
        *self.waveform.borrow_mut() = levels;
        *self.project.borrow_mut() = project;
        self.save();
        self.sync_params(&params);
        self.refresh();
        self.set_status(
            format!(
                "已导入 {} · {} 个片段 · 原始文件只读",
                path.file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or("媒体"),
                self.project.borrow().segments.len()
            ),
            false,
        );
    }
}
