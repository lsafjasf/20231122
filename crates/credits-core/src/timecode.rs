use crate::{CoreError, Result};

pub const DEFAULT_FPS: f64 = 24.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FramePosition {
    pub frame: u64,
    pub fps_num: u32,
    pub fps_den: u32,
}

impl FramePosition {
    pub fn from_seconds(seconds: f64, fps: f64) -> Result<Self> {
        if !seconds.is_finite() || seconds < 0.0 {
            return Err(CoreError::InvalidTime(seconds.to_string()));
        }
        if !fps.is_finite() || fps <= 0.0 {
            return Err(CoreError::InvalidTime(format!("fps {fps}")));
        }
        let (fps_num, fps_den) = rational_fps(fps);
        let frame = (seconds * fps_num as f64 / fps_den as f64).round() as u64;
        Ok(Self {
            frame,
            fps_num,
            fps_den,
        })
    }

    pub fn fps(self) -> f64 {
        self.fps_num as f64 / self.fps_den as f64
    }

    pub fn seconds(self) -> f64 {
        self.frame as f64 * self.fps_den as f64 / self.fps_num as f64
    }

    pub fn timecode(self) -> String {
        format_frame_timecode(self.frame, self.fps())
    }
}

pub fn rational_fps(fps: f64) -> (u32, u32) {
    if (fps - 23.976).abs() < 0.01 {
        (24000, 1001)
    } else if (fps - 29.97).abs() < 0.01 {
        (30000, 1001)
    } else if (fps - 59.94).abs() < 0.01 {
        (60000, 1001)
    } else {
        (fps.round().max(1.0) as u32, 1)
    }
}

pub fn format_clock_time(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return "--:--:--.---".to_string();
    }
    let total_ms = (seconds * 1000.0).round() as u64;
    let ms = total_ms % 1000;
    let total_seconds = total_ms / 1000;
    let s = total_seconds % 60;
    let m = (total_seconds / 60) % 60;
    let h = total_seconds / 3600;
    format!("{h:02}:{m:02}:{s:02}.{ms:03}")
}

pub fn format_frame_timecode(frame: u64, fps: f64) -> String {
    let nominal = fps.round().max(1.0) as u64;
    let total_seconds = frame / nominal;
    let frames = frame % nominal;
    let s = total_seconds % 60;
    let m = (total_seconds / 60) % 60;
    let h = total_seconds / 3600;
    format!("{h:02}:{m:02}:{s:02}:{frames:02}")
}

pub fn parse_time(value: &serde_json::Value) -> Result<f64> {
    match value {
        serde_json::Value::Number(n) => n
            .as_f64()
            .filter(|v| v.is_finite() && *v >= 0.0)
            .ok_or_else(|| CoreError::InvalidTime(n.to_string())),
        serde_json::Value::String(raw) => parse_time_str(raw),
        other => Err(CoreError::InvalidTime(other.to_string())),
    }
}

pub fn parse_time_str(raw: &str) -> Result<f64> {
    let value = raw.trim();
    if let Some(frames) = value.strip_prefix("frame:") {
        let frame = frames
            .trim()
            .parse::<f64>()
            .map_err(|_| CoreError::InvalidTime(raw.to_string()))?;
        return Ok(frame / DEFAULT_FPS);
    }
    if !value.contains(':') {
        return value
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite() && *v >= 0.0)
            .ok_or_else(|| CoreError::InvalidTime(raw.to_string()));
    }

    let parts: Vec<&str> = value.split(':').collect();
    if !(2..=3).contains(&parts.len()) {
        return Err(CoreError::InvalidTime(raw.to_string()));
    }
    let (hours, minutes, seconds) = match parts.as_slice() {
        [m, s] => (0.0, parse_part(m)?, parse_part(s)?),
        [h, m, s] => (parse_part(h)?, parse_part(m)?, parse_part(s)?),
        _ => unreachable!(),
    };
    if minutes >= 60.0 || seconds >= 60.0 {
        return Err(CoreError::InvalidTime(raw.to_string()));
    }
    Ok(hours * 3600.0 + minutes * 60.0 + seconds)
}

fn parse_part(value: &str) -> Result<f64> {
    value
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && *v >= 0.0)
        .ok_or_else(|| CoreError::InvalidTime(value.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_clock_and_frames() {
        assert_eq!(parse_time_str("00:01:02.500").unwrap(), 62.5);
        assert_eq!(parse_time_str("frame:48").unwrap(), 2.0);
        assert_eq!(format_clock_time(62.5), "00:01:02.500");
    }

    #[test]
    fn maps_frames_and_timecode() {
        let position = FramePosition::from_seconds(2.0, 24.0).unwrap();
        assert_eq!(position.frame, 48);
        assert_eq!(position.timecode(), "00:00:02:00");
    }

    #[test]
    fn rejects_out_of_range_parts() {
        assert!(parse_time_str("00:60:00").is_err());
        assert!(parse_time_str("-1").is_err());
    }
}
