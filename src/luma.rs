use crate::model::{
    AutoConclusion, BlackInterval, DetectorParams, IntervalState, Verdict, WaveSample, Waveform,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LumaStats {
    pub mean: f32,
    pub min: u8,
    pub max: u8,
    pub black_pixel_ratio: f32,
}

pub fn gray8_stats(
    data: &[u8],
    width: u32,
    height: u32,
    stride: usize,
    pixel_threshold: u8,
) -> LumaStats {
    let mut sum = 0u64;
    let mut min = u8::MAX;
    let mut max = 0u8;
    let mut black_pixels = 0u64;
    let mut used_rows = 0u32;
    for row in 0..height as usize {
        let start = row * stride;
        let end = start + width as usize;
        if end > data.len() {
            break;
        }
        used_rows += 1;
        for &pixel in &data[start..end] {
            sum += pixel as u64;
            min = min.min(pixel);
            max = max.max(pixel);
            if pixel <= pixel_threshold {
                black_pixels += 1;
            }
        }
    }
    let count = (width as u64 * used_rows as u64).max(1);
    LumaStats {
        mean: (sum as f32 / count as f32) / 255.0,
        min,
        max,
        black_pixel_ratio: black_pixels as f32 / count as f32,
    }
}

pub fn is_black(stats: LumaStats, params: &DetectorParams) -> bool {
    stats.black_pixel_ratio >= params.black_pixel_ratio || stats.mean <= params.luma_threshold
}

pub fn waveform_from_samples(mut samples: Vec<WaveSample>, period_ms: u64) -> Waveform {
    samples.sort_by_key(|sample| (sample.time_ms, sample.sample));
    let covered_ms = samples
        .last()
        .map(|sample| sample.time_ms.saturating_add(period_ms))
        .unwrap_or(0);
    Waveform {
        source: "decoded video luma (Y), sampled after videoscale/videorate".into(),
        unit: "Y8 normalized 0..1".into(),
        sample_period_ms: period_ms,
        covered_ms,
        samples,
    }
}

pub fn detect_intervals(
    waveform: &Waveform,
    params: &DetectorParams,
    track_id: &str,
) -> Vec<BlackInterval> {
    let mut intervals = Vec::new();
    let mut run: Vec<&WaveSample> = Vec::new();

    let flush = |run: &mut Vec<&WaveSample>, intervals: &mut Vec<BlackInterval>| {
        if run.is_empty() {
            return;
        }
        let first = run[0];
        let last = *run.last().expect("run is non-empty");
        let end_ms = last.time_ms.saturating_add(waveform.sample_period_ms);
        if end_ms.saturating_sub(first.time_ms) >= params.min_duration_ms {
            let mean_luma = run.iter().map(|sample| sample.y_mean).sum::<f32>() / run.len() as f32;
            let lowest_luma = run
                .iter()
                .map(|sample| sample.y_mean)
                .fold(f32::INFINITY, f32::min);
            intervals.push(BlackInterval {
                id: format!("bf-{:04}", intervals.len() + 1),
                track_id: track_id.into(),
                start_ms: first.time_ms,
                end_ms,
                start_frame: first.frame,
                end_frame: last.frame,
                sample_start: first.sample,
                sample_end: last.sample,
                mean_luma,
                lowest_luma,
                params_fingerprint: params.fingerprint(),
                evidence_refs: Vec::new(),
                state: IntervalState::AutoDetected,
            });
        }
        run.clear();
    };

    let mut previous_time: Option<u64> = None;
    for sample in &waveform.samples {
        let contiguous = previous_time
            .map(|time| sample.time_ms.saturating_sub(time) <= waveform.sample_period_ms * 2)
            .unwrap_or(true);
        if !contiguous {
            flush(&mut run, &mut intervals);
        }
        if sample.is_black && sample.track_id == track_id {
            run.push(sample);
        } else {
            flush(&mut run, &mut intervals);
        }
        previous_time = Some(sample.time_ms);
    }
    flush(&mut run, &mut intervals);
    intervals
}

pub fn conclude(waveform: &Waveform, params: &DetectorParams, track_id: &str) -> AutoConclusion {
    let intervals = detect_intervals(waveform, params, track_id);
    let analyzed_samples = waveform.samples.len() as u64;
    let analyzed_ms = waveform.covered_ms;
    let expected = if params.sample_fps == 0 {
        0
    } else {
        analyzed_ms.saturating_mul(params.sample_fps as u64) / 1000
    };
    let coverage = if expected == 0 {
        0.0
    } else {
        (analyzed_samples as f32 / expected as f32).min(1.0)
    };
    let mut limitations = Vec::new();
    let mut reason_codes = Vec::new();
    if coverage < 0.90 {
        limitations.push(format!(
            "采样覆盖率 {:.1}%（理想值 >= 90%）",
            coverage * 100.0
        ));
        reason_codes.push("LOW_SAMPLE_COVERAGE".into());
    }
    if analyzed_ms < params.min_duration_ms {
        limitations.push("有效时长短于最小黑场阈值，无法作可靠结论".into());
    }
    if params.include_audio {
        limitations.push("本次自动检测仅依据视频亮度，音频轨未参与判定".into());
    }

    let confidence = if analyzed_samples == 0 {
        0.0
    } else {
        (0.55 + coverage * 0.42).min(0.99)
    };
    let (verdict, headline) = if analyzed_samples == 0 {
        reason_codes.push("NO_VIDEO_SAMPLES".into());
        (
            Verdict::Inconclusive,
            "没有收到可分析的视频样本；请检查轨道选择与解码器".to_string(),
        )
    } else if analyzed_ms < params.min_duration_ms {
        reason_codes.push("DURATION_BELOW_MIN_INTERVAL".into());
        (
            Verdict::Inconclusive,
            format!(
                "有效时长 {} ms 小于最短黑场阈值 {} ms，不能可靠判定",
                analyzed_ms, params.min_duration_ms
            ),
        )
    } else if coverage < 0.50 {
        reason_codes.push("COVERAGE_TOO_LOW".into());
        (
            Verdict::Inconclusive,
            format!("采样覆盖率仅 {:.1}%，需要重新采样", coverage * 100.0),
        )
    } else if intervals.is_empty() {
        reason_codes.push("NO_INTERVAL_ABOVE_MIN_DURATION".into());
        (
            Verdict::Clean,
            format!("未发现持续 {} ms 以上的黑场", params.min_duration_ms),
        )
    } else {
        reason_codes.push("BLACK_INTERVAL_MATCHED".into());
        let total: u64 = intervals
            .iter()
            .map(|interval| interval.end_ms.saturating_sub(interval.start_ms))
            .sum();
        (
            Verdict::BlackFramesFound,
            format!("发现 {} 段黑场，共 {} ms", intervals.len(), total),
        )
    };

    AutoConclusion {
        verdict,
        headline,
        confidence,
        reason_codes,
        limitations,
        analyzed_samples,
        analyzed_ms,
        intervals,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(index: u64, time_ms: u64, mean: f32, black: bool) -> WaveSample {
        WaveSample {
            sample: index,
            time_ms,
            pts_ns: time_ms * 1_000_000,
            frame: index,
            track_id: "video-0".into(),
            y_mean: mean,
            y_min: 0,
            y_max: 255,
            black_pixel_ratio: if black { 1.0 } else { 0.0 },
            is_black: black,
            params_fingerprint: "test".into(),
        }
    }

    #[test]
    fn computes_gray8_statistics_with_padding() {
        let data = [0, 10, 0, 0, 255, 255, 255, 0, 0];
        let stats = gray8_stats(&data, 2, 2, 3, 25);
        assert!((stats.mean - (265.0 / 4.0 / 255.0)).abs() < 0.001);
        assert_eq!(stats.min, 0);
        assert_eq!(stats.max, 255);
    }

    #[test]
    fn detects_contiguous_black_interval() {
        let params = DetectorParams {
            min_duration_ms: 400,
            sample_fps: 5,
            ..DetectorParams::default()
        };
        let waveform = waveform_from_samples(
            vec![
                sample(0, 0, 0.8, false),
                sample(1, 200, 0.0, true),
                sample(2, 400, 0.0, true),
                sample(3, 600, 0.0, true),
                sample(4, 800, 0.8, false),
            ],
            200,
        );
        let conclusion = conclude(&waveform, &params, "video-0");
        assert_eq!(conclusion.verdict, Verdict::BlackFramesFound);
        assert_eq!(conclusion.intervals.len(), 1);
        assert_eq!(conclusion.intervals[0].start_ms, 200);
        assert_eq!(conclusion.intervals[0].end_ms, 800);
    }

    #[test]
    fn separates_non_contiguous_black_runs() {
        let params = DetectorParams {
            min_duration_ms: 200,
            sample_fps: 5,
            ..DetectorParams::default()
        };
        let waveform = waveform_from_samples(
            vec![
                sample(0, 0, 0.0, true),
                sample(1, 200, 0.8, false),
                sample(2, 800, 0.0, true),
                sample(3, 1000, 0.0, true),
            ],
            200,
        );
        let conclusion = conclude(&waveform, &params, "video-0");
        assert_eq!(conclusion.intervals.len(), 2);
    }

    #[test]
    fn short_or_sparse_waveform_is_inconclusive_not_clean() {
        let params = DetectorParams {
            min_duration_ms: 2_000,
            ..DetectorParams::default()
        };
        let waveform =
            waveform_from_samples(vec![sample(0, 0, 0.8, false)], params.sample_period_ms());
        let conclusion = conclude(&waveform, &params, "video-0");
        assert_eq!(conclusion.verdict, Verdict::Inconclusive);
        assert!(conclusion
            .reason_codes
            .iter()
            .any(|code| code == "DURATION_BELOW_MIN_INTERVAL"));
    }
}
