//! GTK4 interface: waveform views, candidate list, crossfade controls,
//! audition buttons (original / repaired / difference) and metrics.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow};

use crate::player::Player;
use crate::wav::WavData;
use crate::{audition, dsp, samples, wav};

const SEARCH_MS: f64 = 120.0;

struct AppState {
    name: String,
    wav: WavData,
    mono: Vec<f32>,
    candidates: Vec<dsp::Candidate>,
    selected: usize,
    fade_ms: f64,
    curve: dsp::FadeCurve,
    loop_samples: Vec<f32>,
    metrics: dsp::SeamMetrics,
    naive_metrics: dsp::SeamMetrics,
    drift: dsp::Drift,
}

impl AppState {
    fn load(name: String, wav: WavData) -> Self {
        let mono = wav.mono();
        let candidates = dsp::find_candidates(&mono, wav.sample_rate, SEARCH_MS, 8);
        let naive_metrics = dsp::seam_metrics(&mono, wav.sample_rate);
        let mut st = AppState {
            name,
            wav,
            mono,
            candidates,
            selected: 0,
            fade_ms: 30.0,
            curve: dsp::FadeCurve::EqualPower,
            loop_samples: Vec::new(),
            metrics: dsp::SeamMetrics::default(),
            naive_metrics,
            drift: dsp::Drift {
                period_frames: None,
                residual_per_loop: 0,
            },
        };
        st.recompute();
        st
    }

    fn cut(&self) -> dsp::Candidate {
        self.candidates[self.selected]
    }

    fn fade_frames(&self) -> usize {
        (self.fade_ms * 0.001 * self.wav.sample_rate as f64) as usize
    }

    fn recompute(&mut self) {
        let cut = self.cut();
        self.loop_samples = dsp::build_loop(&self.mono, &cut, self.fade_frames(), self.curve);
        self.metrics = dsp::seam_metrics(&self.loop_samples, self.wav.sample_rate);
        let period = dsp::estimate_period(&self.loop_samples, self.wav.sample_rate);
        self.drift = dsp::compute_drift(self.loop_samples.len(), period);
    }
}

/// All widgets that need to be refreshed when the state changes.
#[derive(Clone)]
struct Widgets {
    overview: gtk4::DrawingArea,
    seam_view: gtk4::DrawingArea,
    candidate_list: gtk4::ListBox,
    fade_scale: gtk4::Scale,
    status: gtk4::Label,
    m_cut: gtk4::Label,
    m_fade: gtk4::Label,
    m_disc: gtk4::Label,
    m_energy: gtk4::Label,
    m_bands: gtk4::Label,
    m_drift: gtk4::Label,
    m_drift_cum: gtk4::Label,
}

pub fn run() {
    run_app(None);
}

/// Launch the UI, render one frame to `png`, and quit. Used as a headless
/// smoke test of the interface.
pub fn run_screenshot(png: std::path::PathBuf) {
    run_app(Some(png));
}

fn run_app(screenshot: Option<std::path::PathBuf>) {
    gstreamer::init().ok();
    let app = Application::builder()
        .application_id("dev.loopjoiner.app")
        .build();
    app.connect_activate(move |app| {
        let window = build_ui(app);
        if let Some(png) = screenshot.clone() {
            gtk4::glib::timeout_add_local_once(std::time::Duration::from_millis(1200), move || {
                snapshot_window(&window, &png);
                eprintln!("screenshot written");
                std::process::exit(0);
            });
        }
    });
    app.run_with_args::<&str>(&[]);
}

fn snapshot_window(window: &ApplicationWindow, png: &std::path::Path) {
    let w = window.width().max(800);
    let h = window.height().max(500);
    let paintable = gtk4::WidgetPaintable::new(Some(window));
    let snapshot = gtk4::Snapshot::new();
    paintable.snapshot(&snapshot, w as f64, h as f64);
    if let Some(node) = snapshot.to_node() {
        if let Ok(surface) = gtk4::cairo::ImageSurface::create(gtk4::cairo::Format::ARgb32, w, h) {
            let cr = gtk4::cairo::Context::new(&surface).unwrap();
            node.draw(&cr);
            drop(cr);
            if let Ok(mut f) = std::fs::File::create(png) {
                let _ = surface.write_to_png(&mut f);
            }
        }
    }
}

