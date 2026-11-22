//! Resource observation and degradation policy.
//!
//! Readings come from `/proc/self/status` (VmRSS) and `statvfs` on the output
//! directory, so the same logic works on any Linux host without extra
//! privileges. Decisions are pure functions of the readings plus the case's
//! resource policy, which keeps them reproducible and testable.

use crate::model::ResourcePolicy;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceReading {
    pub rss_mb: u64,
    pub free_disk_mb: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceVerdict {
    Continue,
    Degrade,
    Pause,
}

pub fn observe(output_dir: &Path) -> ResourceReading {
    ResourceReading {
        rss_mb: read_rss_mb(),
        free_disk_mb: read_free_disk_mb(output_dir),
    }
}

pub fn decide(reading: ResourceReading, policy: &ResourcePolicy, already_reduced: bool) -> ResourceVerdict {
    if reading.free_disk_mb < policy.min_free_disk_mb || reading.rss_mb >= policy.pause_rss_mb {
        return ResourceVerdict::Pause;
    }
    if !already_reduced && reading.rss_mb >= policy.degrade_rss_mb {
        return ResourceVerdict::Degrade;
    }
    ResourceVerdict::Continue
}

fn read_rss_mb() -> u64 {
    let Ok(status) = fs::read_to_string("/proc/self/status") else {
        return 0;
    };
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            let kb: u64 = rest
                .split_whitespace()
                .next()
                .and_then(|value| value.parse().ok())
                .unwrap_or(0);
            return kb / 1024;
        }
    }
    0
}

fn read_free_disk_mb(path: &Path) -> u64 {
    let probe = if path.exists() {
        path.to_path_buf()
    } else {
        Path::new("/").to_path_buf()
    };
    match statvfs_free_bytes(&probe) {
        Some(bytes) => bytes / (1024 * 1024),
        None => u64::MAX / (1024 * 1024),
    }
}

#[cfg(target_os = "linux")]
fn statvfs_free_bytes(path: &Path) -> Option<u64> {
    use std::ffi::CString;
    use std::mem::MaybeUninit;
    #[repr(C)]
    struct Statvfs {
        f_bsize: u64,
        f_frsize: u64,
        f_blocks: u64,
        f_bfree: u64,
        f_bavail: u64,
        f_files: u64,
        f_ffree: u64,
        f_favail: u64,
        f_fsid: u64,
        f_flag: u64,
        f_namemax: u64,
        f_spare: [u64; 6],
    }
    extern "C" {
        fn statvfs(path: *const std::os::raw::c_char, buf: *mut Statvfs) -> i32;
    }
    let c_path = CString::new(path.to_str()?).ok()?;
    let mut buf = MaybeUninit::<Statvfs>::uninit();
    let rc = unsafe { statvfs(c_path.as_ptr(), buf.as_mut_ptr()) };
    if rc != 0 {
        return None;
    }
    let stat = unsafe { buf.assume_init() };
    Some(stat.f_bavail.saturating_mul(stat.f_frsize))
}

#[cfg(not(target_os = "linux"))]
fn statvfs_free_bytes(_path: &Path) -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> ResourcePolicy {
        ResourcePolicy {
            min_free_disk_mb: 32,
            degrade_rss_mb: 512,
            pause_rss_mb: 1536,
            reduced_sample_rate: 4000,
            reduced_frame_us: 160_000,
            allow_wav_fallback: true,
            fallback_to_file_window: true,
        }
    }

    #[test]
    fn policy_boundaries() {
        let policy = policy();
        assert_eq!(
            decide(ResourceReading { rss_mb: 100, free_disk_mb: 10_000 }, &policy, false),
            ResourceVerdict::Continue
        );
        assert_eq!(
            decide(ResourceReading { rss_mb: 600, free_disk_mb: 10_000 }, &policy, false),
            ResourceVerdict::Degrade
        );
        assert_eq!(
            decide(ResourceReading { rss_mb: 600, free_disk_mb: 10_000 }, &policy, true),
            ResourceVerdict::Continue
        );
        assert_eq!(
            decide(ResourceReading { rss_mb: 2000, free_disk_mb: 10_000 }, &policy, false),
            ResourceVerdict::Pause
        );
        assert_eq!(
            decide(ResourceReading { rss_mb: 100, free_disk_mb: 10 }, &policy, false),
            ResourceVerdict::Pause
        );
    }
}
