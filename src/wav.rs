use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::error::{io_error, AppError, AppResult};
use crate::model::{TimelineEntry, WaveformStats};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WavInfo {
    pub format_tag: u16,
    pub channels: u16,
    pub sample_rate_hz: u32,
    pub bits_per_sample: u16,
    pub frames: u64,
    pub duration_us: u64,
    pub data_offset: u64,
    pub data_bytes: u64,
}

impl WavInfo {
    pub fn duration_us(&self, sample_rate_hz: u32) -> u64 {
        if sample_rate_hz == 0 {
            0
        } else {
            self.frames.saturating_mul(1_000_000) / sample_rate_hz as u64
        }
    }
}

pub fn probe_wav(path: &Path) -> AppResult<WavInfo> {
    let mut file = File::open(path).map_err(|err| io_error(path, err))?;
    let file_len = file.metadata().map_err(|err| io_error(path, err))?.len();
    let mut header = [0_u8; 12];
    file.read_exact(&mut header)
        .map_err(|_| AppError::Audio(format!("{}: truncated RIFF header", path.display())))?;
    if &header[0..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        return Err(AppError::Audio(format!(
            "{}: only RIFF/WAVE input is accepted; use a lossless WAV fixture",
            path.display()
        )));
    }

    let mut format_tag = 0_u16;
    let mut channels = 0_u16;
    let mut sample_rate_hz = 0_u32;
    let mut bits_per_sample = 0_u16;
    let mut block_align = 0_u16;
    let mut data_offset = 0_u64;
    let mut data_bytes = 0_u64;

    loop {
        let mut chunk_header = [0_u8; 8];
        if file.read_exact(&mut chunk_header).is_err() {
            break;
        }
        let chunk_size = u32::from_le_bytes(chunk_header[4..8].try_into().unwrap()) as u64;
        let chunk_data_offset = file.stream_position().map_err(|err| io_error(path, err))?;
        match &chunk_header[0..4] {
            b"fmt " => {
                if chunk_size < 16 {
                    return Err(AppError::Audio(format!(
                        "{}: fmt chunk is too short",
                        path.display()
                    )));
                }
                let mut fmt = [0_u8; 40];
                let read_len = (chunk_size as usize).min(fmt.len());
                file.read_exact(&mut fmt[..read_len])
                    .map_err(|err| io_error(path, err))?;
                format_tag = u16::from_le_bytes([fmt[0], fmt[1]]);
                channels = u16::from_le_bytes([fmt[2], fmt[3]]);
                sample_rate_hz = u32::from_le_bytes(fmt[4..8].try_into().unwrap());
                block_align = u16::from_le_bytes([fmt[12], fmt[13]]);
                bits_per_sample = u16::from_le_bytes([fmt[14], fmt[15]]);
                if format_tag == 0xfffe && read_len >= 26 {
                    format_tag = u16::from_le_bytes([fmt[24], fmt[25]]);
                }
                let consumed = read_len as u64;
                file.seek(SeekFrom::Start(
                    chunk_data_offset + consumed + (chunk_size - consumed) + (chunk_size & 1),
                ))
                .map_err(|err| io_error(path, err))?;
            }
            b"data" => {
                data_offset = chunk_data_offset;
                data_bytes = chunk_size.min(file_len.saturating_sub(chunk_data_offset));
                file.seek(SeekFrom::Start(
                    chunk_data_offset + chunk_size + (chunk_size & 1),
                ))
                .map_err(|err| io_error(path, err))?;
            }
            _ => {
                file.seek(SeekFrom::Start(
                    chunk_data_offset + chunk_size + (chunk_size & 1),
                ))
                .map_err(|err| io_error(path, err))?;
            }
        }
        if data_offset != 0 && format_tag != 0 {
            break;
        }
    }

    if format_tag == 0 || sample_rate_hz == 0 || channels == 0 || block_align == 0 {
        return Err(AppError::Audio(format!(
            "{}: missing or invalid fmt chunk",
            path.display()
        )));
    }
    if data_offset == 0 {
        return Err(AppError::Audio(format!(
            "{}: missing data chunk",
            path.display()
        )));
    }
    let supported = matches!(
        (format_tag, bits_per_sample),
        (1, 8 | 16 | 24 | 32) | (3, 32 | 64)
    );
    if !supported {
        return Err(AppError::Audio(format!(
            "{}: unsupported WAV format tag {format_tag} / {bits_per_sample} bits",
            path.display()
        )));
    }
    let frames = data_bytes / block_align as u64;
    let duration_us = if sample_rate_hz == 0 {
        0
    } else {
        frames.saturating_mul(1_000_000) / sample_rate_hz as u64
    };
    Ok(WavInfo {
        format_tag,
        channels,
        sample_rate_hz,
        bits_per_sample,
        frames,
        duration_us,
        data_offset,
        data_bytes,
    })
}

