use std::f32::consts::TAU;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::model::{
    AudioVersion, DeviceClock, Processor, RepairInput, RepairInterval, SCHEMA_VERSION, SourcePoint,
    Track, VersionManifest,
};

#[derive(Debug, Clone)]
pub struct DemoPaths {
    pub recording: PathBuf,
    pub intervals: PathBuf,
    pub versions: PathBuf,
}

pub fn write_demo_inputs(directory: &Path) -> Result<DemoPaths> {
    std::fs::create_dir_all(directory)
        .with_context(|| format!("cannot create demo directory {}", directory.display()))?;
    let recording = directory.join("original.wav");
    let intervals = directory.join("intervals.json");
    let versions = directory.join("versions.json");
    write_wav(&recording)?;
    std::fs::write(&intervals, serde_json::to_vec_pretty(&demo_intervals())?)?;
    std::fs::write(&versions, serde_json::to_vec_pretty(&demo_versions())?)?;
    Ok(DemoPaths {
        recording,
        intervals,
        versions,
    })
}

pub fn write_empty_inputs(directory: &Path) -> Result<DemoPaths> {
    std::fs::create_dir_all(directory)?;
    let recording = directory.join("original.wav");
    let intervals_path = directory.join("intervals.json");
    let versions_path = directory.join("versions.json");
    write_wav(&recording)?;
    let mut intervals = demo_intervals();
    intervals.intervals.clear();
    let versions = VersionManifest {
        schema_version: SCHEMA_VERSION,
        versions: vec![AudioVersion {
            id: "empty-pass-through".to_string(),
            label: "空修复集合".to_string(),
            interval_ids: Vec::new(),
            note: "验证空输入仍产生可复核的规范化输出".to_string(),
        }],
    };
    std::fs::write(&intervals_path, serde_json::to_vec_pretty(&intervals)?)?;
    std::fs::write(&versions_path, serde_json::to_vec_pretty(&versions)?)?;
    Ok(DemoPaths {
        recording,
        intervals: intervals_path,
        versions: versions_path,
    })
}

pub fn write_oversized_inputs(directory: &Path, max_intervals: usize) -> Result<DemoPaths> {
    std::fs::create_dir_all(directory)?;
    let recording = directory.join("original.wav");
    let intervals_path = directory.join("intervals.json");
    let versions_path = directory.join("versions.json");
    write_wav(&recording)?;
    let mut intervals = demo_intervals();
    intervals.intervals.clear();
    for index in 0..=max_intervals {
        intervals.intervals.push(RepairInterval {
            id: format!("limit-{index:06}"),
            track_id: 1,
            device_id: "device-a".to_string(),
            start: SourcePoint {
                device_ms: 1_000.0 + index as f64,
                frame: None,
            },
            end: SourcePoint {
                device_ms: 1_000.5 + index as f64,
                frame: None,
            },
            processor: Processor::Declick,
            note: "oversized deterministic rejection fixture".to_string(),
        });
    }
    let versions = VersionManifest {
        schema_version: SCHEMA_VERSION,
        versions: vec![AudioVersion {
            id: "oversized".to_string(),
            label: "超大输入".to_string(),
            interval_ids: Vec::new(),
            note: "应在解码前被限制器拒绝".to_string(),
        }],
    };
    std::fs::write(&intervals_path, serde_json::to_vec_pretty(&intervals)?)?;
    std::fs::write(&versions_path, serde_json::to_vec_pretty(&versions)?)?;
    Ok(DemoPaths {
        recording,
        intervals: intervals_path,
        versions: versions_path,
    })
}

pub fn demo_intervals() -> RepairInput {
    RepairInput {
        schema_version: SCHEMA_VERSION,
        reference_device_id: "device-a".to_string(),
        tolerance_ms: 40.0,
        devices: vec![
            DeviceClock {
                id: "device-a".to_string(),
                label: "主录音机 A（参考时钟）".to_string(),
                offset_ms: 0.0,
                drift_ppm: 0.0,
            },
            DeviceClock {
                id: "device-b".to_string(),
                label: "备份录音机 B（未同步）".to_string(),
                offset_ms: 7_312.0,
                drift_ppm: -140.0,
            },
        ],
        tracks: vec![
            Track {
                id: 1,
                device_id: "device-a".to_string(),
                sample_rate: 48_000,
                label: "A / 主轨".to_string(),
            },
            Track {
                id: 2,
                device_id: "device-b".to_string(),
                sample_rate: 48_000,
                label: "B / 备份轨".to_string(),
            },
        ],
        intervals: vec![
            RepairInterval {
                id: "repair-01-burst".to_string(),
                track_id: 2,
                device_id: "device-b".to_string(),
                start: SourcePoint {
                    device_ms: 12_311.3,
                    frame: Some(590_942),
                },
                end: SourcePoint {
                    device_ms: 12_811.23,
                    frame: Some(614_939),
                },
                processor: Processor::Gain {
                    gain_db: -9.0,
                    fade_ms: 20.0,
                },
                note: "B 钟读数比统一时间轴晚约 7.3 秒；降低爆音".to_string(),
            },
            RepairInterval {
                id: "repair-02-click".to_string(),
                track_id: 1,
                device_id: "device-a".to_string(),
                start: SourcePoint {
                    device_ms: 8_000.0,
                    frame: Some(384_000),
                },
                end: SourcePoint {
                    device_ms: 8_500.0,
                    frame: Some(408_000),
                },
                processor: Processor::Declick,
                note: "A 轨点击声线性桥接".to_string(),
            },
            RepairInterval {
                id: "repair-03-hum".to_string(),
                track_id: 2,
                device_id: "device-b".to_string(),
                start: SourcePoint {
                    device_ms: 15_510.852,
                    frame: Some(744_520),
                },
                end: SourcePoint {
                    device_ms: 16_010.782,
                    frame: Some(768_517),
                },
                processor: Processor::Normalize {
                    target_peak_db: -6.0,
                    fade_ms: 15.0,
                },
                note: "与 repair-02 重叠，验证版本内应用顺序".to_string(),
            },
        ],
    }
}

