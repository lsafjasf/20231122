use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::failure::{Failure, Result};
use crate::manifest::LossEvent;

pub const ANALYSIS_RATE: u32 = 48_000;
pub const DEFAULT_MAX_SECONDS: u64 = 60 * 60;

#[derive(Debug, Clone, Serialize)]
pub struct WaveBin {
    pub start_ms: f64,
    pub end_ms: f64,
    pub min: f32,
    pub max: f32,
    pub rms: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct WaveformSummary {
    pub sample_rate: u32,
    pub total_samples: u64,
    pub duration_ms: f64,
    pub bins: Vec<WaveBin>,
    pub peak: f32,
    pub peak_dbfs: f32,
    pub rms_dbfs: f32,
    pub clipped_fraction: f64,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntervalMetrics {
    pub start_ms: f64,
    pub end_ms: f64,
    pub bins: usize,
    pub peak: f32,
    pub peak_dbfs: f32,
    pub rms_dbfs: f32,
    pub silent_fraction: f64,
    pub clipped_fraction: f64,
    pub pre_rms_dbfs: Option<f32>,
    pub post_rms_dbfs: Option<f32>,
    pub delta_db: Option<f32>,
    pub max_step_db: f32,
}

#[derive(Debug, Clone)]
pub struct WaveAccumulator {
    sample_rate: u32,
    bin_samples: u64,
    max_samples: u64,
    total_samples: u64,
    bins: Vec<WaveBin>,
    current_min: f32,
    current_max: f32,
    current_sum_sq: f64,
    current_count: u64,
    peak: f32,
    sum_sq: f64,
    clipped_samples: u64,
    truncated: bool,
}

impl WaveAccumulator {
    pub fn new(sample_rate: u32, max_seconds: u64) -> Self {
        let bin_samples = (sample_rate as u64 / 100).max(1);
        Self {
            sample_rate,
            bin_samples,
            max_samples: sample_rate as u64 * max_seconds,
            total_samples: 0,
            bins: Vec::new(),
            current_min: 1.0,
            current_max: -1.0,
            current_sum_sq: 0.0,
            current_count: 0,
            peak: 0.0,
            sum_sq: 0.0,
            clipped_samples: 0,
            truncated: false,
        }
    }

    pub fn push(&mut self, samples: &[f32]) {
        for &sample in samples {
            if self.total_samples >= self.max_samples {
                self.truncated = true;
                break;
            }
            let sample = if sample.is_finite() { sample } else { 0.0 };
            let absolute = sample.abs();
            self.peak = self.peak.max(absolute);
            self.sum_sq += f64::from(sample) * f64::from(sample);
            if absolute >= 0.999 {
                self.clipped_samples += 1;
            }
            self.current_min = self.current_min.min(sample);
            self.current_max = self.current_max.max(sample);
            self.current_sum_sq += f64::from(sample) * f64::from(sample);
            self.current_count += 1;
            self.total_samples += 1;
            if self.current_count >= self.bin_samples {
                self.flush_bin();
            }
        }
    }

    pub fn is_truncated(&self) -> bool {
        self.truncated
    }

    pub fn finish(mut self) -> WaveformSummary {
        if self.current_count > 0 {
            self.flush_bin();
        }
        let duration_ms = self.total_samples as f64 * 1000.0 / f64::from(self.sample_rate);
        let rms = if self.total_samples == 0 {
            0.0
        } else {
            (self.sum_sq / self.total_samples as f64).sqrt()
        };
        WaveformSummary {
            sample_rate: self.sample_rate,
            total_samples: self.total_samples,
            duration_ms,
            bins: self.bins,
            peak: self.peak,
            peak_dbfs: amplitude_db(self.peak),
            rms_dbfs: amplitude_db(rms as f32),
            clipped_fraction: if self.total_samples == 0 {
                0.0
            } else {
                self.clipped_samples as f64 / self.total_samples as f64
            },
            truncated: self.truncated,
        }
    }

    fn flush_bin(&mut self) {
        if self.current_count == 0 {
            return;
        }
        let start_sample = self.total_samples - self.current_count;
        let end_sample = self.total_samples;
        self.bins.push(WaveBin {
            start_ms: start_sample as f64 * 1000.0 / f64::from(self.sample_rate),
            end_ms: end_sample as f64 * 1000.0 / f64::from(self.sample_rate),
            min: self.current_min,
            max: self.current_max,
            rms: (self.current_sum_sq / self.current_count as f64).sqrt() as f32,
        });
        self.current_min = 1.0;
        self.current_max = -1.0;
        self.current_sum_sq = 0.0;
        self.current_count = 0;
    }
}

impl WaveformSummary {
    pub fn metrics_for(&self, start_ms: f64, end_ms: f64) -> Option<IntervalMetrics> {
        let selected: Vec<_> = self
            .bins
            .iter()
            .filter(|bin| bin.end_ms > start_ms && bin.start_ms < end_ms.max(start_ms))
            .collect();
        if selected.is_empty() {
            return None;
        }

        let mut peak = 0.0_f32;
        let mut sum_sq = 0.0_f64;
        let mut weight = 0_u64;
        let mut silent_weight = 0_u64;
        let mut clipped_weight = 0_u64;
        for bin in &selected {
            let bin_weight = ((bin.end_ms - bin.start_ms) * 1000.0).max(1.0) as u64;
            peak = peak.max(bin.min.abs()).max(bin.max.abs());
            sum_sq += f64::from(bin.rms) * f64::from(bin.rms) * bin_weight as f64;
            weight += bin_weight;
            if bin.rms < 0.001 {
                silent_weight += bin_weight;
            }
            if bin.min.abs() >= 0.999 || bin.max.abs() >= 0.999 {
                clipped_weight += bin_weight;
            }
        }
        let rms = if weight == 0 {
            0.0
        } else {
            (sum_sq / weight as f64).sqrt() as f32
        };
        let pre_rms = self.region_rms((start_ms - 50.0).max(0.0), start_ms);
        let post_rms = self.region_rms(end_ms.max(start_ms), end_ms.max(start_ms) + 50.0);
        let delta_db = pre_rms
            .filter(|value| *value > 1.0e-6)
            .map(|value| amplitude_db(rms) - amplitude_db(value));
        let max_step_db = selected
            .windows(2)
            .map(|pair| (amplitude_db(pair[0].rms) - amplitude_db(pair[1].rms)).abs())
            .fold(0.0, f32::max);

        Some(IntervalMetrics {
            start_ms,
            end_ms,
            bins: selected.len(),
            peak,
            peak_dbfs: amplitude_db(peak),
            rms_dbfs: amplitude_db(rms),
            silent_fraction: if weight == 0 {
                0.0
            } else {
                silent_weight as f64 / weight as f64
            },
            clipped_fraction: if weight == 0 {
                0.0
            } else {
                clipped_weight as f64 / weight as f64
            },
            pre_rms_dbfs: pre_rms.map(amplitude_db),
            post_rms_dbfs: post_rms.map(amplitude_db),
            delta_db,
            max_step_db,
        })
    }

    fn region_rms(&self, start_ms: f64, end_ms: f64) -> Option<f32> {
        if end_ms <= start_ms {
            return None;
        }
        let mut sum_sq = 0.0_f64;
        let mut weight = 0_u64;
        for bin in self
            .bins
            .iter()
            .filter(|bin| bin.end_ms > start_ms && bin.start_ms < end_ms)
        {
            let bin_weight = (bin.end_ms - bin.start_ms).max(1.0) as u64;
            sum_sq += f64::from(bin.rms) * f64::from(bin.rms) * bin_weight as f64;
            weight += bin_weight;
        }
        (weight > 0).then(|| (sum_sq / weight as f64).sqrt() as f32)
    }
}

pub fn amplitude_db(amplitude: f32) -> f32 {
    if amplitude <= 1.0e-6 {
        -120.0
    } else {
        20.0 * amplitude.log10()
    }
}

pub fn write_svg(path: &Path, waveform: &WaveformSummary, events: &[LossEvent]) -> Result<()> {
    let width = 1200.0;
    let height = 280.0;
    let margin = 52.0;
    let plot_width = width - 2.0 * margin;
    let duration = waveform.duration_ms.max(1.0);

    let mut svg = String::new();
    svg.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\">\n"
    ));
    svg.push_str("<rect width=\"100%\" height=\"100%\" fill=\"#111820\"/>\n");
    svg.push_str("<rect x=\"52\" y=\"34\" width=\"1096\" height=\"210\" fill=\"#18232d\" stroke=\"#33444f\"/>\n");
    svg.push_str("<text x=\"52\" y=\"22\" fill=\"#e6edf3\" font-family=\"sans-serif\" font-size=\"13\">Recovered output waveform - mono 48 kHz analysis</text>\n");

