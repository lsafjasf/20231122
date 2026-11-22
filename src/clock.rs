use std::collections::{BTreeMap, BTreeSet};

use crate::limits::Limits;
use crate::model::{
    ClockEvidence, DeviceClock, IntervalEvidence, RepairInput, RepairInterval, SourcePoint, Track,
};
use crate::util::{ms_to_frame, rounded};

pub const CLOCK_FORMULA: &str = "canonical_ms=(device_ms-offset_ms)/(1+drift_ppm/1_000_000)";

#[derive(Debug, Clone)]
pub struct ClockAnalysis {
    pub clocks: Vec<ClockEvidence>,
    pub intervals: Vec<IntervalEvidence>,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct MappedPoint {
    pub canonical_ms: f64,
    pub canonical_frame: u64,
    pub source_frame_delta: i64,
    pub warnings: Vec<String>,
}

struct IntervalContext<'a> {
    devices: &'a BTreeMap<String, DeviceClock>,
    tracks: &'a BTreeMap<u32, Track>,
    output_sample_rate: u32,
    total_frames: u64,
    duration_ms: f64,
    tolerance_ms: f64,
}

pub fn canonical_ms(clock: &DeviceClock, device_ms: f64) -> f64 {
    let scale = 1.0 + clock.drift_ppm / 1_000_000.0;
    if scale.abs() < f64::EPSILON {
        return f64::NAN;
    }
    (device_ms - clock.offset_ms) / scale
}

pub fn map_point(
    clock: &DeviceClock,
    track: &Track,
    point: &SourcePoint,
    output_sample_rate: u32,
    tolerance_ms: f64,
) -> MappedPoint {
    let mapped_ms = canonical_ms(clock, point.device_ms);
    let canonical_frame = ms_to_frame(mapped_ms, output_sample_rate);
    let expected_source_frame = ms_to_frame(point.device_ms, track.sample_rate);
    let mut warnings = Vec::new();
    let mut delta = 0_i64;
    if let Some(frame) = point.frame {
        let frame_delta = frame as i128 - expected_source_frame as i128;
        delta = frame_delta.clamp(i64::MIN as i128, i64::MAX as i128) as i64;
        let tolerance_frames = (tolerance_ms * track.sample_rate as f64 / 1000.0).ceil() as i128;
        if frame_delta.abs() > tolerance_frames {
            warnings.push(format!(
                "FRAME_TIME_MISMATCH frame={} expected={} delta_frames={}",
                frame, expected_source_frame, frame_delta
            ));
        }
    }
    MappedPoint {
        canonical_ms: mapped_ms,
        canonical_frame,
        source_frame_delta: delta,
        warnings,
    }
}

