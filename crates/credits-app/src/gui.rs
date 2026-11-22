use std::path::Path;

use anyhow::{Context, Result};
use credits_core::report::{load_report, ObjectReview, ReviewReport};
use gtk4::gio::prelude::ApplicationExtManual;
use gtk4::prelude::*;
use gtk4::{Align, Application, ApplicationWindow, Box as GtkBox, Frame, Label, Orientation};

pub fn run(report_path: &Path) -> Result<()> {
    let mut report = load_report(report_path)
        .with_context(|| format!("cannot open report {}", report_path.display()))?;
    report.tool.gtk_version = Some(format!(
        "{}.{}.{}",
        gtk4::major_version(),
        gtk4::minor_version(),
        gtk4::micro_version()
    ));
    let root = report_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let app = Application::builder()
        .application_id("org.creditsreview.Reviewer")
        .build();
    app.connect_activate(move |app| {
        let window = build_window(app, &report, &root);
        window.present();
    });
    app.run_with_args(&["credits-review-gui"]);
    Ok(())
}

fn build_window(app: &Application, report: &ReviewReport, root: &Path) -> ApplicationWindow {
    install_css();
    let window = ApplicationWindow::builder()
        .application(app)
        .title(format!("Credits Review - {}", report.project_title))
        .default_width(1280)
        .default_height(860)
        .build();

    let header = gtk4::HeaderBar::new();
    header.set_title_widget(Some(&Label::new(Some("End-credits typesetting review"))));
    window.set_titlebar(Some(&header));

    let split = gtk4::Paned::new(gtk4::Orientation::Horizontal);
    split.set_position(330);
    split.set_hexpand(true);
    split.set_vexpand(true);

    let sidebar = GtkBox::new(Orientation::Vertical, 8);
    sidebar.set_margin_top(12);
    sidebar.set_margin_bottom(12);
    sidebar.set_margin_start(12);
    sidebar.set_margin_end(12);
    let summary = Label::new(Some(&format!(
        "{}  |  {} ok / {} partial / {} failed\n{} correlations, {} snapshots",
        report.project_format.as_str(),
        report.summary.ok_count,
        report.summary.partial_count,
        report.summary.failed_count,
        report.summary.correlation_count,
        report.summary.snapshot_count
    )));
    summary.set_xalign(0.0);
    summary.set_wrap(true);
    sidebar.append(&summary);

    let list = gtk4::ListBox::new();
    list.set_selection_mode(gtk4::SelectionMode::Single);
    for object in &report.objects {
        let row = gtk4::ListBoxRow::new();
        let row_box = GtkBox::new(Orientation::Vertical, 3);
        row_box.set_margin_top(8);
        row_box.set_margin_bottom(8);
        row_box.set_margin_start(8);
        row_box.set_margin_end(8);
        let title = Label::new(Some(&object.id));
        title.set_xalign(0.0);
        title.add_css_class("object-title");
        row_box.append(&title);
        let detail = Label::new(Some(&format!(
            "{}  |  {}\n{}",
            object.status_label(),
            object
                .probe
                .container
                .as_deref()
                .unwrap_or("unknown container"),
            if object.media.unknown_extension {
                format!(
                    "unknown extension {} (detected)",
                    object.media.extension.as_deref().unwrap_or("<none>")
                )
            } else {
                object
                    .media
                    .extension
                    .clone()
                    .unwrap_or_else(|| "no extension".to_string())
            }
        )));
        detail.set_xalign(0.0);
        detail.set_wrap(true);
        detail.add_css_class("dim");
        row_box.append(&detail);
        row.set_child(Some(&row_box));
        list.append(&row);
    }
    let sidebar_scroll = gtk4::ScrolledWindow::new();
    sidebar_scroll.set_child(Some(&list));
    sidebar_scroll.set_vexpand(true);
    sidebar.append(&sidebar_scroll);
    split.set_start_child(Some(&sidebar));

    let detail_scroll = gtk4::ScrolledWindow::new();
    detail_scroll.set_hexpand(true);
    detail_scroll.set_vexpand(true);
    let detail = GtkBox::new(Orientation::Vertical, 14);
    detail.set_margin_top(16);
    detail.set_margin_bottom(16);
    detail.set_margin_start(18);
    detail.set_margin_end(18);
    detail_scroll.set_child(Some(&detail));
    split.set_end_child(Some(&detail_scroll));
    window.set_child(Some(&split));

    let report_for_select = report.clone();
    let detail_for_select = detail.clone();
    let root_for_select = root.to_path_buf();
    list.connect_row_selected(move |_, row| {
        if let Some(row) = row {
            let index = row.index();
            if index >= 0 {
                if let Some(object) = report_for_select.objects.get(index as usize) {
                    clear_box(&detail_for_select);
                    populate_object(
                        &detail_for_select,
                        &report_for_select,
                        object,
                        &root_for_select,
                    );
                }
            }
        }
    });

    if let Some(first) = list.row_at_index(0) {
        list.select_row(Some(&first));
    } else {
        populate_empty(&detail, report);
    }
    window
}