pub struct Pcm16Writer {
    writer: BufWriter<File>,
    channels: u16,
    sample_rate_hz: u32,
    frames: u64,
    data_bytes: u64,
}

impl Pcm16Writer {
    pub fn create(path: &Path, sample_rate_hz: u32, channels: u16) -> AppResult<Self> {
        let file = File::create(path).map_err(|err| io_error(path, err))?;
        let mut writer = BufWriter::new(file);
        writer
            .write_all(&[0_u8; 44])
            .map_err(|err| io_error(path, err))?;
        Ok(Self {
            writer,
            channels,
            sample_rate_hz,
            frames: 0,
            data_bytes: 0,
        })
    }

    pub fn write_f32_interleaved(&mut self, samples: &[f32]) -> AppResult<()> {
        let mut bytes = Vec::with_capacity(samples.len() * 2);
        for sample in samples {
            let clamped = sample.clamp(-1.0, 1.0);
            let value = if clamped <= -1.0 {
                i16::MIN
            } else {
                (clamped * i16::MAX as f32).round() as i16
            };
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        let max_data_bytes = u32::MAX as u64 - 36;
        let actual = self.data_bytes.saturating_add(bytes.len() as u64);
        if actual > max_data_bytes {
            return Err(AppError::LimitExceeded {
                what: "output WAV data bytes",
                actual,
                limit: max_data_bytes,
            });
        }
        self.writer
            .write_all(&bytes)
            .map_err(|err| io_error("output.wav", err))?;
        self.frames += (samples.len() / self.channels as usize) as u64;
        self.data_bytes += bytes.len() as u64;
        Ok(())
    }

    pub fn finish(mut self, path: &Path) -> AppResult<WavInfo> {
        self.writer.flush().map_err(|err| io_error(path, err))?;
        let file = self.writer.get_mut();
        let riff_size = 36_u32.saturating_add(self.data_bytes.min(u32::MAX as u64) as u32);
        let byte_rate = self.sample_rate_hz.saturating_mul(self.channels as u32 * 2);
        let block_align = self.channels * 2;
        let mut header = Vec::with_capacity(44);
        header.extend_from_slice(b"RIFF");
        header.extend_from_slice(&riff_size.to_le_bytes());
        header.extend_from_slice(b"WAVEfmt ");
        header.extend_from_slice(&16_u32.to_le_bytes());
        header.extend_from_slice(&1_u16.to_le_bytes());
        header.extend_from_slice(&self.channels.to_le_bytes());
        header.extend_from_slice(&self.sample_rate_hz.to_le_bytes());
        header.extend_from_slice(&byte_rate.to_le_bytes());
        header.extend_from_slice(&block_align.to_le_bytes());
        header.extend_from_slice(&16_u16.to_le_bytes());
        header.extend_from_slice(b"data");
        header.extend_from_slice(&(self.data_bytes.min(u32::MAX as u64) as u32).to_le_bytes());
        file.seek(SeekFrom::Start(0))
            .map_err(|err| io_error(path, err))?;
        file.write_all(&header).map_err(|err| io_error(path, err))?;
        file.sync_all().map_err(|err| io_error(path, err))?;
        probe_wav(path)
    }
}

pub fn write_empty_wav(path: &Path, sample_rate_hz: u32, channels: u16) -> AppResult<WavInfo> {
    Pcm16Writer::create(path, sample_rate_hz, channels)?.finish(path)
}

#[derive(Debug, Clone, Copy, Default)]
struct Accumulator {
    frames: u64,
    min: f64,
    max: f64,
    sum_squares: f64,
}

impl Accumulator {
    fn add(&mut self, sample: f64) {
        if self.frames == 0 {
            self.min = sample;
            self.max = sample;
        } else {
            self.min = self.min.min(sample);
            self.max = self.max.max(sample);
        }
        self.frames += 1;
        self.sum_squares += sample * sample;
    }

