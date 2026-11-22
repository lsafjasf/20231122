use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

pub fn sha256_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

pub fn sha256_file(path: &Path) -> Result<(u64, String)> {
    let file =
        File::open(path).with_context(|| format!("cannot open {} for hashing", path.display()))?;
    let mut reader = BufReader::with_capacity(1024 * 1024, file);
    let mut digest = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .with_context(|| format!("cannot read {} while hashing", path.display()))?;
        if read == 0 {
            break;
        }
        total += read as u64;
        digest.update(&buffer[..read]);
    }
    Ok((total, hex::encode(digest.finalize())))
}

pub fn rounded(value: f64, places: i32) -> f64 {
    if !value.is_finite() {
        return 0.0;
    }
    let factor = 10_f64.powi(places);
    (value * factor).round() / factor
}

pub fn ms_to_frame(ms: f64, sample_rate: u32) -> u64 {
    if !ms.is_finite() || ms <= 0.0 {
        return 0;
    }
    (ms * sample_rate as f64 / 1000.0).round() as u64
}

pub fn frame_to_ms(frame: u64, sample_rate: u32) -> f64 {
    frame as f64 * 1000.0 / sample_rate as f64
}

pub fn unix_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

pub fn iso8601_utc(unix_ms: u128) -> String {
    let seconds = (unix_ms / 1000) as i64;
    let millis = (unix_ms % 1000) as u32;
    let days = seconds.div_euclid(86_400);
    let day_seconds = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = day_seconds / 3600;
    let minute = (day_seconds % 3600) / 60;
    let second = day_seconds % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

fn civil_from_days(days_since_epoch: i64) -> (i64, i64, i64) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month, day)
}

pub fn stable_run_id(parts: &[&str]) -> String {
    let mut digest = Sha256::new();
    for part in parts {
        digest.update((part.len() as u64).to_le_bytes());
        digest.update(part.as_bytes());
    }
    hex::encode(digest.finalize())[..16].to_string()
}

pub fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

pub fn safe_file_stem(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
            output.push(ch);
        } else {
            output.push('_');
        }
    }
    let output = output.trim_matches('.').to_string();
    if output.is_empty() {
        "unnamed".to_string()
    } else {
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_epoch_as_rfc3339() {
        assert_eq!(iso8601_utc(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso8601_utc(1_702_157_046_123), "2023-12-09T21:24:06.123Z");
    }

    #[test]
    fn frame_conversion_rounds_predictably() {
        assert_eq!(ms_to_frame(20.0, 48_000), 960);
        assert!((frame_to_ms(960, 48_000) - 20.0).abs() < f64::EPSILON);
    }

    #[test]
    fn csv_escaping_is_rfc4180_compatible() {
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("a\"b"), "\"a\"\"b\"");
        assert_eq!(csv_field("plain"), "plain");
    }
}
