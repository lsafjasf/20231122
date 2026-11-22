//! `band-energy-v1` content fingerprint.
//!
//! PCM is mono signed 16-bit little endian. Each analysis frame is
//! `frame_us` long with `hop_us` spacing. A frame yields:
//!
//! * `rms` — mean square amplitude on the 0..=65535 scale, zero for silence;
//! * ten `bands` — log-scaled spectral energy bins quantized to 0..=255.
//!
//! The implementation is pure integer/float math with no RNG or platform
//! dependent state, so re-decoding the same window always reproduces the
//! exact same fingerprint bytes.

use crate::digest::{digest_canonical, sha256_hex_bytes};
use crate::model::FingerprintFrame;

pub const ALGORITHM: &str = "band-energy-v1";
pub const BAND_COUNT: usize = 10;

#[derive(Debug, Clone)]
pub struct FingerprintParams {
    pub sample_rate_hz: u32,
    pub frame_us: i64,
    pub hop_us: i64,
}

impl FingerprintParams {
    pub fn strict() -> Self {
        Self {
            sample_rate_hz: 8000,
            frame_us: 100_000,
            hop_us: 50_000,
        }
    }

    pub fn reduced(reduced_rate: u32, reduced_frame_us: i64) -> Self {
        Self {
            sample_rate_hz: reduced_rate,
            frame_us: reduced_frame_us,
            hop_us: reduced_frame_us / 2,
        }
    }

    fn frame_len(&self) -> usize {
        (self.sample_rate_hz as i64 * self.frame_us / 1_000_000) as usize
    }

    fn hop_len(&self) -> usize {
        (self.sample_rate_hz as i64 * self.hop_us / 1_000_000) as usize
    }
}

#[derive(Debug, Clone)]
pub struct ComputedFingerprint {
    pub frames: Vec<FingerprintFrame>,
    pub pcm_sha256: String,
    pub fingerprint_sha256: String,
    pub feature_count: usize,
}

pub fn compute(pcm_s16le: &[u8], params: &FingerprintParams) -> ComputedFingerprint {
    let pcm_sha256 = sha256_hex_bytes(pcm_s16le);
    let samples = bytes_to_i16(pcm_s16le);
    let frame_len = params.frame_len();
    let hop_len = params.hop_len();

    let mut raw_frames: Vec<(i64, u32, Vec<f64>)> = Vec::new();
    if frame_len > 0 && hop_len > 0 {
        let mut start = 0usize;
        while start + frame_len <= samples.len() {
            let window = &samples[start..start + frame_len];
            let offset_us = (start / hop_len) as i64 * params.hop_us;
            let (rms, energies) = analyse_frame(window);
            raw_frames.push((offset_us, rms, energies));
            start += hop_len;
        }
    }

    // One global reference for the whole window keeps band magnitudes
    // comparable across frames and forces true silence to stay at zero.
    let global_peak = raw_frames
        .iter()
        .flat_map(|(_, _, energies)| energies.iter().copied())
        .fold(1f64, f64::max);
    let frames: Vec<FingerprintFrame> = raw_frames
        .into_iter()
        .map(|(offset_us, rms, energies)| FingerprintFrame {
            offset_us,
            rms,
            bands: energies
                .into_iter()
                .map(|energy| {
                    if energy <= 0.0 {
                        0u8
                    } else {
                        ((energy / global_peak).sqrt() * 255.0)
                            .round()
                            .clamp(0.0, 255.0) as u8
                    }
                })
                .collect(),
        })
        .collect();

    let feature_count = frames.len();
    let fingerprint_sha256 = digest_canonical(&frames);
    ComputedFingerprint {
        frames,
        pcm_sha256,
        fingerprint_sha256,
        feature_count,
    }
}

fn analyse_frame(window: &[i16]) -> (u32, Vec<f64>) {
    let rms = rms_scale(window);
    if rms == 0 {
        return (0, vec![0f64; BAND_COUNT]);
    }

    // Hann window reduces spectral leakage before the naive DFT.
    let n = window.len();
    let mut re = vec![0f64; n];
    let im = vec![0f64; n];
    for (i, sample) in window.iter().enumerate() {
        let hann = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / (n as f64 - 1.0)).cos();
        re[i] = f64::from(*sample) * hann;
    }

    // Only the first half of the spectrum carries independent information for
    // real input. Bins are grouped into ten logarithmically spaced bands.
    let nyquist = n / 2;
    let mut energies = vec![0f64; BAND_COUNT];
    for bin in 1..=nyquist {
        let mut sum_re = 0f64;
        let mut sum_im = 0f64;
        let angle = -2.0 * std::f64::consts::PI * bin as f64 / n as f64;
        for t in 0..n {
            let phase = angle * t as f64;
            sum_re += re[t] * phase.cos() - im[t] * phase.sin();
            sum_im += re[t] * phase.sin() + im[t] * phase.cos();
        }
        let magnitude = (sum_re * sum_re + sum_im * sum_im).sqrt();
        let band = ((bin as f64 / nyquist as f64).log10() * (BAND_COUNT as f64))
            .floor()
            .clamp(0.0, (BAND_COUNT - 1) as f64) as usize;
        energies[band] += magnitude;
    }
    (rms, energies)
}

fn rms_scale(window: &[i16]) -> u32 {
    if window.is_empty() {
        return 0;
    }
    let sum_sq: f64 = window
        .iter()
        .map(|sample| {
            let v = f64::from(*sample);
            v * v
        })
        .sum();
    let rms = (sum_sq / window.len() as f64).sqrt();
    (rms.round().clamp(0.0, 65535.0)) as u32
}

fn bytes_to_i16(bytes: &[u8]) -> Vec<i16> {
    bytes
        .chunks_exact(2)
        .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_is_all_zero() {
        let params = FingerprintParams::strict();
        let pcm = vec![0u8; params.frame_len() * 4 * 2];
        let fp = compute(&pcm, &params);
        assert_eq!(fp.feature_count, 7);
        assert!(fp.frames.iter().all(|f| f.rms == 0 && f.bands.iter().all(|b| *b == 0)));
    }

    #[test]
    fn deterministic_and_offset_ordered() {
        let params = FingerprintParams::strict();
        let pcm: Vec<u8> = (0..params.frame_len() * 4)
            .map(|i| ((i as i32 * 97) % 8000) as i16)
            .flat_map(i16::to_le_bytes)
            .collect();
        let first = compute(&pcm, &params);
        let second = compute(&pcm, &params);
        assert_eq!(first.frames, second.frames);
        assert_eq!(first.fingerprint_sha256, second.fingerprint_sha256);
        let offsets: Vec<i64> = first.frames.iter().map(|f| f.offset_us).collect();
        assert_eq!(offsets, vec![0, 50_000, 100_000, 150_000, 200_000, 250_000, 300_000]);
    }
}
