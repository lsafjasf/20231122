use std::fs::{self, File};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: u32 = 8;
pub const BITS_PER_SAMPLE: u32 = 24;
pub const DURATION_SECS: f64 = 18.4;
pub const SOURCE_START_SECS: f64 = 5.0;
pub const GAP_START_SECS: f64 = 6.47;
pub const GAP_END_SECS: f64 = 6.52;
pub const REPLAY_START_SECS: f64 = 6.52;
pub const REPLAY_END_SECS: f64 = 7.99;
pub const CROSSFADE_FRAMES: u64 = 41;
pub const XMTA_FOURCC: [u8; 4] = *b"XMTA";
pub const XMTA_PAYLOAD_LEN: u32 = 2_784;
pub const CHANNEL_LABELS: [(&str, &str); 8] = [
    ("L", "左前"),
    ("R", "右前"),
    ("C", "中置"),
    ("LFE", "低频"),
    ("Ls", "左环绕"),
    ("Rs", "右环绕"),
    ("Lb", "左后"),
    ("Rb", "右后"),
];

#[derive(Clone, Debug)]
pub struct RiffInfo {
    pub path: PathBuf,
    pub file_size: u64,
    pub sha256: String,
    pub channels: u32,
    pub sample_rate: u32,
    pub bits_per_sample: u32,
    pub data_offset: u64,
    pub data_size: u64,
    pub duration_secs: f64,
    pub extension_offset: u64,
    pub extension_size: u64,
    pub extension_sha256: String,
    pub extension_preview: String,
    pub extension_error: String,
}

pub fn demo_path() -> PathBuf {
    Path::new("artifacts").join("demo-source.wav")
}

pub fn ensure_demo_source() -> Result<PathBuf> {
    let path = demo_path();
    if !path.exists() {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        generate_demo_wav(&path)?;
    }
    Ok(path)
}

fn frame_sample(channel: usize, frame: u64) -> f32 {
    let t = frame as f64 / SAMPLE_RATE as f64;
    let frequencies = [220.0, 330.0, 440.0, 55.0, 550.0, 660.0, 770.0, 880.0];
    let amplitudes = [0.22, 0.18, 0.14, 0.27, 0.10, 0.10, 0.08, 0.08];
    let base = (std::f64::consts::TAU * frequencies[channel] * t
        + 0.15 * (std::f64::consts::TAU * 0.31 * t + channel as f64).sin())
    .sin();
    let slow = 0.72 + 0.28 * (std::f64::consts::TAU * (0.071 + channel as f64 * 0.003) * t).sin();

    let mut hash = frame.wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (channel as u64 + 1).wrapping_mul(0xD6E8_FEB8_6659_FD93);
    hash ^= hash >> 30;
    hash = hash.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    hash ^= hash >> 27;
    let dither = (hash as u32 as f64 / u32::MAX as f64 - 0.5) * 0.003;

    (base * amplitudes[channel] * slow) as f32 + dither as f32
}

fn build_demo_frames() -> Vec<[f32; 8]> {
    let frames = (DURATION_SECS * SAMPLE_RATE as f64).round() as u64;
    let mut pcm = Vec::with_capacity(frames as usize);
    for frame in 0..frames {
        let mut values = [0.0_f32; 8];
        for (channel, value) in values.iter_mut().enumerate() {
            *value = frame_sample(channel, frame);
        }
        pcm.push(values);
    }

    let gap_start = (GAP_START_SECS * SAMPLE_RATE as f64).round() as u64;
    let gap_end = (GAP_END_SECS * SAMPLE_RATE as f64).round() as u64;
    for frame in gap_start..gap_end {
        pcm[frame as usize] = [0.0; 8];
    }

    let source_start = (SOURCE_START_SECS * SAMPLE_RATE as f64).round() as u64;
    let replay_start = (REPLAY_START_SECS * SAMPLE_RATE as f64).round() as u64;
    let replay_frames = ((REPLAY_END_SECS - REPLAY_START_SECS) * SAMPLE_RATE as f64).round() as u64;
    for offset in 0..replay_frames {
        let source_frame = source_start + offset;
        let replay_frame = replay_start + offset;
        if source_frame >= pcm.len() as u64 || replay_frame >= pcm.len() as u64 {
            break;
        }
        if offset < CROSSFADE_FRAMES {
            let alpha = offset as f32 / CROSSFADE_FRAMES as f32;
            for channel in 0..8 {
                let live = pcm[replay_frame as usize][channel];
                pcm[replay_frame as usize][channel] =
                    live * (1.0 - alpha) + pcm[source_frame as usize][channel] * alpha;
            }
        } else {
            pcm[replay_frame as usize] = pcm[source_frame as usize];
        }
    }
    pcm
}

