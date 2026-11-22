use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use gstreamer as gst;
use gtk::prelude::*;
use gtk::{
    Application, ApplicationWindow, Box as GtkBox, Button, CheckButton, DrawingArea, Entry,
    FileDialog, Grid, Label, Orientation, ScrolledWindow, TextView,
};
use gtk4 as gtk;
use tempo_pitch_debug::audit::verify_artifact;
use tempo_pitch_debug::engine::{run_with_renderer, RunRequest};
use tempo_pitch_debug::gst_engine::{play_artifact, GStreamerRenderer};
use tempo_pitch_debug::model::{
    DebugRecord, Limits, TimeSpan, DEFAULT_PITCH_GAP_US, DEFAULT_RENDER_QUANTUM_US,
    DEFAULT_WINDOW_US,
};

fn main() {
    let app = Application::builder()
        .application_id("dev.tempo-pitch.debug-gui")
        .build();
    app.connect_activate(build_ui);
    app.run();
}

fn build_ui(app: &Application) {
    let window = ApplicationWindow::builder()
        .application(app)
        .title("变速不变调试听 / Tempo-Pitch Debug")
        .default_width(1120)
        .default_height(760)
        .build();

    let root = GtkBox::new(Orientation::Vertical, 8);
    root.set_margin_top(12);
    root.set_margin_bottom(12);
    root.set_margin_start(12);
    root.set_margin_end(12);
    window.set_child(Some(&root));

    let grid = Grid::builder().column_spacing(8).row_spacing(6).build();
    grid.set_hexpand(true);
    root.append(&grid);

    let audio_entry = Entry::new();
    let speed_entry = Entry::new();
    let pitch_entry = Entry::new();
    let output_entry = Entry::new();
    let fixture_audio = "fixtures/missing_segment/original.wav";
    let fixture_speed = "fixtures/missing_segment/speed_curve.csv";
    let fixture_pitch = "fixtures/missing_segment/pitch_trajectory.csv";
    audio_entry.set_text(fixture_audio);
    speed_entry.set_text(fixture_speed);
    pitch_entry.set_text(fixture_pitch);
    output_entry.set_text("artifacts/gui-run");

    add_file_row(&grid, 0, "原始音频", &audio_entry, "选择原始音频");
    add_file_row(&grid, 1, "速度曲线", &speed_entry, "选择速度曲线");
    add_file_row(&grid, 2, "音高轨迹", &pitch_entry, "选择音高轨迹");
    add_file_row(&grid, 3, "输出目录", &output_entry, "选择输出目录");

    let missing_check = CheckButton::with_label("启用缺失片段");
    missing_check.set_active(true);
    let missing_start = Entry::builder().text("1250000").width_chars(12).build();
    let missing_end = Entry::builder().text("1500000").width_chars(12).build();
    let window_entry = Entry::builder()
        .text(DEFAULT_WINDOW_US.to_string())
        .width_chars(10)
        .build();
    let quantum_entry = Entry::builder()
        .text(DEFAULT_RENDER_QUANTUM_US.to_string())
        .width_chars(10)
        .build();
    let channel_entry = Entry::builder().text("0").width_chars(5).build();

    let controls = GtkBox::new(Orientation::Horizontal, 8);
    controls.append(&missing_check);
    controls.append(&Label::new(Some("start us")));
    controls.append(&missing_start);
    controls.append(&Label::new(Some("end us")));
    controls.append(&missing_end);
    controls.append(&Label::new(Some("window us")));
    controls.append(&window_entry);
    controls.append(&Label::new(Some("quantum us")));
    controls.append(&quantum_entry);
    controls.append(&Label::new(Some("channel")));
    controls.append(&channel_entry);
    root.append(&controls);

    let render_button = Button::with_label("运行并生成复核产物");
    let verify_button = Button::with_label("复核产物");
    let play_button = Button::with_label("GStreamer 试听");
    let buttons = GtkBox::new(Orientation::Horizontal, 8);
    buttons.append(&render_button);
    buttons.append(&verify_button);
    buttons.append(&play_button);
    root.append(&buttons);

    let status = Label::new(Some(
        "就绪。输出包含 record.json、timeline.csv、waveform.csv 和 output.wav。",
    ));
    status.set_xalign(0.0);
    status.set_wrap(true);
    root.append(&status);

    let draw_area = DrawingArea::new();
    draw_area.set_content_height(180);
    draw_area.set_hexpand(true);
    draw_area.set_vexpand(false);
    root.append(&draw_area);

    let text_view = TextView::new();
    text_view.set_editable(false);
    text_view.set_monospace(true);
    let scroll = ScrolledWindow::new();
    scroll.set_hexpand(true);
    scroll.set_vexpand(true);
    scroll.set_child(Some(&text_view));
    root.append(&scroll);

    let record_slot: Rc<RefCell<Option<DebugRecord>>> = Rc::new(RefCell::new(None));
    let player_slot: Rc<RefCell<Option<gst::Element>>> = Rc::new(RefCell::new(None));

    {
        let record_slot = Rc::clone(&record_slot);
        let draw_area = draw_area.clone();
        draw_area.set_draw_func(move |_, cr, width, height| {
            cr.set_source_rgb(0.08, 0.09, 0.11);
            let _ = cr.paint();
            cr.set_source_rgb(0.25, 0.28, 0.32);
            cr.set_line_width(1.0);
            cr.move_to(0.0, height as f64 / 2.0);
            cr.line_to(width as f64, height as f64 / 2.0);
            let _ = cr.stroke();
            let record_ref = record_slot.borrow();
            let Some(record) = record_ref.as_ref() else {
                return;
            };
            if record.output.duration_us == 0 {
                return;
            }
            for anomaly in &record.anomalies {
                let x0 = anomaly.output_span.start_us as f64 / record.output.duration_us as f64
                    * width as f64;
                let x1 = anomaly.output_span.end_us as f64 / record.output.duration_us as f64
                    * width as f64;
                cr.set_source_rgba(0.85, 0.25, 0.22, 0.22);
                cr.rectangle(x0, 0.0, (x1 - x0).max(1.0), height as f64);
                let _ = cr.fill();
            }
            cr.set_source_rgb(0.22, 0.76, 0.86);
            cr.set_line_width(1.0);
            for entry in &record.timeline {
                let center = (entry.output_start_us + entry.output_end_us) as f64 / 2.0;
                let x = center / record.output.duration_us as f64 * width as f64;
                let mid = height as f64 / 2.0;
                let scale = height as f64 * 0.45;
                let y0 = mid - entry.waveform.max * scale;
                let y1 = mid - entry.waveform.min * scale;
                cr.move_to(x, y0);
                cr.line_to(x, y1);
            }
            let _ = cr.stroke();
            cr.set_source_rgb(0.92, 0.72, 0.24);
            for anomaly in &record.anomalies {
                if record.output.duration_us == 0 {
                    continue;
                }
                let x = anomaly.output_span.start_us as f64 / record.output.duration_us as f64
                    * width as f64;
                cr.move_to(x, 0.0);
                cr.line_to(x, height as f64);
            }
            let _ = cr.stroke();
        });
    }

    {
        let audio_entry = audio_entry.clone();
        let speed_entry = speed_entry.clone();
        let pitch_entry = pitch_entry.clone();
        let output_entry = output_entry.clone();
        let missing_check = missing_check.clone();
        let missing_start = missing_start.clone();
        let missing_end = missing_end.clone();
        let window_entry = window_entry.clone();
        let quantum_entry = quantum_entry.clone();
        let channel_entry = channel_entry.clone();
        let status = status.clone();
        let text_view = text_view.clone();
        let draw_area = draw_area.clone();
        let record_slot = Rc::clone(&record_slot);
        let render_button_for_task = render_button.clone();
        render_button.connect_clicked(move |_| {
            let request = match request_from_entries(
                &audio_entry,
                &speed_entry,
                &pitch_entry,
                &output_entry,
                &missing_check,
                &missing_start,
                &missing_end,
                &window_entry,
                &quantum_entry,
                &channel_entry,
            ) {
                Ok(request) => request,
                Err(error) => {
                    status.set_text(&format!("输入无效: {error}"));
                    return;
                }
            };
            render_button_for_task.set_sensitive(false);
            status.set_text("GStreamer 正在渲染，请稍候…");
            let (sender, receiver) = mpsc::channel();
            std::thread::spawn(move || {
                let result = (|| {
                    let mut renderer = GStreamerRenderer::new()?;
                    run_with_renderer(request, &mut renderer)
                })();
                let _ = sender.send(result);
            });
            let status = status.clone();
            let text_view = text_view.clone();
            let draw_area = draw_area.clone();
            let record_slot = Rc::clone(&record_slot);
            let render_button = render_button_for_task.clone();
            gtk::glib::timeout_add_local(Duration::from_millis(100), move || {
                match receiver.try_recv() {
                    Ok(Ok(record)) => {
                        let summary = summary_text(&record);
                        text_view.buffer().set_text(&summary);
                        status.set_text(&format!(
                            "完成: {}，{} 个异常。产物目录: {}",
                            status_name(record.status),
                            record.anomalies.len(),
                            record.request.output_dir
                        ));
                        *record_slot.borrow_mut() = Some(record);
                        draw_area.queue_draw();
                        render_button.set_sensitive(true);
                        gtk::glib::ControlFlow::Break
                    }
                    Ok(Err(error)) => {
                        status.set_text(&format!("运行失败: {error}"));
                        render_button.set_sensitive(true);
                        gtk::glib::ControlFlow::Break
                    }
                    Err(mpsc::TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        status.set_text("渲染线程意外结束。");
                        render_button.set_sensitive(true);
                        gtk::glib::ControlFlow::Break
                    }
                }
            });
        });
    }

    {
        let status = status.clone();
        let record_slot = Rc::clone(&record_slot);
        verify_button.connect_clicked(move |_| {
            let record_ref = record_slot.borrow();
            let Some(record) = record_ref.as_ref() else {
                status.set_text("请先运行一次渲染。");
                return;
            };
            match verify_artifact(std::path::Path::new(&record.request.output_dir)) {
                Ok(report) => {
                    let passed = report.checks.iter().filter(|check| check.passed).count();
                    status.set_text(&format!(
                        "复核{}: {}/{} 项通过。",
                        if report.passed() { "成功" } else { "失败" },
                        passed,
                        report.checks.len()
                    ));
                }
                Err(error) => status.set_text(&format!("复核失败: {error}")),
            }
        });
    }

    {
        let status = status.clone();
        let record_slot = Rc::clone(&record_slot);
        let player_slot = Rc::clone(&player_slot);
        play_button.connect_clicked(move |_| {
            let record_ref = record_slot.borrow();
            let Some(record) = record_ref.as_ref() else {
                status.set_text("请先运行一次渲染。");
                return;
            };
            match play_artifact(std::path::Path::new(&record.request.output_dir)) {
                Ok(player) => {
                    *player_slot.borrow_mut() = Some(player);
                    status.set_text("GStreamer playbin 正在播放 output.wav。");
                }
                Err(error) => status.set_text(&format!("试听失败: {error}")),
            }
        });
    }

    window.present();
}

