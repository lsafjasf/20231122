use anyhow::Result;
use gtk4::pango;
use serde::{Deserialize, Serialize};

use crate::model::{
    BlockStyle, CreditsBlock, CreditsSample, LayoutConfig, Rect,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VisualLine {
    pub line_id: String,
    pub block_id: String,
    pub hard_line_index: usize,
    pub wrap_index: usize,
    pub style: BlockStyle,
    pub text: String,
    pub font_size: f64,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub baseline_y: f64,
    pub line_height: f64,
    pub entry_time: f64,
    pub safe_time: f64,
    pub exit_time: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HardLineMeasure {
    pub block_id: String,
    pub hard_line_index: usize,
    pub text: String,
    pub natural_width: f64,
    pub limit_width: f64,
    pub overflow_amount: f64,
    pub overflows: bool,
    pub wrap_count: usize,
}

pub struct BlockLayout {
    pub block_id: String,
    pub y: f64,
    pub style: BlockStyle,
    pub font_size: f64,
    pub layout: pango::Layout,
    pub height: f64,
}

pub struct CreditsLayout {
    pub sample_id: String,
    pub config: LayoutConfig,
    pub canvas: Rect,
    pub safe_area: Rect,
    pub lines: Vec<VisualLine>,
    pub blocks: Vec<BlockLayout>,
    pub hard_lines: Vec<HardLineMeasure>,
    pub content_height: f64,
    pub duration: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Anchor {
    pub line_id: String,
    pub block_id: String,
    pub hard_line_index: usize,
    pub wrap_index: usize,
    pub text: String,
    pub screen_baseline_y: f64,
    pub viewport_fraction: f64,
}

impl CreditsLayout {
    pub fn content_origin_y(&self) -> f64 {
        self.safe_area.bottom() + self.config.lead_in_px
    }

    pub fn scroll_offset(&self, time: f64) -> f64 {
        self.config.speed_px_per_second * time.clamp(0.0, self.duration)
    }

    pub fn screen_rect(&self, line: &VisualLine, time: f64) -> Rect {
        let shift = self.content_origin_y() - self.scroll_offset(time);
        Rect {
            x: line.x,
            y: shift + line.y,
            width: line.width,
            height: line.height,
        }
    }

    pub fn screen_baseline(&self, line: &VisualLine, time: f64) -> f64 {
        self.content_origin_y() - self.scroll_offset(time) + line.baseline_y
    }

    pub fn fully_out_time(&self, line: &VisualLine) -> f64 {
        ((self.content_origin_y() + line.y + line.height - self.safe_area.y)
            / self.config.speed_px_per_second.max(1.0))
        .clamp(0.0, self.duration)
    }

    pub fn line_at_screen_y(&self, time: f64, screen_y: f64) -> Option<&VisualLine> {
        self.lines
            .iter()
            .filter(|line| {
                let rect = self.screen_rect(line, time);
                screen_y >= rect.y && screen_y <= rect.bottom()
            })
            .min_by(|a, b| {
                let da = (self.screen_baseline(a, time) - screen_y).abs();
                let db = (self.screen_baseline(b, time) - screen_y).abs();
                da.total_cmp(&db)
            })
            .or_else(|| self.nearest_line_to_screen_y(time, screen_y))
    }

    pub fn nearest_line_to_screen_y(&self, time: f64, screen_y: f64) -> Option<&VisualLine> {
        self.lines.iter().min_by(|a, b| {
            let da = (self.screen_baseline(a, time) - screen_y).abs();
            let db = (self.screen_baseline(b, time) - screen_y).abs();
            da.total_cmp(&db)
        })
    }

    pub fn anchor_at(&self, time: f64, focus_y: f64) -> Option<Anchor> {
        let line = self.line_at_screen_y(time, focus_y)?;
        let baseline = self.screen_baseline(line, time);
        Some(Anchor {
            line_id: line.line_id.clone(),
            block_id: line.block_id.clone(),
            hard_line_index: line.hard_line_index,
            wrap_index: line.wrap_index,
            text: line.text.clone(),
            screen_baseline_y: baseline,
            viewport_fraction: (baseline - self.safe_area.y) / self.safe_area.height,
        })
    }

    pub fn time_for_anchor(
        &self,
        block_id: &str,
        hard_line_index: usize,
        target_screen_y: f64,
    ) -> Option<(f64, String)> {
        let candidates: Vec<&VisualLine> = self
            .lines
            .iter()
            .filter(|line| {
                line.block_id == block_id && line.hard_line_index == hard_line_index
            })
            .collect();
        let line = candidates.iter().min_by(|a, b| {
            let target = self.safe_area.y + self.safe_area.height * 0.5;
            let da = (self.screen_baseline(a, 0.0) - target).abs();
            let db = (self.screen_baseline(b, 0.0) - target).abs();
            da.total_cmp(&db)
        })?;
        let raw = (self.content_origin_y() + line.baseline_y - target_screen_y)
            / self.config.speed_px_per_second.max(1.0);
        Some((raw.clamp(0.0, self.duration), line.line_id.clone()))
    }

    pub fn is_fully_safe(&self, line: &VisualLine, time: f64) -> bool {
        let rect = self.screen_rect(line, time);
        rect.y >= self.safe_area.y - 0.5
            && rect.bottom() <= self.safe_area.bottom() + 0.5
            && rect.x >= self.safe_area.x - 0.5
            && rect.right() <= self.safe_area.right() + 0.5
    }
}

pub fn build(sample: &CreditsSample, config: &LayoutConfig) -> Result<CreditsLayout> {
    let width = config.aspect.width as f64;
    let height = config.aspect.height as f64;
    let safe_area = Rect {
        x: width * config.safe_insets.left,
        y: height * config.safe_insets.top,
        width: width * (1.0 - config.safe_insets.left - config.safe_insets.right),
        height: height * (1.0 - config.safe_insets.top - config.safe_insets.bottom),
    };
    let context = crate::pango_cairo::context_with_default_font_map();
    let mut lines = Vec::new();
    let mut blocks = Vec::new();
    let mut hard_lines = Vec::new();
    let mut cursor_y = 0.0;

    for block in &sample.blocks {
        let font_size = config.size_for(block.style);
        let layout = make_layout(
            &context,
            block,
            &config.font_family,
            font_size,
            safe_area.width,
            config.line_gap_factor,
        )?;
        let (_, logical) = layout.extents();
        let block_height = pango::units_to_double(logical.height()).max(font_size * 0.5);
        let block_y = cursor_y;

        collect_block_lines(
            block,
            &layout,
            block_y,
            safe_area,
            font_size,
            &mut lines,
        )?;
        collect_hard_line_measures(
            block,
            &context,
            &config.font_family,
            font_size,
            safe_area.width,
            &mut hard_lines,
        )?;

        blocks.push(BlockLayout {
            block_id: block.id.clone(),
            y: block_y,
            style: block.style,
            font_size,
            layout,
            height: block_height,
        });
        cursor_y += block_height + block_gap(block.style, font_size);
    }

    let content_height = cursor_y.max(1.0);
    let duration = timeline_duration(content_height, safe_area, config);
    let mut result = CreditsLayout {
        sample_id: sample.id.clone(),
        config: config.clone(),
        canvas: Rect { x: 0.0, y: 0.0, width, height },
        safe_area,
        lines,
        blocks,
        hard_lines,
        content_height,
        duration,
    };
    apply_timings(&mut result);
    Ok(result)
}

fn make_layout(
    context: &pango::Context,
    block: &CreditsBlock,
    font_family: &str,
    font_size: f64,
    wrap_width: f64,
    line_gap_factor: f64,
) -> Result<pango::Layout> {
    let layout = pango::Layout::new(context);
    let text = if block.hard_lines.is_empty() {
        String::new()
    } else {
        block.hard_lines.join("\n")
    };
    layout.set_text(&text);
    layout.set_width(pango::units_from_double(wrap_width.max(1.0)));
    layout.set_wrap(pango::WrapMode::WordChar);
    layout.set_alignment(pango::Alignment::Center);
    layout.set_ellipsize(pango::EllipsizeMode::None);
    layout.set_spacing(pango::units_from_double(
        (font_size * line_gap_factor).max(0.0),
    ));
    let mut description = pango::FontDescription::new();
    description.set_family(font_family);
    description.set_absolute_size(pango::units_from_double(font_size) as f64);
    layout.set_font_description(Some(&description));
    let _ = layout.context();
    Ok(layout)
}

fn block_gap(style: BlockStyle, font_size: f64) -> f64 {
    match style {
        BlockStyle::Title => font_size * 0.55,
        BlockStyle::Heading => font_size * 0.35,
        BlockStyle::Body => font_size * 0.22,
        BlockStyle::Caption => font_size * 0.12,
    }
}

fn timeline_duration(content_height: f64, safe_area: Rect, config: &LayoutConfig) -> f64 {
    let distance = config.lead_in_px + content_height + config.tail_hold_px + safe_area.height;
    distance / config.speed_px_per_second.max(1.0)
}

fn collect_block_lines(
    block: &CreditsBlock,
    layout: &pango::Layout,
    block_y: f64,
    safe_area: Rect,
    font_size: f64,
    out: &mut Vec<VisualLine>,
) -> Result<()> {
    let text = block.hard_lines.join("\n");
    let source_line_starts = line_starts(&text);
    let mut iter = layout.iter();
    let mut visual_index = 0usize;
    let mut last_hard_line = None;
    let mut hard_wrap_index = 0usize;
    loop {
        let Some(pango_line) = iter.line_readonly() else {
            break;
        };
        let start = pango_line.start_index().max(0) as usize;
        let length = pango_line.length().max(0) as usize;
        let end = (start + length).min(text.len());
        let fragment = text.get(start..end).unwrap_or_default();
        let hard_line_index = source_line_starts
            .iter()
            .rposition(|offset| *offset <= start)
            .unwrap_or(0);
        if last_hard_line == Some(hard_line_index) {
            hard_wrap_index += 1;
        } else {
            last_hard_line = Some(hard_line_index);
            hard_wrap_index = 0;
        }
        let (_, logical) = iter.line_extents();
        let baseline_units = iter.baseline();
        let (y0, y1) = iter.line_yrange();
        let fragment_trimmed = fragment.trim_end_matches('\n').to_string();
        if !fragment_trimmed.is_empty() {
            let line_height = pango::units_to_double((y1 - y0).max(1));
            out.push(VisualLine {
                line_id: format!("{}-L{}", block.id, visual_index),
                block_id: block.id.clone(),
                hard_line_index,
                wrap_index: hard_wrap_index,
                style: block.style,
                text: fragment_trimmed,
                font_size,
                x: safe_area.x + pango::units_to_double(logical.x()),
                y: block_y + pango::units_to_double(logical.y()),
                width: pango::units_to_double(logical.width()),
                height: line_height,
                baseline_y: block_y + pango::units_to_double(baseline_units),
                line_height,
                entry_time: 0.0,
                safe_time: 0.0,
                exit_time: 0.0,
            });
            visual_index += 1;
        }
        if iter.at_last_line() {
            break;
        }
        iter.next_line();
    }
    Ok(())
}

fn collect_hard_line_measures(
    block: &CreditsBlock,
    context: &pango::Context,
    font_family: &str,
    font_size: f64,
    limit_width: f64,
    out: &mut Vec<HardLineMeasure>,
) -> Result<()> {
    for (hard_line_index, text) in block.hard_lines.iter().enumerate() {
        let layout = pango::Layout::new(context);
        layout.set_text(text);
        let mut description = pango::FontDescription::new();
        description.set_family(font_family);
        description.set_absolute_size(pango::units_from_double(font_size) as f64);
        layout.set_font_description(Some(&description));
        let (_, logical) = layout.extents();
        let natural_width = pango::units_to_double(logical.width());
        let single_line = CreditsBlock {
            id: block.id.clone(),
            style: block.style,
            hard_lines: vec![text.clone()],
        };
        let wrapped = make_layout(
            context,
            &single_line,
            font_family,
            font_size,
            limit_width,
            0.0,
        )?;
        let wrap_count = wrapped.lines().len().max(1);
        let overflow_amount = (natural_width - limit_width).max(0.0);
        out.push(HardLineMeasure {
            block_id: block.id.clone(),
            hard_line_index,
            text: text.clone(),
            natural_width,
            limit_width,
            overflow_amount,
            overflows: overflow_amount > 0.5,
            wrap_count,
        });
    }
    Ok(())
}

fn line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0usize];
    for (index, character) in text.char_indices() {
        if character == '\n' {
            starts.push(index + 1);
        }
    }
    starts
}

fn apply_timings(layout: &mut CreditsLayout) {
    let speed = layout.config.speed_px_per_second.max(1.0);
    let origin = layout.content_origin_y();
    for line in &mut layout.lines {
        line.entry_time = ((origin + line.y - layout.safe_area.bottom()) / speed).max(0.0);
        line.safe_time = ((origin + line.y + line.height - layout.safe_area.bottom()) / speed)
            .max(line.entry_time);
        line.exit_time = ((origin + line.y - layout.safe_area.y) / speed)
            .clamp(0.0, layout.duration);
    }
}
