use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::state::Degradation;

pub const MIN_FREE_MEM_MB: u64 = 512;
pub const MIN_FREE_DISK_MB: u64 = 256;
pub const XF_NORMAL: u64 = 4096;
pub const XF_DEGRADED: u64 = 1024;
pub const CHUNK_NORMAL: u64 = 1 << 20; // analysis batch, samples
pub const CHUNK_DEGRADED: u64 = 1 << 16;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ResourceProbe {
    pub mem_available_mb: u64,
    pub disk_available_mb: u64,
}

/// Optional test override to deterministically exercise the degraded path.
#[derive(Debug, Clone, Copy, Default)]
pub struct SimulatedLimits {
    pub mem_mb: Option<u64>,
    pub disk_mb: Option<u64>,
}

pub fn probe(work_dir: &Path, sim: &SimulatedLimits) -> ResourceProbe {
    let mem_available_mb = sim.mem_mb.unwrap_or_else(read_mem_available_mb);
    let disk_available_mb = sim
        .disk_mb
        .unwrap_or_else(|| read_disk_available_mb(work_dir));
    ResourceProbe { mem_available_mb, disk_available_mb }
}

fn read_mem_available_mb() -> u64 {
    std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|t| {
            t.lines()
                .find(|l| l.starts_with("MemAvailable:"))
                .and_then(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok())
        })
        .map(|kb| kb / 1024)
        .unwrap_or(u64::MAX) // unknown -> assume plenty
}

fn read_disk_available_mb(path: &Path) -> u64 {
    let c = std::ffi::CString::new(path.to_string_lossy().as_bytes()).unwrap();
    unsafe {
        let mut st: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(c.as_ptr(), &mut st) == 0 {
            (st.f_bavail as u64 * st.f_frsize as u64) / (1024 * 1024)
        } else {
            u64::MAX
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ResourceVerdict {
    Normal,
    Degraded,
    /// Hard stop: not enough resources even for the degraded path.
    Insufficient,
}

pub fn evaluate(p: &ResourceProbe, needed_disk_mb: u64) -> (ResourceVerdict, Vec<String>) {
    let mut reasons = Vec::new();
    if p.mem_available_mb < MIN_FREE_MEM_MB / 4 {
        reasons.push(format!(
            "mem_available {}MB < hard floor {}MB",
            p.mem_available_mb,
            MIN_FREE_MEM_MB / 4
        ));
        return (ResourceVerdict::Insufficient, reasons);
    }
    if p.disk_available_mb < needed_disk_mb / 4 + 16 {
        reasons.push(format!(
            "disk_available {}MB below hard floor for estimated need {}MB",
            p.disk_available_mb, needed_disk_mb
        ));
        return (ResourceVerdict::Insufficient, reasons);
    }
    if p.mem_available_mb < MIN_FREE_MEM_MB {
        reasons.push(format!(
            "mem_available {}MB < {}MB -> chunked decode, coarse analysis, short crossfade",
            p.mem_available_mb, MIN_FREE_MEM_MB
        ));
    }
    if p.disk_available_mb < needed_disk_mb {
        reasons.push(format!(
            "disk_available {}MB < estimated need {}MB -> minimal artifacts",
            p.disk_available_mb, needed_disk_mb
        ));
    }
    if reasons.is_empty() {
        (ResourceVerdict::Normal, reasons)
    } else {
        (ResourceVerdict::Degraded, reasons)
    }
}

pub fn degradation(p: &ResourceProbe, verdict: ResourceVerdict, reasons: Vec<String>) -> Degradation {
    let degraded = verdict == ResourceVerdict::Degraded;
    Degradation {
        mode: if degraded { "degraded".into() } else { "normal".into() },
        reasons,
        mem_available_mb: p.mem_available_mb,
        disk_available_mb: p.disk_available_mb,
        crossfade_samples: if degraded { XF_DEGRADED } else { XF_NORMAL },
        analysis_stride: if degraded { 4 } else { 1 },
        chunk_samples: if degraded { CHUNK_DEGRADED } else { CHUNK_NORMAL },
    }
}
