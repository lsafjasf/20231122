use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MaskShape {
    Ellipse,
    Rectangle,
}

impl MaskShape {
    pub const ALL: [Self; 2] = [Self::Ellipse, Self::Rectangle];

    pub fn label(self) -> &'static str {
        match self {
            Self::Ellipse => "椭圆",
            Self::Rectangle => "矩形",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MaskConfig {
    pub shape: MaskShape,
    pub center_x: f64,
    pub center_y: f64,
    pub radius_x: f64,
    pub radius_y: f64,
    pub feather_px: f64,
    pub expand_px: f64,
    pub outside_darkness: f64,
}

impl Default for MaskConfig {
    fn default() -> Self {
        Self {
            shape: MaskShape::Ellipse,
            center_x: 0.5,
            center_y: 0.5,
            radius_x: 0.24,
            radius_y: 0.34,
            feather_px: 18.0,
            expand_px: 0.0,
            outside_darkness: 0.58,
        }
    }
}

impl MaskConfig {
    pub fn sanitized(mut self) -> Self {
        self.center_x = self.center_x.clamp(0.0, 1.0);
        self.center_y = self.center_y.clamp(0.0, 1.0);
        self.radius_x = self.radius_x.clamp(0.01, 1.0);
        self.radius_y = self.radius_y.clamp(0.01, 1.0);
        self.feather_px = self.feather_px.clamp(0.0, 200.0);
        self.expand_px = self.expand_px.clamp(-200.0, 200.0);
        self.outside_darkness = self.outside_darkness.clamp(0.0, 1.0);
        self
    }

    pub fn edge_rect(&self, width: f64, height: f64) -> (f64, f64, f64, f64) {
        let cx = self.center_x * width;
        let cy = self.center_y * height;
        let rx = (self.radius_x * width + self.expand_px).max(1.0);
        let ry = (self.radius_y * height + self.expand_px).max(1.0);
        (cx, cy, rx, ry)
    }

    pub fn summary(&self) -> String {
        format!(
            "{} · 中心 {:.1}%/{:.1}% · 半径 {:.1}%/{:.1}% · 羽化 {:.0}px · 边缘 {:+.0}px",
            self.shape.label(),
            self.center_x * 100.0,
            self.center_y * 100.0,
            self.radius_x * 100.0,
            self.radius_y * 100.0,
            self.feather_px,
            self.expand_px
        )
    }
}

pub fn draw_mask(
    context: &cairo::Context,
    config: &MaskConfig,
    width: f64,
    height: f64,
    badge: &str,
) {
    let config = config.clone().sanitized();
    let (cx, cy, rx, ry) = config.edge_rect(width, height);
    let feather = config.feather_px.min(width.min(height) / 4.0);

    context.new_path();
    context.rectangle(0.0, 0.0, width, height);
    append_shape(context, config.shape, cx, cy, rx, ry);
    context.set_fill_rule(cairo::FillRule::EvenOdd);
    context.set_source_rgba(0.02, 0.03, 0.04, config.outside_darkness);
    let _ = context.fill();

    if config.outside_darkness > 0.0 {
        context.new_path();
        append_shape(context, config.shape, cx, cy, rx, ry);
        context.set_line_width((feather * 2.0).max(1.0));
        context.set_source_rgba(0.10, 0.76, 0.86, 0.18);
        let _ = context.stroke();
    }

    context.new_path();
    append_shape(context, config.shape, cx, cy, rx, ry);
    context.set_line_width(2.2);
    context.set_dash(&[10.0, 6.0], 0.0);
    context.set_source_rgba(0.30, 0.94, 1.0, 0.96);
    let _ = context.stroke();
    context.set_dash(&[], 0.0);

    if feather > 0.5 {
        let inner_rx = (rx - feather).max(1.0);
        let inner_ry = (ry - feather).max(1.0);
        context.new_path();
        append_shape(context, config.shape, cx, cy, inner_rx, inner_ry);
        context.set_line_width(1.0);
        context.set_source_rgba(1.0, 0.78, 0.30, 0.85);
        let _ = context.stroke();
    }

    let handle = (width.min(height) * 0.009).clamp(3.0, 8.0);
    for (px, py) in [
        (cx, cy),
        (cx - rx, cy),
        (cx + rx, cy),
        (cx, cy - ry),
        (cx, cy + ry),
    ] {
        context.arc(px, py, handle, 0.0, std::f64::consts::TAU);
        context.set_source_rgba(0.06, 0.08, 0.10, 0.90);
        let _ = context.fill_preserve();
        context.set_line_width(1.4);
        context.set_source_rgba(0.45, 1.0, 0.95, 1.0);
        let _ = context.stroke();
    }

    if !badge.is_empty() {
        context.select_font_face("Sans", cairo::FontSlant::Normal, cairo::FontWeight::Bold);
        context.set_font_size((width.min(height) * 0.028).clamp(12.0, 28.0));
        context.set_source_rgba(0.04, 0.06, 0.08, 0.82);
        context.rectangle(14.0, 14.0, 188.0, 38.0);
        let _ = context.fill();
        context.move_to(24.0, 41.0);
        context.set_source_rgba(0.55, 1.0, 0.96, 1.0);
        let _ = context.show_text(badge);
    }
}

fn append_shape(context: &cairo::Context, shape: MaskShape, cx: f64, cy: f64, rx: f64, ry: f64) {
    match shape {
        MaskShape::Ellipse => {
            let _ = context.save();
            context.translate(cx, cy);
            context.scale(rx, ry);
            context.arc(0.0, 0.0, 1.0, 0.0, std::f64::consts::TAU);
            let _ = context.restore();
        }
        MaskShape::Rectangle => {
            context.rectangle(cx - rx, cy - ry, rx * 2.0, ry * 2.0);
        }
    }
}
