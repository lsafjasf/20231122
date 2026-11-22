use std::collections::BTreeSet;

use crate::curve::{Curve, CurveKind};
use crate::error::{AppError, AppResult};
use crate::model::{
    EvidenceState, FrameSpan, ParameterRange, TimeSpan, TimelineEntry, WaveformStats,
};
use crate::wav::round6;

#[derive(Debug, Clone)]
pub struct PlanConfig {
    pub duration_us: u64,
    pub sample_rate_hz: u32,
    pub analysed_channel: u16,
    pub window_us: u64,
    pub render_quantum_us: u64,
    pub gap_spans: Vec<TimeSpan>,
    pub max_timeline_entries: u64,
    pub max_render_segments: u64,
}

#[derive(Debug, Clone)]
pub struct RenderPlan {
    pub duration_us: u64,
    pub sample_rate_hz: u32,
    pub analysed_channel: u16,
    pub effective_window_us: u64,
    pub effective_render_quantum_us: u64,
    pub adaptive_window_applied: bool,
    pub segments: Vec<RenderSegment>,
    pub timeline: Vec<TimelineDraft>,
    pub speed_range: ParameterRange,
    pub pitch_range: ParameterRange,
}

#[derive(Debug, Clone)]
pub struct RenderSegment {
    pub index: u64,
    pub source_start_us: u64,
    pub source_end_us: u64,
    pub speed_rate: f64,
    pub pitch_semitones: f64,
    pub pitch_ratio: f64,
    pub pitch_evidence: EvidenceState,
}

#[derive(Debug, Clone)]
pub struct TimelineDraft {
    pub render_segment_index: u64,
    pub source_start_us: u64,
    pub source_end_us: u64,
    pub speed_rate_curve: f64,
    pub speed_rate_applied: f64,
    pub pitch_semitones_curve: f64,
    pub pitch_semitones_applied: f64,
    pub pitch_ratio_applied: f64,
    pub pitch_evidence: EvidenceState,
}

#[derive(Debug, Clone, Copy)]
pub struct SegmentRenderResult {
    pub index: u64,
    pub output_frames: u64,
}

