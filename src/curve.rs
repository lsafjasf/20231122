use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{io_error, AppError, AppResult};
use crate::model::{CurvePoint, TimeSpan};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CurveKind {
    Speed,
    Pitch,
}

#[derive(Debug, Clone)]
pub struct Curve {
    pub kind: CurveKind,
    pub points: Vec<CurvePoint>,
    pub time_unit: String,
}

#[derive(Debug, Clone, Copy)]
pub struct CurveLimits {
    pub max_bytes: u64,
    pub max_points: u64,
}

impl Curve {
    pub fn internal_gaps(&self, threshold_us: u64) -> Vec<TimeSpan> {
        self.points
            .windows(2)
            .filter_map(|pair| {
                let gap = pair[1].time_us.saturating_sub(pair[0].time_us);
                (gap > threshold_us).then_some(TimeSpan {
                    start_us: pair[0].time_us,
                    end_us: pair[1].time_us,
                })
            })
            .collect()
    }

    pub fn value_at(&self, time_us: u64) -> Option<f64> {
        if self.points.is_empty() {
            return None;
        }
        if time_us <= self.points[0].time_us {
            return Some(self.points[0].value);
        }
        let last = self.points.last().expect("non-empty");
        if time_us >= last.time_us {
            return Some(last.value);
        }
        let upper = self
            .points
            .partition_point(|point| point.time_us <= time_us);
        let right = self.points[upper];
        let left = self.points[upper - 1];
        if right.time_us == left.time_us {
            return Some(left.value);
        }
        let ratio = (time_us - left.time_us) as f64 / (right.time_us - left.time_us) as f64;
        Some(left.value + (right.value - left.value) * ratio)
    }

    pub fn is_in_internal_gap(&self, time_us: u64, threshold_us: u64) -> bool {
        self.points.windows(2).any(|pair| {
            pair[1].time_us.saturating_sub(pair[0].time_us) > threshold_us
                && time_us > pair[0].time_us
                && time_us < pair[1].time_us
        })
    }
}

pub fn load_curve(path: &Path, kind: CurveKind, limits: CurveLimits) -> AppResult<Curve> {
    let metadata = std::fs::metadata(path).map_err(|err| io_error(path, err))?;
    if metadata.len() > limits.max_bytes {
        return Err(AppError::LimitExceeded {
            what: "curve file bytes",
            actual: metadata.len(),
            limit: limits.max_bytes,
        });
    }

    let file = File::open(path).map_err(|err| io_error(path, err))?;
    let mut reader = csv::ReaderBuilder::new()
        .trim(csv::Trim::All)
        .flexible(false)
        .comment(Some(b'#'))
        .from_reader(BufReader::new(file));

    let mut time_unit = "microseconds".to_string();
    let mut time_scale = 1.0_f64;
    let mut points = Vec::new();
    let mut previous_time: Option<u64> = None;
    let mut first_record = true;

    for result in reader.records() {
        let record = result?;
        if record.len() < 2 {
            return Err(AppError::InvalidInput(format!(
                "{}: expected two columns, got {}",
                path.display(),
                record.len()
            )));
        }
        let first = record.get(0).unwrap_or("").trim();
        let second = record.get(1).unwrap_or("").trim();
        if first.is_empty() || second.is_empty() {
            continue;
        }
        if first_record && !looks_numeric(first) {
            let (unit_name, scale) = time_header_unit(first).ok_or_else(|| {
                AppError::InvalidInput(format!(
                    "{}: unsupported time header '{first}'",
                    path.display()
                ))
            })?;
            time_unit = unit_name.to_string();
            time_scale = scale;
            first_record = false;
            continue;
        }
        first_record = false;
        let raw_time = first.parse::<f64>().map_err(|_| {
            AppError::InvalidInput(format!("{}: invalid time '{first}'", path.display()))
        })?;
        let value = second.parse::<f64>().map_err(|_| {
            AppError::InvalidInput(format!("{}: invalid value '{second}'", path.display()))
        })?;
        if !raw_time.is_finite() || raw_time < 0.0 || !value.is_finite() {
            return Err(AppError::InvalidInput(format!(
                "{}: non-finite or negative point ({first}, {second})",
                path.display()
            )));
        }
        let time_us = (raw_time * time_scale).round() as u64;
        validate_value(kind, value, path)?;
        if let Some(previous) = previous_time {
            if time_us <= previous {
                return Err(AppError::InvalidInput(format!(
                    "{}: time must be strictly increasing ({previous} then {time_us})",
                    path.display()
                )));
            }
        }
        previous_time = Some(time_us);
        points.push(CurvePoint { time_us, value });
        if points.len() as u64 > limits.max_points {
            return Err(AppError::LimitExceeded {
                what: "curve points",
                actual: points.len() as u64,
                limit: limits.max_points,
            });
        }
    }

    Ok(Curve {
        kind,
        points,
        time_unit,
    })
}

fn looks_numeric(value: &str) -> bool {
    value.parse::<f64>().is_ok()
}

fn time_header_unit(header: &str) -> Option<(&'static str, f64)> {
    let normalized = header.trim().to_ascii_lowercase().replace(['-', ' '], "_");
    match normalized.as_str() {
        "time_us" | "timestamp_us" | "us" => Some(("microseconds", 1.0)),
        "time_ms" | "timestamp_ms" | "ms" => Some(("milliseconds", 1_000.0)),
        "time_s" | "timestamp_s" | "seconds" | "s" => Some(("seconds", 1_000_000.0)),
        _ => None,
    }
}

fn validate_value(kind: CurveKind, value: f64, path: &Path) -> AppResult<()> {
    let valid = match kind {
        CurveKind::Speed => (0.1..=4.0).contains(&value),
        CurveKind::Pitch => (-24.0..=24.0).contains(&value),
    };
    if !valid {
        let expected = match kind {
            CurveKind::Speed => "speed in [0.1, 4.0]",
            CurveKind::Pitch => "pitch semitones in [-24, 24]",
        };
        return Err(AppError::InvalidInput(format!(
            "{}: value {value} is outside {expected}",
            path.display()
        )));
    }
    Ok(())
}

pub fn sha256_file(path: &Path) -> AppResult<String> {
    use sha2::{Digest, Sha256};

    let file = File::open(path).map_err(|err| io_error(path, err))?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|err| io_error(path, err))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}
