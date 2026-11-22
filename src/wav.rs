//! Deterministic PCM WAV splice used to insert a missing-segment gap.
//!
//! The heavy lifting (decoding, resampling, silence synthesis) is done by
//! GStreamer. This module only rewrites RIFF headers and copies exact PCM
//! frame ranges, so identical inputs always produce byte-identical output.

use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};

use crate::util::{read_wav_info, WavInfo};

/// Write a 16-bit PCM silence WAV with an exact number of frames. Pure byte
/// synthesis, so the frame count is guaranteed exact regardless of any
/// GStreamer buffer sizing.
pub fn write_silence_wav(
    path: &Path,
    sample_rate: u32,
    channels: u16,
    frames: u64,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }
    let bits = 16u16;
    let frame_size = channels as u64 * bits as u64 / 8;
    let data_len = (frames * frame_size) as u32;
    let header = build_pcm_header(sample_rate, channels, bits, data_len);
    let mut file = fs::File::create(path)
        .with_context(|| format!("cannot create {}", path.display()))?;
    use std::io::Write;
    file.write_all(&header)?;
    // Zero-filled silence; write in chunks to avoid large allocations.
    let chunk = vec![0u8; 64 * 1024];
    let mut remaining = data_len as u64;
    while remaining > 0 {
        let take = remaining.min(chunk.len() as u64) as usize;
        file.write_all(&chunk[..take])?;
        remaining -= take as u64;
    }
    Ok(())
}

/// Insert `gap_wav` into `src_wav` at PCM frame `gap_start_frame`, writing
/// a new 16-bit PCM WAV at `out_wav`. All three streams must share rate,
/// channel count and sample width.
pub fn splice_gap(
    src_wav: &Path,
    gap_wav: &Path,
    out_wav: &Path,
    gap_start_frame: u64,
) -> Result<()> {
    let src = read_wav_info(src_wav)?.context("resampled output is not WAV")?;
    let gap = read_wav_info(gap_wav)?.context("gap output is not WAV")?;
    validate_compatible(&src, &gap)?;
    if gap_start_frame > src.frames() {
        bail!(
            "gap start frame {} exceeds resampled length {}",
            gap_start_frame,
            src.frames()
        );
    }

    let src_bytes = fs::read(src_wav)
        .with_context(|| format!("cannot read {}", src_wav.display()))?;
    let gap_bytes = fs::read(gap_wav)
        .with_context(|| format!("cannot read {}", gap_wav.display()))?;
    let src_data = locate_data(&src_bytes)?;
    let gap_data = locate_data(&gap_bytes)?;

    let frame_size = (src.channels as usize) * (src.bits_per_sample as usize) / 8;
    let split_byte = gap_start_frame as usize * frame_size;
    let before = &src_data[..split_byte.min(src_data.len())];
    let after = &src_data[split_byte.min(src_data.len())..];
    let total_data_len = before.len() + gap_data.len() + after.len();

    let header = build_pcm_header(src.sample_rate, src.channels, src.bits_per_sample, total_data_len as u32);
    let mut out = Vec::with_capacity(header.len() + total_data_len);
    out.extend_from_slice(&header);
    out.extend_from_slice(before);
    out.extend_from_slice(gap_data);
    out.extend_from_slice(after);
    fs::write(out_wav, out)
        .with_context(|| format!("cannot write {}", out_wav.display()))?;
    Ok(())
}

fn validate_compatible(a: &WavInfo, b: &WavInfo) -> Result<()> {
    if a.sample_rate != b.sample_rate || a.channels != b.channels || a.bits_per_sample != b.bits_per_sample
    {
        bail!(
            "gap format {}/{}/{}bit does not match source {}/{}/{}bit",
            b.sample_rate,
            b.channels,
            b.bits_per_sample,
            a.sample_rate,
            a.channels,
            a.bits_per_sample
        );
    }
    if a.bits_per_sample != 16 {
        bail!("only 16-bit PCM WAV splicing is supported");
    }
    Ok(())
}

/// Return the data-chunk payload slice of an in-memory WAV file.
fn locate_data(bytes: &[u8]) -> Result<&[u8]> {
    let mut pos = 12;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body_start = pos + 8;
        if id == b"data" {
            let end = (body_start + size).min(bytes.len());
            return bytes
                .get(body_start..end)
                .context("truncated WAV data chunk");
        }
        pos = body_start + size + (size & 1);
    }
    bail!("WAV has no data chunk")
}

fn build_pcm_header(sample_rate: u32, channels: u16, bits: u16, data_len: u32) -> Vec<u8> {
    let byte_rate = sample_rate * channels as u32 * bits as u32 / 8;
    let block_align = channels * bits / 8;
    let riff_len = 36 + data_len;
    let mut h = Vec::with_capacity(44);
    h.extend_from_slice(b"RIFF");
    h.extend_from_slice(&riff_len.to_le_bytes());
    h.extend_from_slice(b"WAVE");
    h.extend_from_slice(b"fmt ");
    h.extend_from_slice(&16u32.to_le_bytes());
    h.extend_from_slice(&1u16.to_le_bytes()); // PCM
    h.extend_from_slice(&channels.to_le_bytes());
    h.extend_from_slice(&sample_rate.to_le_bytes());
    h.extend_from_slice(&byte_rate.to_le_bytes());
    h.extend_from_slice(&block_align.to_le_bytes());
    h.extend_from_slice(&bits.to_le_bytes());
    h.extend_from_slice(b"data");
    h.extend_from_slice(&data_len.to_le_bytes());
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_path(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("srcrepair-test-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn silence_has_exact_frames() {
        let path = temp_path("gap.wav");
        write_silence_wav(&path, 48000, 2, 12000).unwrap();
        let info = crate::util::read_wav_info(&path).unwrap().unwrap();
        assert_eq!(info.sample_rate, 48000);
        assert_eq!(info.channels, 2);
        assert_eq!(info.frames(), 12000);
    }

    #[test]
    fn splice_inserts_exact_gap() {
        let dir = temp_path("x.wav").parent().unwrap().to_path_buf();
        let src = dir.join("src.wav");
        let gap = dir.join("gap.wav");
        let out = dir.join("out.wav");
        write_silence_wav(&src, 48000, 2, 1000).unwrap();
        write_silence_wav(&gap, 48000, 2, 250).unwrap();
        splice_gap(&src, &gap, &out, 400).unwrap();
        let info = crate::util::read_wav_info(&out).unwrap().unwrap();
        assert_eq!(info.frames(), 1250);
    }

    #[test]
    fn splice_at_zero_prepends_gap() {
        let dir = temp_path("y.wav").parent().unwrap().to_path_buf();
        let src = dir.join("s2.wav");
        let gap = dir.join("g2.wav");
        let out = dir.join("o2.wav");
        write_silence_wav(&src, 32000, 1, 500).unwrap();
        write_silence_wav(&gap, 32000, 1, 100).unwrap();
        splice_gap(&src, &gap, &out, 0).unwrap();
        let info = crate::util::read_wav_info(&out).unwrap().unwrap();
        assert_eq!(info.frames(), 600);
    }
}
