use crate::model::{ANCHOR_COUNT, AnchorSpec, now_rfc3339};
use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct WavInfo {
    pub file_size: u64,
    pub data_offset: u64,
    pub data_size: u64,
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
    pub block_align: u16,
    pub sample_format: String,
    pub frame_count: u64,
    pub duration_ns: u64,
}

fn read_u16(bytes: &[u8], at: usize) -> Result<u16> {
    let slice = bytes.get(at..at + 2).context("WAV 字段越界")?;
    Ok(u16::from_le_bytes([slice[0], slice[1]]))
}

fn read_u32(bytes: &[u8], at: usize) -> Result<u32> {
    let slice = bytes.get(at..at + 4).context("WAV 字段越界")?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

pub fn probe_wav(path: &Path) -> Result<WavInfo> {
    let mut file =
        File::open(path).with_context(|| format!("无法打开原始 WAV: {}", path.display()))?;
    let file_size = file.metadata()?.len();
    ensure!(file_size >= 44, "WAV 文件过短");
    let mut header = vec![0u8; file_size.min(1024 * 1024) as usize];
    file.read_exact(&mut header)?;
    ensure!(&header[0..4] == b"RIFF", "缺少 RIFF 标记");
    ensure!(&header[8..12] == b"WAVE", "缺少 WAVE 标记");

    let mut cursor = 12usize;
    let mut fmt: Option<(u16, u16, u32, u16, u16)> = None;
    let mut data: Option<(u64, u64)> = None;
    while cursor + 8 <= header.len() {
        let chunk_id = &header[cursor..cursor + 4];
        let chunk_size = read_u32(&header, cursor + 4)? as u64;
        let body = cursor + 8;
        if chunk_id == b"fmt " {
            ensure!(chunk_size >= 16, "fmt 块过短");
            let format = read_u16(&header, body)?;
            let channels = read_u16(&header, body + 2)?;
            let rate = read_u32(&header, body + 4)?;
            let block_align = read_u16(&header, body + 12)?;
            let bits = read_u16(&header, body + 14)?;
            fmt = Some((format, channels, rate, block_align, bits));
        } else if chunk_id == b"data" {
            data = Some((
                body as u64,
                chunk_size.min(file_size.saturating_sub(body as u64)),
            ));
        }
        let padded = chunk_size + (chunk_size & 1);
        let next = body as u64 + padded;
        if next >= header.len() as u64 {
            if data.is_none() && cursor + 8 <= header.len() {
                bail!("数据块头超出预读范围");
            }
            break;
        }
        cursor = next as usize;
    }
    let (format, channels, sample_rate, block_align, bits_per_sample) =
        fmt.context("WAV 缺少 fmt 块")?;
    let (data_offset, data_size) = data.context("WAV 缺少 data 块")?;
    ensure!(channels > 0 && sample_rate > 0, "WAV 声道或采样率无效");
    ensure!(block_align > 0, "WAV block_align 无效");
    let sample_format = match (format, bits_per_sample) {
        (1, 16) => "S16LE".to_string(),
        (1, 24) => "S24LE".to_string(),
        (1, 32) => "S32LE".to_string(),
        (3, 32) => "F32LE".to_string(),
        (3, 64) => "F64LE".to_string(),
        _ => format!("PCM/{}bit-format{}", bits_per_sample, format),
    };
    ensure!(
        sample_format == "S16LE" || sample_format == "F32LE",
        "当前分段处理器要求 S16LE 或 F32LE，实际为 {sample_format}"
    );
    ensure!(data_size % block_align as u64 == 0, "WAV 数据区未按帧对齐");
    let frame_count = data_size / block_align as u64;
    let duration_ns = frame_count.saturating_mul(1_000_000_000) / sample_rate as u64;
    Ok(WavInfo {
        file_size,
        data_offset,
        data_size,
        sample_rate,
        channels,
        bits_per_sample,
        block_align,
        sample_format,
        frame_count,
        duration_ns,
    })
}

pub fn build_anchors(info: &WavInfo) -> Result<Vec<AnchorSpec>> {
    ensure!(
        info.frame_count >= ANCHOR_COUNT as u64,
        "音频帧数不足以切成三段"
    );
    let mut anchors = Vec::with_capacity(ANCHOR_COUNT);
    for index in 0..ANCHOR_COUNT {
        let start_frame = info.frame_count * index as u64 / ANCHOR_COUNT as u64;
        let end_frame = info.frame_count * (index + 1) as u64 / ANCHOR_COUNT as u64;
        let start_ns = start_frame.saturating_mul(1_000_000_000) / info.sample_rate as u64;
        let end_ns = end_frame.saturating_mul(1_000_000_000) / info.sample_rate as u64;
        let payload_start = start_frame * info.block_align as u64;
        let payload_end = end_frame * info.block_align as u64;
        anchors.push(AnchorSpec {
            id: char::from(b'A' + index as u8).to_string(),
            segment_index: index + 1,
            segment_start_ns: start_ns,
            segment_end_ns: end_ns,
            anchor_ns: end_ns,
            segment_start_frame: start_frame,
            segment_end_frame: end_frame,
            anchor_frame: end_frame,
            payload_start_byte: payload_start,
            payload_end_byte: payload_end,
            file_start_byte: info.data_offset + payload_start,
            file_end_byte: info.data_offset + payload_end,
        });
    }
    Ok(anchors)
}

pub fn file_sha256(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

pub fn bytes_sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

pub fn copy_readonly(source: &Path, destination: &Path) -> Result<String> {
    if destination.exists() {
        let source_hash = file_sha256(source)?;
        let destination_hash = file_sha256(destination)?;
        ensure!(
            source_hash == destination_hash,
            "目标原件副本已存在且内容不同: {}",
            destination.display()
        );
        fs::set_permissions(destination, fs::Permissions::from_mode(0o444))?;
        return Ok(destination_hash);
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(source, destination)
        .with_context(|| format!("复制原始材料失败: {}", destination.display()))?;
    fs::set_permissions(destination, fs::Permissions::from_mode(0o444))?;
    file_sha256(destination)
}

pub fn read_raw_block(source: &Path, start_byte: u64, max_bytes: usize) -> Result<(u64, Vec<u8>)> {
    let mut file = File::open(source)?;
    use std::io::{Seek, SeekFrom};
    let file_size = file.metadata()?.len();
    let start = start_byte.saturating_sub(max_bytes as u64).min(file_size);
    let len = file_size.saturating_sub(start).min(max_bytes as u64) as usize;
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = vec![0u8; len];
    file.read_exact(&mut bytes)?;
    Ok((start, bytes))
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension(format!(
        "{}.tmp",
        path.extension().and_then(|s| s.to_str()).unwrap_or("json")
    ));
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&temporary, path)?;
    if let Some(parent) = path.parent()
        && let Ok(directory) = File::open(parent)
    {
        let _ = directory.sync_all();
    }
    Ok(())
}

pub fn atomic_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    atomic_write(path, &bytes)
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = fs::read(path).with_context(|| format!("读取 JSON 失败: {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("解析 JSON 失败: {}", path.display()))
}

pub fn append_json_line<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    serde_json::to_writer(&mut file, value)?;
    file.write_all(b"\n")?;
    file.sync_data()?;
    Ok(())
}

pub fn store_anchor_raw(
    source: &Path,
    output: &Path,
    anchor_byte: u64,
    max_bytes: usize,
) -> Result<(u64, u64, String)> {
    let (start, bytes) = read_raw_block(source, anchor_byte, max_bytes)?;
    atomic_write(output, &bytes)?;
    Ok((start, bytes.len() as u64, bytes_sha256(&bytes)))
}

pub fn absolute_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

pub fn write_acceptance_line(path: &Path, line: &str) -> Result<()> {
    let mut existing = if path.exists() {
        fs::read_to_string(path)?
    } else {
        String::new()
    };
    if !existing.is_empty() && !existing.ends_with('\n') {
        existing.push('\n');
    }
    existing.push_str(line);
    existing.push('\n');
    atomic_write(path, existing.as_bytes())
}

pub fn utc_stamp() -> String {
    now_rfc3339()
}
