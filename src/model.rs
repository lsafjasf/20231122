use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlockStyle {
    Title,
    Heading,
    Body,
    Caption,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreditsBlock {
    pub id: String,
    pub style: BlockStyle,
    pub hard_lines: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreditsSample {
    pub id: String,
    pub label: String,
    pub blocks: Vec<CreditsBlock>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct AspectPreset {
    pub label: &'static str,
    pub width: u32,
    pub height: u32,
}

pub const ASPECTS: [AspectPreset; 4] = [
    AspectPreset { label: "16:9", width: 1920, height: 1080 },
    AspectPreset { label: "4:3", width: 1440, height: 1080 },
    AspectPreset { label: "21:9", width: 2520, height: 1080 },
    AspectPreset { label: "9:16", width: 1080, height: 1920 },
];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SafeInsets {
    pub left: f64,
    pub right: f64,
    pub top: f64,
    pub bottom: f64,
}

impl Default for SafeInsets {
    fn default() -> Self {
        Self { left: 0.05, right: 0.05, top: 0.10, bottom: 0.10 }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct LayoutConfig {
    pub aspect: AspectPreset,
    pub font_family: String,
    pub body_size: f64,
    pub heading_size: f64,
    pub title_size: f64,
    pub line_gap_factor: f64,
    pub safe_insets: SafeInsets,
    pub speed_px_per_second: f64,
    pub lead_in_px: f64,
    pub tail_hold_px: f64,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            aspect: ASPECTS[0],
            font_family: "Noto Sans CJK SC".to_string(),
            body_size: 36.0,
            heading_size: 52.0,
            title_size: 72.0,
            line_gap_factor: 0.35,
            safe_insets: SafeInsets::default(),
            speed_px_per_second: 120.0,
            lead_in_px: 56.0,
            tail_hold_px: 80.0,
        }
    }
}

impl LayoutConfig {
    pub fn size_for(&self, style: BlockStyle) -> f64 {
        match style {
            BlockStyle::Title => self.title_size,
            BlockStyle::Heading => self.heading_size,
            BlockStyle::Body => self.body_size,
            BlockStyle::Caption => self.body_size * 0.86,
        }
    }

    pub fn scaled_fonts(&mut self, factor: f64) {
        let factor = factor.clamp(0.55, 2.2);
        self.body_size = (36.0 * factor).round();
        self.heading_size = (52.0 * factor).round();
        self.title_size = (72.0 * factor).round();
    }

    pub fn font_scale(&self) -> f64 {
        self.body_size / 36.0
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub fn right(&self) -> f64 {
        self.x + self.width
    }

    pub fn bottom(&self) -> f64 {
        self.y + self.height
    }
}
