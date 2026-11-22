use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};
use gtk4 as gtk;

use crate::demo;
use crate::engine::{RunOptions, run};
use crate::model::{RepairReport, WaveformSummary};
use crate::util::{iso8601_utc, unix_millis};

pub fn run_gui() -> glib::ExitCode {
    let app = gtk::Application::builder()
        .application_id("io.github.audio_repair.Studio")
        .build();
    app.connect_activate(build_window);
    app.run()
}

fn build_window(app: &gtk::Application) {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("音频修复版本管理")
        .default_width(1100)
        .default_height(780)
        .build();
    let header = gtk::HeaderBar::new();
    header.set_title_widget(Some(&gtk::Label::new(Some("音频修复版本管理"))));
    window.set_titlebar(Some(&header));

    let root = gtk::Box::new(gtk::Orientation::Vertical, 12);
    root.set_margin_top(16);
    root.set_margin_bottom(16);
    root.set_margin_start(18);
    root.set_margin_end(18);
    window.set_child(Some(&root));

    let intro = gtk::Label::new(Some(
        "统一设备时钟后按版本清单顺序应用修复；产物包含区间/帧映射、处理参数、波形与 SHA-256 证据。",
    ));
    intro.set_xalign(0.0);
    intro.set_wrap(true);
    root.append(&intro);

    let grid = gtk::Grid::new();
    grid.set_column_spacing(10);
    grid.set_row_spacing(8);
    root.append(&grid);
    let audio = gtk::Entry::new();
    let intervals = gtk::Entry::new();
    let versions = gtk::Entry::new();
    let output = gtk::Entry::new();
    add_path_row(&grid, 0, "原始录音", &audio, &window, false);
    add_path_row(&grid, 1, "修复区间 JSON", &intervals, &window, false);
    add_path_row(&grid, 2, "版本清单 JSON", &versions, &window, false);
    add_path_row(&grid, 3, "产物目录（须为新目录）", &output, &window, true);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let demo_button = gtk::Button::with_label("载入失步演示");
    let run_button = gtk::Button::with_label("开始处理");
    run_button.add_css_class("suggested-action");
    let verify_button = gtk::Button::with_label("校验产物");
    let spinner = gtk::Spinner::new();
    let status = gtk::Label::new(Some("就绪"));
    status.set_xalign(0.0);
    status.set_hexpand(true);
    actions.append(&demo_button);
    actions.append(&run_button);
    actions.append(&verify_button);
    actions.append(&spinner);
    actions.append(&status);
    root.append(&actions);

    let paned = gtk::Paned::new(gtk::Orientation::Vertical);
    paned.set_vexpand(true);
    let waveform_area = gtk::DrawingArea::new();
    waveform_area.set_content_height(190);
    waveform_area.set_hexpand(true);
    let waveform_title = gtk::Label::new(Some("输出波形（峰值包络）"));
    waveform_title.set_xalign(0.0);
    let waveform_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let version_dropdown = gtk::DropDown::from_strings(&["等待处理"]);
    waveform_box.append(&waveform_title);
    waveform_box.append(&version_dropdown);
    waveform_box.append(&waveform_area);
    paned.set_start_child(Some(&waveform_box));

    let summary_view = gtk::TextView::new();
    summary_view.set_editable(false);
    summary_view.set_monospace(true);
    summary_view.set_wrap_mode(gtk::WrapMode::WordChar);
    summary_view.buffer().set_text(
        "报告、review.md、review.csv 与 SHA256SUMS 将写入所选产物目录。\n\
         同事可在同一环境执行：audio-repair-cli verify --dir <产物目录>",
    );
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_vexpand(true);
    scroll.set_child(Some(&summary_view));
    paned.set_end_child(Some(&scroll));
    paned.set_position(280);
    root.append(&paned);

    let waveform_store = Rc::new(RefCell::new(Vec::<WaveformSummary>::new()));
    install_version_selector(&version_dropdown, waveform_store.clone(), &waveform_area);

    {
        let audio = audio.clone();
        let intervals = intervals.clone();
        let versions = versions.clone();
        let output = output.clone();
        let status = status.clone();
        demo_button.connect_clicked(move |_| {
            let input_dir = PathBuf::from("demo-input");
            match demo::write_demo_inputs(&input_dir) {
                Ok(paths) => {
                    audio.set_text(&paths.recording.display().to_string());
                    intervals.set_text(&paths.intervals.display().to_string());
                    versions.set_text(&paths.versions.display().to_string());
                    output.set_text(&format!(
                        "demo-output-{}",
                        iso8601_utc(unix_millis()).replace([':', '.'], "-")
                    ));
                    status.set_text("演示输入已生成：B 设备偏移 7312 ms、漂移 -140 ppm");
                }
                Err(error) => status.set_text(&format!("演示场景生成失败：{error:#}")),
            }
        });
    }

    {
        let audio = audio.clone();
        let intervals = intervals.clone();
        let versions = versions.clone();
        let output = output.clone();
        let status = status.clone();
        let spinner = spinner.clone();
        let run_button_clone = run_button.clone();
        let verify_button_clone = verify_button.clone();
        let summary_view = summary_view.clone();
        let version_dropdown = version_dropdown.clone();
        let waveform_area = waveform_area.clone();
        let waveform_store = waveform_store.clone();
        run_button.connect_clicked(move |_| {
            let audio_path = audio.text().to_string();
            let intervals_path = intervals.text().to_string();
            let versions_path = versions.text().to_string();
            let output_path = output.text().to_string();
            if [&audio_path, &intervals_path, &versions_path, &output_path]
                .iter()
                .any(|value| value.trim().is_empty())
            {
                status.set_text("请先选择三份输入并指定新的产物目录");
                return;
            }
            spinner.start();
            run_button_clone.set_sensitive(false);
            verify_button_clone.set_sensitive(false);
            status.set_text("GStreamer 正在解码并生成版本…");
            let options = RunOptions::new(audio_path, intervals_path, versions_path, output_path);
            let status = status.clone();
            let spinner = spinner.clone();
            let run_button = run_button_clone.clone();
            let verify_button = verify_button_clone.clone();
            let summary_view = summary_view.clone();
            let version_dropdown = version_dropdown.clone();
            let waveform_area = waveform_area.clone();
            let waveform_store = waveform_store.clone();
            let (sender, receiver) = std::sync::mpsc::channel::<Result<_, String>>();
            std::thread::spawn(move || {
                let result = run(options).map_err(|error| format!("{error:#}"));
                let _ = sender.send(result);
            });
            glib::timeout_add_local(std::time::Duration::from_millis(80), move || {
                let result = match receiver.try_recv() {
                    Ok(result) => result,
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        return glib::ControlFlow::Continue;
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        spinner.stop();
                        run_button.set_sensitive(true);
                        verify_button.set_sensitive(true);
                        status.set_text("处理线程异常退出");
                        return glib::ControlFlow::Break;
                    }
                };
                spinner.stop();
                run_button.set_sensitive(true);
                verify_button.set_sensitive(true);
                match result {
                    Ok(outcome) => {
                        status.set_text(&format!(
                            "{:?} · run {} · 报告 {}",
                            outcome.report.status,
                            outcome.report.run_id,
                            outcome.report_path.display()
                        ));
                        summary_view
                            .buffer()
                            .set_text(&render_summary(&outcome.report));
                        populate_waveforms(
                            &version_dropdown,
                            &waveform_area,
                            &waveform_store,
                            &outcome.report,
                        );
                    }
                    Err(error) => {
                        status.set_text("处理失败");
                        summary_view
                            .buffer()
                            .set_text(&format!("处理失败：\n{error}"));
                    }
                }
                glib::ControlFlow::Break
            });
        });
    }

    {
        let output = output.clone();
        let status = status.clone();
        let spinner = spinner.clone();
        let verify_button_clone = verify_button.clone();
        let summary_view = summary_view.clone();
        verify_button.connect_clicked(move |_| {
            let path = PathBuf::from(output.text().to_string());
            spinner.start();
            verify_button_clone.set_sensitive(false);
            status.set_text("正在重算产物哈希并核对顺序…");
            let status = status.clone();
            let spinner = spinner.clone();
            let button = verify_button_clone.clone();
            let summary_view = summary_view.clone();
            let (sender, receiver) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let result = crate::verify_bundle(&path).map_err(|error| format!("{error:#}"));
                let _ = sender.send(result);
            });
            glib::timeout_add_local(std::time::Duration::from_millis(80), move || {
                let result = match receiver.try_recv() {
                    Ok(result) => result,
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        return glib::ControlFlow::Continue;
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        spinner.stop();
                        button.set_sensitive(true);
                        status.set_text("校验线程异常退出");
                        return glib::ControlFlow::Break;
                    }
                };
                    spinner.stop();
                    button.set_sensitive(true);
                    match result {
                        Ok(summary) => {
                            status.set_text("校验通过");
                            summary_view.buffer().set_text(&format!(
                                "独立复核通过\nrun_id={}\nstatus={}\nartifacts_verified={}\ninputs_verified={} inputs_missing={}\nwarnings={}",
                                summary.run_id,
                                summary.status,
                                summary.artifacts_verified,
                                summary.inputs_verified,
                                summary.inputs_missing,
                                summary.warnings.len()
                            ));
                        }
                        Err(error) => {
                            status.set_text("校验失败");
                            summary_view
                                .buffer()
                                .set_text(&format!("校验失败：\n{error}"));
                        }
                    }
                glib::ControlFlow::Break
            });
        });
    }

    window.present();
    if std::env::var_os("AUDIO_REPAIR_SMOKE_TEST").is_some() {
        let window = window.clone();
        glib::timeout_add_seconds_local_once(1, move || window.close());
    }
}