fn write_demo_wav(path: &Path) -> Result<()> {
    let pcm = build_demo_frames();
    let data_size = (pcm.len() as u64) * CHANNELS as u64 * 3;
    let riff_size = 4 + 24 + (8 + XMTA_PAYLOAD_LEN as u64) + 8 + data_size;
    let mut writer = BufWriter::new(File::create(path)?);

    writer.write_all(b"RIFF")?;
    writer.write_all(&(riff_size as u32).to_le_bytes())?;
    writer.write_all(b"WAVEfmt ")?;
    writer.write_all(&16u32.to_le_bytes())?;
    writer.write_all(&1u16.to_le_bytes())?;
    writer.write_all(&(CHANNELS as u16).to_le_bytes())?;
    writer.write_all(&SAMPLE_RATE.to_le_bytes())?;
    writer.write_all(&(SAMPLE_RATE * CHANNELS * 3).to_le_bytes())?;
    writer.write_all(&((CHANNELS * 3) as u16).to_le_bytes())?;
    writer.write_all(&(BITS_PER_SAMPLE as u16).to_le_bytes())?;

    let mut extension = vec![0u8; XMTA_PAYLOAD_LEN as usize];
    extension[..4].copy_from_slice(b"MRX1");
    extension[4..6].copy_from_slice(&1u16.to_le_bytes());
    extension[6..8].copy_from_slice(&32u16.to_le_bytes());
    extension[8..10].copy_from_slice(&97u16.to_le_bytes());
    extension[10..12].copy_from_slice(&32u16.to_le_bytes());
    extension[16..29].copy_from_slice(b"BUS-A/MON-8CH");
    let mut state = 0x5A17_C0DE_u32;
    for byte in &mut extension[32..] {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        *byte = state as u8;
    }
    writer.write_all(&XMTA_FOURCC)?;
    writer.write_all(&XMTA_PAYLOAD_LEN.to_le_bytes())?;
    writer.write_all(&extension)?;

    writer.write_all(b"data")?;
    writer.write_all(&(data_size as u32).to_le_bytes())?;
    let mut frame_bytes = Vec::with_capacity(24);
    for frame in &pcm {
        frame_bytes.clear();
        for sample in frame {
            let value = (sample.clamp(-1.0, 1.0) * 8_388_607.0).round() as i32;
            frame_bytes.extend_from_slice(&value.to_le_bytes()[..3]);
        }
        writer.write_all(&frame_bytes)?;
    }
    writer.flush()?;
    Ok(())
}

pub fn generate_demo_wav(path: &Path) -> Result<()> {
    let temp = path.with_extension("wav.part");
    write_demo_wav(&temp).with_context(|| format!("写演示音频失败: {}", temp.display()))?;
    fs::rename(&temp, path)?;
    Ok(())
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(hex::encode(digest.finalize()))
}