pub fn analyze(
    input: &RepairInput,
    output_sample_rate: u32,
    total_frames: u64,
    duration_ms: f64,
    limits: &Limits,
) -> ClockAnalysis {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let mut devices = BTreeMap::<String, DeviceClock>::new();
    let mut tracks = BTreeMap::<u32, Track>::new();

    if input.schema_version != crate::model::SCHEMA_VERSION {
        errors.push(format!(
            "intervals schema_version={} expected={}",
            input.schema_version,
            crate::model::SCHEMA_VERSION
        ));
    }
    if input.tolerance_ms < 0.0 || !input.tolerance_ms.is_finite() {
        errors.push("tolerance_ms must be finite and non-negative".to_string());
    }
    if input.devices.len() > 128 {
        errors.push(format!(
            "max_devices exceeded: actual={} allowed=128",
            input.devices.len()
        ));
    }
    for device in &input.devices {
        if device.id.trim().is_empty() {
            errors.push("device id must not be empty".to_string());
        }
        if devices.insert(device.id.clone(), device.clone()).is_some() {
            errors.push(format!("duplicate device id={}", device.id));
        }
        if !device.offset_ms.is_finite() || !device.drift_ppm.is_finite() {
            errors.push(format!("device {} clock values must be finite", device.id));
        }
        if device.drift_ppm.abs() > 10_000.0 {
            warnings.push(format!(
                "CLOCK_DRIFT_LARGE device={} drift_ppm={}",
                device.id, device.drift_ppm
            ));
        }
    }
    if !devices.contains_key(&input.reference_device_id) {
        errors.push(format!(
            "reference device not found: {}",
            input.reference_device_id
        ));
    }
    for track in &input.tracks {
        if track.sample_rate == 0 {
            errors.push(format!("track {} sample_rate must be > 0", track.id));
        }
        if !devices.contains_key(&track.device_id) {
            errors.push(format!(
                "track {} references unknown device {}",
                track.id, track.device_id
            ));
        }
        if tracks.insert(track.id, track.clone()).is_some() {
            errors.push(format!("duplicate track id={}", track.id));
        }
    }
    if input.intervals.len() > limits.max_intervals {
        errors.push(format!(
            "max_intervals exceeded: actual={} allowed={}",
            input.intervals.len(),
            limits.max_intervals
        ));
    }

    let mut seen = BTreeSet::new();
    let mut intervals = Vec::with_capacity(input.intervals.len());
    let context = IntervalContext {
        devices: &devices,
        tracks: &tracks,
        output_sample_rate,
        total_frames,
        duration_ms,
        tolerance_ms: input.tolerance_ms,
    };
    for interval in &input.intervals {
        if !seen.insert(interval.id.clone()) {
            errors.push(format!("duplicate interval id={}", interval.id));
        }
        let evidence = analyze_interval(interval, &context, &mut errors);
        intervals.push(evidence);
    }

    let mut clocks = Vec::with_capacity(input.devices.len());
    for device in &input.devices {
        let max_skew = intervals
            .iter()
            .filter(|interval| interval.device_id == device.id)
            .map(|interval| interval.clock_skew_ms.abs())
            .fold(0.0_f64, f64::max);
        let max_interval = intervals
            .iter()
            .filter(|interval| interval.device_id == device.id)
            .max_by(|left, right| {
                left.clock_skew_ms
                    .abs()
                    .total_cmp(&right.clock_skew_ms.abs())
            })
            .map(|interval| interval.interval_id.clone())
            .unwrap_or_default();
        let warning = if device.id != input.reference_device_id && max_skew > input.tolerance_ms {
            Some(format!(
                "CLOCK_NOT_SYNCHRONIZED observed_skew_ms={} tolerance_ms={}",
                rounded(max_skew, 3),
                rounded(input.tolerance_ms, 3)
            ))
        } else {
            None
        };
        clocks.push(ClockEvidence {
            device_id: device.id.clone(),
            label: device.label.clone(),
            role: if device.id == input.reference_device_id {
                "reference".to_string()
            } else {
                "source".to_string()
            },
            offset_ms: rounded(device.offset_ms, 6),
            drift_ppm: rounded(device.drift_ppm, 6),
            conversion_formula: CLOCK_FORMULA.to_string(),
            max_observed_skew_ms: rounded(max_skew, 6),
            max_observed_skew_interval: max_interval,
            warning,
        });
    }

    collect_overlaps(&mut intervals, &mut warnings);
    ClockAnalysis {
        clocks,
        intervals,
        warnings,
        errors,
    }
}

