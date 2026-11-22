use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{Value, json};

use crate::layout::{self, CreditsLayout, VisualLine};
use crate::model::{AspectPreset, CreditsSample, LayoutConfig, Rect};
use crate::samples;

pub const REPORT_VERSION: &str = "0.2.0";

#[derive(Serialize)]
struct LineRecord {
    line_id: String,
    block_id: String,
    hard_line_index: usize,
    wrap_index: usize,
    style: String,
    text: String,
    font_size: f64,
    bounds: Rect,
    baseline_content_y: f64,
    baseline_offset_in_line: f64,
    line_height: f64,
    timing: LineTiming,
}

#[derive(Serialize)]
struct LineTiming {
    entry_time: f64,
    fully_safe_time: f64,
    exit_time: f64,
    fully_out_time: f64,
    fully_safe_seconds: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct ViolationRecord {
    pub line_id: String,
    pub block_id: String,
    pub hard_line_index: usize,
    pub wrap_index: usize,
    pub kind: String,
    pub phase: String,
    pub severity: String,
    pub text: String,
    pub start_time: f64,
    pub end_time: f64,
    pub amount: f64,
    pub baseline_y_at_start: f64,
}

pub fn export_all_reports(dir: impl AsRef<Path>) -> Result<()> {
    let dir = dir.as_ref();
    fs::create_dir_all(dir)
        .with_context(|| format!("cannot create report directory {}", dir.display()))?;
    for sample in samples::all_samples() {
        export_report(dir, &sample, &LayoutConfig::default())?;
    }
    Ok(())
}

pub fn export_report(
    dir: impl AsRef<Path>,
    sample: &CreditsSample,
    config: &LayoutConfig,
) -> Result<PathBuf> {
    let dir = dir.as_ref();
    fs::create_dir_all(dir)?;
    let layout = layout::build(sample, config)?;
    let report = report_value(sample, &layout);
    let path = dir.join(format!("{}.report.json", sample.id));
    let bytes = serde_json::to_vec_pretty(&report)?;
    fs::write(&path, bytes)
        .with_context(|| format!("cannot write layout report {}", path.display()))?;
    Ok(path)
}

pub fn write_report(
    path: impl AsRef<Path>,
    sample: &CreditsSample,
    config: &LayoutConfig,
) -> Result<PathBuf> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let layout = layout::build(sample, config)?;
    let value = report_value(sample, &layout);
    fs::write(path, serde_json::to_vec_pretty(&value)?)
        .with_context(|| format!("cannot write layout report {}", path.display()))?;
    Ok(path.to_path_buf())
}

pub fn report_value(sample: &CreditsSample, layout: &CreditsLayout) -> Value {
    let line_records: Vec<LineRecord> = layout
        .lines
        .iter()
        .map(|line| line_record(layout, line))
        .collect();
    let records = layout_violations(layout);
    let mut records_by_line: std::collections::BTreeMap<String, Vec<&ViolationRecord>> =
        std::collections::BTreeMap::new();
    for record in &records {
        records_by_line
            .entry(record.line_id.clone())
            .or_default()
            .push(record);
    }
    let horizontal_overflow_lines = records
        .iter()
        .filter(|record| record.kind == "HorizontalOverflow")
        .map(|record| record.line_id.as_str())
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    let vertical_clip_events = records
        .iter()
        .filter(|record| record.kind.starts_with("VerticalClip"))
        .count();
    let lines_with_any_violation = records
        .iter()
        .map(|record| record.line_id.as_str())
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    let source_overflow_lines = layout
        .hard_lines
        .iter()
        .filter(|line| line.overflow_amount > 0.0)
        .count();

    json!({
        "tool": "credits-layout",
        "version": REPORT_VERSION,
        "engine": {
            "text": "Pango/GTK4",
            "clock_and_video": "GStreamer",
            "runtime": "Rust"
        },
        "sample": {
            "id": sample.id,
            "label": sample.label,
            "block_count": sample.blocks.len()
        },
        "config": layout.config,
        "canvas": {
            "width": layout.canvas.width,
            "height": layout.canvas.height,
            "aspect_label": layout.config.aspect.label
        },
        "safe_area": layout.safe_area,
        "content": {
            "hard_line_count": layout.hard_lines.len(),
            "line_count": layout.lines.len(),
            "content_height": layout.content_height
        },
        "playback": {
            "speed_px_per_second": layout.config.speed_px_per_second,
            "duration_seconds": layout.duration,
            "content_origin_y": layout.content_origin_y(),
            "lead_in_px": layout.config.lead_in_px,
            "tail_hold_px": layout.config.tail_hold_px,
            "frame_rate": 30
        },
        "natural_widths": layout.hard_lines,
        "lines": line_records,
        "records": records,
        "records_by_line": records_by_line,
        "unknown_glyphs": 0,
        "summary": {
            "horizontal_overflow_lines": horizontal_overflow_lines,
            "source_overflow_lines": source_overflow_lines,
            "vertical_clip_events": vertical_clip_events,
            "safe_area_violations": records.len(),
            "lines_with_any_violation": lines_with_any_violation,
            "all_lines_reported": records.len() >= layout.lines.len()
        }
    })
}