fn add_file_row(grid: &Grid, row: i32, label: &str, entry: &Entry, title: &str) {
    grid.attach(&Label::new(Some(label)), 0, row, 1, 1);
    entry.set_hexpand(true);
    grid.attach(entry, 1, row, 1, 1);
    let button = Button::with_label("浏览…");
    grid.attach(&button, 2, row, 1, 1);
    let entry = entry.clone();
    let title = title.to_string();
    button.connect_clicked(move |button| {
        let window = button
            .root()
            .and_then(|root| root.downcast::<ApplicationWindow>().ok());
        let dialog = FileDialog::builder().title(&title).build();
        let entry = entry.clone();
        dialog.open(
            window.as_ref(),
            None::<&gtk::gio::Cancellable>,
            move |result| {
                if let Ok(file) = result {
                    if let Some(path) = file.path() {
                        entry.set_text(&path.display().to_string());
                    }
                }
            },
        );
    });
}

#[allow(clippy::too_many_arguments)]
fn request_from_entries(
    audio: &Entry,
    speed: &Entry,
    pitch: &Entry,
    output: &Entry,
    missing_check: &CheckButton,
    missing_start: &Entry,
    missing_end: &Entry,
    window: &Entry,
    quantum: &Entry,
    channel: &Entry,
) -> Result<RunRequest, String> {
    let parse_u64 = |entry: &Entry, name: &str| -> Result<u64, String> {
        entry
            .text()
            .parse::<u64>()
            .map_err(|_| format!("{name} 必须是整数"))
    };
    let channel = channel
        .text()
        .parse::<u16>()
        .map_err(|_| "channel 必须是 0..65535".to_string())?;
    let missing_span = if missing_check.is_active() {
        Some(TimeSpan {
            start_us: parse_u64(missing_start, "缺失起点")?,
            end_us: parse_u64(missing_end, "缺失终点")?,
        })
    } else {
        None
    };
    Ok(RunRequest {
        audio_path: PathBuf::from(audio.text().as_str()),
        speed_curve_path: PathBuf::from(speed.text().as_str()),
        pitch_track_path: PathBuf::from(pitch.text().as_str()),
        output_dir: PathBuf::from(output.text().as_str()),
        scenario_name: "gtk4-debug-session".to_string(),
        analysed_channel: channel,
        window_us: parse_u64(window, "window_us")?,
        render_quantum_us: parse_u64(quantum, "quantum_us")?,
        pitch_gap_threshold_us: DEFAULT_PITCH_GAP_US,
        missing_span,
        limits: Limits::default(),
    })
}

