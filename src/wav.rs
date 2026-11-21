//! Minimal PCM16 WAV reader/writer (no external deps).

use anyhow::{bail, Context, Result};

#[derive(Debug, Clone, Copy)]
pub struct WavSpec {
    pub sample_rate: u32,
    pub channels: usize,
}

#[derive(Debug, Clone)]
pub struct Wav {
    pub spec: WavSpec,
    /// Interleaved samples normalized to [-1.0, 1.0].
    pub samples: Vec<f32>,
}

impl Wav {
    pub fn frames(&self) -> usize {
        self.samples.len() / self.spec.channels.max(1)
    }

    pub fn channel(&self, ch: usize) -> Vec<f32> {
        self.samples
            .iter()
            .skip(ch)
            .step_by(self.spec.channels)
            .copied()
            .collect()
    }

    pub fn read(path: &std::path::Path) -> Result<Wav> {
        let data = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
        Self::parse(&data)
    }

    pub fn parse(data: &[u8]) -> Result<Wav> {
        if data.len() < 12 || &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" {
            bail!("not a RIFF/WAVE file");
        }
        let mut pos = 12usize;
        let mut fmt: Option<(u16, u16, u32, u16)> = None;
        let mut pcm: Option<&[u8]> = None;
        while pos + 8 <= data.len() {
            let id = &data[pos..pos + 4];
            let size = u32::from_le_bytes(data[pos + 4..pos + 8].try_into().unwrap()) as usize;
            let body_start = pos + 8;
            let body_end = (body_start + size).min(data.len());
            let body = &data[body_start..body_end];
            match id {
                b"fmt " => {
                    if body.len() < 16 {
                        bail!("short fmt chunk");
                    }
                    let format = u16::from_le_bytes(body[0..2].try_into().unwrap());
                    let channels = u16::from_le_bytes(body[2..4].try_into().unwrap());
                    let rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
                    let bits = u16::from_le_bytes(body[14..16].try_into().unwrap());
                    fmt = Some((format, channels, rate, bits));
                }
                b"data" => pcm = Some(body),
                _ => {}
            }
            pos = body_start + size + (size & 1);
        }
        let (format, channels, rate, bits) = fmt.context("missing fmt chunk")?;
        if format != 1 || bits != 16 {
            bail!("only 16-bit PCM WAV supported (got format={format} bits={bits})");
        }
        let pcm = pcm.context("missing data chunk")?;
        let mut samples = Vec::with_capacity(pcm.len() / 2);
        for pair in pcm.chunks_exact(2) {
            let v = i16::from_le_bytes([pair[0], pair[1]]);
            samples.push(v as f32 / 32768.0);
        }
        Ok(Wav {
            spec: WavSpec {
                sample_rate: rate,
                channels: channels as usize,
            },
            samples,
        })
    }

    pub fn write(&self, path: &std::path::Path) -> Result<()> {
        let ch = self.spec.channels as u16;
        let byte_rate = self.spec.sample_rate * ch as u32 * 2;
        let block_align = ch * 2;
        let data_len = (self.samples.len() * 2) as u32;
        let mut out = Vec::with_capacity(44 + data_len as usize);
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data_len).to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&ch.to_le_bytes());
        out.extend_from_slice(&self.spec.sample_rate.to_le_bytes());
        out.extend_from_slice(&byte_rate.to_le_bytes());
        out.extend_from_slice(&block_align.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data_len.to_le_bytes());
        for &s in &self.samples {
            let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
            out.extend_from_slice(&v.to_le_bytes());
        }
        std::fs::write(path, &out).with_context(|| format!("write {}", path.display()))
    }
}
