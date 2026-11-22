//! Small shared helpers: hashing, WAV header inspection, timestamps.

use std::fs::File;
use std::io::{BufReader, Read, Seek};
use std::path::Path;

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

/// Streaming SHA-256 so large media never have to fit in memory.
pub fn sha256_file(path: &Path) -> Result<String> {
    let file = File::open(path).with_context(|| format!("cannot open {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex_encode(&hasher.finalize()))
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

/// Metadata parsed directly out of a PCM WAV header (no GStreamer needed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WavInfo {
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
    pub data_bytes: u64,
}

impl WavInfo {
    pub fn frames(&self) -> u64 {
        let frame_size = (self.channels as u64) * (self.bits_per_sample as u64) / 8;
        if frame_size == 0 {
            0
        } else {
            self.data_bytes / frame_size
        }
    }
}

/// Minimal RIFF/WAVE parser supporting the chunks our pipeline emits.
/// Returns `Ok(None)` for files that are not WAV (e.g. a source container).
pub fn read_wav_info(path: &Path) -> Result<Option<WavInfo>> {
    let file = File::open(path).with_context(|| format!("cannot open {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut head = [0u8; 12];
    if reader.read(&mut head)? != 12 {
        return Ok(None);
    }
    if &head[0..4] != b"RIFF" || &head[8..12] != b"WAVE" {
        return Ok(None);
    }

    let mut sample_rate = None;
    let mut channels = None;
    let mut bits = None;
    let mut data_bytes = None;

    loop {
        let mut id = [0u8; 4];
        match read_full(&mut reader, &mut id)? {
            // Natural end of file after the last chunk: the scan is done.
            None => break,
            Some(()) => {}
        }
        let mut size_buf = [0u8; 4];
        if read_full(&mut reader, &mut size_buf)?.is_none() {
            break;
        }
        let chunk_size = u32::from_le_bytes(size_buf) as u64;
        match &id {
            b"fmt " => {
                let mut fmt = [0u8; 16];
                read_full(&mut reader, &mut fmt)?;
                channels = Some(u16::from_le_bytes([fmt[2], fmt[3]]));
                sample_rate = Some(u32::from_le_bytes([fmt[4], fmt[5], fmt[6], fmt[7]]));
                bits = Some(u16::from_le_bytes([fmt[14], fmt[15]]));
                if chunk_size > 16 {
                    skip(&mut reader, chunk_size - 16)?;
                }
            }
            b"data" => {
                // Some producers (GStreamer wavenc on a truncated final
                // buffer) declare more bytes than the file contains. Trust
                // the physical payload and remember the declared size.
                let available = reader.get_ref().metadata().map(|m| m.len()).unwrap_or(0);
                let actual = chunk_size.min(available.saturating_sub(reader.stream_position()?));
                data_bytes = Some(actual);
                skip(&mut reader, actual)?;
            }
            other => {
                let _ = other;
                skip(&mut reader, chunk_size)?;
            }
        }
        if chunk_size & 1 == 1 {
            let mut pad = [0u8; 1];
            if read_full(&mut reader, &mut pad)?.is_none() {
                break;
            }
        }
    }

    match (sample_rate, channels, bits, data_bytes) {
        (Some(sample_rate), Some(channels), Some(bits_per_sample), Some(data_bytes)) => Ok(Some(
            WavInfo {
                sample_rate,
                channels,
                bits_per_sample,
                data_bytes,
            },
        )),
        _ => Ok(None),
    }
}

fn read_full<R: Read>(reader: &mut R, buf: &mut [u8]) -> Result<Option<()>> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]) {
            Ok(0) => return Ok(None),
            Ok(n) => filled += n,
            Err(err) => return Err(err.into()),
        }
    }
    Ok(Some(()))
}

fn skip<R: Read>(reader: &mut R, mut bytes: u64) -> Result<()> {
    let mut buf = [0u8; 4096];
    while bytes > 0 {
        let take = bytes.min(buf.len() as u64) as usize;
        reader.read_exact(&mut buf[..take])?;
        bytes -= take as u64;
    }
    Ok(())
}

pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_is_stable_and_hex() {
        let path = std::env::temp_dir().join(format!(
            "srcrepair-hash-{}.bin",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, b"deterministic contents").unwrap();
        let first = sha256_file(&path).unwrap();
        let second = sha256_file(&path).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.len(), 64);
        assert!(first.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn non_wav_returns_none() {
        let path = std::env::temp_dir().join(format!(
            "srcrepair-nowav-{}.bin",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, b"not a wav file at all").unwrap();
        assert!(read_wav_info(&path).unwrap().is_none());
    }
}
