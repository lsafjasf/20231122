use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use anyhow::Result;
use gtk::prelude::*;
use gtk4 as gtk;

use crate::model::{
    MetricLevel, ProcessingParams, StatisticsProcessor, StatsSnapshot, WaveformPoint,
};
use crate::pipeline::{LiveCapture, PipelineConfig};

const APP_ID: &str = "io.github.encode_panel";
const POLL_MS: u32 = 80;

struct UiState {
    processor: StatisticsProcessor,
    capture: LiveCapture,
    snapshot: StatsSnapshot,
    ingest_errors: u64,
}

pub fn run(
    config: PipelineConfig,
    smoke_ms: Option<u64>,
    screenshot: Option<PathBuf>,
) -> Result<()> {
    let app = gtk::Application::builder().application_id(APP_ID).build();
    app.connect_activate(move |app| {
        if let Err(error) = activate(app, config.clone(), smoke_ms, screenshot.clone()) {
            eprintln!("GTK UI error: {error:#}");
        }
    });
    let _ = app.run_with_args(&["encode-panel"]);
    Ok(())
}

fn activate(
    app: &gtk::Application,
    pipeline_config: PipelineConfig,
    smoke_ms: Option<u64>,
    screenshot: Option<PathBuf>,
) -> Result<()> {
    install_css();
    let capture = LiveCapture::start(pipeline_config.clone())?;
    let params = if pipeline_config.bitrate_kbps < 100 || pipeline_config.nominal_fps() > 120.0 {
        ProcessingParams::extreme()
    } else {
        ProcessingParams::default()
    };
    let state = Rc::new(RefCell::new(UiState {
        processor: StatisticsProcessor::new(params),
        capture,
        snapshot: StatsSnapshot::empty(),
        ingest_errors: 0,
    }));

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Encode Pulse · realtime statistics")
        .default_width(1180)
        .default_height(760)
        .build();
    window.add_css_class("app-window");

    let root = gtk::Box::new(gtk::Orientation::Vertical, 16);
    root.set_margin_top(22);
    root.set_margin_bottom(18);
    root.set_margin_start(22);
    root.set_margin_end(22);
    window.set_child(Some(&root));

    root.append(&build_header(&pipeline_config));
    let cards = build_metric_cards();
    root.append(&cards.grid);

    let waveform = gtk::DrawingArea::new();
    waveform.set_hexpand(true);
    waveform.set_vexpand(true);
    waveform.set_content_width(900);
    waveform.set_content_height(350);
    waveform.add_css_class("waveform");
    let draw_state = state.clone();
    waveform.set_draw_func(move |_, context, width, height| {
        let state = draw_state.borrow();
        draw_waveform(
            context,
            width as f64,
            height as f64,
            &state.snapshot,
            state.processor.params(),
        );
    });
    root.append(&waveform);

    let footer = gtk::Label::new(None);
    footer.set_xalign(0.0);
    footer.add_css_class("footer");
    root.append(&footer);

    window.present();

    let tick_state = state.clone();
    let tick_cards = cards.clone();
    let tick_footer = footer.clone();
    let tick_waveform = waveform.clone();
    gtk::glib::timeout_add_local(Duration::from_millis(POLL_MS as u64), move || {
        {
            let mut state = tick_state.borrow_mut();
            for _ in 0..2_048 {
                let Some(sample) = state.capture.poll_sample() else {
                    break;
                };
                if state.processor.ingest(sample).is_err() {
                    state.ingest_errors = state.ingest_errors.saturating_add(1);
                }
            }
            state.capture.pump_bus();
            state.snapshot = state.processor.snapshot();
            update_cards(&tick_cards, &state.snapshot);
            tick_footer.set_markup(&footer_markup(
                &state.snapshot,
                state.processor.params(),
                state.ingest_errors,
                state.capture.description(),
            ));
        }
        tick_waveform.queue_draw();
        gtk::glib::ControlFlow::Continue
    });

    if let Some(smoke_ms) = smoke_ms {
        let smoke_window = window.clone();
        gtk::glib::timeout_add_local(Duration::from_millis(smoke_ms), move || {
            if let Some(path) = screenshot.as_ref()
                && let Err(error) = capture_window_png(&smoke_window, path)
            {
                eprintln!("GTK screenshot failed: {error:#}");
            }
            smoke_window.close();
            gtk::glib::ControlFlow::Break
        });
    }
    Ok(())
}

