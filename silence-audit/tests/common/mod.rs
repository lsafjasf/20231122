//! 测试夹具：生成带可控静音段的 PCM16 单声道 WAV 文件。

use std::fs;
use std::path::Path;

/// 一个时间区间（秒），`true` 表示该区间为静音（数字零），否则生成正弦音。
pub struct AudioSpec {
    pub duration_secs: f64,
    pub sample_rate: u32,
    /// (起始秒, 结束秒, 是否静音) 的有序区间，需覆盖整个时长。
    pub windows: Vec<(f64, f64, bool)>,
}

/// 写出 16-bit PCM 单声道 WAV。
pub fn write_wav(path: &Path, spec: &AudioSpec) -> std::io::Result<()> {
    let total = (spec.duration_secs * spec.sample_rate as f64) as usize;
    let mut samples: Vec<i16> = Vec::with_capacity(total);
    for n in 0..total {
        let t = n as f64 / spec.sample_rate as f64;
        let silent = spec
            .windows
            .iter()
            .find(|(start, end, _)| t >= *start && t < *end)
            .map(|(_, _, s)| *s)
            .unwrap_or(false);
        if silent {
            samples.push(0);
        } else {
            // 440Hz 正弦，幅度约 0.5。
            let value = (2.0 * std::f64::consts::PI * 440.0 * t).sin() * 16_000.0;
            samples.push(value as i16);
        }
    }

    let data_bytes = samples.len() * 2;
    let mut out = Vec::with_capacity(44 + data_bytes);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36u32 + data_bytes as u32).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&spec.sample_rate.to_le_bytes());
    out.extend_from_slice(&(spec.sample_rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data_bytes as u32).to_le_bytes());
    for sample in samples {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    fs::write(path, out)
}

/// 标准测试规格：5 秒音频，1.0–2.5s 与 3.5–4.5s 两段静音。
pub fn standard_spec() -> AudioSpec {
    AudioSpec {
        duration_secs: 5.0,
        sample_rate: 48_000,
        windows: vec![
            (0.0, 1.0, false),
            (1.0, 2.5, true),
            (2.5, 3.5, false),
            (3.5, 4.5, true),
            (4.5, 5.0, false),
        ],
    }
}