fn add_path_row(
    grid: &gtk::Grid,
    row: i32,
    label: &str,
    entry: &gtk::Entry,
    window: &gtk::ApplicationWindow,
    folder: bool,
) {
    let label = gtk::Label::new(Some(label));
    label.set_xalign(0.0);
    entry.set_hexpand(true);
    let button = gtk::Button::with_label(if folder {
        "选择目录"
    } else {
        "选择文件"
    });
    grid.attach(&label, 0, row, 1, 1);
    grid.attach(entry, 1, row, 1, 1);
    grid.attach(&button, 2, row, 1, 1);
    let entry = entry.clone();
    let window = window.clone();
    button.connect_clicked(move |_| {
        let dialog = gtk::FileDialog::builder()
            .title(if folder {
                "选择目录"
            } else {
                "选择文件"
            })
            .modal(true)
            .build();
        let entry = entry.clone();
        if folder {
            dialog.select_folder(Some(&window), None::<&gio::Cancellable>, move |result| {
                if let Ok(file) = result
                    && let Some(path) = file.path()
                {
                    entry.set_text(&path.display().to_string());
                }
            });
        } else {
            dialog.open(Some(&window), None::<&gio::Cancellable>, move |result| {
                if let Ok(file) = result
                    && let Some(path) = file.path()
                {
                    entry.set_text(&path.display().to_string());
                }
            });
        }
    });
}

