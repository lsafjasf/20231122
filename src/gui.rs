use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use anyhow::Context;
use gtk4::cairo;
use gtk4::prelude::*;
use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, Button, DrawingArea, Frame, Grid,
    HeaderBar, Label, Orientation, Picture, Scale,
};

use crate::evidence;
use crate::model::{EvidenceFile, FrameSample};

struct InspectorData {
    evidence: EvidenceFile,
    samples: Vec<FrameSample>,
    out: PathBuf,
}

pub fn run(out: &Path) -> anyhow::Result<()> {
    let evidence = evidence::read_evidence(&out.join("evidence.json"))?;
    let trace_path = Path::new(&evidence.outputs.trace_jsonl);
    let samples = load_trace(trace_path)
        .with_context(|| format!("cannot load trace {}", trace_path.display()))?;
    let data = Rc::new(InspectorData {
        evidence,
        samples,
        out: out.to_path_buf(),
    });
    let app = Application::builder()
        .application_id("dev.mousetrail.evidence")
        .build();
    app.connect_activate(move |app| build_window(app, data.clone()));
    app.run_with_args::<&str>(&[]);
    Ok(())
}

fn load_trace(path: &Path) -> anyhow::Result<Vec<FrameSample>> {
    let text = fs::read_to_string(path)?;
    let mut samples = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let sample = serde_json::from_str(line)
            .with_context(|| format!("invalid trace row {}", index + 1))?;
        samples.push(sample);
    }
    Ok(samples)
}

fn build_window(app: &Application, data: Rc<InspectorData>) {
    let window = ApplicationWindow::builder()
        .application(app)
        .title("Mouse Trail Evidence Inspector")
        .default_width(1180)
        .default_height(850)
        .build();
    let header = HeaderBar::new();
    let title = Label::new(Some("Mouse Trail Evidence Inspector"));
    title.add_css_class("title");
    header.set_title_widget(Some(&title));
    let status = Label::new(Some(&format!(
        "{} | {} anchors | source_unchanged={}",
        data.evidence.status,
        data.evidence.anchors.len(),
        data.evidence.source_unchanged
    )));
    header.pack_end(&status);
    window.set_titlebar(Some(&header));

    let root = GtkBox::new(Orientation::Vertical, 8);
    root.set_margin_top(12);
    root.set_margin_bottom(12);
    root.set_margin_start(12);
    root.set_margin_end(12);
    let summary = Label::new(Some(&format!(
        "GStreamer {} | GTK {} | params {} | video {} frames @ {}x{}",
        data.evidence.implementation.media_version,
        data.evidence.implementation.ui_version,
        &data.evidence.params_sha256[..16],
        data.evidence.inputs.video.frames,
        data.evidence.inputs.video.width.unwrap_or(0),
        data.evidence.inputs.video.height.unwrap_or(0),
    )));
    summary.set_xalign(0.0);
    summary.add_css_class("dim-label");
    root.append(&summary);

    let content = GtkBox::new(Orientation::Horizontal, 16);
    let preview_box = GtkBox::new(Orientation::Vertical, 8);
    let picture = Picture::new();
    picture.set_size_request(640, 360);
    picture.set_content_fit(gtk4::ContentFit::Contain);
    picture.set_hexpand(true);
    picture.set_vexpand(true);
    preview_box.append(&picture);
    let anchor_label = Label::new(Some("select an anchor"));
    anchor_label.set_xalign(0.0);
    anchor_label.set_selectable(true);
    preview_box.append(&anchor_label);
    let details = Label::new(None);
    details.set_xalign(0.0);
    details.set_yalign(0.0);
    details.set_wrap(true);
    details.set_selectable(true);
    details.set_hexpand(true);
    details.set_width_chars(48);
    let details_frame = Frame::new(Some("raw evidence"));
    details_frame.set_child(Some(&details));
    preview_box.append(&details_frame);
    preview_box.set_size_request(660, -1);
    content.append(&preview_box);

    let side = GtkBox::new(Orientation::Vertical, 8);
    side.set_hexpand(true);
    let anchor_title = Label::new(Some("three anchors"));
    anchor_title.set_xalign(0.0);
    anchor_title.add_css_class("title-4");
    side.append(&anchor_title);
    let anchor_grid = Grid::new();
    anchor_grid.set_row_spacing(8);
    anchor_grid.set_column_spacing(8);
    let mut buttons = Vec::new();
    for (index, anchor) in data.evidence.anchors.iter().enumerate() {
        let button = Button::with_label(&format!(
            "{}  {:.3}s  offset {}",
            anchor.id,
            anchor.pts_ns as f64 / 1e9,
            anchor.raw_offset
        ));
        button.set_halign(Align::Fill);
        anchor_grid.attach(&button, 0, index as i32, 1, 1);
        buttons.push(button);
    }
    side.append(&anchor_grid);

    let selected = Rc::new(Cell::new(0usize));
    let waveform = DrawingArea::new();
    waveform.set_content_height(260);
    waveform.set_hexpand(true);
    let data_for_draw = data.clone();
    let selected_for_draw = selected.clone();
    waveform.set_draw_func(move |_, cr, width, height| {
        draw_waveform(cr, width, height, &data_for_draw, selected_for_draw.get());
    });
    let waveform_frame = Frame::new(Some("output waveform"));
    waveform_frame.set_child(Some(&waveform));
    side.append(&waveform_frame);

    let last_frame = data.samples.len().saturating_sub(1) as f64;
    let scale = Scale::with_range(Orientation::Horizontal, 0.0, last_frame.max(1.0), 1.0);
    scale.set_digits(0);
    scale.set_draw_value(true);
    scale.set_hexpand(true);
    side.append(&scale);
    let cursor = Label::new(Some(""));
    cursor.set_xalign(0.0);
    cursor.set_selectable(true);
    side.append(&cursor);
    content.append(&side);
    root.append(&content);
    window.set_child(Some(&root));

    wire_controls(
        &scale,
        &picture,
        &anchor_label,
        &details,
        &cursor,
        &waveform,
        &buttons,
        &selected,
        data,
    );
    window.present();
    scale.set_value(10.0);
}