fn populate_object(container: &GtkBox, report: &ReviewReport, object: &ObjectReview, root: &Path) {
    let title = Label::new(Some(&format!("{}  [{}]", object.id, object.status_label())));
    title.set_xalign(0.0);
    title.add_css_class("title-1");
    container.append(&title);

    let meta = Label::new(Some(&format!(
        "{}\nsha256 {}\nsize {} bytes | extension {} | unknown={} | magic {}",
        object.media.path.display(),
        object.media.sha256,
        object.media.size_bytes,
        object.media.extension.as_deref().unwrap_or("<none>"),
        object.media.unknown_extension,
        object.media.magic_hex
    )));
    meta.set_xalign(0.0);
    meta.set_wrap(true);
    meta.add_css_class("evidence");
    container.append(&meta);

    if object.media.unknown_extension {
        let warning = Label::new(Some(
            "Unknown extension: processing was driven by GStreamer content detection, not the filename.",
        ));
        warning.set_xalign(0.0);
        warning.set_wrap(true);
        warning.add_css_class("warning");
        container.append(&warning);
    }

    container.append(&probe_grid(&object.probe));
    if !object.warnings.is_empty() {
        let warning = Label::new(Some(&object.warnings.join("\n")));
        warning.set_xalign(0.0);
        warning.set_wrap(true);
        warning.add_css_class("warning");
        container.append(&warning);
    }
    for correlation in &object.correlations {
        container.append(&correlation_card(report, object, correlation, root));
    }
    container.append(&evidence_list(object));
    if !report.failures.is_empty() {
        container.append(&failure_panel(report));
    }
}

fn populate_empty(container: &gtk4::Box, report: &ReviewReport) {
    let label = Label::new(Some("No review objects were produced."));
    label.set_xalign(0.0);
    container.append(&label);
    if !report.failures.is_empty() {
        container.append(&failure_panel(report));
    }
}

fn clear_box(container: &GtkBox) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

