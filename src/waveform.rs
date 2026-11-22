use crate::model::{Processor, WaveformBucket, WaveformSummary};
use crate::util::{frame_to_ms, rounded};

pub fn compute(samples: &[f32], sample_rate: u32, max_buckets: usize) -> WaveformSummary {
    let max_buckets = max_buckets.max(1);
    let total_frames = samples.len() as u64;
    let bucket_frames = (samples.len() as f64 / max_buckets as f64).ceil().max(1.0) as usize;
    let bucket_ms = frame_to_ms(bucket_frames as u64, sample_rate);
    let mut buckets = Vec::with_capacity((samples.len() / bucket_frames).saturating_add(1));
    let mut global_min = 0.0_f32;
    let mut global_max = 0.0_f32;
    let mut global_peak = 0.0_f32;
    let mut squared_total = 0.0_f64;

    for (index, chunk) in samples.chunks(bucket_frames).enumerate() {
        let mut peak = 0.0_f32;
        let mut sum_squares = 0.0_f64;
        for sample in chunk {
            let finite = if sample.is_finite() { *sample } else { 0.0 };
            peak = peak.max(finite.abs());
            sum_squares += (finite as f64).powi(2);
            global_min = global_min.min(finite);
            global_max = global_max.max(finite);
        }
        let rms = (sum_squares / chunk.len().max(1) as f64).sqrt() as f32;
        global_peak = global_peak.max(peak);
        squared_total += sum_squares;
        let start_frame = (index * bucket_frames) as u64;
        let end_frame = ((index + 1) * bucket_frames).min(samples.len()) as u64;
        buckets.push(WaveformBucket {
            index,
            start_frame,
            end_frame,
            peak: rounded_f32(peak),
            rms: rounded_f32(rms),
        });
    }

    let global_rms = (squared_total / samples.len().max(1) as f64).sqrt() as f32;
    WaveformSummary {
        bucket_ms: rounded(bucket_ms, 6),
        bucket_count: buckets.len(),
        source_frame_count: total_frames,
        min: rounded_f32(global_min),
        max: rounded_f32(global_max),
        rms: rounded_f32(global_rms),
        peak: rounded_f32(global_peak),
        buckets,
    }
}

pub fn to_csv(summary: &WaveformSummary, sample_rate: u32) -> String {
    let mut output = String::from("bucket_index,start_frame,end_frame,start_ms,end_ms,peak,rms\n");
    for bucket in &summary.buckets {
        output.push_str(&format!(
            "{},{},{},{:.6},{:.6},{:.6},{:.6}\n",
            bucket.index,
            bucket.start_frame,
            bucket.end_frame,
            frame_to_ms(bucket.start_frame, sample_rate),
            frame_to_ms(bucket.end_frame, sample_rate),
            bucket.peak,
            bucket.rms
        ));
    }
    output
}

pub fn apply_processor(
    samples: &mut [f32],
    sample_rate: u32,
    processor: &Processor,
) -> Vec<String> {
    let mut warnings = Vec::new();
    if samples.is_empty() {
        return warnings;
    }
    match processor {
        Processor::Gain { gain_db, fade_ms } => {
            let gain = 10_f64.powf(gain_db / 20.0);
            apply_gain(samples, sample_rate, gain, *fade_ms);
        }
        Processor::Mute { fade_ms } => {
            apply_gain(samples, sample_rate, 0.0, *fade_ms);
        }
        Processor::ReplaceSilence { fade_ms } => {
            apply_gain(samples, sample_rate, 0.0, *fade_ms);
        }
        Processor::Normalize {
            target_peak_db,
            fade_ms,
        } => {
            let current_peak = samples
                .iter()
                .filter(|sample| sample.is_finite())
                .map(|sample| sample.abs())
                .fold(0.0_f32, f32::max);
            if current_peak <= f32::EPSILON {
                warnings.push("NORMALIZE_SKIPPED_SILENT_REGION".to_string());
            } else {
                let target = 10_f64.powf(target_peak_db / 20.0) as f32;
                let gain = (target / current_peak) as f64;
                apply_gain(samples, sample_rate, gain, *fade_ms);
            }
        }
        Processor::Declick => {
            let start = samples.first().copied().unwrap_or_default();
            let end = samples.last().copied().unwrap_or_default();
            let denominator = (samples.len() - 1).max(1) as f32;
            for (index, sample) in samples.iter_mut().enumerate() {
                if sample.is_finite() {
                    *sample = start + (end - start) * (index as f32 / denominator);
                }
            }
        }
    }
    let mut clipped = 0_usize;
    for sample in samples.iter_mut() {
        if !sample.is_finite() {
            *sample = 0.0;
            clipped += 1;
        } else if *sample > 1.0 {
            *sample = 1.0;
            clipped += 1;
        } else if *sample < -1.0 {
            *sample = -1.0;
            clipped += 1;
        }
    }
    if clipped > 0 {
        warnings.push(format!("OUTPUT_CLIPPED_SAMPLES count={clipped}"));
    }
    warnings
}

fn apply_gain(samples: &mut [f32], sample_rate: u32, gain: f64, fade_ms: f64) {
    let fade_frames =
        ((fade_ms.max(0.0) * sample_rate as f64 / 1000.0).round() as usize).min(samples.len() / 2);
    let length = samples.len();
    for (index, sample) in samples.iter_mut().enumerate() {
        let mut weight = 1.0_f64;
        if fade_frames > 0 {
            if index < fade_frames {
                weight = index as f64 / fade_frames as f64;
            } else if index >= length - fade_frames {
                weight = (length - 1 - index) as f64 / fade_frames as f64;
            }
        }
        let multiplier = 1.0 + (gain - 1.0) * weight;
        *sample = (*sample as f64 * multiplier) as f32;
    }
}

fn rounded_f32(value: f32) -> f32 {
    let finite = if value.is_finite() { value } else { 0.0 };
    (finite as f64 * 1_000_000.0).round() as f32 / 1_000_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_count_is_bounded_and_frames_cover_input() {
        let samples = vec![0.25_f32; 48_000];
        let summary = compute(&samples, 48_000, 100);
        assert!(summary.bucket_count <= 100);
        assert_eq!(summary.source_frame_count, 48_000);
        assert_eq!(summary.buckets.last().unwrap().end_frame, 48_000);
        assert!((summary.peak - 0.25).abs() < 1e-6);
    }

    #[test]
    fn empty_waveform_is_explicit() {
        let summary = compute(&[], 48_000, 100);
        assert_eq!(summary.bucket_count, 0);
        assert_eq!(summary.source_frame_count, 0);
        assert_eq!(summary.peak, 0.0);
    }

    #[test]
    fn gain_and_clipping_are_reported() {
        let mut samples = vec![0.9_f32, 0.0, -0.9];
        let warnings = apply_processor(
            &mut samples,
            48_000,
            &Processor::Gain {
                gain_db: 12.0,
                fade_ms: 0.0,
            },
        );
        assert!(warnings.iter().any(|item| item.contains("CLIPPED")));
        assert!(samples.iter().all(|sample| sample.abs() <= 1.0));
    }

    #[test]
    fn fade_preserves_interior_but_softens_edges() {
        let mut samples = vec![1.0_f32; 1_000];
        apply_processor(
            &mut samples,
            48_000,
            &Processor::Gain {
                gain_db: -120.0,
                fade_ms: 10.0,
            },
        );
        assert_eq!(samples[0], 1.0);
        assert!(samples[500] < 0.001);
        assert_eq!(*samples.last().unwrap(), 1000.0_f32 / 1000.0);
    }
}
