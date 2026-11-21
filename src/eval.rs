//! Evaluation engine: run every strategy on every gap, score, and pick
//! the best strategy per gap. Also computes the "reduction" (削减量):
//! how much energy error (dB) the chosen strategy removes compared with
//! the silence baseline.

use crate::loss::LossManifest;
use crate::metrics::{self, GapMetrics};
use crate::plc;
use crate::wav::Wav;
use crate::Strategy;

#[derive(Debug, Clone)]
pub struct StrategyScore {
    pub strategy: Strategy,
    pub metrics: GapMetrics,
}

#[derive(Debug, Clone)]
pub struct GapReport {
    pub gap_index: usize,
    pub start_ms: usize,
    pub duration_ms: usize,
    pub channel: usize,
    /// RMS of the original (clean) signal inside the gap.
    pub ref_rms: f32,
    pub scores: Vec<StrategyScore>,
    pub best: Strategy,
    /// Energy-error reduction of the best strategy vs silence, in dB.
    pub reduction_db: f32,
}

impl GapReport {
    pub fn best_metrics(&self) -> &GapMetrics {
        &self
            .scores
            .iter()
            .find(|s| s.strategy == self.best)
            .expect("best strategy must have a score")
            .metrics
    }

    pub fn score_of(&self, strategy: Strategy) -> &GapMetrics {
        &self
            .scores
            .iter()
            .find(|s| s.strategy == strategy)
            .expect("all strategies scored")
            .metrics
    }
}

/// Rank key: prefer no artifact, then smallest |energy diff|, then
/// smallest edge discontinuity.
fn rank(m: &GapMetrics) -> (u8, i64, i64) {
    (
        m.artifact as u8,
        (m.energy_diff_db.abs() * 1000.0) as i64,
        (m.edge_discontinuity * 1000.0) as i64,
    )
}

pub fn evaluate(clean: &Wav, degraded: &Wav, manifest: &LossManifest) -> Vec<GapReport> {
    let mut reports = Vec::new();
    for (idx, gap) in manifest.gaps.iter().enumerate() {
        let mut scores = Vec::new();
        for strategy in Strategy::ALL {
            let mut candidate = degraded.clone();
            plc::conceal(&mut candidate, manifest, gap, strategy);
            let m = metrics::score(clean, &candidate, gap, manifest.frame_ms);
            scores.push(StrategyScore {
                strategy,
                metrics: m,
            });
        }
        let best = scores
            .iter()
            .min_by_key(|s| rank(&s.metrics))
            .map(|s| s.strategy)
            .unwrap();
        let silence_err = scores
            .iter()
            .find(|s| s.strategy == Strategy::Silence)
            .map(|s| s.metrics.energy_diff_db.abs())
            .unwrap_or(0.0);
        let best_err = scores
            .iter()
            .find(|s| s.strategy == best)
            .map(|s| s.metrics.energy_diff_db.abs())
            .unwrap_or(0.0);
        let rate = degraded.spec.sample_rate as usize;
        let frame_len = rate * manifest.frame_ms / 1000;
        let ch = clean.spec.channels;
        let c = if ch > 1 { gap.channel.min(ch - 1) } else { 0 };
        let start = gap.start_frame * frame_len;
        let end = ((gap.start_frame + gap.frames) * frame_len).min(clean.frames());
        let mut e = 0.0f32;
        for f in start..end {
            let s = clean.samples[f * ch + c];
            e += s * s;
        }
        let ref_rms = (e / (end - start).max(1) as f32).sqrt();
        reports.push(GapReport {
            gap_index: idx,
            start_ms: gap.start_frame * frame_len * 1000 / rate,
            duration_ms: gap.duration_ms(manifest.frame_ms),
            channel: gap.channel,
            ref_rms,
            reduction_db: silence_err - best_err,
            scores,
            best,
        });
    }
    reports
}

/// Apply the per-gap best strategy to the whole degraded file.
pub fn recover(degraded: &Wav, manifest: &LossManifest, reports: &[GapReport]) -> Wav {
    let mut out = degraded.clone();
    for (gap, report) in manifest.gaps.iter().zip(reports.iter()) {
        plc::conceal(&mut out, manifest, gap, report.best);
    }
    out
}

/// Recover using one fixed strategy for every gap (for A/B listening).
pub fn recover_with(degraded: &Wav, manifest: &LossManifest, strategy: Strategy) -> Wav {
    let mut out = degraded.clone();
    for gap in &manifest.gaps {
        plc::conceal(&mut out, manifest, gap, strategy);
    }
    out
}

/// Render reports as a fixed-width text table.
pub fn format_reports(reports: &[GapReport]) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    writeln!(
        s,
        "{:>4} {:>8} {:>8} {:>3} {:>12} {:>10} {:>8} {:>8}",
        "gap", "start_ms", "dur_ms", "ch", "strategy", "dE(dB)", "disc", "artifact"
    )
    .unwrap();
    for r in reports {
        let m = r.best_metrics();
        writeln!(
            s,
            "{:>4} {:>8} {:>8} {:>3} {:>12} {:>10.2} {:>8.2} {:>8}",
            r.gap_index,
            r.start_ms,
            r.duration_ms,
            r.channel,
            r.best.label(),
            m.energy_diff_db,
            m.edge_discontinuity,
            if m.artifact { "YES" } else { "no" },
        )
        .unwrap();
    }
    s
}

/// Render every strategy's score for every gap (diagnostics).
pub fn format_all_scores(reports: &[GapReport]) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    for r in reports {
        writeln!(
            s,
            "gap {} (start {} ms, dur {} ms, ch {}, ref_rms {:.3})",
            r.gap_index, r.start_ms, r.duration_ms, r.channel, r.ref_rms
        )
        .unwrap();
        for sc in &r.scores {
            writeln!(
                s,
                "    {:>12}  dE {:>8.2} dB  disc {:>6.2}  artifact {}",
                sc.strategy.label(),
                sc.metrics.energy_diff_db,
                sc.metrics.edge_discontinuity,
                if sc.metrics.artifact { "YES" } else { "no" },
            )
            .unwrap();
        }
    }
    s
}

/// Summary per strategy: mean |energy diff| and artifact count.
pub fn summarize(reports: &[GapReport]) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    writeln!(
        s,
        "{:>12} {:>10} {:>10}",
        "strategy", "mean|dE|", "artifacts"
    )
    .unwrap();
    for strategy in Strategy::ALL {
        let mut sum = 0.0f32;
        let mut arts = 0usize;
        for r in reports {
            let m = r.score_of(strategy);
            sum += m.energy_diff_db.abs();
            arts += m.artifact as usize;
        }
        let n = reports.len().max(1) as f32;
        writeln!(s, "{:>12} {:>10.2} {:>10}", strategy.label(), sum / n, arts).unwrap();
    }
    let mean_red =
        reports.iter().map(|r| r.reduction_db).sum::<f32>() / reports.len().max(1) as f32;
    writeln!(s, "mean reduction vs silence: {mean_red:.2} dB").unwrap();
    s
}