    for step in 0..=10 {
        let ratio = step as f64 / 10.0;
        let x = margin + plot_width * ratio;
        let time = duration * ratio / 1000.0;
        svg.push_str(&format!(
            "<line x1=\"{x:.1}\" y1=\"34\" x2=\"{x:.1}\" y2=\"244\" stroke=\"#26343f\"/>\n"
        ));
        svg.push_str(&format!(
            "<text x=\"{x:.1}\" y=\"262\" fill=\"#8fa3b5\" text-anchor=\"middle\" font-family=\"monospace\" font-size=\"10\">{time:.2}s</text>\n"
        ));
    }

    let bins = waveform.bins.len().max(1);
    let step = (bins as f64 / plot_width).max(1.0);
    let mut index = 0;
    while index < waveform.bins.len() {
        let end = ((index as f64 + step) as usize).min(waveform.bins.len());
        let slice = &waveform.bins[index..end];
        let x = margin + plot_width * (index as f64 / bins as f64);
        let min = slice.iter().map(|bin| bin.min).fold(0.0_f32, f32::min);
        let max = slice.iter().map(|bin| bin.max).fold(0.0_f32, f32::max);
        let rms = (slice
            .iter()
            .map(|bin| f64::from(bin.rms) * f64::from(bin.rms))
            .sum::<f64>()
            / slice.len().max(1) as f64)
            .sqrt() as f32;
        let center = 139.0;
        let scale = 98.0;
        svg.push_str(&format!(
            "<line x1=\"{x:.1}\" y1=\"{:.1}\" x2=\"{x:.1}\" y2=\"{:.1}\" stroke=\"#61d0ff\" stroke-width=\"1\" opacity=\"0.8\"/>\n",
            center - max * scale,
            center - min * scale
        ));
        svg.push_str(&format!(
            "<line x1=\"{x:.1}\" y1=\"{:.1}\" x2=\"{x:.1}\" y2=\"{:.1}\" stroke=\"#2ed6a1\" stroke-width=\"1\"/>\n",
            center - rms * scale,
            center + rms * scale
        ));
        index = end;
    }