#[allow(clippy::too_many_arguments)]
fn wire_controls(
    scale: &Scale,
    picture: &Picture,
    anchor_label: &Label,
    details: &Label,
    cursor: &Label,
    waveform: &DrawingArea,
    buttons: &[Button],
    selected: &Rc<Cell<usize>>,
    data: Rc<InspectorData>,
) {
    let picture_for_scale = picture.clone();
    let anchor_label_for_scale = anchor_label.clone();
    let details_for_scale = details.clone();
    let cursor_for_scale = cursor.clone();
    let waveform_for_scale = waveform.clone();
    let data_for_scale = data.clone();
    let selected_for_scale = selected.clone();
    scale.connect_value_changed(move |scale| {
        let index = scale.value().round().max(0.0) as usize;
        selected_for_scale.set(index);
        waveform_for_scale.queue_draw();
        if let Some(sample) = data_for_scale.samples.get(index) {
            cursor_for_scale.set_text(&format!(
                "frame {} | PTS {:.3}s | x={:?} y={:?} | speed {:.1}px/s | flags={}",
                sample.frame_index,
                sample.pts_ns as f64 / 1e9,
                sample.x,
                sample.y,
                sample.speed_px_s,
                if sample.flags.is_empty() {
                    "none".to_string()
                } else {
                    sample.flags.join(",")
                }
            ));
        }
        if let Some(anchor_index) = data_for_scale
            .evidence
            .anchors
            .iter()
            .position(|anchor| anchor.frame_index == index as u64)
        {
            let anchor = &data_for_scale.evidence.anchors[anchor_index];
            if let Some(frame) = data_for_scale.evidence.outputs.anchor_frames.get(anchor_index) {
                picture_for_scale.set_filename(Some(data_for_scale.out.join(frame)));
            }
            anchor_label_for_scale.set_text(&format!(
                "{} | {} | raw offset {} length {}",
                anchor.id, anchor.kind, anchor.raw_offset, anchor.raw_length
            ));
            details_for_scale.set_text(&format!(
                "anchor: {}\nPTS: {} ns\nframe: {}\ntrack: {}\nraw: ({}, {})\noutput: ({}, {})\nraw offset: {}\nraw length: {}\nraw sha256: {}\nparams sha256: {}\nflags: {}\nraw JSON: {}",
                anchor.id,
                anchor.pts_ns,
                anchor.frame_index,
                anchor.track_id,
                anchor.raw_x,
                anchor.raw_y,
                anchor.output_x,
                anchor.output_y,
                anchor.raw_offset,
                anchor.raw_length,
                anchor.raw_sha256,
                anchor.params_sha256,
                anchor.flags.join(", "),
                anchor.raw_json.trim_end(),
            ));
        }
    });

    for (index, button) in buttons.iter().enumerate() {
        let picture = picture.clone();
        let anchor_label = anchor_label.clone();
        let details = details.clone();
        let scale = scale.clone();
        let data = data.clone();
        button.connect_clicked(move |_| {
            let Some(anchor) = data.evidence.anchors.get(index) else {
                return;
            };
            if let Some(frame) = data.evidence.outputs.anchor_frames.get(index) {
                picture.set_filename(Some(data.out.join(frame)));
            }
            anchor_label.set_text(&format!(
                "{} | {} | raw offset {} length {}",
                anchor.id, anchor.kind, anchor.raw_offset, anchor.raw_length
            ));
            details.set_text(&format!(
                "anchor: {}\nPTS: {} ns\nframe: {}\nraw: ({}, {})\noutput: ({}, {})\nraw offset: {}\nraw sha256: {}\nflags: {}\nraw JSON: {}",
                anchor.id,
                anchor.pts_ns,
                anchor.frame_index,
                anchor.raw_x,
                anchor.raw_y,
                anchor.output_x,
                anchor.output_y,
                anchor.raw_offset,
                anchor.raw_sha256,
                anchor.flags.join(", "),
                anchor.raw_json.trim_end(),
            ));
            scale.set_value(anchor.frame_index as f64);
        });
    }
}