fn install_version_selector(
    dropdown: &gtk::DropDown,
    store: Rc<RefCell<Vec<WaveformSummary>>>,
    area: &gtk::DrawingArea,
) {
    let selected = Rc::new(std::cell::Cell::new(0_usize));
    let area_for_selector = area.clone();
    let selected_clone = selected.clone();
    dropdown.connect_selected_notify(move |dropdown| {
        selected_clone.set(dropdown.selected() as usize);
        area_for_selector.queue_draw();
    });
    let selected_for_draw = selected.clone();
    let area_for_draw = area.clone();
    area.set_draw_func(move |_, context, width, height| {
        let width = width.max(1);
        let height = height.max(1);
        context.set_source_rgb(0.08, 0.09, 0.11);
        context.rectangle(0.0, 0.0, width as f64, height as f64);
        let _ = context.fill();
        context.set_source_rgb(0.26, 0.29, 0.34);
        context.set_line_width(1.0);
        context.move_to(0.0, height as f64 / 2.0);
        context.line_to(width as f64, height as f64 / 2.0);
        let _ = context.stroke();
        let data = store.borrow();
        let Some(summary) = data.get(selected_for_draw.get()) else {
            return;
        };
        let count = summary.bucket_count.max(1);
        let step = (width as f64 / count as f64).max(1.0);
        context.set_source_rgba(0.38, 0.78, 0.92, 0.85);
        context.set_line_width(1.0);
        for bucket in &summary.buckets {
            let amplitude = bucket.peak.clamp(0.0, 1.0) as f64;
            let bar_height = amplitude * (height as f64 - 24.0);
            let x = bucket.index as f64 * step + step / 2.0;
            context.move_to(x, height as f64 / 2.0 - bar_height / 2.0);
            context.line_to(x, height as f64 / 2.0 + bar_height / 2.0);
        }
        let _ = context.stroke();
        context.set_source_rgba(1.0, 1.0, 1.0, 0.65);
        context.set_font_size(11.0);
        context.move_to(8.0, 17.0);
        let _ = context.show_text(&format!(
            "peak {:.4}  rms {:.4}  bucket {:.1} ms",
            summary.peak, summary.rms, summary.bucket_ms
        ));
    });
    area_for_draw.queue_draw();
}

