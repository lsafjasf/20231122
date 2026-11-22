use std::fmt;
use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

#[derive(Clone, Debug)]
pub struct WavData {
    /// Interleaved samples, normalized to [-1, 1].
    pub samples: Vec<f32>,
    pub channels: u16,
    pub sample_rate: u32,
}

impl WavData {
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels as usize
    }

    pub fn duration_ms(&self) -> f64 {
        self.frames() as f64 * 1000.0 / self.sample_rate as f64
    }

    /// Mono mixdown used by the analysis stage.
    pub fn mono(&self) -> Vec<f32> {
        let ch = self.channels as usize;
        if ch == 1 {
            return self.samples.clone();
        }
        self.samples
            .chunks_exact(ch)
            .map(|f| f.iter().sum::<f32>() / ch as f32)
            .collect()
    }
}

#[derive(Debug)]
pub struct WavError(pub String);

impl fmt::Display for WavError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "WAV error: {}", self.0)
    }
}

impl std::error::Error for WavError {}

fn err<T>(msg: impl Into<String>) -> Result<T, WavError> {
    Err(WavError(msg.into()))
}

pub fn read_wav(path: &Path) -> Result<WavData, WavError> {
    let mut f = File::open(path).map_err(|e| WavError(e.to_string()))?;
    let mut riff = [0u8; 12];
    f.read_exact(&mut riff)
        .map_err(|e| WavError(e.to_string()))?;
    if &riff[0..4] != b"RIFF" || &riff[8..12] != b"WAVE" {
        return err("not a RIFF/WAVE file");
    }
    let mut fmt_tag = 0u16;
    let mut channels = 0u16;
    let mut sample_rate = 0u32;
    let mut bits = 0u16;
    let mut data: Vec<u8> = Vec::new();
    loop {
        let mut hdr = [0u8; 8];
        if f.read_exact(&mut hdr).is_err() {
            break;
        }
        let size = u32::from_le_bytes(hdr[4..8].try_into().unwrap()) as u64;
        match &hdr[0..4] {
            b"fmt " => {
                let mut buf = vec![0u8; size as usize];
                f.read_exact(&mut buf)
                    .map_err(|e| WavError(e.to_string()))?;
                fmt_tag = u16::from_le_bytes(buf[0..2].try_into().unwrap());
                channels = u16::from_le_bytes(buf[2..4].try_into().unwrap());
                sample_rate = u32::from_le_bytes(buf[4..8].try_into().unwrap());
                bits = u16::from_le_bytes(buf[14..16].try_into().unwrap());
            }
            b"data" => {
                data = vec![0u8; size as usize];
                f.read_exact(&mut data)
                    .map_err(|e| WavError(e.to_string()))?;
            }
            _ => {
                f.seek(SeekFrom::Current(size as i64 + (size as i64 & 1)))
                    .map_err(|e| WavError(e.to_string()))?;
            }
        }
    }
    if channels == 0 || data.is_empty() {
        return err("missing fmt/data chunk");
    }
    let samples: Vec<f32> = match (fmt_tag, bits) {
        (1, 16) => data
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
            .collect(),
        (1, 24) => data
            .chunks_exact(3)
            .map(|b| {
                let v = (b[0] as i32) | ((b[1] as i32) << 8) | ((b[2] as i32) << 16);
                ((v << 8) >> 8) as f32 / 8388608.0
            })
            .collect(),
        (1, 32) => data
            .chunks_exact(4)
            .map(|b| i32::from_le_bytes(b.try_into().unwrap()) as f32 / 2147483648.0)
            .collect(),
        (3, 32) => data
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
            .collect(),
        _ => return err(format!("unsupported format tag={fmt_tag} bits={bits}")),
    };
    Ok(WavData {
        samples,
        channels,
        sample_rate,
    })
}

pub fn write_wav(path: &Path, wav: &WavData) -> Result<(), WavError> {
    let mut f = BufWriter::new(File::create(path).map_err(|e| WavError(e.to_string()))?);
    let ch = wav.channels;
    let data_len = (wav.samples.len() * 2) as u32;
    let mut w = |bytes: &[u8]| -> Result<(), WavError> {
        f.write_all(bytes).map_err(|e| WavError(e.to_string()))
    };
    w(b"RIFF")?;
    w(&(36u32 + data_len).to_le_bytes())?;
    w(b"WAVE")?;
    w(b"fmt ")?;
    w(&16u32.to_le_bytes())?;
    w(&1u16.to_le_bytes())?;
    w(&ch.to_le_bytes())?;
    w(&wav.sample_rate.to_le_bytes())?;
    w(&(wav.sample_rate * ch as u32 * 2).to_le_bytes())?;
    w(&(ch * 2).to_le_bytes())?;
    w(&16u16.to_le_bytes())?;
    w(b"data")?;
    w(&data_len.to_le_bytes())?;
    for &s in &wav.samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
        w(&v.to_le_bytes())?;
    }
    Ok(())
}
