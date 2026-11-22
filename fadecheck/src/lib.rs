//! 库入口：复用与二进制相同的模块，并通过 `analyzer_public`
//! 向集成测试暴露受控的分析接口（不破坏模块封装）。

pub mod analyzer_public {
    pub use crate::analyzer::{analyze as analyze_snapshots, EdgeMeasurement, FadeThresholds};
    pub use crate::model::{Evidence, Snapshot, Verdict};

    /// 测试/复用入口：对单组样本执行首尾淡化判定。
    pub fn analyze_samples(
        samples: &[f32],
        thresholds: &FadeThresholds,
    ) -> (Verdict, Vec<Evidence>) {
        let head = crate::analyzer::measure_head(samples, thresholds);
        let tail = crate::analyzer::measure_tail(samples, thresholds);
        match (head, tail) {
            (Some(head), Some(tail)) => {
                let mut evidence = Vec::new();
                let head_v = crate::analyzer::classify_edge_public(&head, thresholds);
                let tail_v = crate::analyzer::classify_edge_public(&tail, thresholds);
                crate::analyzer::push_edge_evidence_public(
                    &mut evidence,
                    "test",
                    "head",
                    &head,
                    head_v,
                    thresholds,
                );
                crate::analyzer::push_edge_evidence_public(
                    &mut evidence,
                    "test",
                    "tail",
                    &tail,
                    tail_v,
                    thresholds,
                );
                let inconclusive = head_v == crate::analyzer::EdgeVerdict::Inconclusive
                    || tail_v == crate::analyzer::EdgeVerdict::Inconclusive;
                let verdict = if inconclusive {
                    Verdict::Inconclusive
                } else {
                    match (
                        head_v == crate::analyzer::EdgeVerdict::Faded,
                        tail_v == crate::analyzer::EdgeVerdict::Faded,
                    ) {
                        (true, true) => Verdict::FadeInAndOut,
                        (true, false) => Verdict::FadeInOnly,
                        (false, true) => Verdict::FadeOutOnly,
                        (false, false) => Verdict::NoFade,
                    }
                };
                (verdict, evidence)
            }
            _ => (Verdict::NotAnalyzable, Vec::new()),
        }
    }
}

pub mod engine_public {
    pub use crate::engine::run_batch;
    pub use crate::util::TempState;
}

pub mod model_public {
    pub use crate::model::{Conclusion, Report, SourceKind, Verdict};
}

pub mod decoder_public {
    pub use crate::decoder::init;
}

mod analyzer;
mod decoder;
mod engine;
mod model;
mod util;