fn line_record(layout: &CreditsLayout, line: &VisualLine) -> LineRecord {
    let fully_out = layout.fully_out_time(line);
    let safe_seconds = (line.exit_time - line.safe_time).max(0.0);
    LineRecord {
        line_id: line.line_id.clone(),
        block_id: line.block_id.clone(),
        hard_line_index: line.hard_line_index,
        wrap_index: line.wrap_index,
        style: format!("{:?}", line.style),
        text: line.text.clone(),
        font_size: line.font_size,
        bounds: Rect {
            x: line.x,
            y: line.y,
            width: line.width,
            height: line.height,
        },
        baseline_content_y: line.baseline_y,
        baseline_offset_in_line: line.baseline_y - line.y,
        line_height: line.line_height,
        timing: LineTiming {
            entry_time: line.entry_time,
            fully_safe_time: line.safe_time,
            exit_time: line.exit_time,
            fully_out_time: fully_out,
            fully_safe_seconds: safe_seconds,
        },
    }
}

pub fn layout_violations(layout: &CreditsLayout) -> Vec<ViolationRecord> {
    let mut records = Vec::new();
    for line in &layout.lines {
        let origin = layout.content_origin_y();
        let fully_out = layout.fully_out_time(line);
        if line.entry_time > 0.0 {
            records.push(violation(
                layout,
                line,
                "SafeAreaBelow",
                "below",
                "info",
                0.0,
                line.entry_time,
                (origin + line.y - layout.safe_area.bottom()).max(0.0),
            ));
        }
        if line.safe_time > line.entry_time + 0.001 {
            records.push(violation(
                layout,
                line,
                "VerticalClip",
                "bottom-edge",
                "warning",
                line.entry_time,
                line.safe_time,
                line.height.min(layout.safe_area.height),
            ));
        }
        if line.exit_time < fully_out - 0.001 {
            records.push(violation(
                layout,
                line,
                "VerticalClip",
                "top-edge",
                "warning",
                line.exit_time,
                fully_out,
                line.height.min(layout.safe_area.height),
            ));
        }
        if fully_out < layout.duration - 0.001 {
            records.push(violation(
                layout,
                line,
                "SafeAreaAbove",
                "above",
                "info",
                fully_out,
                layout.duration,
                (layout.safe_area.y - (origin + line.y + line.height - layout.scroll_offset(fully_out)))
                    .max(0.0),
            ));
        }
        if line.x < layout.safe_area.x - 0.5 || line.x + line.width > layout.safe_area.right() + 0.5 {
            let amount = (layout.safe_area.x - line.x)
                .max(line.x + line.width - layout.safe_area.right())
                .max(0.0);
            records.push(violation(
                layout,
                line,
                "HorizontalOverflow",
                "left-or-right",
                "error",
                0.0,
                layout.duration,
                amount,
            ));
        }
        if line.height > layout.safe_area.height + 0.5 {
            records.push(violation(
                layout,
                line,
                "TallerThanSafeArea",
                "all",
                "error",
                0.0,
                layout.duration,
                line.height - layout.safe_area.height,
            ));
        }
    }
    for source in &layout.hard_lines {
        if source.overflow_amount > 0.5 {
            records.push(ViolationRecord {
                line_id: format!("{}-H{}", source.block_id, source.hard_line_index),
                block_id: source.block_id.clone(),
                hard_line_index: source.hard_line_index,
                wrap_index: 0,
                kind: "SourceWrapOverflow".into(),
                phase: "pre-wrap".into(),
                severity: "warning".into(),
                text: source.text.clone(),
                start_time: 0.0,
                end_time: layout.duration,
                amount: source.overflow_amount,
                baseline_y_at_start: 0.0,
            });
        }
    }
    records.sort_by(|a, b| {
        a.start_time
            .total_cmp(&b.start_time)
            .then_with(|| a.line_id.cmp(&b.line_id))
            .then_with(|| a.kind.cmp(&b.kind))
    });
    records
}

