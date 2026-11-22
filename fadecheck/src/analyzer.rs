//! 淡化（fade-in / fade-out）检测。
//!
//! 方法：对单声道 PCM 计算首尾窗口的 RMS 与中部基准 RMS 的比值。
//! 真实淡化表现为端部能量显著低于中部且呈单调爬升/下降趋势。
//!
//! 边界处理：阈值两侧各设一个“边界带”。测量值落入边界带时，
//! 证据不足以支持任一确定结论，返回 [`Verdict::Inconclusive`]，
//! 并保留触发边界的关键证据，而不是强行归类。

use crate::model::{Evidence, Snapshot, Verdict};

/// 判定参数，集中定义以便复核与测试。
#[derive(Debug, Clone)]
pub struct FadeThresholds {
    /// 端窗占总帧数的比例（首尾各一个窗口）。
    pub window_ratio: f64,
    /// 低于该比值判定为“存在淡化”。
    pub fade_ratio: f64,
    /// 高于该比值判定为“无淡化”。
    pub no_fade_ratio: f64,
    /// 单调性占比门槛：窗口内相邻帧递增/递减比例。
    pub monotonic_ratio: f64,
}

impl Default for FadeThresholds {
    fn default() -> Self {
        // fade_ratio 与 no_fade_ratio 之间即“边界带”。
        // 线性淡化端部窗口的理论 RMS 比值为 1/sqrt(3) ≈ 0.577，
        // 故把判定上沿放在其下方，边界带留给深度不足或证据冲突的片段。
        Self {
            window_ratio: 0.05,
            fade_ratio: 0.65,
            no_fade_ratio: 0.85,
            monotonic_ratio: 0.75,
        }
    }
}

/// 一次端部分析的原始测量值。
#[derive(Debug, Clone)]
pub struct EdgeMeasurement {
    pub rms: f64,
    pub reference_rms: f64,
    pub ratio: f64,
    pub monotonic_fraction: f64,
    pub window: (u64, u64),
}

fn rms(samples: &[f32], from: usize, to: usize) -> f64 {
    if to <= from {
        return 0.0;
    }
    let sum_sq: f64 = samples[from..to]
        .iter()
        .map(|s| {
            let v = *s as f64;
            v * v
        })
        .sum();
    (sum_sq / (to - from) as f64).sqrt()
}

fn monotonic_fraction(samples: &[f32], from: usize, to: usize, rising: bool) -> f64 {
    let len = to.saturating_sub(from);
    if len < 16 {
        return 0.0;
    }
    // 正弦等载波逐样本抖动会破坏逐点单调性，改为分块 RMS 趋势：
    // 把窗口均分为 16 个子块，比较相邻子块的能量方向。
    let blocks = 16usize;
    let block_len = len / blocks;
    let mut block_rms = Vec::with_capacity(blocks);
    for index in 0..blocks {
        let start = from + index * block_len;
        let end = if index + 1 == blocks {
            to
        } else {
            start + block_len
        };
        block_rms.push(rms(samples, start, end));
    }
    let mut hits = 0u64;
    let total = (block_rms.len() - 1) as u64;
    for pair in block_rms.windows(2) {
        let ok = if rising {
            pair[1] >= pair[0]
        } else {
            pair[1] <= pair[0]
        };
        if ok {
            hits += 1;
        }
    }
    if total == 0 {
        0.0
    } else {
        hits as f64 / total as f64
    }
}

/// 计算首端（淡入候选）测量值。
pub fn measure_head(samples: &[f32], t: &FadeThresholds) -> Option<EdgeMeasurement> {
    let n = samples.len();
    let win = (n as f64 * t.window_ratio).round() as usize;
    if win < 4 || n < win * 4 {
        return None;
    }
    let head = rms(samples, 0, win);
    let reference = rms(samples, win, n - win);
    let ratio = if reference > f64::EPSILON {
        head / reference
    } else {
        1.0
    };
    Some(EdgeMeasurement {
        rms: head,
        reference_rms: reference,
        ratio,
        monotonic_fraction: monotonic_fraction(samples, 0, win, true),
        window: (0, win as u64),
    })
}

/// 计算尾端（淡出候选）测量值。
pub fn measure_tail(samples: &[f32], t: &FadeThresholds) -> Option<EdgeMeasurement> {
    let n = samples.len();
    let win = (n as f64 * t.window_ratio).round() as usize;
    if win < 4 || n < win * 4 {
        return None;
    }
    let tail = rms(samples, n - win, n);
    let reference = rms(samples, win, n - win);
    let ratio = if reference > f64::EPSILON {
        tail / reference
    } else {
        1.0
    };
    Some(EdgeMeasurement {
        rms: tail,
        reference_rms: reference,
        ratio,
        monotonic_fraction: monotonic_fraction(samples, n - win, n, false),
        window: ((n - win) as u64, n as u64),
    })
}

/// 单个端的三态判定：有淡化 / 无淡化 / 边界无法判断。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeVerdict {
    Faded,
    NotFaded,
    Inconclusive,
}

