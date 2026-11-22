#![allow(dead_code)]

use fadecheck::analyzer_public::{FadeThresholds, Verdict};

pub fn thresholds() -> FadeThresholds {
    FadeThresholds::default()
}

#[derive(Clone, Copy)]
pub enum FadeShape {
    FullFade,
    Constant,
    BoundaryBand,
}

const RATE: usize = 48_000;
const SECONDS: usize = 2;

/// 与 tests/make_fixtures.py 等价的纯 Rust 样本生成，避免外部进程依赖。
pub fn fixture_samples(shape: FadeShape) -> Vec<f32> {
    let n = RATE * SECONDS;
    let base: Vec<f32> = (0..n)
        .map(|i| 0.6 * (2.0 * std::f64::consts::PI * 440.0 * i as f64 / RATE as f64).sin() as f32)
        .collect();
    let depth = match shape {
        FadeShape::FullFade => 1.0,
        FadeShape::Constant => 0.0,
        FadeShape::BoundaryBand => 0.48,
    };
    apply_fades(base, depth)
}

fn apply_fades(mut samples: Vec<f32>, depth: f64) -> Vec<f32> {
    let n = samples.len();
    let win = (n as f64 * 0.05) as usize;
    for i in 0..win {
        let gain = ((1.0 - depth) + depth * (i as f64 / (win - 1) as f64)) as f32;
        samples[i] *= gain;
        samples[n - 1 - i] *= gain;
    }
    samples
}

pub fn expected_verdict(shape: FadeShape) -> Verdict {
    match shape {
        FadeShape::FullFade => Verdict::FadeInAndOut,
        FadeShape::Constant => Verdict::NoFade,
        FadeShape::BoundaryBand => Verdict::Inconclusive,
    }
}