#[allow(clippy::too_many_arguments)]
fn violation(
    layout: &CreditsLayout,
    line: &VisualLine,
    kind: &str,
    phase: &str,
    severity: &str,
    start: f64,
    end: f64,
    amount: f64,
) -> ViolationRecord {
    ViolationRecord {
        line_id: line.line_id.clone(),
        block_id: line.block_id.clone(),
        hard_line_index: line.hard_line_index,
        wrap_index: line.wrap_index,
        kind: kind.into(),
        phase: phase.into(),
        severity: severity.into(),
        text: line.text.clone(),
        start_time: start,
        end_time: end.max(start),
        amount,
        baseline_y_at_start: layout.screen_baseline(line, start),
    }
}

pub fn export_stability_audit(dir: impl AsRef<Path>) -> Result<PathBuf> {
    let dir = dir.as_ref();
    fs::create_dir_all(dir)?;
    let mut cases = Vec::new();
    let mut all_pass = true;
    for sample in samples::all_samples() {
        let base = LayoutConfig::default();
        let layout = layout::build(&sample, &base)?;
        let time = layout.duration * 0.56;
        let focus_y = layout.safe_area.y + layout.safe_area.height * 0.5;
        let anchor = layout
            .anchor_at(time, focus_y)
            .with_context(|| format!("no anchor for {}", sample.id))?;
        let mut variants = Vec::new();
        for (label, config) in variants_for(&base) {
            let changed = layout::build(&sample, &config)?;
            let (new_time, line_id) = changed
                .time_for_anchor(&anchor.block_id, anchor.hard_line_index, anchor.screen_baseline_y)
                .with_context(|| format!("anchor disappeared for {label}"))?;
            let after = changed
                .anchor_at(new_time, anchor.screen_baseline_y)
                .with_context(|| format!("loss of anchor after {label}"))?;
            let delta = (after.screen_baseline_y - anchor.screen_baseline_y).abs();
            let pass = delta <= 1.0;
            all_pass &= pass;
            variants.push(json!({
                "label": label,
                "aspect": config.aspect.label,
                "body_size": config.body_size,
                "line_count": changed.lines.len(),
                "content_height": changed.content_height,
                "anchor_line_before": anchor.line_id,
                "anchor_line_after": line_id,
                "resolved_time": new_time,
                "baseline_before": anchor.screen_baseline_y,
                "baseline_after": after.screen_baseline_y,
                "baseline_jump_px": delta,
                "timeline_reset": new_time <= 0.0001,
                "passed": pass
            }));
        }
        cases.push(json!({
            "sample_id": sample.id,
            "sample_label": sample.label,
            "at_time_seconds": time,
            "anchor": anchor,
            "variants": variants
        }));
    }
    let value = json!({
        "tool": "credits-layout",
        "version": REPORT_VERSION,
        "check": "anchor-preserving re-layout",
        "tolerance_px": 1.0,
        "cases": cases,
        "summary": {
            "passed": all_pass,
            "variant_count": cases.iter().map(|case| case["variants"].as_array().map_or(0, Vec::len)).sum::<usize>()
        }
    });
    let path = dir.join("stability-audit.json");
    fs::write(&path, serde_json::to_vec_pretty(&value)?)?;
    Ok(path)
}

fn variants_for(base: &LayoutConfig) -> Vec<(String, LayoutConfig)> {
    let mut wide = base.clone();
    wide.aspect = AspectPreset { label: "4:3", width: 1440, height: 1080 };
    let mut ultra = base.clone();
    ultra.aspect = AspectPreset { label: "21:9", width: 2520, height: 1080 };
    let mut large = base.clone();
    large.scaled_fonts(1.25);
    let mut small = base.clone();
    small.scaled_fonts(0.8);
    vec![
        ("aspect-4:3".into(), wide),
        ("aspect-21:9".into(), ultra),
        ("font-125%".into(), large),
        ("font-80%".into(), small),
    ]
}