    for (position, event) in events.iter().enumerate() {
        let x = margin + plot_width * (event.start_ms / duration).clamp(0.0, 1.0);
        let end_x = margin + plot_width * (event.end_or_start_ms() / duration).clamp(0.0, 1.0);
        let color = if position % 2 == 0 {
            "#ffb84d"
        } else {
            "#ff6b8a"
        };
        svg.push_str(&format!(
            "<rect x=\"{x:.1}\" y=\"34\" width=\"{:.1}\" height=\"210\" fill=\"{color}\" opacity=\"0.16\"><title>{} @ {:.3}s; track {:?}; frame {:?}</title></rect>\n",
            (end_x - x).max(1.0),
            escape_xml(&event.id),
            event.start_ms / 1000.0,
            event.track_id,
            event.frame
        ));
        svg.push_str(&format!(
            "<line x1=\"{x:.1}\" y1=\"34\" x2=\"{x:.1}\" y2=\"244\" stroke=\"{color}\" stroke-width=\"1.5\"/>\n"
        ));
        if position < 12 {
            svg.push_str(&format!(
                "<text x=\"{x:.1}\" y=\"{:.1}\" fill=\"{color}\" font-family=\"monospace\" font-size=\"9\">{:.2}s</text>\n",
                42.0 + (position % 4) as f64 * 12.0,
                event.start_ms / 1000.0
            ));
        }
    }
    svg.push_str(&format!(
        "<text x=\"52\" y=\"276\" fill=\"#8fa3b5\" font-family=\"monospace\" font-size=\"10\">duration {:.3}s | peak {:.1} dBFS | RMS {:.1} dBFS | clipping {:.4}%</text>\n",
        waveform.duration_ms / 1000.0,
        waveform.peak_dbfs,
        waveform.rms_dbfs,
        waveform.clipped_fraction * 100.0
    ));
    svg.push_str("</svg>\n");
    fs::write(path, svg).map_err(|e| Failure::io("写入波形 SVG", path, e))
}

fn escape_xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accumulates_bins_and_interval_metrics() {
        let mut accumulator = WaveAccumulator::new(48_000, 10);
        for _ in 0..2400 {
            accumulator.push(&[0.5, -0.5]);
        }
        let summary = accumulator.finish();
        assert!((summary.duration_ms - 100.0).abs() < 0.01);
        assert!(summary.peak > 0.49);
        let metrics = summary.metrics_for(20.0, 60.0).unwrap();
        assert!(metrics.rms_dbfs < -5.0);
        assert_eq!(metrics.clipped_fraction, 0.0);
    }
}
