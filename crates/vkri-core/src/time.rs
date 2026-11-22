//! Time model.
//!
//! All absolute time points are stored as Unix time in nanoseconds (`i64`).
//! Each media segment carries the device wall-clock reading at presentation
//! timestamp zero (`clock_origin_unix_ns`). A frame's device wall-clock time is
//! therefore `clock_origin_unix_ns + pts_ns`, independent of container
//! timestamps. Device clock offsets correct the two unsynchronised clocks onto
//! a shared reference timeline.

use serde::{Deserialize, Serialize};

/// A nanosecond-precision Unix timestamp.
pub type UnixNanos = i64;

/// A presentation timestamp relative to segment start, in nanoseconds.
pub type PtsNanos = i64;

/// Offset applied to a device clock to map it onto the reference timeline:
/// `reference_time = device_time + offset_ns`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClockOffset {
    pub offset_ns: i64,
    pub uncertainty_ns: i64,
}

impl ClockOffset {
    pub const fn zero() -> Self {
        Self {
            offset_ns: 0,
            uncertainty_ns: 0,
        }
    }

    /// Map a device wall-clock timestamp onto the reference timeline.
    pub fn apply(&self, device_time_ns: UnixNanos) -> UnixNanos {
        device_time_ns + self.offset_ns
    }
}

/// Convert a frame PTS to the device wall-clock time of that frame.
pub fn pts_to_device_time(clock_origin_unix_ns: UnixNanos, pts_ns: PtsNanos) -> UnixNanos {
    clock_origin_unix_ns + pts_ns
}

/// Convert a frame PTS to the corrected reference-timeline time.
pub fn pts_to_reference_time(
    clock_origin_unix_ns: UnixNanos,
    pts_ns: PtsNanos,
    offset: ClockOffset,
) -> UnixNanos {
    offset.apply(pts_to_device_time(clock_origin_unix_ns, pts_ns))
}

/// Human-readable UTC rendering (`YYYY-MM-DDTHH:MM:SS.nnnnnnnnnZ`).
pub fn format_unix_nanos(value: UnixNanos) -> String {
    use chrono::{DateTime, Utc};
    let secs = value.div_euclid(1_000_000_000);
    let nanos = value.rem_euclid(1_000_000_000) as u32;
    let utc: DateTime<Utc> = DateTime::from_timestamp(secs, nanos)
        .unwrap_or_else(|| DateTime::from_timestamp(0, 0).unwrap());
    utc.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)
}

/// Parse the output of [`format_unix_nanos`] back into nanoseconds.
pub fn parse_unix_nanos(text: &str) -> Option<UnixNanos> {
    use chrono::{DateTime, Utc};
    let parsed: DateTime<Utc> = DateTime::parse_from_rfc3339(text).ok()?.with_timezone(&Utc);
    Some(parsed.timestamp() * 1_000_000_000 + parsed.timestamp_subsec_nanos() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offset_round_trip() {
        let origin = 1_700_000_000_000_000_000;
        let pts = 2_500_000_000;
        let offset = ClockOffset {
            offset_ns: -150_000_000,
            uncertainty_ns: 10_000_000,
        };
        let reference = pts_to_reference_time(origin, pts, offset);
        assert_eq!(reference, origin + pts - 150_000_000);
    }

    #[test]
    fn timestamp_format_round_trip() {
        let value = 1_700_000_000_123_456_789;
        let text = format_unix_nanos(value);
        assert_eq!(parse_unix_nanos(&text), Some(value));
    }
}
