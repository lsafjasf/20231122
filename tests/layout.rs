use std::collections::BTreeSet;

use credits_layout::layout;
use credits_layout::model::{CreditsSample, LayoutConfig};
use credits_layout::report;
use credits_layout::samples;

#[test]
fn all_three_samples_produce_stable_unique_lines() {
    let config = LayoutConfig::default();
    for sample in samples::all_samples() {
        let built = layout::build(&sample, &config).unwrap();
        assert!(!built.lines.is_empty(), "{} has no visual lines", sample.id);
        assert!(built.duration > 0.0);
        let ids: BTreeSet<_> = built.lines.iter().map(|line| line.line_id.as_str()).collect();
        assert_eq!(ids.len(), built.lines.len(), "duplicate line id in {}", sample.id);
        for pair in built.lines.windows(2) {
            assert!(
                pair[1].baseline_y >= pair[0].baseline_y,
                "baseline order regressed in {}",
                sample.id
            );
        }
    }
}

#[test]
fn unbroken_line_wraps_inside_the_safe_width() {
    let sample = samples::long_line_sample();
    let built = layout::build(&sample, &LayoutConfig::default()).unwrap();
    let source = built
        .hard_lines
        .iter()
        .find(|line| line.text.starts_with("https://"))
        .unwrap();
    assert!(source.overflow_amount > 1000.0);
    assert!(source.wrap_count > 1);
    let wrapped: Vec<_> = built
        .lines
        .iter()
        .filter(|line| line.block_id == source.block_id)
        .collect();
    assert_eq!(wrapped.len(), source.wrap_count);
    for line in wrapped {
        assert!(
            line.width <= built.safe_area.width + 1.0,
            "{} is {} px wide, safe width is {}",
            line.line_id,
            line.width,
            built.safe_area.width
        );
    }
}

#[test]
fn aspect_and_font_changes_preserve_the_anchor_baseline() {
    let base = LayoutConfig::default();
    for sample in samples::all_samples() {
        let before = layout::build(&sample, &base).unwrap();
        let time = before.duration * 0.56;
        let focus = before.safe_area.y + before.safe_area.height * 0.5;
        let anchor = before.anchor_at(time, focus).unwrap();
        for variant in variants(&base) {
            let changed = layout::build(&sample, &variant).unwrap();
            let (new_time, line_id) = changed
                .time_for_anchor(
                    &anchor.block_id,
                    anchor.hard_line_index,
                    anchor.screen_baseline_y,
                )
                .unwrap();
            let after = changed
                .anchor_at(new_time, anchor.screen_baseline_y)
                .unwrap();
            assert!(
                (after.screen_baseline_y - anchor.screen_baseline_y).abs() <= 1.0,
                "{} jumped on {}: {} -> {}",
                sample.id,
                variant.aspect.label,
                anchor.line_id,
                line_id
            );
            assert_eq!(after.block_id, anchor.block_id);
            assert_eq!(after.hard_line_index, anchor.hard_line_index);
        }
    }
}

#[test]
fn report_contains_per_line_baselines_and_overflow_records() {
    for sample in samples::all_samples() {
        let config = LayoutConfig::default();
        let built = layout::build(&sample, &config).unwrap();
        let value = report::report_value(&sample, &built);
        let lines = value["lines"].as_array().unwrap();
        let records = value["records"].as_array().unwrap();
        assert_eq!(lines.len(), built.lines.len());
        assert!(lines.iter().all(|line| line["baseline_content_y"].is_number()));
        let reported: BTreeSet<_> = records
            .iter()
            .filter_map(|record| record["line_id"].as_str())
            .collect();
        for line in &built.lines {
            assert!(
                reported.contains(line.line_id.as_str()),
                "{} missing from overflow records",
                line.line_id
            );
        }
    }
}

fn variants(base: &LayoutConfig) -> Vec<LayoutConfig> {
    let mut wide = base.clone();
    wide.aspect = credits_layout::model::ASPECTS[1];
    let mut ultra = base.clone();
    ultra.aspect = credits_layout::model::ASPECTS[2];
    let mut large = base.clone();
    large.scaled_fonts(1.25);
    let mut small = base.clone();
    small.scaled_fonts(0.8);
    vec![wide, ultra, large, small]
}

#[allow(dead_code)]
fn _sample_type_check(_sample: &CreditsSample) {}
