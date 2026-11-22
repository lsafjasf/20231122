pub const NANOS_PER_MILLI: u64 = 1_000_000;
pub const NANOS_PER_SECOND: u64 = 1_000_000_000;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TimecodeError {
    #[error("时间码为空")]
    Empty,
    #[error("时间码格式应为 HH:MM:SS.mmm、MM:SS.mmm 或秒数")]
    InvalidFormat,
    #[error("时间码包含无效数字")]
    InvalidNumber,
    #[error("时间码不能为负数")]
    Negative,
}

pub fn format_timecode(ns: u64) -> String {
    let total_ms = ns / NANOS_PER_MILLI;
    let millis = total_ms % 1_000;
    let total_seconds = total_ms / 1_000;
    let seconds = total_seconds % 60;
    let total_minutes = total_seconds / 60;
    let minutes = total_minutes % 60;
    let hours = total_minutes / 60;
    format!("{hours:02}:{minutes:02}:{seconds:02}.{millis:03}")
}

pub fn format_signed_offset(ns: i64) -> String {
    let sign = if ns < 0 { '-' } else { '+' };
    format!("{sign}{}", format_timecode(ns.unsigned_abs()))
}

pub fn parse_timecode(input: &str) -> Result<u64, TimecodeError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(TimecodeError::Empty);
    }
    if input.starts_with('-') {
        return Err(TimecodeError::Negative);
    }
    parse_unsigned_parts(input)
}

pub fn parse_signed_offset(input: &str) -> Result<i64, TimecodeError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(TimecodeError::Empty);
    }
    let (negative, body) = match input.as_bytes()[0] {
        b'-' => (true, &input[1..]),
        b'+' => (false, &input[1..]),
        _ => (false, input),
    };
    if body.is_empty() {
        return Err(TimecodeError::InvalidFormat);
    }
    let value = parse_unsigned_parts(body)?;
    if value > i64::MAX as u64 {
        return Err(TimecodeError::InvalidNumber);
    }
    Ok(if negative {
        -(value as i64)
    } else {
        value as i64
    })
}

fn parse_unsigned_parts(input: &str) -> Result<u64, TimecodeError> {
    let parts: Vec<&str> = input.split(':').collect();
    if parts.len() > 3 || parts.iter().any(|part| part.is_empty()) {
        return Err(TimecodeError::InvalidFormat);
    }

    let seconds_part = parts.last().copied().ok_or(TimecodeError::InvalidFormat)?;
    let (whole_seconds, fractional_ns) = parse_seconds(seconds_part)?;
    let mut total_seconds = whole_seconds;

    if parts.len() >= 2 {
        let minutes: u64 = parts[parts.len() - 2]
            .parse()
            .map_err(|_| TimecodeError::InvalidNumber)?;
        total_seconds = total_seconds
            .checked_add(
                minutes
                    .checked_mul(60)
                    .ok_or(TimecodeError::InvalidNumber)?,
            )
            .ok_or(TimecodeError::InvalidNumber)?;
    }
    if parts.len() == 3 {
        let hours: u64 = parts[0].parse().map_err(|_| TimecodeError::InvalidNumber)?;
        total_seconds = total_seconds
            .checked_add(
                hours
                    .checked_mul(3_600)
                    .ok_or(TimecodeError::InvalidNumber)?,
            )
            .ok_or(TimecodeError::InvalidNumber)?;
    }

    total_seconds
        .checked_mul(NANOS_PER_SECOND)
        .and_then(|value| value.checked_add(fractional_ns))
        .ok_or(TimecodeError::InvalidNumber)
}

fn parse_seconds(input: &str) -> Result<(u64, u64), TimecodeError> {
    let mut pieces = input.splitn(2, '.');
    let whole: u64 = pieces
        .next()
        .ok_or(TimecodeError::InvalidFormat)?
        .parse()
        .map_err(|_| TimecodeError::InvalidNumber)?;
    let fraction = pieces.next().unwrap_or("");
    if fraction.len() > 9 || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(TimecodeError::InvalidNumber);
    }
    let fractional_ns = if fraction.is_empty() {
        0
    } else {
        let value: u64 = fraction.parse().map_err(|_| TimecodeError::InvalidNumber)?;
        value * 10_u64.pow((9 - fraction.len()) as u32)
    };
    Ok((whole, fractional_ns))
}

pub fn millis_to_nanos(millis: i64) -> i64 {
    millis.saturating_mul(NANOS_PER_MILLI as i64)
}

pub fn nanos_to_millis(ns: i64) -> i64 {
    ns / NANOS_PER_MILLI as i64
}

pub fn frame_duration_ns(fps_num: u32, fps_den: u32) -> Option<u64> {
    if fps_num == 0 || fps_den == 0 {
        return None;
    }
    Some((NANOS_PER_SECOND as u128 * fps_den as u128 / fps_num as u128) as u64)
}

pub fn frame_number(ns: u64, fps_num: u32, fps_den: u32) -> Option<u64> {
    if fps_num == 0 || fps_den == 0 {
        return None;
    }
    Some((ns as u128 * fps_num as u128 / (NANOS_PER_SECOND as u128 * fps_den as u128)) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_and_parses_timecode() {
        assert_eq!(format_timecode(61_250_000_000), "00:01:01.250");
        assert_eq!(parse_timecode("00:01:01.250").unwrap(), 61_250_000_000);
        assert_eq!(parse_timecode("1:01.25").unwrap(), 61_250_000_000);
        assert_eq!(parse_timecode("12.5").unwrap(), 12_500_000_000);
    }

    #[test]
    fn formats_signed_offsets() {
        assert_eq!(format_signed_offset(-750_000_000), "-00:00:00.750");
        assert_eq!(format_signed_offset(1_500_000_000), "+00:00:01.500");
        assert_eq!(parse_signed_offset("-00:00:00.750").unwrap(), -750_000_000);
    }

    #[test]
    fn rejects_invalid_input() {
        assert_eq!(parse_timecode(""), Err(TimecodeError::Empty));
        assert_eq!(parse_timecode("-1.0"), Err(TimecodeError::Negative));
        assert!(parse_timecode("aa:bb").is_err());
        assert!(parse_timecode("1:2:3:4").is_err());
    }
}