fn analyze_interval(
    interval: &RepairInterval,
    context: &IntervalContext<'_>,
    errors: &mut Vec<String>,
) -> IntervalEvidence {
    let mut warnings = Vec::new();
    let device = context.devices.get(&interval.device_id);
    let track = context.tracks.get(&interval.track_id);
    if device.is_none() {
        errors.push(format!(
            "interval {} references unknown device {}",
            interval.id, interval.device_id
        ));
    }
    if track.is_none() {
        errors.push(format!(
            "interval {} references unknown track {}",
            interval.id, interval.track_id
        ));
    }
    if !interval.start.device_ms.is_finite() || !interval.end.device_ms.is_finite() {
        errors.push(format!(
            "interval {} time values must be finite",
            interval.id
        ));
    }
    if interval.end.device_ms <= interval.start.device_ms {
        errors.push(format!(
            "interval {} end must be greater than start",
            interval.id
        ));
    }
    validate_processor(&interval.processor, &interval.id, errors);

    let (start_ms, start_frame, start_delta) = device
        .zip(track)
        .map(|(device, track)| {
            let point = map_point(
                device,
                track,
                &interval.start,
                context.output_sample_rate,
                context.tolerance_ms,
            );
            warnings.extend(point.warnings);
            (
                point.canonical_ms,
                point.canonical_frame,
                point.source_frame_delta,
            )
        })
        .unwrap_or((0.0, 0, 0));
    let (end_ms, end_frame, end_delta) = device
        .zip(track)
        .map(|(device, track)| {
            let point = map_point(
                device,
                track,
                &interval.end,
                context.output_sample_rate,
                context.tolerance_ms,
            );
            warnings.extend(point.warnings);
            (
                point.canonical_ms,
                point.canonical_frame,
                point.source_frame_delta,
            )
        })
        .unwrap_or((0.0, 0, 0));

    let mut applied_start = start_frame.min(context.total_frames);
    let mut applied_end = end_frame.min(context.total_frames);
    let mut applied = true;
    let mut disposition = "applied".to_string();
    if start_ms < 0.0 || end_ms < 0.0 {
        warnings.push(format!(
            "INTERVAL_NEGATIVE_CANONICAL start_ms={} end_ms={}",
            rounded(start_ms, 3),
            rounded(end_ms, 3)
        ));
    }
    if end_ms > context.duration_ms {
        warnings.push(format!(
            "INTERVAL_TRIMMED end_ms={} media_duration_ms={}",
            rounded(end_ms, 3),
            rounded(context.duration_ms, 3)
        ));
    }
    if end_frame <= start_frame || applied_end <= applied_start {
        applied = false;
        disposition = "outside_media".to_string();
        applied_start = applied_start.min(context.total_frames);
        applied_end = applied_start;
        warnings.push("INTERVAL_OUTSIDE_MEDIA".to_string());
    }
    if device.is_none() || track.is_none() {
        applied = false;
        disposition = "validation_error".to_string();
        applied_start = 0;
        applied_end = 0;
    }

    let clock_skew = interval.start.device_ms - start_ms;
    let raw_duration = interval.end.device_ms - interval.start.device_ms;
    let canonical_duration = end_ms - start_ms;
    IntervalEvidence {
        interval_id: interval.id.clone(),
        track_id: interval.track_id,
        track_label: track.map(|value| value.label.clone()).unwrap_or_default(),
        device_id: interval.device_id.clone(),
        processor: interval.processor.clone(),
        requested_start_ms: rounded(interval.start.device_ms, 6),
        requested_end_ms: rounded(interval.end.device_ms, 6),
        requested_start_frame: interval.start.frame,
        requested_end_frame: interval.end.frame,
        canonical_start_ms: rounded(start_ms, 6),
        canonical_end_ms: rounded(end_ms, 6),
        applied_start_frame: applied_start,
        applied_end_frame: applied_end,
        source_frame_delta: start_delta,
        end_source_frame_delta: end_delta,
        clock_skew_ms: rounded(clock_skew, 6),
        duration_delta_ms: rounded(canonical_duration - raw_duration, 6),
        applied,
        disposition,
        warnings,
    }
}

fn validate_processor(
    processor: &crate::model::Processor,
    interval_id: &str,
    errors: &mut Vec<String>,
) {
    use crate::model::Processor;
    match processor {
        Processor::Gain { gain_db, fade_ms } => {
            if !gain_db.is_finite() || *gain_db < -120.0 || *gain_db > 24.0 {
                errors.push(format!(
                    "interval {interval_id} gain_db must be finite and in [-120, 24]"
                ));
            }
            if !fade_ms.is_finite() || *fade_ms < 0.0 {
                errors.push(format!("interval {interval_id} fade_ms must be >= 0"));
            }
        }
        Processor::Normalize {
            target_peak_db,
            fade_ms,
        } => {
            if !target_peak_db.is_finite() || *target_peak_db < -120.0 || *target_peak_db > 0.0 {
                errors.push(format!(
                    "interval {interval_id} target_peak_db must be in [-120, 0]"
                ));
            }
            if !fade_ms.is_finite() || *fade_ms < 0.0 {
                errors.push(format!("interval {interval_id} fade_ms must be >= 0"));
            }
        }
        Processor::Mute { fade_ms } | Processor::ReplaceSilence { fade_ms } => {
            if !fade_ms.is_finite() || *fade_ms < 0.0 {
                errors.push(format!("interval {interval_id} fade_ms must be >= 0"));
            }
        }
        Processor::Declick => {}
    }
}

