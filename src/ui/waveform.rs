use gtk::cairo::{self, Context};
use gtk::prelude::*;
use gtk4 as gtk;

use crate::model::InspectionReport;

pub fn build_waveform(report: &InspectionReport) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.add_css_class("wave-area");
    area.set_content_height(248);
    area.set_hexpand(true);
    let data = report.clone();
    area.set_draw_func(move |_, cr, width, height| {
        let _ = draw(cr, width, height, &data);
    });
    area
}

fn draw(
    cr: &Context,
    width: i32,
    height: i32,
    report: &InspectionReport,
) -> Result<(), cairo::Error> {
    let width = width as f64;
    let height = height as f64;
    let left = 66.0;
    let right = 18.0;
    let top = 28.0;
    let bottom = 20.0;
    let plot_width = (width - left - right).max(10.0);
    let plot_height = (height - top - bottom).max(10.0);
    let duration = report.source.duration_secs.max(1.0);
    let x_for = |seconds: f64| left + (seconds / duration).clamp(0.0, 1.0) * plot_width;
    let replay_start = report.recovery.replay_start_s;
    let replay_end = report.recovery.replay_end_s;
    let gap_start = report.recovery.replay_start_s - 0.05;

    cr.set_source_rgb(0.047, 0.071, 0.098);
    cr.paint()?;

    cr.set_source_rgba(0.88, 0.93, 0.98, 0.055);
    cr.set_line_width(1.0);
    for second in (0..=18).step_by(3) {
        let x = x_for(second as f64);
        cr.move_to(x, top - 4.0);
        cr.line_to(x, height - bottom);
        cr.stroke()?;
    }

    cr.select_font_face(
        "Noto Sans CJK SC",
        cairo::FontSlant::Normal,
        cairo::FontWeight::Normal,
    );
    cr.set_font_size(8.5);
    cr.set_source_rgba(0.56, 0.64, 0.74, 0.75);
    for second in (0..=18).step_by(3) {
        let x = x_for(second as f64);
        cr.move_to(x + 2.0, 15.0);
        cr.show_text(&format!("00:{second:02}"))?;
    }

    cr.set_source_rgba(0.95, 0.72, 0.36, 0.075);
    cr.rectangle(
        x_for(replay_start),
        top,
        (x_for(replay_end) - x_for(replay_start)).max(1.0),
        plot_height,
    );
    cr.fill()?;
    cr.set_source_rgba(1.0, 0.48, 0.48, 0.10);
    cr.rectangle(
        x_for(gap_start),
        top,
        (x_for(replay_start) - x_for(gap_start)).max(1.0),
        plot_height,
    );
    cr.fill()?;

    let lane_count = report.lanes.len().max(1);
    let lane_height = plot_height / lane_count as f64;
    let colors = [
        (0.38, 0.84, 0.90),
        (0.51, 0.67, 0.98),
        (0.48, 0.87, 0.75),
        (0.95, 0.72, 0.36),
        (0.57, 0.79, 0.90),
        (0.66, 0.62, 0.93),
        (0.45, 0.82, 0.70),
        (0.91, 0.63, 0.63),
    ];
    for (lane_index, lane) in report.lanes.iter().enumerate() {
        let lane_top = top + lane_index as f64 * lane_height;
        let center = lane_top + lane_height * 0.5;
        cr.set_source_rgba(0.88, 0.93, 0.98, 0.055);
        cr.set_line_width(1.0);
        cr.move_to(left, center + 0.5);
        cr.line_to(left + plot_width, center + 0.5);
        cr.stroke()?;

        let color = colors[lane_index % colors.len()];
        let point_count = lane.points.len().max(2);
        let steps = (plot_width.round() as usize).clamp(120, 760);
        let amplitude = lane_height * 0.37;
        for step in 0..steps {
            let ratio = step as f64 / (steps - 1) as f64;
            let index = (ratio * (point_count - 1) as f64).round() as usize;
            let seconds = ratio * duration;
            let (red, green, blue) = if seconds >= replay_start && seconds <= replay_end {
                (0.96, 0.73, 0.36)
            } else if seconds >= gap_start && seconds < replay_start {
                (1.0, 0.47, 0.47)
            } else {
                color
            };
            let alpha = if seconds >= replay_start && seconds <= replay_end {
                0.92
            } else {
                0.47
            };
            cr.set_source_rgba(red, green, blue, alpha);
            cr.set_line_width(1.05);
            let x = left + ratio * plot_width;
            let value = (lane.points[index] as f64).clamp(0.0, 1.0);
            let peak = amplitude * value.max(0.045);
            cr.move_to(x, center - peak);
            cr.line_to(x, center + peak);
            cr.stroke()?;
        }

        cr.set_source_rgba(0.72, 0.80, 0.89, 0.85);
        cr.set_font_size(8.5);
        cr.move_to(8.0, center + 3.0);
        cr.show_text(&format!("{} {}", lane.label, lane.name))?;
        cr.set_source_rgba(0.46, 0.54, 0.63, 0.75);
        cr.set_font_size(6.5);
        cr.move_to(48.0, center + 2.5);
        cr.show_text(&format!("{:.0}", lane.rms_db))?;
    }

    let recovery_x = x_for(replay_start);
    let boundary_band = 5.0;
    cr.set_source_rgba(0.95, 0.72, 0.36, 0.20);
    cr.rectangle(
        recovery_x - boundary_band,
        top,
        boundary_band * 2.0,
        plot_height,
    );
    cr.fill()?;
    cr.set_source_rgba(0.96, 0.75, 0.40, 0.94);
    cr.set_line_width(1.2);
    cr.set_dash(&[4.0, 3.0], 0.0);
    cr.move_to(recovery_x, top - 2.0);
    cr.line_to(recovery_x, height - bottom);
    cr.stroke()?;
    cr.set_dash(&[], 0.0);

    cr.set_source_rgba(0.95, 0.75, 0.42, 0.95);
    cr.move_to(recovery_x, top);
    cr.line_to(recovery_x + 5.0, top + 5.0);
    cr.line_to(recovery_x, top + 10.0);
    cr.line_to(recovery_x - 5.0, top + 5.0);
    cr.close_path();
    cr.fill()?;

    let ribbon_x = x_for(replay_start) + 8.0;
    let ribbon_width = (x_for(replay_end) - x_for(replay_start) - 12.0).max(64.0);
    cr.set_source_rgba(0.95, 0.72, 0.36, 0.16);
    rounded_rect(cr, ribbon_x, 5.0, ribbon_width.min(150.0), 16.0, 5.0);
    cr.fill()?;
    cr.set_source_rgba(0.98, 0.79, 0.47, 0.95);
    cr.set_font_size(8.0);
    cr.move_to(ribbon_x + 7.0, 16.0);
    cr.show_text("RECOVERY REPLAY x1")?;
    Ok(())
}

fn rounded_rect(cr: &Context, x: f64, y: f64, width: f64, height: f64, radius: f64) {
    let radius = radius.min(width / 2.0).min(height / 2.0);
    cr.new_sub_path();
    cr.arc(
        x + width - radius,
        y + radius,
        radius,
        -std::f64::consts::FRAC_PI_2,
        0.0,
    );
    cr.arc(
        x + width - radius,
        y + height - radius,
        radius,
        0.0,
        std::f64::consts::FRAC_PI_2,
    );
    cr.arc(
        x + radius,
        y + height - radius,
        radius,
        std::f64::consts::FRAC_PI_2,
        std::f64::consts::PI,
    );
    cr.arc(
        x + radius,
        y + radius,
        radius,
        std::f64::consts::PI,
        3.0 * std::f64::consts::FRAC_PI_2,
    );
    cr.close_path();
}
