use crate::io::probe_wav;
use crate::model::{
    CalibrationConfig, DEGRADED_RATE, DegradationRecord, ResourceSnapshot, SourceInfo, now_rfc3339,
};
use anyhow::{Context, Result, bail, ensure};
use gstreamer as gst;
use std::fs;
use std::path::Path;

const REQUIRED_FACTORIES: &[&str] = &[
    "filesrc",
    "wavparse",
    "audioconvert",
    "audioresample",
    "capsfilter",
    "appsink",
    "fakesink",
    "volume",
    "wavenc",
    "filesink",
    "concat",
];

fn mem_available_mb() -> u64 {
    let Ok(text) = fs::read_to_string("/proc/meminfo") else {
        return 0;
    };
    text.lines()
        .find_map(|line| {
            let mut fields = line.split_whitespace();
            if fields.next()? != "MemAvailable:" {
                return None;
            }
            fields.next()?.parse::<u64>().ok().map(|kb| kb / 1024)
        })
        .unwrap_or(0)
}

fn available_disk_mb(path: &Path) -> u64 {
    let mut probe = path;
    while !probe.exists() {
        let Some(parent) = probe.parent() else {
            return 0;
        };
        probe = parent;
    }
    fs2::available_space(probe)
        .map(|bytes| bytes / (1024 * 1024))
        .unwrap_or(0)
}

pub fn ensure_gstreamer() -> Result<()> {
    gst::init().context("GStreamer 初始化失败")?;
    let missing: Vec<&str> = REQUIRED_FACTORIES
        .iter()
        .copied()
        .filter(|name| gst::ElementFactory::find(name).is_none())
        .collect();
    ensure!(
        missing.is_empty(),
        "缺少 GStreamer 元素: {}。请安装 gstreamer1.0-plugins-base/good。",
        missing.join(", ")
    );
    Ok(())
}

pub fn probe(
    source: &SourceInfo,
    config: &CalibrationConfig,
    run_dir: &Path,
) -> Result<ResourceSnapshot> {
    let wav = probe_wav(Path::new(&source.path))?;
    let memory_available_mb = mem_available_mb();
    let disk_available_mb = available_disk_mb(run_dir);
    let cpu_count = std::thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(1);
    let segment_bytes =
        wav.data_size as f64 / 3.0 + (wav.sample_rate as f64 * 4.0 * 2.0).max(65536.0) * 3.0;
    let memory_required_mb = ((segment_bytes / (1024.0 * 1024.0)).ceil() as u64).max(64);
    let disk_required_mb =
        (((wav.data_size as f64 * 7.0) / (1024.0 * 1024.0)).ceil() as u64).max(64);

    let mut reasons = Vec::new();
    if memory_available_mb > 0
        && memory_available_mb
            < config
                .memory_floor_mb
                .max(memory_required_mb.saturating_mul(2))
    {
        reasons.push(format!(
            "available_memory={}MB below floor={}MB",
            memory_available_mb,
            config
                .memory_floor_mb
                .max(memory_required_mb.saturating_mul(2))
        ));
    }
    if disk_available_mb > 0
        && disk_available_mb < config.disk_floor_mb.max(disk_required_mb.saturating_mul(2))
    {
        reasons.push(format!(
            "available_disk={}MB below floor={}MB",
            disk_available_mb,
            config.disk_floor_mb.max(disk_required_mb.saturating_mul(2))
        ));
    }
    if cpu_count < 2 {
        reasons.push(format!("available_cpu={cpu_count} below 2"));
    }

    let env_force = std::env::var("LOUDNESS_FORCE_DEGRADE")
        .map(|value| matches!(value.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false);
    let forced = config.force_degrade || env_force;
    if forced {
        reasons.push("degradation_forced_by_operator".to_string());
    }
    let mode = if reasons.is_empty() {
        "full"
    } else {
        "degraded"
    };
    Ok(ResourceSnapshot {
        captured_at: now_rfc3339(),
        memory_available_mb,
        disk_available_mb,
        cpu_count,
        memory_required_mb,
        disk_required_mb,
        mode: mode.to_string(),
        reasons,
        forced,
    })
}

pub fn degradation_for(snapshot: &ResourceSnapshot, source_rate: u32) -> Option<DegradationRecord> {
    if snapshot.mode != "degraded" {
        return None;
    }
    let output_sample_rate = if source_rate > DEGRADED_RATE {
        DEGRADED_RATE
    } else {
        source_rate
    };
    Some(DegradationRecord {
        entered_at: now_rfc3339(),
        mode: "degraded".to_string(),
        reasons: snapshot.reasons.clone(),
        strategy: "streaming_half_rate; no retained PCM; 512-byte anchor probes".to_string(),
        output_sample_rate,
        anchor_probe_bytes: 512,
    })
}

pub fn preflight_resource(snapshot: &ResourceSnapshot) -> Result<()> {
    if snapshot.memory_available_mb > 0
        && snapshot.memory_available_mb < snapshot.memory_required_mb / 2
    {
        bail!(
            "可用内存 {}MB 低于安全下限 {}MB，拒绝启动以避免 OOM；请释放资源后恢复",
            snapshot.memory_available_mb,
            snapshot.memory_required_mb / 2
        );
    }
    if snapshot.disk_available_mb > 0 && snapshot.disk_available_mb < snapshot.disk_required_mb {
        bail!(
            "可用磁盘 {}MB 低于输出下限 {}MB；断点与证据已保留",
            snapshot.disk_available_mb,
            snapshot.disk_required_mb
        );
    }
    Ok(())
}