fn collect_overlaps(intervals: &mut [IntervalEvidence], warnings: &mut Vec<String>) {
    let mut ordered: Vec<(usize, u64, u64, String)> = intervals
        .iter()
        .enumerate()
        .filter(|(_, interval)| interval.applied)
        .map(|(index, interval)| {
            (
                index,
                interval.applied_start_frame,
                interval.applied_end_frame,
                interval.interval_id.clone(),
            )
        })
        .collect();
    ordered.sort_by_key(|(_, start, end, _)| (*start, *end));
    for pair in ordered.windows(2) {
        let (_, _, previous_end, previous_id) = &pair[0];
        let (current_index, current_start, _, current_id) = &pair[1];
        if *current_start < *previous_end {
            let message = format!(
                "OVERLAPPING_REPAIR intervals={} order_is_version_manifest",
                [previous_id.as_str(), current_id.as_str()].join(",")
            );
            warnings.push(message.clone());
            intervals[*current_index].warnings.push(message);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Processor, SCHEMA_VERSION};

    fn input() -> RepairInput {
        RepairInput {
            schema_version: SCHEMA_VERSION,
            reference_device_id: "A".to_string(),
            tolerance_ms: 20.0,
            devices: vec![
                DeviceClock {
                    id: "A".to_string(),
                    label: "Recorder A".to_string(),
                    offset_ms: 0.0,
                    drift_ppm: 0.0,
                },
                DeviceClock {
                    id: "B".to_string(),
                    label: "Recorder B".to_string(),
                    offset_ms: 7_312.0,
                    drift_ppm: -140.0,
                },
            ],
            tracks: vec![
                Track {
                    id: 1,
                    device_id: "A".to_string(),
                    sample_rate: 48_000,
                    label: "A main".to_string(),
                },
                Track {
                    id: 2,
                    device_id: "B".to_string(),
                    sample_rate: 48_000,
                    label: "B main".to_string(),
                },
            ],
            intervals: vec![RepairInterval {
                id: "repair-1".to_string(),
                track_id: 2,
                device_id: "B".to_string(),
                start: SourcePoint {
                    device_ms: 27_309.2,
                    frame: Some(1_310_842),
                },
                end: SourcePoint {
                    device_ms: 27_809.23,
                    frame: Some(1_334_843),
                },
                processor: Processor::Gain {
                    gain_db: -6.0,
                    fade_ms: 10.0,
                },
                note: "overlap repair".to_string(),
            }],
        }
    }

    #[test]
    fn inverse_mapping_absorbs_offset_and_drift() {
        let clock = &input().devices[1];
        let mapped = canonical_ms(clock, 27_309.2);
        assert!((mapped - 20_000.0).abs() < 1.0);
    }

    #[test]
    fn anomaly_is_kept_as_evidence_and_remains_applied() {
        let analysis = analyze(&input(), 48_000, 1_920_000, 40_000.0, &Limits::default());
        assert!(analysis.errors.is_empty(), "{:?}", analysis.errors);
        let interval = &analysis.intervals[0];
        assert!(interval.applied);
        assert!((interval.applied_start_frame as f64 - 960_000.0).abs() < 60.0);
        assert!(interval.clock_skew_ms > 7_000.0);
        assert!(analysis.clocks[1].warning.is_some());
    }

    #[test]
    fn empty_interval_list_is_valid() {
        let mut value = input();
        value.intervals.clear();
        let analysis = analyze(&value, 48_000, 100, 10.0, &Limits::default());
        assert!(analysis.errors.is_empty());
        assert!(analysis.intervals.is_empty());
    }

    #[test]
    fn out_of_range_is_trimmed_without_losing_requested_times() {
        let mut value = input();
        value.intervals[0].end.device_ms = 100_000.0;
        let analysis = analyze(&value, 48_000, 1_920_000, 40_000.0, &Limits::default());
        let interval = &analysis.intervals[0];
        assert!(interval.applied_start_frame < interval.applied_end_frame);
        assert_eq!(interval.requested_end_ms, 100_000.0);
        assert_eq!(interval.applied_end_frame, 1_920_000);
        assert!(
            interval
                .warnings
                .iter()
                .any(|item| item.contains("TRIMMED"))
        );
    }

    #[test]
    fn missing_version_reference_is_a_clock_error() {
        let mut value = input();
        value.intervals[0].device_id = "Z".to_string();
        let analysis = analyze(&value, 48_000, 100_000, 2_000.0, &Limits::default());
        assert!(!analysis.errors.is_empty());
        assert!(!analysis.intervals[0].applied);
        assert_eq!(analysis.intervals[0].disposition, "validation_error");
    }
}
