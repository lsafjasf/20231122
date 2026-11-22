use anyhow::{ensure, Result};

use crate::model::{
    sha256_hex, Degradation, ExtremeParams, SampleSpec, SamplingPlan, SourceInfo, SOURCE_HEIGHT,
    SOURCE_RGB_BPP, SOURCE_WIDTH,
};

pub const SAMPLE_TIMESTAMPS_MS: [u64; 5] = [0, 100, 3_000, 5_800, 5_900];
pub const SAMPLE_POINTS: [(u32, u32); 5] = [
    (0, 0),
    (SOURCE_WIDTH - 1, 0),
    (0, SOURCE_HEIGHT - 1),
    (SOURCE_WIDTH - 1, SOURCE_HEIGHT - 1),
    (SOURCE_WIDTH / 2, SOURCE_HEIGHT / 2),
];

const MAX_BUFFER_RESERVE: u64 = 64 * 1024;
const SCALE_CHOICES: [u8; 7] = [100, 75, 50, 25, 10, 5, 1];

pub fn build_demo_plan(source: SourceInfo, params: ExtremeParams) -> SamplingPlan {
    let mut samples = Vec::with_capacity(SAMPLE_TIMESTAMPS_MS.len() * SAMPLE_POINTS.len());
    for (group_index, timestamp_ms) in SAMPLE_TIMESTAMPS_MS.iter().enumerate() {
        for (point_index, (x, y)) in SAMPLE_POINTS.iter().enumerate() {
            let seq = samples.len();
            samples.push(SampleSpec {
                seq,
                timestamp_ms: *timestamp_ms,
                x: *x,
                y: *y,
                raw_offset: (*y as u64 * SOURCE_WIDTH as u64 + *x as u64) * SOURCE_RGB_BPP,
                group_index,
                point_index,
            });
        }
    }
    SamplingPlan {
        version: 1,
        source,
        params,
        samples,
    }
}

pub fn plan_sha256(plan: &SamplingPlan) -> Result<String> {
    Ok(sha256_hex(&serde_json::to_vec(plan)?))
}

pub fn validate_demo_plan(plan: &SamplingPlan) -> Result<()> {
    ensure!(
        plan.version == 1,
        "unsupported plan version {}",
        plan.version
    );
    ensure!(
        plan.samples.len() == 25,
        "demo plan must contain 25 repeated samples, got {}",
        plan.samples.len()
    );
    for (index, sample) in plan.samples.iter().enumerate() {
        ensure!(sample.seq == index, "sample sequence is not contiguous");
    }
    ensure!(
        plan.samples.first().map(|s| (s.timestamp_ms, s.raw_offset)) == Some((0, 0)),
        "first anchor must be timestamp 0 ms, raw offset 0"
    );
    ensure!(
        plan.samples.last().map(|s| (s.timestamp_ms, s.raw_offset)) == Some((5_900, 115_680)),
        "last anchor must be timestamp 5900 ms, raw offset 115680"
    );
    Ok(())
}

pub fn choose_degradation(
    width: u32,
    height: u32,
    requested_scale_percent: u8,
    memory_budget_bytes: u64,
    decoder: &str,
) -> Degradation {
    let requested = requested_scale_percent.clamp(1, 100);
    let frame_buffer_bytes = width as u64 * height as u64 * 4;
    let buffer_reserve = (memory_budget_bytes / 4).min(MAX_BUFFER_RESERVE);
    let mut selected = 1u8;
    let mut satisfied = false;

    for scale in SCALE_CHOICES {
        if scale > requested {
            continue;
        }
        let (scaled_w, scaled_h) = scaled_size(width, height, scale);
        let bytes = scaled_w as u64 * scaled_h as u64 * 4;
        if bytes.saturating_add(buffer_reserve) <= memory_budget_bytes {
            selected = scale;
            satisfied = true;
            break;
        }
    }

    if !satisfied {
        let (scaled_w, scaled_h) = scaled_size(width, height, 1);
        satisfied = scaled_w as u64 * scaled_h as u64 * 4 <= memory_budget_bytes;
    }

    let (effective_width, effective_height) = scaled_size(width, height, selected);
    let reason = if selected < requested {
        format!(
            "memory budget {memory_budget_bytes} is below full-frame requirement {frame_buffer_bytes}; \
             scale reduced from {requested}% to {selected}%"
        )
    } else {
        "memory budget accepts requested scale".to_string()
    };

    Degradation {
        requested_scale_percent: requested,
        effective_scale_percent: selected,
        effective_width,
        effective_height,
        frame_buffer_bytes,
        memory_budget_bytes,
        budget_satisfied: satisfied,
        reason,
        decoder: decoder.to_string(),
    }
}

pub fn scaled_size(width: u32, height: u32, scale_percent: u8) -> (u32, u32) {
    let scale = scale_percent.clamp(1, 100) as u64;
    (
        (width as u64 * scale).div_ceil(100).max(1) as u32,
        (height as u64 * scale).div_ceil(100).max(1) as u32,
    )
}

pub fn effective_point(x: u32, y: u32, width: u32, height: u32, scale: u8) -> (u32, u32) {
    let (scaled_w, scaled_h) = scaled_size(width, height, scale);
    let ex = (x as u64 * scaled_w as u64 / width.max(1) as u64) as u32;
    let ey = (y as u64 * scaled_h as u64 / height.max(1) as u64) as u32;
    (ex.min(scaled_w - 1), ey.min(scaled_h - 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> SourceInfo {
        SourceInfo {
            path: "originals/source.mkv".into(),
            sha256: "0".repeat(64),
            size_bytes: 1,
            width: SOURCE_WIDTH,
            height: SOURCE_HEIGHT,
            duration_ms: 6_000,
            frame_rate: "10/1".into(),
        }
    }

    #[test]
    fn repeated_plan_has_concrete_anchors() {
        let plan = build_demo_plan(source(), ExtremeParams::demo());
        validate_demo_plan(&plan).unwrap();
        assert_eq!(plan.samples.len(), 25);
        assert_eq!(plan.samples[0].timestamp_ms, 0);
        assert_eq!(plan.samples[0].raw_offset, 0);
        assert_eq!(plan.samples[24].timestamp_ms, 5_900);
        assert_eq!(plan.samples[24].raw_offset, 115_680);
    }

    #[test]
    fn budget_of_100k_degrades_to_25_percent() {
        let d = choose_degradation(320, 240, 100, 100_000, "pngdec");
        assert_eq!(d.effective_scale_percent, 25);
        assert_eq!((d.effective_width, d.effective_height), (80, 60));
        assert!(d.budget_satisfied);
    }

    #[test]
    fn zero_budget_marks_resource_shortage() {
        let d = choose_degradation(320, 240, 100, 0, "pngdec");
        assert_eq!(d.effective_scale_percent, 1);
        assert_eq!((d.effective_width, d.effective_height), (4, 3));
        assert!(!d.budget_satisfied);
    }

    #[test]
    fn medium_budget_keeps_a_usable_scale() {
        let d = choose_degradation(320, 240, 100, 50_000, "pngdec");
        assert_eq!(d.effective_scale_percent, 25);
        assert!(d.budget_satisfied);
    }
}