fn classify_edge(m: &EdgeMeasurement, t: &FadeThresholds) -> EdgeVerdict {
    // 能量比值本身已明确达到“无淡化”水平时直接判定，
    // 恒定电平片段的分块趋势天然接近随机，不应反过来造成无法判断。
    if m.ratio >= t.no_fade_ratio {
        return EdgeVerdict::NotFaded;
    }
    // 单调性不足时，即使能量偏低也不能认定为淡化——同样进入无法判断。
    if m.monotonic_fraction + f64::EPSILON < t.monotonic_ratio {
        return EdgeVerdict::Inconclusive;
    }
    if m.ratio <= t.fade_ratio {
        EdgeVerdict::Faded
    } else {
        EdgeVerdict::Inconclusive
    }
}

/// 测试出口：暴露边缘三态判定。
pub fn classify_edge_public(m: &EdgeMeasurement, t: &FadeThresholds) -> EdgeVerdict {
    classify_edge(m, t)
}

/// 测试出口：暴露证据构造逻辑。
pub fn push_edge_evidence_public(
    out: &mut Vec<Evidence>,
    label: &str,
    edge: &str,
    m: &EdgeMeasurement,
    v: EdgeVerdict,
    t: &FadeThresholds,
) {
    push_edge_evidence(out, label, edge, m, v, t);
}

/// 依据两个版本快照中的样本给出综合结论与证据。
///
/// 两个快照独立判定；任一快照处于边界带即整体标记为无法判断，
/// 并把两边的测量值都作为证据保留下来。
pub fn analyze(
    snapshots: &[(Snapshot, Vec<f32>)],
    thresholds: &FadeThresholds,
) -> (Verdict, Vec<Evidence>) {
    let mut evidence = Vec::new();
    let mut any_inconclusive = false;
    let mut head_faded = false;
    let mut tail_faded = false;
    let mut analyzable = false;

    for (snapshot, samples) in snapshots {
        let (head, tail) = match (
            measure_head(samples, thresholds),
            measure_tail(samples, thresholds),
        ) {
            (Some(h), Some(tl)) => (h, tl),
            _ => {
                evidence.push(Evidence {
                    metric: "sample_coverage".to_string(),
                    value: samples.len() as f64,
                    threshold: format!(">= {} 帧", (thresholds.window_ratio * 100.0) as u32),
                    window_frames: None,
                    snapshot: snapshot.label.clone(),
                    note: "样本过少，窗口不足以支撑淡化判定".to_string(),
                });
                any_inconclusive = true;
                continue;
            }
        };
        analyzable = true;

        let head_v = classify_edge(&head, thresholds);
        let tail_v = classify_edge(&tail, thresholds);

        push_edge_evidence(
            &mut evidence,
            &snapshot.label,
            "head",
            &head,
            head_v,
            thresholds,
        );
        push_edge_evidence(
            &mut evidence,
            &snapshot.label,
            "tail",
            &tail,
            tail_v,
            thresholds,
        );

        if head_v == EdgeVerdict::Inconclusive || tail_v == EdgeVerdict::Inconclusive {
            any_inconclusive = true;
        }
        if head_v == EdgeVerdict::Faded {
            head_faded = true;
        }
        if tail_v == EdgeVerdict::Faded {
            tail_faded = true;
        }
    }

    if !analyzable {
        return (Verdict::NotAnalyzable, evidence);
    }
    if any_inconclusive {
        // 关键边界证据已在上面保留；结论保持保守。
        return (Verdict::Inconclusive, evidence);
    }
    let verdict = match (head_faded, tail_faded) {
        (true, true) => Verdict::FadeInAndOut,
        (true, false) => Verdict::FadeInOnly,
        (false, true) => Verdict::FadeOutOnly,
        (false, false) => Verdict::NoFade,
    };
    (verdict, evidence)
}

fn push_edge_evidence(
    out: &mut Vec<Evidence>,
    label: &str,
    edge: &str,
    m: &EdgeMeasurement,
    v: EdgeVerdict,
    t: &FadeThresholds,
) {
    let note = match v {
        EdgeVerdict::Faded => format!(
            "{edge} 端 RMS 比值 {:.3} <= {}，且单调占比 {:.2} 达标，判定存在淡化",
            m.ratio, t.fade_ratio, m.monotonic_fraction
        ),
        EdgeVerdict::NotFaded => format!(
            "{edge} 端 RMS 比值 {:.3} >= {}，端部能量接近中部，判定无淡化",
            m.ratio, t.no_fade_ratio
        ),
        EdgeVerdict::Inconclusive => format!(
            "{edge} 端测量处于边界带（{} < 比值 {:.3} < {}）或单调性 {:.2} 不足，证据冲突，无法判断",
            t.fade_ratio, m.ratio, t.no_fade_ratio, m.monotonic_fraction
        ),
    };
    out.push(Evidence {
        metric: format!("{edge}_rms_ratio"),
        value: m.ratio,
        threshold: format!(
            "fade<={}; no-fade>={}; 边界带=({},{})",
            t.fade_ratio, t.no_fade_ratio, t.fade_ratio, t.no_fade_ratio
        ),
        window_frames: Some(m.window),
        snapshot: label.to_string(),
        note,
    });
}