fn draw_waveform(
    cr: &cairo::Context,
    width: i32,
    height: i32,
    data: &InspectorData,
    selected: usize,
) {
    let width = width as f64;
    let height = height as f64;
    cr.set_source_rgb(0.055, 0.085, 0.115);
    cr.paint().ok();
    let left = 48.0;
    let right = width - 20.0;
    let top = 30.0;
    let bottom = height - 34.0;
    let plot_width = (right - left).max(1.0);
    let plot_height = (bottom - top).max(1.0);
    cr.set_source_rgb(0.12, 0.16, 0.21);
    cr.rectangle(left, top, plot_width, plot_height);
    cr.fill().ok();
    cr.set_source_rgb(0.20, 0.26, 0.33);
    cr.set_line_width(1.0);
    for step in 0..=4 {
        let y = top + plot_height * step as f64 / 4.0;
        cr.move_to(left, y);
        cr.line_to(right, y);
        cr.stroke().ok();
    }
    cr.select_font_face(
        "monospace",
        cairo::FontSlant::Normal,
        cairo::FontWeight::Normal,
    );
    cr.set_font_size(12.0);
    cr.set_source_rgb(0.72, 0.80, 0.88);
    cr.move_to(left, 18.0);
    cr.show_text("x/y position waveform | teal X | orange Y | purple speed | A/B/C anchors")
        .ok();
    cr.set_source_rgb(0.45, 0.53, 0.62);
    cr.move_to(right - 76.0, bottom + 22.0);
    cr.show_text("4.000s").ok();

    let count = data.samples.len().max(1) as f64;
    let x_at = |index: usize| left + (index as f64 / (count - 1.0).max(1.0)) * plot_width;
    let max_speed = data
        .samples
        .iter()
        .map(|sample| sample.speed_px_s)
        .fold(1.0_f64, f64::max);

    draw_series(
        cr,
        &data.samples,
        x_at,
        |sample| sample.x.map(|value| value / 319.0),
        top,
        plot_height,
        (0.26, 0.85, 0.78),
    );
    draw_series(
        cr,
        &data.samples,
        x_at,
        |sample| sample.y.map(|value| value / 179.0),
        top,
        plot_height,
        (1.0, 0.70, 0.33),
    );
    draw_series(
        cr,
        &data.samples,
        x_at,
        |sample| Some(sample.speed_px_s / max_speed),
        top,
        plot_height,
        (0.91, 0.42, 0.61),
    );

    for anchor in &data.evidence.anchors {
        let index = anchor.frame_index as usize;
        let x = x_at(index);
        let (red, green, blue) = match anchor.id.as_str() {
            "A" => (0.48, 0.63, 0.97),
            "B" => (0.75, 0.60, 1.0),
            _ => (0.97, 0.46, 0.56),
        };
        cr.set_source_rgb(red, green, blue);
        cr.set_line_width(1.4);
        cr.set_dash(&[5.0, 5.0], 0.0);
        cr.move_to(x, top);
        cr.line_to(x, bottom);
        cr.stroke().ok();
        cr.set_dash(&[], 0.0);
        cr.move_to(x + 4.0, top - 4.0);
        cr.show_text(&format!("{} {:.1}s", anchor.id, anchor.pts_ns as f64 / 1e9))
            .ok();
    }

    let selected_x = x_at(selected.min(data.samples.len().saturating_sub(1)));
    cr.set_source_rgb(0.94, 0.98, 1.0);
    cr.set_line_width(1.0);
    cr.move_to(selected_x, top);
    cr.line_to(selected_x, bottom);
    cr.stroke().ok();
    if let Some(sample) = data.samples.get(selected) {
        cr.set_source_rgb(0.72, 0.80, 0.88);
        cr.move_to(left, 18.0);
        cr.set_source_rgba(0.055, 0.085, 0.115, 0.0);
        cr.rectangle(left, 5.0, plot_width, 16.0);
        cr.fill().ok();
        cr.set_source_rgb(0.72, 0.80, 0.88);
        cr.move_to(right - 235.0, 18.0);
        cr.show_text(&format!(
            "frame {} @ {:.3}s",
            sample.frame_index,
            sample.pts_ns as f64 / 1e9
        ))
        .ok();
    }
}

fn draw_series<F, G>(
    cr: &cairo::Context,
    samples: &[FrameSample],
    x_at: F,
    value: G,
    top: f64,
    height: f64,
    color: (f64, f64, f64),
) where
    F: Fn(usize) -> f64,
    G: Fn(&FrameSample) -> Option<f64>,
{
    cr.set_source_rgb(color.0, color.1, color.2);
    cr.set_line_width(1.8);
    let mut drawing = false;
    for (index, sample) in samples.iter().enumerate() {
        let Some(current) = value(sample).filter(|_| sample.visible) else {
            drawing = false;
            continue;
        };
        let x = x_at(index);
        let y = top + height - current.clamp(0.0, 1.0) * height;
        if drawing {
            cr.line_to(x, y);
        } else {
            cr.move_to(x, y);
            drawing = true;
        }
    }
    cr.stroke().ok();
}