fn summary_text(record: &DebugRecord) -> String {
    let mut text = format!(
        "job: {}\nstatus: {}\noutput: {} frames, {} us\nspeed: {} .. {}\npitch: {} .. {} st\n\n",
        record.job_id,
        status_name(record.status),
        record.output.frames,
        record.output.duration_us,
        record.processing.speed_parameter.min,
        record.processing.speed_parameter.max,
        record.processing.pitch_parameter.min,
        record.processing.pitch_parameter.max,
    );
    if record.anomalies.is_empty() {
        text.push_str("no anomalies\n");
    } else {
        for anomaly in &record.anomalies {
            text.push_str(&format!(
                "{} {:?}\n  source {}..{} us / frames {}..{}\n  output {}..{} us / frames {}..{}\n  speed {} applied {}; pitch {} st applied {} st (ratio {})\n  evidence {}\n\n",
                anomaly.id,
                anomaly.kind,
                anomaly.source_span.start_us,
                anomaly.source_span.end_us,
                anomaly.source_frames.start,
                anomaly.source_frames.end,
                anomaly.output_span.start_us,
                anomaly.output_span.end_us,
                anomaly.output_frames.start,
                anomaly.output_frames.end,
                anomaly.parameters.speed_rate_curve,
                anomaly.parameters.speed_rate_applied,
                anomaly.parameters.pitch_semitones_curve,
                anomaly.parameters.pitch_semitones_applied,
                anomaly.parameters.pitch_ratio_applied,
                anomaly.message,
            ));
        }
    }
    text.push_str("\n逐窗证据:\n");
    for entry in record
        .timeline
        .iter()
        .filter(|entry| entry.pitch_evidence != tempo_pitch_debug::model::EvidenceState::Observed)
    {
        text.push_str(&format!(
            "#{} src {}..{} out {}..{} f{}..{} speed={} pitch={} ratio={} rms={}\n",
            entry.index,
            entry.source_start_us,
            entry.source_end_us,
            entry.output_start_us,
            entry.output_end_us,
            entry.output_frame_start,
            entry.output_frame_end,
            entry.speed_rate_applied,
            entry.pitch_semitones_applied,
            entry.pitch_ratio_applied,
            entry.waveform.rms,
        ));
    }
    text
}

fn status_name(status: tempo_pitch_debug::model::RunStatus) -> &'static str {
    match status {
        tempo_pitch_debug::model::RunStatus::Complete => "complete",
        tempo_pitch_debug::model::RunStatus::CompleteWithAnomalies => "complete_with_anomalies",
        tempo_pitch_debug::model::RunStatus::EmptyInput => "empty_input",
        tempo_pitch_debug::model::RunStatus::Rejected => "rejected",
    }
}