fn populate_waveforms(
    dropdown: &gtk::DropDown,
    area: &gtk::DrawingArea,
    store: &Rc<RefCell<Vec<WaveformSummary>>>,
    report: &RepairReport,
) {
    let labels = report
        .versions
        .iter()
        .map(|version| format!("{} · {}", version.version_id, version.label))
        .collect::<Vec<_>>();
    let label_refs = labels.iter().map(String::as_str).collect::<Vec<_>>();
    let model = gtk::StringList::new(&label_refs);
    dropdown.set_model(Some(&model));
    dropdown.set_selected(0);
    *store.borrow_mut() = report
        .versions
        .iter()
        .map(|version| version.waveform.clone())
        .collect();
    area.queue_draw();
}

fn render_summary(report: &RepairReport) -> String {
    let mut output = String::new();
    output.push_str(&format!(
        "run_id: {}\nstatus: {:?}\ncreated: {}\nreport: {}\n\n",
        report.run_id, report.status, report.created_at, report.integrity.report_path
    ));
    output.push_str(&format!(
        "media: {} Hz / {} ch / {} frames / {:.3} ms\n\n",
        report.media.decoded_sample_rate,
        report.media.channels,
        report.media.total_frames,
        report.media.duration_ms
    ));
    output.push_str(" clocks\n");
    for clock in &report.clocks {
        output.push_str(&format!(
            "  {} {} offset={:.3}ms drift={:.3}ppm skew={:.3}ms\n",
            clock.device_id,
            clock.role,
            clock.offset_ms,
            clock.drift_ppm,
            clock.max_observed_skew_ms
        ));
    }
    output.push_str("\n interval mapping\n");
    for interval in &report.intervals {
        output.push_str(&format!(
            "  {} [{}] {:.3}..{:.3}ms -> frames {}..{} ({})\n",
            interval.interval_id,
            interval.device_id,
            interval.canonical_start_ms,
            interval.canonical_end_ms,
            interval.applied_start_frame,
            interval.applied_end_frame,
            interval.disposition
        ));
    }
    output.push_str("\n versions (manifest order)\n");
    for version in &report.versions {
        output.push_str(&format!(
            "  #{} {}  output={}  sha256={}  warnings={}\n",
            version.manifest_index,
            version.version_id,
            version.output_path,
            version.output_sha256,
            version.warnings.len()
        ));
    }
    let warnings = report
        .evidence
        .iter()
        .filter(|entry| entry.severity == "warning")
        .collect::<Vec<_>>();
    if !warnings.is_empty() {
        output.push_str("\n warnings / key evidence\n");
        for warning in warnings.iter().take(40) {
            output.push_str(&format!("  {}: {}\n", warning.subject, warning.message));
        }
    }
    output
}
