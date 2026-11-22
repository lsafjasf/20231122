use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use anyhow::{Context, Result};

const SAMPLE_RATE: u32 = 48_000;
const DURATION_SECONDS: u32 = 24;

pub fn ensure_demo_media(path: &Path) -> Result<()> {
    let expected_size = 44 + (SAMPLE_RATE as u64 * DURATION_SECONDS as u64 * 2);
    if std::fs::metadata(path)
        .map(|metadata| metadata.len() == expected_size)
        .unwrap_or(false)
    {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension("wav.tmp");
    let file =
        File::create(&temp).with_context(|| format!("创建演示音频失败: {}", path.display()))?;
    let mut writer = BufWriter::new(file);
    let sample_count = SAMPLE_RATE * DURATION_SECONDS;
    let data_size = sample_count * 2;

    writer.write_all(b"RIFF")?;
    writer.write_all(&(36 + data_size).to_le_bytes())?;
    writer.write_all(b"WAVE")?;
    writer.write_all(b"fmt ")?;
    writer.write_all(&16u32.to_le_bytes())?;
    writer.write_all(&1u16.to_le_bytes())?;
    writer.write_all(&1u16.to_le_bytes())?;
    writer.write_all(&SAMPLE_RATE.to_le_bytes())?;
    writer.write_all(&(SAMPLE_RATE * 2).to_le_bytes())?;
    writer.write_all(&2u16.to_le_bytes())?;
    writer.write_all(&16u16.to_le_bytes())?;
    writer.write_all(b"data")?;
    writer.write_all(&data_size.to_le_bytes())?;

    let mut noise_state = 0x51a7_31c9u32;
    for index in 0..sample_count {
        let t = index as f32 / SAMPLE_RATE as f32;
        let mut sample = match t {
            value if value < 4.0 => {
                let fade = (value / 0.35).min(1.0);
                0.21 * fade * sine(175.0, value) + 0.06 * sine(261.0, value)
            }
            value if value < 8.0 => {
                let swell = 0.75 + 0.25 * sine(0.7, value).abs();
                let overload = 1.0 + 0.35 * ((value - 6.2) / 0.15).clamp(0.0, 1.0);
                0.82 * swell * overload * sine(880.0, value) + 0.16 * sine(1_760.0, value)
            }
            value if value < 12.0 => 0.0,
            value if value < 16.0 => {
                let beat = (value - 12.0) % 0.62;
                let transient = (-beat * 18.0).exp();
                0.52 * sine(196.0, value) + 0.95 * transient * sine(1_240.0, value)
            }
            value if value < 20.0 => {
                let envelope = ((value - 16.0) / 0.8).min(1.0) * ((20.0 - value) / 1.1).min(1.0);
                envelope
                    * (0.55 * sine(330.0, value)
                        + 0.16 * sine(495.0, value)
                        + 0.08 * sine(660.0, value))
            }
            value => {
                let fade = ((24.0 - value) / 4.0).clamp(0.0, 1.0);
                fade * 0.38 * sine(440.0, value)
            }
        };
        noise_state = noise_state
            .wrapping_mul(1_664_525)
            .wrapping_add(1_013_904_223);
        let noise = ((noise_state >> 9) as f32 / 8_388_607.5) - 1.0;
        if !(8.0..12.0).contains(&t) {
            sample += noise * 0.008;
        }
        let quantized = (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        writer.write_all(&quantized.to_le_bytes())?;
    }
    writer.flush()?;
    drop(writer);
    std::fs::rename(&temp, path)
        .with_context(|| format!("提交演示音频失败: {}", path.display()))?;
    Ok(())
}

fn sine(frequency: f32, seconds: f32) -> f32 {
    (std::f32::consts::TAU * frequency * seconds).sin()
}
