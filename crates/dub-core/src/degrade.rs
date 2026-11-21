//! Resource probing and the degradation policy.
//!
//! Before every segment we check (a) free disk in the output directory and
//! (b) availability of the GStreamer elements the full-quality path needs.
//! Shortage selects a degraded mode instead of failing the job, and the
//! decision is recorded as evidence.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Processing mode for a segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    /// Opus re-encode of the selected dubbing track.
    Full,
    /// Degraded: uncompressed 8 kHz mono WAV (tiny CPU, no encoder needed).
    LowResource,
}

impl Mode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Mode::Full => "full",
            Mode::LowResource => "low-resource",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResourceReport {
    pub free_disk_bytes: u64,
    pub missing_elements: Vec<String>,
}

/// Elements required by the full-quality path.
pub const FULL_PATH_ELEMENTS: [&str; 6] = [
    "filesrc",
    "matroskademux",
    "opusdec",
    "audioconvert",
    "audioresample",
    "opusenc",
];

/// Free bytes on the filesystem containing `dir` (statvfs).
pub fn free_disk_bytes(dir: &Path) -> anyhow::Result<u64> {
    let c_path = std::ffi::CString::new(dir.as_os_str().as_encoded_bytes())?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) };
    anyhow::ensure!(rc == 0, "statvfs({}) failed", dir.display());
    Ok(stat.f_bavail as u64 * stat.f_frsize as u64)
}

pub fn probe(output_dir: &Path) -> ResourceReport {
    let free = free_disk_bytes(output_dir).unwrap_or(0);
    let missing = FULL_PATH_ELEMENTS
        .iter()
        .filter(|name| gstreamer::ElementFactory::find(name).is_none())
        .map(|s| s.to_string())
        .collect();
    ResourceReport {
        free_disk_bytes: free,
        missing_elements: missing,
    }
}

pub struct Policy {
    /// Below this many free bytes, degrade. Configurable so tests and the
    /// CLI can simulate shortage without filling a disk.
    pub min_free_bytes: u64,
    /// Simulated shortage knobs (CLI flags) — recorded as evidence reasons.
    pub simulate_low_disk: bool,
    pub simulate_missing_encoder: bool,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            min_free_bytes: 64 * 1024 * 1024,
            simulate_low_disk: false,
            simulate_missing_encoder: false,
        }
    }
}

/// Decision of the policy for one segment.
pub struct Decision {
    pub mode: Mode,
    /// `None` on the full path; `Some((reason, detail))` when degraded.
    pub degraded: Option<(String, String)>,
}

impl Policy {
    pub fn decide(&self, report: &ResourceReport) -> Decision {
        if self.simulate_low_disk || report.free_disk_bytes < self.min_free_bytes {
            return Decision {
                mode: Mode::LowResource,
                degraded: Some((
                    "insufficient-disk".into(),
                    format!(
                        "free={} required>={} simulated={}",
                        report.free_disk_bytes, self.min_free_bytes, self.simulate_low_disk
                    ),
                )),
            };
        }
        if self.simulate_missing_encoder || !report.missing_elements.is_empty() {
            return Decision {
                mode: Mode::LowResource,
                degraded: Some((
                    "missing-elements".into(),
                    format!(
                        "missing={:?} simulated={}",
                        report.missing_elements, self.simulate_missing_encoder
                    ),
                )),
            };
        }
        Decision {
            mode: Mode::Full,
            degraded: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortage_selects_degraded_mode() {
        let policy = Policy {
            min_free_bytes: 1_000,
            ..Default::default()
        };
        let rep = ResourceReport {
            free_disk_bytes: 10,
            missing_elements: vec![],
        };
        assert_eq!(policy.decide(&rep).mode, Mode::LowResource);
        let rep = ResourceReport {
            free_disk_bytes: 10_000,
            missing_elements: vec!["opusenc".into()],
        };
        assert_eq!(policy.decide(&rep).mode, Mode::LowResource);
        let rep = ResourceReport {
            free_disk_bytes: 10_000,
            missing_elements: vec![],
        };
        assert_eq!(policy.decide(&rep).mode, Mode::Full);
    }
}
