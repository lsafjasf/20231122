use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;

pub fn sha256_file(path: &Path) -> Result<String, String> {
    let data = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let mut h = Sha256::new();
    h.update(&data);
    Ok(format!("{:x}", h.finalize()))
}

/// Deterministic source tone: 440 Hz sine, 0.5 amplitude, 10 ms raised-cosine fades.
pub fn generate_source(path: &Path, rate: u32, secs: f64) -> Result<u64, String> {
    let n = (rate as f64 * secs) as usize;
    let fade = (0.010 * rate as f64) as usize;
    let mut buf = Vec::with_capacity(n * 4);
    for i in 0..n {
        let t = i as f64 / rate as f64;
        let mut s = 0.5f64 * (2.0 * std::f64::consts::PI * 440.0 * t).sin();
        let env = if i < fade {
            0.5 - 0.5 * (std::f64::consts::PI * i as f64 / fade as f64).cos()
        } else if i >= n - fade {
            0.5 - 0.5 * (std::f64::consts::PI * (n - i) as f64 / fade as f64).cos()
        } else {
            1.0
        };
        s *= env;
        buf.extend_from_slice(&(s as f32).to_le_bytes());
    }
    std::fs::write(path, &buf).map_err(|e| format!("write source: {e}"))?;
    Ok(n as u64)
}

pub fn read_f32le(path: &Path) -> Result<Vec<f32>, String> {
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    Ok(data
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect())
}

/// Extract interleaved S16LE samples from the first `data` chunk of a WAV file.
pub fn read_wav_s16(path: &Path) -> Result<Vec<i16>, String> {
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    if data.len() < 12 || &data[0..4] != b"RIFF" {
        return Err("not a RIFF file".into());
    }
    let mut off = 12usize;
    while off + 8 <= data.len() {
        let id = &data[off..off + 4];
        let size = u32::from_le_bytes([data[off + 4], data[off + 5], data[off + 6], data[off + 7]])
            as usize;
        let body = off + 8;
        if id == b"data" {
            let end = (body + size).min(data.len());
            return Ok(data[body..end]
                .chunks_exact(2)
                .map(|c| i16::from_le_bytes([c[0], c[1]]))
                .collect());
        }
        off = body + size + (size & 1);
    }
    Err("no data chunk found".into())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorMetrics {
    pub samples: usize,
    pub err_rms_lsb: f64,
    pub err_peak_lsb: f64,
    pub err_signal_corr: f64,
}

/// Compare exported S16 samples against the float source; errors in LSB units.
pub fn error_metrics(source_f32: &[f32], exported_s16: &[i16]) -> ErrorMetrics {
    let n = source_f32.len().min(exported_s16.len());
    let mut sum_sq = 0.0f64;
    let mut peak = 0.0f64;
    let mut sum_es = 0.0f64;
    let mut sum_e = 0.0f64;
    let mut sum_s = 0.0f64;
    let mut sum_e2 = 0.0f64;
    let mut sum_s2 = 0.0f64;
    for i in 0..n {
        let ideal = source_f32[i] as f64 * 32768.0;
        let e = exported_s16[i] as f64 - ideal;
        sum_sq += e * e;
        peak = peak.max(e.abs());
        sum_es += e * ideal;
        sum_e += e;
        sum_s += ideal;
        sum_e2 += e * e;
        sum_s2 += ideal * ideal;
    }
    let n = n.max(1) as f64;
    let mean_e = sum_e / n;
    let mean_s = sum_s / n;
    let cov = sum_es / n - mean_e * mean_s;
    let var_e = (sum_e2 / n - mean_e * mean_e).max(1e-12);
    let var_s = (sum_s2 / n - mean_s * mean_s).max(1e-12);
    ErrorMetrics {
        samples: n as usize,
        err_rms_lsb: (sum_sq / n).sqrt(),
        err_peak_lsb: peak,
        err_signal_corr: cov / (var_e.sqrt() * var_s.sqrt()),
    }
}