fn capture_window_png(window: &gtk::ApplicationWindow, path: &std::path::Path) -> Result<()> {
    let native = window
        .native()
        .ok_or_else(|| anyhow::anyhow!("window is not realized"))?;
    let renderer = native
        .renderer()
        .ok_or_else(|| anyhow::anyhow!("window has no GSK renderer"))?;
    let width = window.width().max(1) as f64;
    let height = window.height().max(1) as f64;
    let paintable = gtk::WidgetPaintable::new(Some(window));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, width, height);
    let node = snapshot
        .to_node()
        .ok_or_else(|| anyhow::anyhow!("GTK snapshot produced no render node"))?;
    let viewport = gtk::graphene::Rect::new(0.0, 0.0, width as f32, height as f32);
    let texture = renderer.render_texture(&node, Some(&viewport));
    texture.save_to_png(path)?;
    Ok(())
}

#[derive(Clone)]
struct MetricCards {
    grid: gtk::Grid,
    fps: gtk::Label,
    bitrate: gtk::Label,
    dropped: gtk::Label,
    samples: gtk::Label,
    state: gtk::Label,
}

fn build_header(config: &PipelineConfig) -> gtk::Box {
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let title_box = gtk::Box::new(gtk::Orientation::Vertical, 3);
    title_box.set_hexpand(true);
    let title = gtk::Label::new(Some("ENCODE PULSE"));
    title.set_xalign(0.0);
    title.add_css_class("title");
    let subtitle = gtk::Label::new(Some(&format!(
        "Rust · GStreamer {} · GTK4  |  {}×{} @ {:.0} fps / {} kbps",
        "1.28",
        config.width,
        config.height,
        config.nominal_fps(),
        config.bitrate_kbps
    )));
    subtitle.set_xalign(0.0);
    subtitle.add_css_class("subtitle");
    title_box.append(&title);
    title_box.append(&subtitle);
    header.append(&title_box);

    let badge = gtk::Label::new(Some("LIVE"));
    badge.add_css_class("live-badge");
    header.append(&badge);
    header
}

fn build_metric_cards() -> MetricCards {
    let grid = gtk::Grid::new();
    grid.set_column_spacing(12);
    grid.set_row_spacing(12);
    grid.set_hexpand(true);

    let (fps_card, fps) = metric_card("FRAME RATE", "— fps");
    let (bitrate_card, bitrate) = metric_card("BITRATE", "— kbps");
    let (dropped_card, dropped) = metric_card("DROPPED / WINDOW", "—");
    let (samples_card, samples) = metric_card("ENCODED / SAMPLES", "0 B · 0");
    let (state_card, state) = metric_card("PIPELINE STATE", "WAITING");

    for (column, card) in [
        &fps_card,
        &bitrate_card,
        &dropped_card,
        &samples_card,
        &state_card,
    ]
    .into_iter()
    .enumerate()
    {
        card.set_hexpand(true);
        grid.attach(card, column as i32, 0, 1, 1);
    }
    MetricCards {
        grid,
        fps,
        bitrate,
        dropped,
        samples,
        state,
    }
}

fn metric_card(name: &str, initial: &str) -> (gtk::Box, gtk::Label) {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 5);
    card.add_css_class("card");
    card.set_size_request(150, 92);
    let caption = gtk::Label::new(Some(name));
    caption.set_xalign(0.0);
    caption.add_css_class("card-caption");
    let value = gtk::Label::new(Some(initial));
    value.set_xalign(0.0);
    value.add_css_class("card-value");
    card.append(&caption);
    card.append(&value);
    (card, value)
}