    fn finish(self) -> WaveformStats {
        if self.frames == 0 {
            return WaveformStats::default();
        }
        let rms = (self.sum_squares / self.frames as f64).sqrt();
        let peak = self.min.abs().max(self.max.abs()).max(1e-12);
        WaveformStats {
            sample_frames: self.frames,
            min: round6(self.min),
            max: round6(self.max),
            rms: round6(rms),
            peak_dbfs: round6(20.0 * peak.log10()),
        }
    }
}

pub fn analyze_timeline_waveforms(
    wav_path: &Path,
    entries: &mut [TimelineEntry],
    channel: u16,
) -> AppResult<()> {
    let info = probe_wav(wav_path)?;
    if channel >= info.channels {
        return Err(AppError::Audio(format!(
            "analysed channel {} is outside WAV channel count {}",
            channel, info.channels
        )));
    }
    if entries.is_empty() {
        return Ok(());
    }

    let mut accumulators = vec![Accumulator::default(); entries.len()];
    let mut file = File::open(wav_path).map_err(|err| io_error(wav_path, err))?;
    file.seek(SeekFrom::Start(info.data_offset))
        .map_err(|err| io_error(wav_path, err))?;
    let bytes_per_sample = (info.bits_per_sample / 8) as usize;
    let block_align = info.channels as usize * bytes_per_sample;
    let frames_per_block = 8_192_usize;
    let mut raw = vec![0_u8; frames_per_block * block_align];
    let mut current_frame = 0_u64;
    let mut entry_index = 0_usize;

    loop {
        let read = file.read(&mut raw).map_err(|err| io_error(wav_path, err))?;
        if read == 0 {
            break;
        }
        let complete_frames = read / block_align;
        for frame_offset in 0..complete_frames {
            let frame = current_frame + frame_offset as u64;
            while entry_index < entries.len() && frame >= entries[entry_index].output_frame_end {
                entry_index += 1;
            }
            if entry_index >= entries.len() {
                break;
            }
            let entry = &entries[entry_index];
            if frame < entry.output_frame_start {
                continue;
            }
            let base = frame_offset * block_align + channel as usize * bytes_per_sample;
            let sample = decode_sample(
                &raw[base..base + bytes_per_sample],
                info.format_tag,
                info.bits_per_sample,
            );
            accumulators[entry_index].add(sample);
        }
        current_frame += complete_frames as u64;
        if complete_frames == 0 {
            break;
        }
    }

    for (entry, accumulator) in entries.iter_mut().zip(accumulators) {
        entry.waveform = accumulator.finish();
    }
    Ok(())
}

pub fn output_peak_and_clipping(wav_path: &Path) -> AppResult<(f64, u64)> {
    let info = probe_wav(wav_path)?;
    if info.format_tag != 1 || info.bits_per_sample != 16 {
        return Ok((0.0, 0));
    }
    let mut file = File::open(wav_path).map_err(|err| io_error(wav_path, err))?;
    file.seek(SeekFrom::Start(info.data_offset))
        .map_err(|err| io_error(wav_path, err))?;
    let mut reader = BufReader::new(file);
    let mut buffer = [0_u8; 64 * 1024];
    let mut peak = 0_i32;
    let mut clipping = 0_u64;
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|err| io_error(wav_path, err))?;
        if read == 0 {
            break;
        }
        for pair in buffer[..read].chunks_exact(2) {
            let sample = i16::from_le_bytes([pair[0], pair[1]]) as i32;
            let abs = sample.abs();
            peak = peak.max(abs);
            if abs >= i16::MAX as i32 {
                clipping += 1;
            }
        }
    }
    let peak_dbfs = if peak == 0 {
        -240.0
    } else {
        20.0 * (peak as f64 / i16::MAX as f64).log10()
    };
    Ok((round6(peak_dbfs), clipping))
}

fn decode_sample(bytes: &[u8], format_tag: u16, bits: u16) -> f64 {
    match (format_tag, bits) {
        (3, 32) => f32::from_le_bytes(bytes[..4].try_into().unwrap()) as f64,
        (3, 64) => f64::from_le_bytes(bytes[..8].try_into().unwrap()),
        (1, 16) => i16::from_le_bytes(bytes[..2].try_into().unwrap()) as f64 / 32768.0,
        (1, 8) => (bytes[0] as f64 - 128.0) / 128.0,
        (1, 24) => {
            let value =
                ((bytes[2] as i32) << 16 | (bytes[1] as i32) << 8 | bytes[0] as i32) << 8 >> 8;
            value as f64 / 8_388_608.0
        }
        (1, 32) => i32::from_le_bytes(bytes[..4].try_into().unwrap()) as f64 / 2_147_483_648.0,
        _ => 0.0,
    }
}

pub fn round6(value: f64) -> f64 {
    if value.is_finite() {
        (value * 1_000_000.0).round() / 1_000_000.0
    } else {
        value
    }
}