fn ms(frames: usize, sr: u32) -> f64 {
    frames as f64 * 1000.0 / sr as f64
}

fn build_ui(app: &Application) -> ApplicationWindow {
    let state = Rc::new(RefCell::new(AppState::load(
        "drums (内置样例)".to_string(),
        samples::drums(),
    )));
    let player = Player::new();

    let window = ApplicationWindow::builder()
        .application(app)
        .title("Loop Joiner — 循环接缝修复")
        .default_width(1180)
        .default_height(720)
        .build();

    // Header: sample picker, open, export.
    let header = gtk4::HeaderBar::new();
    let sample_picker = gtk4::DropDown::from_strings(&["鼓点样例", "持续音样例", "尾响样例"]);
    header.pack_start(&sample_picker);
    let open_btn = gtk4::Button::with_label("打开 WAV…");
    header.pack_start(&open_btn);
    let export_btn = gtk4::Button::with_label("导出循环…");
    header.pack_end(&export_btn);
    window.set_titlebar(Some(&header));

    // Left: waveform overview + seam zoom.
    let overview = gtk4::DrawingArea::builder()
        .hexpand(true)
        .vexpand(true)
        .build();
    let seam_view = gtk4::DrawingArea::builder()
        .hexpand(true)
        .height_request(180)
        .build();
    let left = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
    left.append(&gtk4::Label::new(Some(
        "整段波形（绿=循环起点，红=循环终点，蓝=淡化区）",
    )));
    left.append(&overview);
    left.append(&gtk4::Label::new(Some("修复后接缝（环绕点 ±100 ms）")));
    left.append(&seam_view);

    // Right: candidates, controls, metrics, audition.
    let candidate_list = gtk4::ListBox::new();
    candidate_list.set_selection_mode(gtk4::SelectionMode::Single);
    let fade_scale = gtk4::Scale::with_range(gtk4::Orientation::Horizontal, 0.0, 250.0, 1.0);
    fade_scale.set_value(30.0);
    let curve_picker = gtk4::DropDown::from_strings(&["等功率", "线性", "S 曲线"]);

    let mk_metric = |caption: &str| -> (gtk4::Label, gtk4::Box) {
        let value = gtk4::Label::new(Some("—"));
        value.set_halign(gtk4::Align::Start);
        value.set_selectable(true);
        let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        let cap = gtk4::Label::new(Some(caption));
        cap.set_width_chars(14);
        cap.set_halign(gtk4::Align::Start);
        row.append(&cap);
        row.append(&value);
        (value, row)
    };
    let (m_cut, r_cut) = mk_metric("切点");
    let (m_fade, r_fade) = mk_metric("淡化长度");
    let (m_disc, r_disc) = mk_metric("接缝步进");
    let (m_energy, r_energy) = mk_metric("首尾能量差");
    let (m_bands, r_bands) = mk_metric("频带差 L/M/H");
    let (m_drift, r_drift) = mk_metric("漂移/循环");
    let (m_drift_cum, r_drift_cum) = mk_metric("累计漂移(100)");

    let metrics_box = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
    for r in [
        r_cut,
        r_fade,
        r_disc,
        r_energy,
        r_bands,
        r_drift,
        r_drift_cum,
    ] {
        metrics_box.append(&r);
    }

    let play_orig = gtk4::Button::with_label("试听原始接缝");
    let play_loop = gtk4::Button::with_label("试听修复循环");
    let play_diff = gtk4::Button::with_label("试听差分");
    let stop_btn = gtk4::Button::with_label("停止");
    let audition = gtk4::FlowBox::new();
    audition.set_max_children_per_line(2);
    for b in [&play_orig, &play_loop, &play_diff, &stop_btn] {
        audition.insert(b, -1);
    }

    let status = gtk4::Label::new(Some(""));
    status.set_halign(gtk4::Align::Start);

    let right = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
    right.set_width_request(360);
    right.append(&gtk4::Label::new(Some("候选切点（按平滑度排序）")));
    right.append(&candidate_list);
    right.append(&gtk4::Label::new(Some("交叉淡化长度 (ms)")));
    right.append(&fade_scale);
    right.append(&gtk4::Label::new(Some("淡化曲线")));
    right.append(&curve_picker);
    right.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));
    right.append(&metrics_box);
    right.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));
    right.append(&audition);
    right.append(&status);

    let paned = gtk4::Paned::new(gtk4::Orientation::Horizontal);
    paned.set_start_child(Some(&left));
    paned.set_end_child(Some(&right));
    paned.set_shrink_end_child(false);
    paned.set_position(780);
    window.set_child(Some(&paned));

    let widgets = Widgets {
        overview,
        seam_view,
        candidate_list,
        fade_scale,
        status,
        m_cut,
        m_fade,
        m_disc,
        m_energy,
        m_bands,
        m_drift,
        m_drift_cum,
    };

    // Waveform drawing.
    {
        let st = state.clone();
        widgets.overview.set_draw_func(move |_, cr, w, h| {
            let st = st.borrow();
            draw_overview(cr, w as f64, h as f64, &st);
        });
    }
    {
        let st = state.clone();
        widgets.seam_view.set_draw_func(move |_, cr, w, h| {
            let st = st.borrow();
            draw_seam(cr, w as f64, h as f64, &st);
        });
    }

    // Recompute + refresh helper.
    let refresh = {
        let state = state.clone();
        let widgets = widgets.clone();
        move || {
            state.borrow_mut().recompute();
            update_metrics(&state.borrow(), &widgets);
            widgets.overview.queue_draw();
            widgets.seam_view.queue_draw();
        }
    };

    rebuild_candidates(&state.borrow(), &widgets);
    refresh();

    // Callbacks.
    {
        let state = state.clone();
        let refresh = refresh.clone();
        widgets.candidate_list.connect_row_selected(move |_, row| {
            if let Some(row) = row {
                state.borrow_mut().selected = row.index().max(0) as usize;
                refresh();
            }
        });
    }
    {
        let state = state.clone();
        let refresh = refresh.clone();
        widgets.fade_scale.connect_value_changed(move |s| {
            state.borrow_mut().fade_ms = s.value();
            refresh();
        });
    }
    {
        let state = state.clone();
        let refresh = refresh.clone();
        curve_picker.connect_selected_notify(move |d| {
            state.borrow_mut().curve = dsp::FadeCurve::ALL[d.selected() as usize];
            refresh();
        });
    }
    {
        let state = state.clone();
        let widgets = widgets.clone();
        let refresh = refresh.clone();
        sample_picker.connect_selected_notify(move |d| {
            let (name, wav) = match d.selected() {
                0 => ("drums (内置样例)".to_string(), samples::drums()),
                1 => ("pad (内置样例)".to_string(), samples::pad()),
                _ => ("reverb_tail (内置样例)".to_string(), samples::reverb_tail()),
            };
            *state.borrow_mut() = AppState::load(name, wav);
            rebuild_candidates(&state.borrow(), &widgets);
            refresh();
        });
    }

    // Open / export.
    {
        let state = state.clone();
        let widgets = widgets.clone();
        let refresh = refresh.clone();
        let window = window.clone();
        open_btn.connect_clicked(move |_| {
            let dialog = gtk4::FileDialog::new();
            let state = state.clone();
            let widgets = widgets.clone();
            let refresh = refresh.clone();
            dialog.open(Some(&window), None::<&gtk4::gio::Cancellable>, move |res| {
                if let Ok(file) = res {
                    if let Some(path) = file.path() {
                        match wav::read_wav(&path) {
                            Ok(w) => {
                                let name = path
                                    .file_name()
                                    .map(|s| s.to_string_lossy().into_owned())
                                    .unwrap_or_default();
                                *state.borrow_mut() = AppState::load(name, w);
                                rebuild_candidates(&state.borrow(), &widgets);
                                refresh();
                            }
                            Err(e) => widgets.status.set_label(&format!("读取失败: {e}")),
                        }
                    }
                }
            });
        });
    }
    {
        let state = state.clone();
        let widgets = widgets.clone();
        let window = window.clone();
        export_btn.connect_clicked(move |_| {
            let dialog = gtk4::FileDialog::new();
            dialog.set_initial_name(Some("loop_fixed.wav"));
            let state = state.clone();
            let widgets = widgets.clone();
            dialog.save(Some(&window), None::<&gtk4::gio::Cancellable>, move |res| {
                if let Ok(file) = res {
                    if let Some(path) = file.path() {
                        let st = state.borrow();
                        let cut = st.cut();
                        let samples = dsp::build_loop_multichannel(
                            &st.wav.samples,
                            st.wav.channels,
                            &cut,
                            st.fade_frames(),
                            st.curve,
                        );
                        let out = WavData {
                            samples,
                            channels: st.wav.channels,
                            sample_rate: st.wav.sample_rate,
                        };
                        let msg = match wav::write_wav(&path, &out) {
                            Ok(()) => format!("已导出 {}", path.display()),
                            Err(e) => format!("导出失败: {e}"),
                        };
                        widgets.status.set_label(&msg);
                    }
                }
            });
        });
    }

    // Audition buttons.
    let play_material = {
        let player = player.clone();
        let state = state.clone();
        move |kind: Audition| {
            let st = state.borrow();
            let sr = st.wav.sample_rate;
            let material: Vec<f32> = match kind {
                Audition::Original => {
                    with_gaps(audition::original_seam(&st.mono, sr, 1500.0), sr, 3)
                }
                Audition::Loop => audition::processed_loop(&st.loop_samples, 8),
                Audition::Difference => with_gaps(
                    audition::difference(&st.mono, &st.loop_samples, sr, 1000.0),
                    sr,
                    4,
                ),
            };
            player.play(Arc::new(material), sr, 1);
        }
    };
    {
        let play = play_material.clone();
        play_orig.connect_clicked(move |_| play(Audition::Original));
    }
    {
        let play = play_material.clone();
        play_loop.connect_clicked(move |_| play(Audition::Loop));
    }
    {
        let play = play_material.clone();
        play_diff.connect_clicked(move |_| play(Audition::Difference));
    }
    {
        let player = player.clone();
        stop_btn.connect_clicked(move |_| player.stop());
    }

    window.present();
    window
}

