use gtk::prelude::*;

pub fn queue_chart(series: Vec<f64>, threshold: f64) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_height(206);
    area.set_hexpand(true);
    area.set_draw_func(move |_, cr, width, height| {
        let width = f64::from(width);
        let height = f64::from(height);
        let left = 42.0;
        let right = 12.0;
        let top = 14.0;
        let bottom = 24.0;
        let plot_w = (width - left - right).max(1.0);
        let plot_h = (height - top - bottom).max(1.0);
        let max_time = 45.0_f64;
        let max_level = 5.0_f64;
        let x_at = |seconds: f64| left + (seconds / max_time) * plot_w;
        let y_at = |level: f64| top + plot_h - (level / max_level) * plot_h;

        cr.set_source_rgb(0.055, 0.075, 0.105);
        cr.paint().ok();

        cr.set_line_width(1.0);
        for level in 0..=5 {
            let y = y_at(f64::from(level));
            cr.set_source_rgba(0.16, 0.21, 0.28, if level == 0 { 0.8 } else { 0.48 });
            cr.move_to(left, y);
            cr.line_to(width - right, y);
            cr.stroke().ok();
        }

        for second in [0.0, 15.0, 30.0, 45.0] {
            let x = x_at(second);
            cr.set_source_rgba(0.16, 0.21, 0.28, 0.35);
            cr.move_to(x, top);
            cr.line_to(x, top + plot_h);
            cr.stroke().ok();
        }

        let threshold_y = y_at(threshold);
        cr.set_source_rgba(1.0, 0.35, 0.38, 0.055);
        cr.rectangle(left, top, plot_w, threshold_y - top);
        cr.fill().ok();

        cr.set_dash(&[5.0, 5.0], 0.0);
        cr.set_source_rgba(1.0, 0.38, 0.40, 0.68);
        cr.set_line_width(1.0);
        cr.move_to(left, threshold_y);
        cr.line_to(width - right, threshold_y);
        cr.stroke().ok();
        cr.set_dash(&[], 0.0);

        let mut points = Vec::new();
        for (index, value) in series.iter().enumerate() {
            points.push((x_at(index as f64), y_at(*value)));
        }

        if let Some((first_x, _)) = points.first() {
            cr.move_to(*first_x, top + plot_h);
            for (x, y) in &points {
                cr.line_to(*x, *y);
            }
            if let Some((last_x, _)) = points.last() {
                cr.line_to(*last_x, top + plot_h);
                cr.close_path();
            }
            cr.set_source_rgba(0.23, 0.82, 0.70, 0.10);
            cr.fill().ok();
        }

        if let Some((first_x, first_y)) = points.first() {
            cr.move_to(*first_x, *first_y);
            for (x, y) in points.iter().skip(1) {
                cr.line_to(*x, *y);
            }
            cr.set_line_width(4.5);
            cr.set_source_rgba(0.25, 0.84, 0.72, 0.10);
            cr.stroke().ok();

            cr.move_to(*first_x, *first_y);
            for (x, y) in points.iter().skip(1) {
                cr.line_to(*x, *y);
            }
            cr.set_line_width(1.8);
            cr.set_source_rgba(0.30, 0.90, 0.78, 1.0);
            cr.stroke().ok();
        }

        let cancel_x = x_at(18.42);
        cr.set_dash(&[2.0, 3.0], 0.0);
        cr.set_source_rgba(1.0, 0.45, 0.48, 0.72);
        cr.move_to(cancel_x, top);
        cr.line_to(cancel_x, top + plot_h);
        cr.stroke().ok();
        cr.set_dash(&[], 0.0);
        draw_marker(cr, cancel_x, top, "CANCEL #1");

        let cancel_x = x_at(41.227);
        cr.set_dash(&[2.0, 3.0], 0.0);
        cr.set_source_rgba(1.0, 0.42, 0.46, 0.85);
        cr.move_to(cancel_x, top);
        cr.line_to(cancel_x, top + plot_h);
        cr.stroke().ok();
        cr.set_dash(&[], 0.0);
        draw_marker(cr, cancel_x, top, "CANCEL #2");

        cr.set_source_rgba(0.03, 0.04, 0.06, 0.68);
        cr.rectangle(cancel_x, top, width - right - cancel_x, plot_h);
        cr.fill().ok();
        cr.set_source_rgba(0.70, 0.55, 0.30, 0.55);
        cr.set_font_size(8.5);
        cr.move_to(cancel_x + 7.0, top + 15.0);
        cr.show_text("GAP").ok();

        if let Some((last_x, last_y)) = points.last() {
            cr.arc(*last_x, *last_y, 3.0, 0.0, std::f64::consts::TAU);
            cr.set_source_rgb(1.0, 0.52, 0.35);
            cr.fill().ok();
        }

        cr.set_font_size(8.5);
        cr.set_source_rgba(0.37, 0.43, 0.52, 1.0);
        for level in 0..=5 {
            let text = format!("{level}");
            let y = y_at(f64::from(level)) + 3.0;
            cr.move_to(13.0, y);
            cr.show_text(&text).ok();
        }
        for (second, text) in [(0.0, "0s"), (15.0, "15s"), (30.0, "30s"), (45.0, "45s")] {
            let x = x_at(second);
            let text_width = if second == 0.0 { 0.0 } else { 9.0 };
            cr.move_to(x - text_width, height - 5.0);
            cr.show_text(text).ok();
        }

        cr.set_font_size(8.0);
        cr.set_source_rgba(0.95, 0.50, 0.32, 0.9);
        cr.move_to(left + 6.0, threshold_y - 5.0);
        cr.show_text("3.50 s LIMIT").ok();
    });
    area
}

fn draw_marker(cr: &gtk::cairo::Context, x: f64, top: f64, text: &str) {
    cr.set_source_rgba(1.0, 0.42, 0.46, 0.9);
    cr.move_to(x, top);
    cr.line_to(x - 4.0, top + 7.0);
    cr.line_to(x + 4.0, top + 7.0);
    cr.close_path();
    cr.fill().ok();
    cr.set_font_size(7.5);
    cr.move_to(x + 6.0, top + 8.0);
    cr.show_text(text).ok();
}

pub fn sparkline(values: Vec<f64>, color: (f64, f64, f64), width: i32) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_width(width);
    area.set_content_height(26);
    area.set_draw_func(move |_, cr, width, height| {
        let width = f64::from(width);
        let height = f64::from(height);
        if values.len() < 2 {
            return;
        }
        let max = values.iter().copied().fold(0.0_f64, f64::max).max(1.0);
        let step = width / (values.len() - 1) as f64;
        cr.move_to(0.0, height);
        for (index, value) in values.iter().enumerate() {
            let x = index as f64 * step;
            let y = height - (value / max) * (height - 4.0) - 2.0;
            cr.line_to(x, y);
        }
        cr.line_to(width, height);
        cr.close_path();
        cr.set_source_rgba(color.0, color.1, color.2, 0.10);
        cr.fill().ok();

        for (index, value) in values.iter().enumerate() {
            let x = index as f64 * step;
            let y = height - (value / max) * (height - 4.0) - 2.0;
            if index == 0 {
                cr.move_to(x, y);
            } else {
                cr.line_to(x, y);
            }
        }
        cr.set_line_width(1.4);
        cr.set_source_rgba(color.0, color.1, color.2, 0.9);
        cr.stroke().ok();
    });
    area
}
