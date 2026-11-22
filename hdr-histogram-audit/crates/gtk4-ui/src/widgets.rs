//! Reusable GTK4 widgets for the source-check page.

use audit_core::model::{SampleRecord, StepState};
use audit_core::pipeline::PipelineRun;
use gtk::cairo;
use gtk::prelude::*;

/// Builds the bar chart rendered from the audited histogram counts.
pub fn histogram_drawing_area(run: &PipelineRun) -> gtk::DrawingArea {
    let run = run.clone();
    let area = gtk::DrawingArea::new();
    area.set_content_width(560);
    area.set_content_height(220);
    area.set_draw_func(move |_widget, cr, width, height| {
        draw_histogram(cr, width, height, &run);
    });
    area
}

fn draw_histogram(cr: &cairo::Context, width: i32, height: i32, run: &PipelineRun) {
    cr.set_source_rgb(0.09, 0.10, 0.12);
    cr.paint().expect("cairo paint");

    let bin_count = run.histogram.counts.len().max(1);
    let max_count = *run.histogram.counts.iter().max().unwrap_or(&1).max(&1) as f64;
    let gap = 4.0_f64;
    let bar_width = (width as f64 - gap * (bin_count + 1) as f64) / bin_count as f64;

    for (index, count) in run.histogram.counts.iter().enumerate() {
        let bar_height = if max_count == 0.0 {
            0.0
        } else {
            (*count as f64 / max_count) * (height as f64 - 48.0)
        };
        let x = gap + index as f64 * (bar_width + gap);
        let y = height as f64 - 28.0 - bar_height;
        let bar_h = bar_height.max(2.0);
        if *count == 0 {
            cr.set_source_rgb(0.28, 0.30, 0.34);
        } else {
            cr.set_source_rgb(0.30, 0.62, 0.95);
        }
        cr.rectangle(x, y, bar_width, bar_h);
        cr.fill().expect("cairo fill");
    }

    cr.set_source_rgb(0.75, 0.77, 0.82);
    cr.set_font_size(11.0);
    cr.move_to(8.0, height as f64 - 10.0);
    let _ = cr.show_text(&format!(
        "分桶数 {} · headroom 1.0 = {:.0} nits",
        bin_count, run.config.headroom_nits
    ));
}

/// Renders the step journal of one record as an evidence list.
pub fn evidence_panel(record: &SampleRecord) -> gtk::Widget {
    let container = gtk::Box::new(gtk::Orientation::Vertical, 6);
    container.set_margin_top(8);
    container.set_margin_bottom(8);
    container.set_margin_start(8);
    container.set_margin_end(8);

    let header = gtk::Label::new(Some(&format!(
        "样本 {} · 来源 {} · {:.1} nits",
        record.raw.sample_id, record.raw.source_element, record.raw.nits
    )));
    header.set_halign(gtk::Align::Start);
    header.add_css_class("heading");
    container.append(&header);

    if let Some(pts) = record.raw.pts_ns {
        let pts_label = gtk::Label::new(Some(&format!("GStreamer PTS: {pts} ns")));
        pts_label.set_halign(gtk::Align::Start);
        pts_label.add_css_class("caption");
        container.append(&pts_label);
    }

    let list = gtk::ListBox::new();
    list.add_css_class("content");
    for step in &record.steps {
        list.append(&step_row(step));
    }
    container.append(&list);
    container.upcast()
}

fn step_row(step: &audit_core::model::ProcessingStep) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Vertical, 4);
    row.set_margin_top(6);
    row.set_margin_bottom(6);

    let title = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let badge = gtk::Label::new(Some(state_label(step.state)));
    badge.add_css_class(state_css_class(step.state));
    let name = gtk::Label::new(Some(&format!("#{} {}", step.index, step.label)));
    name.set_halign(gtk::Align::Start);
    name.set_hexpand(true);
    title.append(&badge);
    title.append(&name);
    row.append(&title);

    let io = gtk::Label::new(Some(&format!(
        "输入 {:.6} → 输出 {:.6}",
        step.input_value, step.output_value
    )));
    io.set_halign(gtk::Align::Start);
    io.add_css_class("caption");
    row.append(&io);

    if let Some(evidence) = &step.evidence {
        let observation = gtk::Label::new(Some(&format!("证据: {}", evidence.observation)));
        observation.set_halign(gtk::Align::Start);
        observation.set_wrap(true);
        row.append(&observation);

        let facts = gtk::Label::new(Some(&evidence.facts.to_string()));
        facts.set_halign(gtk::Align::Start);
        facts.set_wrap(true);
        facts.add_css_class("caption");
        row.append(&facts);

        let source = gtk::Label::new(Some(&format!("回溯原始输入: {}", evidence.source_sample_id)));
        source.set_halign(gtk::Align::Start);
        source.add_css_class("caption");
        row.append(&source);
    }

    row.upcast()
}

fn state_label(state: StepState) -> &'static str {
    match state {
        StepState::Ok => "OK",
        StepState::Indeterminate => "无法判断",
        StepState::Truncated => "已截断",
        StepState::Error => "错误",
    }
}

fn state_css_class(state: StepState) -> &'static str {
    match state {
        StepState::Ok => "badge-ok",
        StepState::Indeterminate => "badge-indeterminate",
        StepState::Truncated => "badge-truncated",
        StepState::Error => "badge-error",
    }
}