pub fn build_plan(
    duration_us: u64,
    speed: &Curve,
    pitch: &Curve,
    config: PlanConfig,
) -> AppResult<RenderPlan> {
    assert_eq!(speed.kind, CurveKind::Speed);
    assert_eq!(pitch.kind, CurveKind::Pitch);
    if duration_us == 0 {
        return Ok(RenderPlan {
            duration_us: 0,
            sample_rate_hz: config.sample_rate_hz,
            analysed_channel: config.analysed_channel,
            effective_window_us: config.window_us.max(1),
            effective_render_quantum_us: config.render_quantum_us.max(1),
            adaptive_window_applied: false,
            segments: Vec::new(),
            timeline: Vec::new(),
            speed_range: ParameterRange { min: 1.0, max: 1.0 },
            pitch_range: ParameterRange { min: 0.0, max: 0.0 },
        });
    }

    let max_segments = config.max_render_segments.max(1);
    let mut render_quantum = config.render_quantum_us.max(1);
    let minimum_quantum = div_ceil(duration_us, max_segments);
    render_quantum = render_quantum.max(minimum_quantum);

    let mut render_boundaries = grid_boundaries(duration_us, render_quantum);
    for gap in &config.gap_spans {
        insert_boundary(&mut render_boundaries, gap.start_us, duration_us);
        insert_boundary(&mut render_boundaries, gap.end_us, duration_us);
    }
    ensure_segment_limit(&render_boundaries, max_segments)?;

    let mut segments = Vec::with_capacity(render_boundaries.len().saturating_sub(1));
    let render_boundaries_vec: Vec<u64> = render_boundaries.into_iter().collect();
    for (index, pair) in render_boundaries_vec.windows(2).enumerate() {
        let start = pair[0];
        let end = pair[1];
        if end <= start {
            continue;
        }
        let midpoint = start + (end - start) / 2;
        let speed_rate = speed.value_at(midpoint).unwrap_or(1.0).clamp(0.1, 4.0);
        let pitch_semitones = pitch.value_at(midpoint).unwrap_or(0.0).clamp(-24.0, 24.0);
        let pitch_evidence = pitch_evidence_at(pitch, midpoint, &config.gap_spans);
        segments.push(RenderSegment {
            index: index as u64,
            source_start_us: start,
            source_end_us: end,
            speed_rate,
            pitch_semitones,
            pitch_ratio: semitones_to_ratio(pitch_semitones),
            pitch_evidence,
        });
    }

    let timeline_limit = config.max_timeline_entries.max(1);
    if segments.len() as u64 > timeline_limit {
        return Err(AppError::LimitExceeded {
            what: "timeline entries required by render boundaries",
            actual: segments.len() as u64,
            limit: timeline_limit,
        });
    }
    let mut window_us = config.window_us.max(1);
    let reserve = segments.len() as u64;
    let target_windows = timeline_limit.saturating_sub(reserve).max(1);
    window_us = window_us.max(div_ceil(duration_us, target_windows));

    let (timeline_boundaries, final_window) = loop {
        let mut boundaries = grid_boundaries(duration_us, window_us);
        for boundary in &render_boundaries_vec {
            insert_boundary(&mut boundaries, *boundary, duration_us);
        }
        let entries = boundaries.len().saturating_sub(1) as u64;
        if entries <= timeline_limit {
            break (boundaries, window_us);
        }
        window_us = window_us.saturating_mul(2).max(window_us.saturating_add(1));
    };

    let mut timeline = Vec::with_capacity(timeline_boundaries.len().saturating_sub(1));
    let mut segment_index = 0_usize;
    let timeline_boundaries_vec: Vec<u64> = timeline_boundaries.into_iter().collect();
    for pair in timeline_boundaries_vec.windows(2) {
        let start = pair[0];
        let end = pair[1];
        if end <= start {
            continue;
        }
        while segment_index + 1 < segments.len() && start >= segments[segment_index].source_end_us {
            segment_index += 1;
        }
        let segment = &segments[segment_index];
        let midpoint = start + (end - start) / 2;
        let speed_rate_curve = speed.value_at(midpoint).unwrap_or(1.0);
        let pitch_semitones_curve = pitch.value_at(midpoint).unwrap_or(0.0);
        let pitch_evidence = pitch_evidence_at(pitch, midpoint, &config.gap_spans);
        timeline.push(TimelineDraft {
            render_segment_index: segment.index,
            source_start_us: start,
            source_end_us: end,
            speed_rate_curve,
            speed_rate_applied: segment.speed_rate,
            pitch_semitones_curve,
            pitch_semitones_applied: segment.pitch_semitones,
            pitch_ratio_applied: segment.pitch_ratio,
            pitch_evidence,
        });
    }

    let speed_range = range_of(segments.iter().map(|segment| segment.speed_rate), 1.0);
    let pitch_range = range_of(segments.iter().map(|segment| segment.pitch_semitones), 0.0);
    Ok(RenderPlan {
        duration_us,
        sample_rate_hz: config.sample_rate_hz,
        analysed_channel: config.analysed_channel,
        effective_window_us: final_window,
        effective_render_quantum_us: render_quantum,
        adaptive_window_applied: final_window > config.window_us.max(1)
            || render_quantum > config.render_quantum_us.max(1),
        segments,
        timeline,
        speed_range,
        pitch_range,
    })
}

pub fn finalize_timeline(
    plan: &RenderPlan,
    rendered: &[SegmentRenderResult],
    output_sample_rate_hz: u32,
) -> AppResult<Vec<TimelineEntry>> {
    if rendered.len() != plan.segments.len() {
        return Err(AppError::GStreamer(format!(
            "renderer returned {} segments, expected {}",
            rendered.len(),
            plan.segments.len()
        )));
    }
    if output_sample_rate_hz == 0 {
        return Err(AppError::InvalidInput(
            "output sample rate must be greater than zero".to_string(),
        ));
    }

    let mut output_cursor = 0_u64;
    let mut entries: Vec<TimelineEntry> = Vec::with_capacity(plan.timeline.len());
    for segment in &plan.segments {
        let segment_result = rendered
            .get(segment.index as usize)
            .ok_or_else(|| AppError::GStreamer("render segment index out of range".to_string()))?;
        let drafts: Vec<&TimelineDraft> = plan
            .timeline
            .iter()
            .filter(|draft| draft.render_segment_index == segment.index)
            .collect();
        if drafts.is_empty() {
            output_cursor = output_cursor.saturating_add(segment_result.output_frames);
            continue;
        }
        let source_total: u64 = drafts
            .iter()
            .map(|draft| draft.source_end_us - draft.source_start_us)
            .sum();
        let mut segment_frame_cursor = output_cursor;
        let mut segment_output_remaining = segment_result.output_frames;
        for (draft_index, draft) in drafts.iter().enumerate() {
            let source_span = draft.source_end_us - draft.source_start_us;
            let frames = if draft_index + 1 == drafts.len() {
                segment_output_remaining
            } else if source_total == 0 {
                0
            } else {
                let allocated = mul_div(segment_result.output_frames, source_span, source_total);
                allocated.min(segment_output_remaining)
            };
            segment_output_remaining = segment_output_remaining.saturating_sub(frames);
            let output_end_frame = segment_frame_cursor.saturating_add(frames);
            let index = entries.len() as u64;
            entries.push(TimelineEntry {
                index,
                render_segment_index: segment.index,
                track_id: format!("audio/channel-{}", plan.analysed_channel + 1),
                channel: plan.analysed_channel,
                source_start_us: draft.source_start_us,
                source_end_us: draft.source_end_us,
                source_frame_start: us_to_frames(draft.source_start_us, plan.sample_rate_hz),
                source_frame_end: us_to_frames(draft.source_end_us, plan.sample_rate_hz),
                output_start_us: frames_to_us(segment_frame_cursor, output_sample_rate_hz),
                output_end_us: frames_to_us(output_end_frame, output_sample_rate_hz),
                output_frame_start: segment_frame_cursor,
                output_frame_end: output_end_frame,
                speed_rate_curve: round6(draft.speed_rate_curve),
                speed_rate_applied: round6(draft.speed_rate_applied),
                pitch_semitones_curve: round6(draft.pitch_semitones_curve),
                pitch_semitones_applied: round6(draft.pitch_semitones_applied),
                pitch_ratio_applied: round6(draft.pitch_ratio_applied),
                pitch_evidence: draft.pitch_evidence,
                waveform: WaveformStats::default(),
            });
            segment_frame_cursor = output_end_frame;
        }
        output_cursor = output_cursor.saturating_add(segment_result.output_frames);
    }
    Ok(entries)
}