#[derive(Clone, Copy)]
enum Audition {
    Original,
    Loop,
    Difference,
}

/// Repeat a short excerpt `n` times with 400 ms of silence between repeats.
fn with_gaps(excerpt: Vec<f32>, sr: u32, n: usize) -> Vec<f32> {
    let gap = vec![0.0f32; (0.4 * sr as f64) as usize];
    let mut out = Vec::new();
    for _ in 0..n {
        out.extend_from_slice(&excerpt);
        out.extend_from_slice(&gap);
    }
    out
}

fn rebuild_candidates(st: &AppState, widgets: &Widgets) {
    while let Some(row) = widgets.candidate_list.row_at_index(0) {
        widgets.candidate_list.remove(&row);
    }
    let sr = st.wav.sample_rate;
    for (i, c) in st.candidates.iter().enumerate() {
        let text = format!(
            "#{}  分数 {:.3}   起 {:.0} ms  止 {:.0} ms  长 {:.0} ms\n跳变 {:.3}  斜率 {:.3}  频带 {:.2}",
            i + 1,
            c.score,
            ms(c.start, sr),
            ms(c.end, sr),
            ms(c.loop_len(), sr),
            c.value_jump,
            c.slope_jump,
            c.band_mismatch
        );
        let label = gtk4::Label::new(Some(&text));
        label.set_halign(gtk4::Align::Start);
        widgets.candidate_list.append(&label);
    }
    widgets.candidate_list.select_row(
        widgets
            .candidate_list
            .row_at_index(st.selected as i32)
            .as_ref(),
    );
}

