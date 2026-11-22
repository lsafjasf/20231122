use std::path::Path;

use anyhow::{Context, Result};

use crate::model::{Config, SourceFrame, FRAME_BYTES, HEIGHT, WIDTH};

const JITTER_SALT: u64 = 0x9e37_79b9_7f4a_7c15;

fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

pub fn jitter_ns(frame_idx: u64, amplitude_ns: u64) -> i64 {
    if amplitude_ns == 0 {
        return 0;
    }
    let span = amplitude_ns.saturating_mul(2).saturating_add(1);
    let value = splitmix64((frame_idx + 1).wrapping_mul(JITTER_SALT)) % span;
    value as i64 - amplitude_ns as i64
}

pub fn source_frame(config: &Config, frame_idx: u64) -> Option<SourceFrame> {
    if frame_idx >= config.frame_count || config.is_missing(frame_idx) {
        return None;
    }
    let nominal_pts_ns = config.nominal_pts_ns(frame_idx);
    let in_extreme_window = config.in_extreme_window(frame_idx);
    let jitter_ns = if in_extreme_window {
        jitter_ns(frame_idx, config.extreme.jitter_ns)
    } else {
        0
    };
    let pts_ns = nominal_pts_ns.saturating_add_signed(jitter_ns);
    Some(SourceFrame {
        idx: frame_idx,
        nominal_pts_ns,
        pts_ns,
        jitter_ns,
        in_extreme_window,
    })
}

/// Stable synthetic RGB frame used as playout content. The pattern is easy to
/// inspect visually and changes with the frame index, so boundary PPMs are
/// useful evidence rather than opaque hashes.
pub fn frame_rgb(frame_idx: u64) -> Vec<u8> {
    let mut bytes = vec![0_u8; FRAME_BYTES];
    let mut offset = 0;
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            bytes[offset] = (frame_idx as u32).wrapping_add(x) as u8;
            bytes[offset + 1] = (2 * y) as u8;
            bytes[offset + 2] = (x as u8) ^ (y as u8);
            offset += 3;
        }
    }
    bytes
}

const fn make_crc_table() -> [u32; 256] {
    let mut table = [0_u32; 256];
    let mut index = 0;
    while index < 256 {
        let mut value = index as u32;
        let mut bit = 0;
        while bit < 8 {
            value = if value & 1 != 0 {
                0xedb8_8320 ^ (value >> 1)
            } else {
                value >> 1
            };
            bit += 1;
        }
        table[index] = value;
        index += 1;
    }
    table
}

const CRC_TABLE: [u32; 256] = make_crc_table();

pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in bytes {
        let index = ((crc ^ u32::from(*byte)) & 0xff) as usize;
        crc = CRC_TABLE[index] ^ (crc >> 8);
    }
    !crc
}

pub fn frame_crc(frame_idx: u64) -> u32 {
    crc32(&frame_rgb(frame_idx))
}

pub fn write_ppm(path: &Path, frame_idx: u64) -> Result<()> {
    let rgb = frame_rgb(frame_idx);
    let mut ppm = Vec::with_capacity(rgb.len() + 32);
    ppm.extend_from_slice(format!("P6\n{WIDTH} {HEIGHT}\n255\n").as_bytes());
    ppm.extend_from_slice(&rgb);
    std::fs::write(path, ppm).with_context(|| format!("write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jitter_is_bounded_and_deterministic() {
        let config = Config::default();
        for idx in 90..196 {
            if let Some(frame) = source_frame(&config, idx) {
                assert!(frame.jitter_ns.abs() <= config.extreme.jitter_ns as i64);
                assert_eq!(frame, source_frame(&config, idx).unwrap());
            }
        }
    }

    #[test]
    fn missing_span_is_skipped() {
        let config = Config::default();
        assert!(source_frame(&config, 119).is_some());
        assert!(source_frame(&config, 120).is_some());
        assert!(source_frame(&config, 121).is_none());
        assert!(source_frame(&config, 165).is_none());
        assert!(source_frame(&config, 166).is_some());
    }
}