fn update_cards(cards: &MetricCards, snapshot: &StatsSnapshot) {
    if !snapshot.has_data {
        cards.fps.set_text("— fps");
        cards.bitrate.set_text("— kbps");
        cards.dropped.set_text("—");
        cards.samples.set_text("0 B · 0");
        cards.state.set_text("WAITING");
        cards.state.remove_css_class("critical");
        cards.state.remove_css_class("warning");
        return;
    }
    cards.fps.set_text(&format!("{:.2} fps", snapshot.last_fps));
    cards.bitrate.set_text(&format!(
        "{:.0} kbps",
        snapshot.last_bitrate_bps as f64 / 1_000.0
    ));
    cards.dropped.set_text(&format!(
        "{} (+{})",
        snapshot.last_dropped_total, snapshot.last_dropped_delta
    ));
    cards.samples.set_text(&format!(
        "{} · {}",
        format_bytes(snapshot.last_encoded_bytes),
        snapshot.samples
    ));
    let (label, class) = match snapshot.current_level {
        MetricLevel::Ok => ("NOMINAL", None),
        MetricLevel::Warning => ("WARNING", Some("warning")),
        MetricLevel::Critical => ("CRITICAL", Some("critical")),
    };
    cards.state.set_text(label);
    cards.state.remove_css_class("warning");
    cards.state.remove_css_class("critical");
    if let Some(class) = class {
        cards.state.add_css_class(class);
    }
}

fn footer_markup(
    snapshot: &StatsSnapshot,
    params: &ProcessingParams,
    ingest_errors: u64,
    description: &str,
) -> String {
    let window = if snapshot.waveform.is_empty() {
        "window=empty".to_string()
    } else {
        format!(
            "window={}/{} points · {:.2} fps EMA · {:.0} kbps EMA",
            snapshot.waveform.len(),
            params.window_capacity,
            snapshot.fps_ema,
            snapshot.bitrate_ema / 1_000.0
        )
    };
    let alerts = format!(
        "boundaries={} retained={} dropped={} · warnings={} critical={} · ingest_errors={}",
        snapshot.boundary_events_total,
        snapshot
            .boundary_events_total
            .saturating_sub(snapshot.boundary_events_dropped),
        snapshot.boundary_events_dropped,
        snapshot.warning_samples,
        snapshot.critical_samples,
        ingest_errors,
    );
    format!(
        "<span foreground=\"#7f8ba3\">{}</span>\n<span foreground=\"#536174\">{}</span>\n<span foreground=\"#3b4658\">{}</span>",
        gtk::glib::markup_escape_text(&window),
        gtk::glib::markup_escape_text(&alerts),
        gtk::glib::markup_escape_text(description),
    )
}