fn update_metrics(st: &AppState, widgets: &Widgets) {
    let sr = st.wav.sample_rate;
    let cut = st.cut();
    widgets.m_cut.set_label(&format!(
        "{} → {} 帧（{:.0} → {:.0} ms）",
        cut.start,
        cut.end,
        ms(cut.start, sr),
        ms(cut.end, sr)
    ));
    widgets.m_fade.set_label(&format!(
        "{:.0} ms（{} 帧，{}）",
        st.fade_ms,
        st.fade_frames(),
        st.curve.label()
    ));
    widgets.m_disc.set_label(&format!(
        "{:.4}（原始 {:.4}）",
        st.metrics.discontinuity, st.naive_metrics.discontinuity
    ));
    widgets.m_energy.set_label(&format!(
        "{:+.2} dB（原始 {:+.2} dB）",
        st.metrics.energy_diff_db, st.naive_metrics.energy_diff_db
    ));
    let b = st.metrics.band_diff_db;
    widgets
        .m_bands
        .set_label(&format!("{:+.1} / {:+.1} / {:+.1} dB", b[0], b[1], b[2]));
    match st.drift.period_frames {
        Some(p) => {
            widgets.m_drift.set_label(&format!(
                "{:+} 帧（{:+.2} ms），周期 {} 帧",
                st.drift.residual_per_loop,
                st.drift.residual_per_loop as f64 * 1000.0 / sr as f64,
                p
            ));
            let cum = st.drift.cumulative(100);
            widgets.m_drift_cum.set_label(&format!(
                "{:+} 帧（{:+.1} ms）",
                cum,
                cum as f64 * 1000.0 / sr as f64
            ));
        }
        None => {
            widgets.m_drift.set_label("无稳定周期（噪声类素材）");
            widgets.m_drift_cum.set_label("—");
        }
    }
    widgets.status.set_label(&format!(
        "{} — 循环长 {:.0} ms",
        st.name,
        ms(st.loop_samples.len(), sr)
    ));
}