pub fn semitones_to_ratio(semitones: f64) -> f64 {
    2.0_f64.powf(semitones / 12.0)
}

pub fn pitch_evidence_at(pitch: &Curve, time_us: u64, gap_spans: &[TimeSpan]) -> EvidenceState {
    if pitch.points.is_empty() {
        return EvidenceState::DefaultNominal;
    }
    if gap_spans
        .iter()
        .any(|span| time_us > span.start_us && time_us < span.end_us)
    {
        EvidenceState::HeldLastKnown
    } else {
        EvidenceState::Observed
    }
}

pub fn source_span_to_frames(span: TimeSpan, sample_rate_hz: u32) -> FrameSpan {
    FrameSpan {
        start: us_to_frames(span.start_us, sample_rate_hz),
        end: us_to_frames(span.end_us, sample_rate_hz),
    }
}

pub fn us_to_frames(us: u64, sample_rate_hz: u32) -> u64 {
    mul_div(us, sample_rate_hz as u64, 1_000_000)
}

pub fn frames_to_us(frames: u64, sample_rate_hz: u32) -> u64 {
    if sample_rate_hz == 0 {
        0
    } else {
        mul_div(frames, 1_000_000, sample_rate_hz as u64)
    }
}

fn mul_div(value: u64, multiplier: u64, divisor: u64) -> u64 {
    if divisor == 0 {
        return 0;
    }
    ((value as u128 * multiplier as u128) / divisor as u128) as u64
}

fn div_ceil(value: u64, divisor: u64) -> u64 {
    value / divisor + u64::from(value % divisor != 0)
}

fn grid_boundaries(duration_us: u64, step_us: u64) -> BTreeSet<u64> {
    let mut boundaries = BTreeSet::new();
    boundaries.insert(0);
    let step = step_us.max(1);
    let mut cursor = step;
    while cursor < duration_us {
        boundaries.insert(cursor);
        cursor = cursor.saturating_add(step);
    }
    boundaries.insert(duration_us);
    boundaries
}

fn insert_boundary(boundaries: &mut BTreeSet<u64>, value: u64, duration_us: u64) {
    if value > 0 && value < duration_us {
        boundaries.insert(value);
    }
}

fn ensure_segment_limit(boundaries: &BTreeSet<u64>, limit: u64) -> AppResult<()> {
    let segments = boundaries.len().saturating_sub(1) as u64;
    if segments > limit {
        return Err(AppError::LimitExceeded {
            what: "render segments",
            actual: segments,
            limit,
        });
    }
    Ok(())
}

fn range_of(values: impl Iterator<Item = f64>, default: f64) -> ParameterRange {
    let mut min = default;
    let mut max = default;
    let mut seen = false;
    for value in values {
        if !seen {
            min = value;
            max = value;
            seen = true;
        } else {
            min = min.min(value);
            max = max.max(value);
        }
    }
    ParameterRange {
        min: round6(min),
        max: round6(max),
    }
}