fn draw_waveform(
    context: &cairo::Context,
    width: f64,
    height: f64,
    snapshot: &StatsSnapshot,
    params: &ProcessingParams,
) {
    context.set_source_rgb(0.035, 0.047, 0.067);
    let _ = context.paint();
    let left = 58.0;
    let right = 20.0;
    let top = 18.0;
    let bottom = 26.0;
    let lane_gap = (height - top - bottom) / 3.0;
    let lane_height = (lane_gap - 10.0).max(28.0);
    let plot_width = (width - left - right).max(20.0);

    type WaveLane = (&'static str, [f64; 3], fn(&WaveformPoint) -> f64);
    let lanes: [WaveLane; 3] = [
        ("FPS", [0.26, 0.85, 0.77], fps_wave_value),
        ("BITRATE", [0.96, 0.72, 0.38], bitrate_wave_value),
        ("DROPPED", [1.0, 0.43, 0.55], dropped_wave_value),
    ];
    for (index, (name, color, value)) in lanes.into_iter().enumerate() {
        let lane_top = top + index as f64 * lane_gap;
        let baseline = lane_top + lane_height;
        context.set_source_rgb(0.05, 0.07, 0.10);
        rounded_rect(context, left, lane_top, plot_width, lane_height, 6.0);
        let _ = context.fill();
        context.set_source_rgb(0.12, 0.16, 0.22);
        context.set_line_width(1.0);
        rounded_rect(context, left, lane_top, plot_width, lane_height, 6.0);
        let _ = context.stroke();

        context.set_source_rgb(color[0], color[1], color[2]);
        context.select_font_face(
            "monospace",
            cairo::FontSlant::Normal,
            cairo::FontWeight::Bold,
        );
        context.set_font_size(10.5);
        context.move_to(12.0, lane_top + 16.0);
        let _ = context.show_text(name);

        for marker in [0.25_f64, 0.5, 0.75] {
            let y = baseline - marker * (lane_height - 8.0);
            context.set_source_rgb(0.09, 0.12, 0.17);
            context.move_to(left, y);
            context.line_to(left + plot_width, y);
            let _ = context.stroke();
        }

        let points = &snapshot.waveform;
        if points.is_empty() {
            continue;
        }
        context.set_source_rgba(color[0], color[1], color[2], 0.95);
        context.set_line_width(1.8);
        for (point_index, point) in points.iter().enumerate() {
            let x = if points.len() <= 1 {
                left + plot_width / 2.0
            } else {
                left + plot_width * point_index as f64 / (points.len() - 1) as f64
            };
            let y = baseline - value(point).clamp(0.0, 1.0) * (lane_height - 8.0);
            if point_index == 0 {
                context.move_to(x, y);
            } else {
                context.line_to(x, y);
            }
        }
        let _ = context.stroke();
    }

    context.set_source_rgb(0.36, 0.42, 0.52);
    context.set_font_size(10.0);
    let status = if snapshot.waveform.is_empty() {
        "NO DATA · waiting for GStreamer buffers".to_string()
    } else {
        format!(
            "{:.2} fps · {:.0} kbps · +{} dropped · {}/{} window",
            snapshot.last_fps,
            snapshot.last_bitrate_bps as f64 / 1_000.0,
            snapshot.last_dropped_delta,
            snapshot.waveform.len(),
            params.window_capacity
        )
    };
    context.move_to(left, height - 8.0);
    let _ = context.show_text(&status);
}

fn rounded_rect(context: &cairo::Context, x: f64, y: f64, width: f64, height: f64, radius: f64) {
    let radius = radius.min(width / 2.0).min(height / 2.0);
    context.new_sub_path();
    context.arc(
        x + width - radius,
        y + radius,
        radius,
        -std::f64::consts::FRAC_PI_2,
        0.0,
    );
    context.arc(
        x + width - radius,
        y + height - radius,
        radius,
        0.0,
        std::f64::consts::FRAC_PI_2,
    );
    context.arc(
        x + radius,
        y + height - radius,
        radius,
        std::f64::consts::FRAC_PI_2,
        std::f64::consts::PI,
    );
    context.arc(
        x + radius,
        y + radius,
        radius,
        std::f64::consts::PI,
        3.0 * std::f64::consts::FRAC_PI_2,
    );
    context.close_path();
}

fn fps_wave_value(point: &WaveformPoint) -> f64 {
    point.fps_normalized
}

fn bitrate_wave_value(point: &WaveformPoint) -> f64 {
    point.bitrate_normalized
}

fn dropped_wave_value(point: &WaveformPoint) -> f64 {
    point.dropped_normalized
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_000_000 {
        format!("{:.1} MB", bytes as f64 / 1_000_000.0)
    } else if bytes >= 1_000 {
        format!("{:.1} kB", bytes as f64 / 1_000.0)
    } else {
        format!("{bytes} B")
    }
}

fn install_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_data(
        r#"
        .app-window { background: #080b10; color: #e8eef8; }
        .title { font-family: monospace; font-size: 24px; font-weight: 800; letter-spacing: 3px; color: #e8eef8; }
        .subtitle { font-family: monospace; font-size: 11px; color: #6e7b92; }
        .live-badge { background: #173d35; color: #43d9c5; border-radius: 9px; padding: 5px 12px; font-family: monospace; font-weight: 800; }
        .card { background: #0d121a; border: 1px solid #222d3d; border-radius: 9px; padding: 10px 12px; }
        .card-caption { font-family: monospace; font-size: 9px; color: #66748c; letter-spacing: 1px; }
        .card-value { font-family: monospace; font-size: 19px; font-weight: 700; color: #dbe5f4; }
        .card-value.warning { color: #f4b860; }
        .card-value.critical { color: #ff6d8c; }
        .waveform { border-radius: 10px; background: #090d13; }
        .footer { font-family: monospace; font-size: 10px; }
        "#,
    );
    gtk::style_context_add_provider_for_display(
        &gtk::gdk::Display::default().expect("a display is required for GTK4"),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}