fn draw_overview(cr: &gtk4::cairo::Context, w: f64, h: f64, st: &AppState) {
    let n = st.mono.len();
    if n == 0 {
        return;
    }
    cr.set_source_rgb(0.13, 0.13, 0.15);
    cr.rectangle(0.0, 0.0, w, h);
    let _ = cr.fill();
    // Min/max envelope per pixel column.
    cr.set_source_rgb(0.45, 0.65, 0.95);
    let mid = h / 2.0;
    let per_px = n as f64 / w;
    for px in 0..w as usize {
        let lo = (px as f64 * per_px) as usize;
        let hi = (((px + 1) as f64 * per_px) as usize).min(n);
        if lo >= hi {
            continue;
        }
        let mut mn = 1.0f32;
        let mut mx = -1.0f32;
        for &s in &st.mono[lo..hi] {
            mn = mn.min(s);
            mx = mx.max(s);
        }
        let y0 = mid - mx as f64 * mid * 0.92;
        let y1 = mid - mn as f64 * mid * 0.92;
        cr.rectangle(px as f64, y0, 1.0, (y1 - y0).max(1.0));
    }
    let _ = cr.fill();
    // Cut markers.
    let cut = st.cut();
    let x_of = |frame: usize| frame as f64 / n as f64 * w;
    cr.set_line_width(2.0);
    cr.set_source_rgb(0.3, 0.9, 0.4);
    cr.move_to(x_of(cut.start), 0.0);
    cr.line_to(x_of(cut.start), h);
    let _ = cr.stroke();
    cr.set_source_rgb(0.95, 0.35, 0.3);
    cr.move_to(x_of(cut.end), 0.0);
    cr.line_to(x_of(cut.end), h);
    let _ = cr.stroke();
    // Fade region shading.
    let fade = st.fade_frames();
    cr.set_source_rgba(0.35, 0.55, 0.95, 0.25);
    cr.rectangle(
        x_of(cut.end - fade),
        0.0,
        x_of(cut.end) - x_of(cut.end - fade),
        h,
    );
    let _ = cr.fill();
}

fn draw_seam(cr: &gtk4::cairo::Context, w: f64, h: f64, st: &AppState) {
    let lp = &st.loop_samples;
    if lp.len() < 8 {
        return;
    }
    let sr = st.wav.sample_rate;
    let half = ((0.1 * sr as f64) as usize).min(lp.len() / 2);
    cr.set_source_rgb(0.13, 0.13, 0.15);
    cr.rectangle(0.0, 0.0, w, h);
    let _ = cr.fill();
    // Wrap point marker.
    cr.set_source_rgba(0.95, 0.8, 0.3, 0.6);
    cr.set_line_width(1.5);
    cr.move_to(w / 2.0, 0.0);
    cr.line_to(w / 2.0, h);
    let _ = cr.stroke();
    // Waveform: last `half` frames ++ first `half` frames.
    cr.set_source_rgb(0.55, 0.85, 0.6);
    cr.set_line_width(1.2);
    let total = 2 * half;
    let mid = h / 2.0;
    let mut first = true;
    for i in 0..total {
        let idx = if i < half {
            lp.len() - half + i
        } else {
            i - half
        };
        let x = i as f64 / total as f64 * w;
        let y = mid - lp[idx] as f64 * mid * 0.92;
        if first {
            cr.move_to(x, y);
            first = false;
        } else {
            cr.line_to(x, y);
        }
    }
    let _ = cr.stroke();
}