pub fn inspect_riff(path: &Path) -> Result<RiffInfo> {
    let file_size = fs::metadata(path)?.len();
    let mut file = File::open(path)?;
    let mut header = [0u8; 12];
    file.read_exact(&mut header)?;
    if &header[..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        bail!("不是 RIFF/WAVE 来源");
    }

    let mut position = 12u64;
    let mut channels = 0u32;
    let mut sample_rate = 0u32;
    let mut bits_per_sample = 0u32;
    let mut data_offset = 0u64;
    let mut data_size = 0u64;
    let mut extension_offset = 0u64;
    let mut extension_size = 0u64;
    let mut extension_sha256 = String::new();
    let mut extension_preview = String::new();
    let mut extension_error = String::new();

    while position + 8 <= file_size {
        file.seek(SeekFrom::Start(position))?;
        let mut chunk_header = [0u8; 8];
        file.read_exact(&mut chunk_header)?;
        let chunk_id = &chunk_header[..4];
        let chunk_size = u32::from_le_bytes(chunk_header[4..8].try_into().unwrap()) as u64;
        let payload_offset = position + 8;
        if payload_offset + chunk_size > file_size {
            bail!(
                "RIFF 块越界: {} @ {}",
                String::from_utf8_lossy(chunk_id),
                position
            );
        }

        if chunk_id == b"fmt " {
            let mut fmt = [0u8; 16];
            file.read_exact(&mut fmt)?;
            channels = u16::from_le_bytes(fmt[2..4].try_into().unwrap()) as u32;
            sample_rate = u32::from_le_bytes(fmt[4..8].try_into().unwrap());
            bits_per_sample = u16::from_le_bytes(fmt[14..16].try_into().unwrap()) as u32;
        } else if chunk_id == XMTA_FOURCC.as_slice() {
            extension_offset = position;
            extension_size = chunk_size;
            let mut payload = vec![0u8; chunk_size as usize];
            file.read_exact(&mut payload)?;
            extension_sha256 = hex::encode(Sha256::digest(&payload));
            let preview_len = payload.len().min(48);
            extension_preview = hex::encode(&payload[..preview_len]);
            if payload.len() >= 12 {
                let header_len = u16::from_le_bytes([payload[6], payload[7]]) as u64;
                let count = u16::from_le_bytes([payload[8], payload[9]]) as u64;
                let record_size = u16::from_le_bytes([payload[10], payload[11]]) as u64;
                let available = (payload.len() as u64).saturating_sub(header_len);
                let declared = count * record_size;
                if declared > available {
                    extension_error = format!(
                        "记录表声明 {count} × {record_size} B = {declared} B，超出可用 {available} B；解析器在读取第一条记录前停止。"
                    );
                }
            }
        } else if chunk_id == b"data" {
            data_offset = payload_offset;
            data_size = chunk_size;
        }

        position = payload_offset + chunk_size + (chunk_size & 1);
    }

    if channels == 0 || sample_rate == 0 || data_size == 0 {
        bail!("RIFF 缺少 fmt 或 data 块");
    }
    if extension_offset == 0 {
        bail!("演示来源缺少 XMTA 扩展");
    }
    let bytes_per_frame = channels as u64 * (bits_per_sample as u64 / 8);
    let duration_secs = data_size as f64 / (sample_rate as f64 * bytes_per_frame as f64);

    Ok(RiffInfo {
        path: path.to_path_buf(),
        file_size,
        sha256: sha256_file(path)?,
        channels,
        sample_rate,
        bits_per_sample,
        data_offset,
        data_size,
        duration_secs,
        extension_offset,
        extension_size,
        extension_sha256,
        extension_preview,
        extension_error,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_keeps_one_extension_and_replay() {
        let path = std::env::temp_dir().join("route-inspector-fixture.wav");
        generate_demo_wav(&path).unwrap();
        let info = inspect_riff(&path).unwrap();
        assert_eq!(info.channels, 8);
        assert_eq!(info.sample_rate, 48_000);
        assert_eq!(info.extension_size, 2_784);
        assert!(info.extension_error.contains("超出可用"));
        assert!(info.duration_secs > 18.0);
        let _ = fs::remove_file(path);
    }
}
