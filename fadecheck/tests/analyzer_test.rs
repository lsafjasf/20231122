//! 分析器单元级集成测试：淡化判定、无淡化与边界无法判断。

mod common;
use common::*;

#[test]
fn full_linear_fades_are_detected_on_both_edges() {
    let samples = fixture_samples(FadeShape::FullFade);
    let thresholds = thresholds();
    let (verdict, evidence) = fadecheck::analyzer_public::analyze_samples(&samples, &thresholds);
    assert_eq!(verdict, fadecheck::analyzer_public::Verdict::FadeInAndOut);
    assert!(evidence.iter().any(|e| e.metric == "head_rms_ratio"));
    assert!(evidence.iter().any(|e| e.metric == "tail_rms_ratio"));
}

#[test]
fn constant_level_is_no_fade() {
    let samples = fixture_samples(FadeShape::Constant);
    let thresholds = thresholds();
    let (verdict, _) = fadecheck::analyzer_public::analyze_samples(&samples, &thresholds);
    assert_eq!(verdict, fadecheck::analyzer_public::Verdict::NoFade);
}

#[test]
fn partial_fade_inside_band_is_inconclusive_and_keeps_evidence() {
    let samples = fixture_samples(FadeShape::BoundaryBand);
    let thresholds = thresholds();
    let (verdict, evidence) = fadecheck::analyzer_public::analyze_samples(&samples, &thresholds);
    assert_eq!(verdict, fadecheck::analyzer_public::Verdict::Inconclusive);
    // 无法判断也必须保留关键证据，且证据要明确引用边界带阈值。
    assert!(evidence.iter().all(|e| e.threshold.contains("边界带")));
    assert!(!evidence.is_empty());
}