fn install_css() {
    let provider = gtk4::CssProvider::new();
    provider.load_from_data(
        "
        .dim { color: #64748b; font-size: 11px; }
        .object-title { font-weight: 700; }
        .warning { color: #b45309; font-weight: 600; }
        .evidence { font-family: monospace; font-size: 10px; color: #475569; }
        .timecode { font-family: monospace; font-size: 13px; font-weight: 700; color: #0f766e; }
        .card { background: #f8fafc; border-radius: 8px; padding: 10px; }
        .snapshot-label { font-weight: 700; color: #334155; }
        ",
    );
    gtk4::style_context_add_provider_for_display(
        &gtk4::gdk::Display::default().expect("GTK4 display must be available"),
        &provider,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

fn probe_grid(probe: &credits_core::report::ProbeEvidence) -> Frame {
    let frame = Frame::new(Some("GStreamer probe"));
    let grid = gtk4::Grid::new();
    grid.set_row_spacing(6);
    grid.set_column_spacing(18);
    grid.set_margin_top(8);
    grid.set_margin_bottom(8);
    grid.set_margin_start(8);
    grid.set_margin_end(8);
    let mut row = 0;
    grid_add(
        &grid,
        row,
        "container",
        probe.container.as_deref().unwrap_or("-"),
    );
    row += 1;
    grid_add(
        &grid,
        row,
        "video codec",
        probe.video_codec.as_deref().unwrap_or("-"),
    );
    row += 1;
    grid_add(
        &grid,
        row,
        "audio codec",
        probe.audio_codec.as_deref().unwrap_or("-"),
    );
    row += 1;
    grid_add(
        &grid,
        row,
        "geometry",
        &format!(
            "{} x {} @ {}",
            probe
                .width
                .map(|v| v.to_string())
                .unwrap_or_else(|| "?".to_string()),
            probe
                .height
                .map(|v| v.to_string())
                .unwrap_or_else(|| "?".to_string()),
            probe
                .fps
                .map(|fps| format!("{fps:.3} fps"))
                .unwrap_or_else(|| "? fps".to_string())
        ),
    );
    row += 1;
    grid_add(
        &grid,
        row,
        "duration",
        &probe
            .duration_s
            .map(|duration| format!("{duration:.3} s"))
            .unwrap_or_else(|| "-".to_string()),
    );
    row += 1;
    grid_add(&grid, row, "uri", &probe.uri);
    row += 1;
    if let Some(error) = &probe.discovery_error {
        grid_add(&grid, row, "discovery error", error);
    }
    frame.set_child(Some(&grid));
    frame
}

fn correlation_card(
    report: &ReviewReport,
    object: &ObjectReview,
    correlation: &credits_core::report::CorrelationEvidence,
    root: &Path,
) -> Frame {
    let frame = Frame::new(Some(&format!(
        "Checkpoint {} / {}",
        correlation.index + 1,
        correlation
            .time
            .label
            .clone()
            .unwrap_or_else(|| correlation.time.clock.clone())
    )));
    frame.add_css_class("card");
    let body = GtkBox::new(Orientation::Vertical, 10);
    body.set_margin_top(10);
    body.set_margin_bottom(10);
    body.set_margin_start(10);
    body.set_margin_end(10);

    let time = Label::new(Some(&format!(
        "{} / {} / frame {} / {} / {}",
        correlation.time.clock,
        correlation.time.timecode,
        correlation.time.frame,
        correlation.time.track,
        correlation
            .time
            .pts_ns
            .map(|pts| format!("pts {pts} ns"))
            .unwrap_or_else(|| "pts unknown".to_string())
    )));
    time.set_xalign(0.0);
    time.add_css_class("timecode");
    body.append(&time);

    let snapshots = GtkBox::new(Orientation::Horizontal, 12);
    snapshots.set_homogeneous(true);
    for snapshot in &correlation.versions {
        snapshots.append(&snapshot_view(snapshot, root));
    }
    body.append(&snapshots);

    let diff = Label::new(Some(&format!(
        "waveform peak {} / window {} samples / pixel diff {} / changed {}",
        correlation.waveform_peak_index,
        correlation.waveform_window.len(),
        correlation
            .pixel_diff
            .as_ref()
            .map(|diff| format!("{:.3}", diff.mean_abs))
            .unwrap_or_else(|| "-".to_string()),
        correlation
            .pixel_diff
            .as_ref()
            .map(|diff| format!("{:.2}%", diff.changed_ratio * 100.0))
            .unwrap_or_else(|| "-".to_string())
    )));
    diff.set_xalign(0.0);
    diff.add_css_class("dim");
    body.append(&diff);

    body.append(&waveform_view(object, correlation, report));
    body.append(&parameter_grid(&correlation.parameters));
    frame.set_child(Some(&body));
    frame
}

fn snapshot_view(snapshot: &credits_core::report::SnapshotEvidence, root: &Path) -> Frame {
    let frame = Frame::new(None);
    let body = GtkBox::new(Orientation::Vertical, 6);
    body.set_margin_top(8);
    body.set_margin_bottom(8);
    body.set_margin_start(8);
    body.set_margin_end(8);
    let label = Label::new(Some(&format!(
        "{} / frame {}",
        snapshot.version.as_str().to_uppercase(),
        snapshot.frame
    )));
    label.set_xalign(0.0);
    label.add_css_class("snapshot-label");
    body.append(&label);
    let path = root.join(&snapshot.file);
    if path.exists() {
        let picture = gtk4::Picture::for_filename(&path);
        picture.set_can_shrink(true);
        picture.set_size_request(420, 236);
        body.append(&picture);
    } else {
        let missing = Label::new(Some(&format!("missing evidence: {}", path.display())));
        missing.add_css_class("warning");
        body.append(&missing);
    }
    let hash = Label::new(Some(&format!(
        "{}... / {} / {}x{}",
        &snapshot.sha256[..snapshot.sha256.len().min(16)],
        snapshot.timecode,
        snapshot.width,
        snapshot.height
    )));
    hash.set_xalign(0.0);
    hash.add_css_class("evidence");
    body.append(&hash);
    frame.set_child(Some(&body));
    frame
}

fn waveform_view(
    object: &ObjectReview,
    correlation: &credits_core::report::CorrelationEvidence,
    _report: &ReviewReport,
) -> Frame {
    let frame = Frame::new(Some("Waveform / output evidence"));
    let body = GtkBox::new(Orientation::Vertical, 4);
    body.set_margin_top(8);
    body.set_margin_bottom(8);
    body.set_margin_start(8);
    body.set_margin_end(8);
    let peaks = object.waveform.peaks.clone();
    let marker = correlation.waveform_peak_index;
    let available = object.waveform.available;
    let reason = object.waveform.reason.clone();
    let area = gtk4::DrawingArea::new();
    area.set_content_height(150);
    area.set_hexpand(true);
    area.set_draw_func(move |_, cr, width, height| {
        draw_waveform(cr, width, height, &peaks, marker, available);
    });
    body.append(&area);
    let text = if available {
        format!(
            "{} peaks @ {} ms | duration {:.3}s | marker {} | {}",
            object.waveform.peaks.len(),
            object.waveform.bucket_ms,
            object.waveform.duration_s,
            marker,
            object.waveform_json
        )
    } else {
        format!(
            "waveform unavailable: {}",
            reason.unwrap_or_else(|| "unknown reason".to_string())
        )
    };
    let label = Label::new(Some(&text));
    label.set_xalign(0.0);
    label.add_css_class("dim");
    body.append(&label);
    frame.set_child(Some(&body));
    frame
}

fn draw_waveform(
    cr: &gtk4::cairo::Context,
    width: i32,
    height: i32,
    peaks: &[credits_core::waveform::Peak],
    marker: usize,
    available: bool,
) {
    let width = width.max(1) as f64;
    let height = height.max(1) as f64;
    cr.set_source_rgb(0.06, 0.09, 0.13);
    cr.rectangle(0.0, 0.0, width, height);
    let _ = cr.fill();
    cr.set_source_rgb(0.25, 0.32, 0.42);
    cr.set_line_width(1.0);
    cr.move_to(0.0, height * 0.5);
    cr.line_to(width, height * 0.5);
    let _ = cr.stroke();
    if !available || peaks.is_empty() {
        return;
    }
    let middle = height * 0.5;
    let half = height * 0.46;
    let step = width / peaks.len().max(1) as f64;
    cr.set_source_rgb(0.22, 0.74, 0.97);
    cr.set_line_width((step * 0.8).clamp(0.4, 2.0));
    for (index, peak) in peaks.iter().enumerate() {
        let x = index as f64 * step + step * 0.5;
        cr.move_to(x, middle - f64::from(peak.max) * half);
        cr.line_to(x, middle - f64::from(peak.min) * half);
    }
    let _ = cr.stroke();
    let x =
        (marker.min(peaks.len().saturating_sub(1)) as f64 * step + step * 0.5).clamp(0.0, width);
    cr.set_source_rgb(0.96, 0.62, 0.04);
    cr.set_line_width(1.5);
    cr.move_to(x, 0.0);
    cr.line_to(x, height);
    let _ = cr.stroke();
}

fn parameter_grid(parameters: &credits_core::model::TypesetParameters) -> Frame {
    let frame = Frame::new(Some("Processing parameters"));
    let grid = gtk4::Grid::new();
    grid.set_row_spacing(5);
    grid.set_column_spacing(16);
    grid.set_margin_top(8);
    grid.set_margin_bottom(8);
    grid.set_margin_start(8);
    grid.set_margin_end(8);
    let rows = [
        (
            "font",
            format!("{} {}", parameters.font_family, parameters.font_size),
        ),
        ("color", parameters.color.clone()),
        ("shadow", parameters.shadow.to_string()),
        ("scroll", format!("{} px/s", parameters.scroll_speed_px_s)),
        ("roll start", format!("{} s", parameters.roll_start_s)),
        ("margin", format!("{:.3}", parameters.horizontal_margin)),
        ("line spacing", format!("{:.2}", parameters.line_spacing)),
        ("opacity", format!("{:.2}", parameters.opacity)),
    ];
    for (row, (key, value)) in rows.iter().enumerate() {
        grid_add(&grid, row as i32, key, value);
    }
    frame.set_child(Some(&grid));
    frame
}

fn evidence_list(object: &ObjectReview) -> Frame {
    let frame = Frame::new(Some("Retained evidence"));
    let text = object
        .evidence_files
        .iter()
        .map(|entry| {
            format!(
                "{}  {}",
                &entry.sha256[..16.min(entry.sha256.len())],
                entry.path
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let label = Label::new(Some(&text));
    label.set_xalign(0.0);
    label.set_wrap(true);
    label.set_selectable(true);
    label.add_css_class("evidence");
    label.set_margin_top(8);
    label.set_margin_bottom(8);
    label.set_margin_start(8);
    label.set_margin_end(8);
    frame.set_child(Some(&label));
    frame
}

fn failure_panel(report: &ReviewReport) -> Frame {
    let frame = Frame::new(Some("Failures (other objects continued)"));
    let body = GtkBox::new(Orientation::Vertical, 6);
    body.set_margin_top(8);
    body.set_margin_bottom(8);
    body.set_margin_start(8);
    body.set_margin_end(8);
    for failure in &report.failures {
        let label = Label::new(Some(&format!(
            "{} [{}] {}\nkept: {}",
            failure.object_id,
            failure.stage,
            failure.message,
            failure.evidence_dir.as_deref().unwrap_or("-")
        )));
        label.set_xalign(0.0);
        label.set_wrap(true);
        label.add_css_class("warning");
        body.append(&label);
    }
    frame.set_child(Some(&body));
    frame
}

fn grid_add(grid: &gtk4::Grid, row: i32, key: &str, value: &str) {
    let key_label = Label::new(Some(key));
    key_label.set_xalign(0.0);
    key_label.set_valign(Align::Start);
    key_label.add_css_class("dim");
    grid.attach(&key_label, 0, row, 1, 1);
    let value_label = Label::new(Some(value));
    value_label.set_xalign(0.0);
    value_label.set_wrap(true);
    if value.starts_with("sha") || value.contains(char::is_whitespace) && value.len() > 80 {
        value_label.add_css_class("evidence");
    }
    grid.attach(&value_label, 1, row, 1, 1);
}

trait ReviewUi {
    fn status_label(&self) -> &'static str;
}

impl ReviewUi for ObjectReview {
    fn status_label(&self) -> &'static str {
        match self.status {
            credits_core::report::ObjectStatus::Ok => "OK",
            credits_core::report::ObjectStatus::Partial => "PARTIAL",
            credits_core::report::ObjectStatus::Failed => "FAILED",
        }
    }
}
