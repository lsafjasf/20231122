use crate::model::{CameraProfile, METADATA_MAGIC, METADATA_RECORD_BYTES, MasteringDisplay};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const FLAG_CAMERA_FAULT: u16 = 1 << 0;
pub const FLAG_REPAIRED: u16 = 1 << 1;
pub const FLAG_REPEAT_OBJECT: u16 = 1 << 2;
pub const FLAG_RESOURCE_DEGRADED: u16 = 1 << 3;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FrameMetadata {
    pub frame: u32,
    pub pts_ns: u64,
    pub source_offset: u64,
    pub max_cll: u16,
    pub max_fall: u16,
    pub max_nits: u16,
    pub avg_nits: u16,
    pub white_point: [u16; 2],
    pub mastering_max_nits: u16,
    pub mastering_min_nits: u16,
    pub flags: u16,
}

impl FrameMetadata {
    pub fn camera_fault(
        frame: u32,
        pts_ns: u64,
        source_offset: u64,
        profile: &CameraProfile,
    ) -> Self {
        Self {
            frame,
            pts_ns,
            source_offset,
            max_cll: profile.fault_profile.max_cll,
            max_fall: profile.fault_profile.max_fall,
            max_nits: profile.fault_profile.peak_nits,
            avg_nits: profile.fault_profile.avg_nits,
            white_point: profile.fault_profile.white_point,
            mastering_max_nits: profile.fault_profile.mastering_max_nits,
            mastering_min_nits: profile.fault_profile.mastering_min_nits,
            flags: FLAG_CAMERA_FAULT,
        }
    }

    pub fn repair(
        &self,
        peak_nits: u16,
        avg_nits: u16,
        object_present: bool,
        degraded: bool,
        mastering: &MasteringDisplay,
    ) -> Self {
        let measured_peak = peak_nits.max(1).min(mastering.max_nits);
        let measured_avg = avg_nits.min(measured_peak);
        let mut flags = FLAG_REPAIRED;
        if object_present {
            flags |= FLAG_REPEAT_OBJECT;
        }
        if degraded {
            flags |= FLAG_RESOURCE_DEGRADED;
        }
        Self {
            frame: self.frame,
            pts_ns: self.pts_ns,
            source_offset: self.source_offset,
            max_cll: measured_peak,
            max_fall: measured_avg,
            max_nits: measured_peak,
            avg_nits: measured_avg,
            white_point: mastering.white_point,
            mastering_max_nits: mastering.max_nits,
            mastering_min_nits: mastering.min_nits,
            flags,
        }
    }

    pub fn encode(&self) -> [u8; METADATA_RECORD_BYTES] {
        let mut bytes = [0u8; METADATA_RECORD_BYTES];
        bytes[0..8].copy_from_slice(METADATA_MAGIC);
        bytes[8..12].copy_from_slice(&self.frame.to_le_bytes());
        bytes[12..14].copy_from_slice(&self.max_cll.to_le_bytes());
        bytes[14..16].copy_from_slice(&self.max_fall.to_le_bytes());
        bytes[16..18].copy_from_slice(&self.max_nits.to_le_bytes());
        bytes[18..20].copy_from_slice(&self.avg_nits.to_le_bytes());
        bytes[20..22].copy_from_slice(&self.white_point[0].to_le_bytes());
        bytes[22..24].copy_from_slice(&self.white_point[1].to_le_bytes());
        bytes[24..26].copy_from_slice(&self.mastering_max_nits.to_le_bytes());
        bytes[26..28].copy_from_slice(&self.mastering_min_nits.to_le_bytes());
        bytes[28..30].copy_from_slice(&self.flags.to_le_bytes());
        bytes[32..40].copy_from_slice(&self.pts_ns.to_le_bytes());
        bytes[40..48].copy_from_slice(&self.source_offset.to_le_bytes());
        let crc = crc32fast::hash(&bytes[..60]);
        bytes[60..64].copy_from_slice(&crc.to_le_bytes());
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, MetadataError> {
        if bytes.len() != METADATA_RECORD_BYTES {
            return Err(MetadataError::Length(bytes.len()));
        }
        if &bytes[0..8] != METADATA_MAGIC {
            return Err(MetadataError::Magic);
        }
        let expected_crc = u32::from_le_bytes(bytes[60..64].try_into().expect("fixed slice"));
        let actual_crc = crc32fast::hash(&bytes[..60]);
        if expected_crc != actual_crc {
            return Err(MetadataError::Checksum {
                expected: expected_crc,
                actual: actual_crc,
            });
        }
        Ok(Self {
            frame: read_u32(bytes, 8),
            max_cll: read_u16(bytes, 12),
            max_fall: read_u16(bytes, 14),
            max_nits: read_u16(bytes, 16),
            avg_nits: read_u16(bytes, 18),
            white_point: [read_u16(bytes, 20), read_u16(bytes, 22)],
            mastering_max_nits: read_u16(bytes, 24),
            mastering_min_nits: read_u16(bytes, 26),
            flags: read_u16(bytes, 28),
            pts_ns: read_u64(bytes, 32),
            source_offset: read_u64(bytes, 40),
        })
    }

    pub fn is_camera_fault(&self) -> bool {
        self.flags & FLAG_CAMERA_FAULT != 0
    }

    pub fn is_repaired(&self) -> bool {
        self.flags & FLAG_REPAIRED != 0
    }

    pub fn has_repeated_object(&self) -> bool {
        self.flags & FLAG_REPEAT_OBJECT != 0
    }
}

#[derive(Debug, Error)]
pub enum MetadataError {
    #[error("metadata record must be {METADATA_RECORD_BYTES} bytes, got {0}")]
    Length(usize),
    #[error("metadata magic does not match")]
    Magic,
    #[error("metadata checksum mismatch: expected {expected:08x}, got {actual:08x}")]
    Checksum { expected: u32, actual: u32 },
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().expect("fixed slice"))
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("fixed slice"))
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().expect("fixed slice"))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FrameAnalysis {
    pub peak_nits: u16,
    pub avg_nits: u16,
    pub object_present: bool,
    pub sampled_pixels: u64,
    pub total_pixels: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::demo_profile;

    #[test]
    fn metadata_round_trip_preserves_checksum() {
        let profile = demo_profile();
        let original = FrameMetadata::camera_fault(72, 2_400_000_000, 1_327_104, &profile);
        let bytes = original.encode();
        assert_eq!(FrameMetadata::decode(&bytes).unwrap(), original);
        let mut broken = bytes;
        broken[12] ^= 0xff;
        assert!(FrameMetadata::decode(&broken).is_err());
    }

    #[test]
    fn repair_replaces_extreme_metadata_but_keeps_identity() {
        let profile = demo_profile();
        let original = FrameMetadata::camera_fault(72, 2_400_000_000, 1_327_104, &profile);
        let repaired = original.repair(10_000, 2_500, true, false, &profile.repaired_mastering);
        assert_eq!(repaired.frame, 72);
        assert_eq!(repaired.pts_ns, 2_400_000_000);
        assert_eq!(repaired.source_offset, 1_327_104);
        assert_eq!(repaired.max_cll, 10_000);
        assert_eq!(repaired.max_fall, 2_500);
        assert!(repaired.is_repaired());
        assert!(repaired.has_repeated_object());
        assert!(!repaired.is_camera_fault());
    }
}