pub fn demo_versions() -> VersionManifest {
    VersionManifest {
        schema_version: SCHEMA_VERSION,
        versions: vec![
            AudioVersion {
                id: "v1-reference".to_string(),
                label: "V1 原始参考".to_string(),
                interval_ids: Vec::new(),
                note: "仅做统一采样率与单声道规范化".to_string(),
            },
            AudioVersion {
                id: "v2-draft".to_string(),
                label: "V2 爆音草稿".to_string(),
                interval_ids: vec!["repair-01-burst".to_string()],
                note: "只处理 B 轨爆音".to_string(),
            },
            AudioVersion {
                id: "v3-final".to_string(),
                label: "V3 最终复核".to_string(),
                interval_ids: vec![
                    "repair-01-burst".to_string(),
                    "repair-02-click".to_string(),
                    "repair-03-hum".to_string(),
                ],
                note: "按清单顺序应用全部修复".to_string(),
            },
        ],
    }
}

fn write_wav(path: &Path) -> Result<()> {
    let sample_rate = 48_000_u32;
    let seconds = 20_u32;
    let sample_count = sample_rate as usize * seconds as usize;
    let data_bytes = sample_count * 2;
    let mut file = std::fs::File::create(path)
        .with_context(|| format!("cannot create demo WAV {}", path.display()))?;
    file.write_all(b"RIFF")?;
    file.write_all(&((36 + data_bytes) as u32).to_le_bytes())?;
    file.write_all(b"WAVEfmt ")?;
    file.write_all(&16_u32.to_le_bytes())?;
    file.write_all(&1_u16.to_le_bytes())?;
    file.write_all(&1_u16.to_le_bytes())?;
    file.write_all(&sample_rate.to_le_bytes())?;
    file.write_all(&(sample_rate * 2).to_le_bytes())?;
    file.write_all(&2_u16.to_le_bytes())?;
    file.write_all(&16_u16.to_le_bytes())?;
    file.write_all(b"data")?;
    file.write_all(&(data_bytes as u32).to_le_bytes())?;
    let mut noise_state = 0x1234_5678_u32;
    for frame in 0..sample_count {
        let time = frame as f32 / sample_rate as f32;
        let mut value = 0.055 * (TAU * 220.0 * time).sin() + 0.018 * (TAU * 440.0 * time).sin();
        if (5.0..5.5).contains(&time) {
            noise_state = noise_state
                .wrapping_mul(1_664_525)
                .wrapping_add(1_013_904_223);
            let noise = ((noise_state >> 8) as f32 / 8_388_608.0) - 1.0;
            value += noise * 0.62;
        }
        if (8.0..8.5).contains(&time) && frame % (sample_rate as usize / 40) == 0 {
            value += 0.7;
        }
        if (8.2..8.7).contains(&time) {
            value += 0.12 * (TAU * 7_900.0 * time).sin();
        }
        let pcm = (value.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        file.write_all(&pcm.to_le_bytes())?;
    }
    file.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock;
    use crate::limits::Limits;

    #[test]
    fn demo_inputs_preserve_declared_version_order() {
        let versions = demo_versions();
        assert_eq!(
            versions
                .versions
                .iter()
                .map(|version| version.id.as_str())
                .collect::<Vec<_>>(),
            ["v1-reference", "v2-draft", "v3-final"]
        );
    }

    #[test]
    fn demo_clock_anomaly_maps_to_five_seconds() {
        let analysis = clock::analyze(
            &demo_intervals(),
            48_000,
            960_000,
            20_000.0,
            &Limits::default(),
        );
        assert!(analysis.errors.is_empty(), "{:?}", analysis.errors);
        assert!((analysis.intervals[0].canonical_start_ms - 5_000.0).abs() < 1.0);
        assert!(analysis.intervals[0].clock_skew_ms > 7_000.0);
        assert!(analysis.intervals.iter().any(|item| {
            item.warnings
                .iter()
                .any(|warning| warning.contains("OVERLAPPING_REPAIR"))
        }));
    }
}
