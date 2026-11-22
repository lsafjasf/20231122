//! Two device clocks that are deliberately not synchronized.
//!
//! The capture device (A) and the analysis device (B) keep their own wall
//! clocks. B runs ahead/behind A by a fixed offset plus a small drift.
//! A calibrator collects paired samples at checkpoint beacons and
//! estimates the offset so analysis-side timestamps can be normalized
//! into the capture clock domain.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::model::{ClockCalibration, ClockSample};

fn real_now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Wall clock of one device.
#[derive(Debug, Clone)]
pub struct DeviceClock {
    pub device_name: String,
    offset_ms: i64,
    drift_ppm: f64,
    boot_ms: i64,
}

impl DeviceClock {
    pub fn new(device_name: &str, offset_ms: i64, drift_ppm: f64) -> Self {
        Self {
            device_name: device_name.to_string(),
            offset_ms,
            drift_ppm,
            boot_ms: real_now_ms(),
        }
    }

    pub fn now_ms(&self) -> i64 {
        let real = real_now_ms();
        let elapsed = (real - self.boot_ms) as f64;
        real + self.offset_ms + (elapsed * self.drift_ppm / 1.0e6) as i64
    }
}

/// The two clocks of the pipeline. Offsets can be injected via the
/// environment so the unsynchronized scenario is reproducible in tests:
/// `SFC_CLOCK_SKEW_MS` (default +1750 ms) and `SFC_CLOCK_DRIFT_PPM`.
#[derive(Debug, Clone)]
pub struct ClockPair {
    pub capture: DeviceClock,
    pub analysis: DeviceClock,
}

impl ClockPair {
    pub fn from_environment() -> Self {
        let skew: i64 = std::env::var("SFC_CLOCK_SKEW_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1750);
        let drift: f64 = std::env::var("SFC_CLOCK_DRIFT_PPM")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(40.0);
        Self {
            capture: DeviceClock::new("capture-device-A", 0, 0.0),
            analysis: DeviceClock::new("analysis-device-B", skew, drift),
        }
    }

    /// A beacon: read both clocks as close together as possible.
    pub fn sample(&self) -> ClockSample {
        ClockSample {
            capture_ts_ms: self.capture.now_ms(),
            analysis_ts_ms: self.analysis.now_ms(),
        }
    }
}

/// Estimates the analysis-minus-capture offset from paired beacon samples.
#[derive(Debug, Default)]
pub struct ClockCalibrator;

impl ClockCalibrator {
    pub fn calibrate(
        capture: &DeviceClock,
        analysis: &DeviceClock,
        samples: &[ClockSample],
    ) -> ClockCalibration {
        let mut diffs: Vec<i64> = samples
            .iter()
            .map(|s| s.analysis_ts_ms - s.capture_ts_ms)
            .collect();
        diffs.sort_unstable();
        let median = if diffs.is_empty() {
            0
        } else {
            diffs[diffs.len() / 2]
        };
        let max_dev = diffs.iter().map(|d| (d - median).abs()).max().unwrap_or(0);
        ClockCalibration {
            capture_device: capture.device_name.clone(),
            analysis_device: analysis.device_name.clone(),
            paired_samples: samples.len() as u64,
            estimated_offset_ms: median,
            max_abs_deviation_ms: max_dev,
        }
    }

    /// Normalize an analysis-side timestamp into the capture clock domain.
    pub fn normalize(analysis_ts_ms: i64, estimated_offset_ms: i64) -> i64 {
        analysis_ts_ms - estimated_offset_ms
    }
}
