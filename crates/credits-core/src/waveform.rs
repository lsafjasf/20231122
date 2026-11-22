use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::timecode::format_clock_time;
use crate::{CoreError, Result};

pub const WAVEFORM_SCHEMA: &str = "credits-review/waveform-v1";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Peak {
    pub min: f32,
    pub max: f32,
    pub rms: f32,
}

impl Peak {
    pub fn new(min: f32, max: f32, rms: f32) -> Self {
        Self {
            min: min.clamp(-1.0, 1.0),
            max: max.clamp(-1.0, 1.0),
            rms: rms.clamp(0.0, 1.0),
        }
    }

    pub fn silence() -> Self {
        Self {
            min: 0.0,
            max: 0.0,
            rms: 0.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WaveformMarker {
    pub seconds: f64,
    pub peak_index: usize,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WaveformEvidence {
    pub schema: String,
    pub available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub sample_rate: u32,
    pub bucket_ms: f64,
    pub duration_s: f64,
    pub peaks: Vec<Peak>,
    pub markers: Vec<WaveformMarker>,
}

impl WaveformEvidence {
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            schema: WAVEFORM_SCHEMA.to_string(),
            available: false,
            reason: Some(reason.into()),
            sample_rate: 0,
            bucket_ms: 0.0,
            duration_s: 0.0,
            peaks: Vec::new(),
            markers: Vec::new(),
        }
    }

    pub fn new(sample_rate: u32, bucket_ms: f64, duration_s: f64, peaks: Vec<Peak>) -> Self {
        Self {
            schema: WAVEFORM_SCHEMA.to_string(),
            available: !peaks.is_empty(),
            reason: None,
            sample_rate,
            bucket_ms,
            duration_s,
            peaks,
            markers: Vec::new(),
        }
    }

    pub fn add_marker(&mut self, seconds: f64, label: Option<String>) {
        let peak_index = self.peak_index(seconds);
        self.markers.push(WaveformMarker {
            seconds,
            peak_index,
            label,
        });
    }

    pub fn peak_index(&self, seconds: f64) -> usize {
        if self.peaks.is_empty() || self.bucket_ms <= 0.0 {
            return 0;
        }
        let index = (seconds.max(0.0) * 1000.0 / self.bucket_ms).floor() as usize;
        index.min(self.peaks.len() - 1)
    }

    pub fn window_around(&self, seconds: f64, radius: usize) -> &[Peak] {
        if self.peaks.is_empty() {
            return &[];
        }
        let center = self.peak_index(seconds);
        let start = center.saturating_sub(radius);
        let end = (center + radius + 1).min(self.peaks.len());
        &self.peaks[start..end]
    }

    pub fn to_svg(&self, title: &str) -> String {
        const WIDTH: f64 = 960.0;
        const HEIGHT: f64 = 220.0;
        const PAD: f64 = 24.0;
        let plot_width = WIDTH - PAD * 2.0;
        let plot_height = HEIGHT - PAD * 2.0;
        let middle = PAD + plot_height * 0.5;
        let mut svg = String::with_capacity(16_384);
        svg.push_str(&format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{WIDTH:.0}\" height=\"{HEIGHT:.0}\" viewBox=\"0 0 {WIDTH:.0} {HEIGHT:.0}\">\n"
        ));
        svg.push_str("<rect width=\"100%\" height=\"100%\" fill=\"#10151d\"/>\n");
        svg.push_str(&format!(
            "<text x=\"{PAD:.0}\" y=\"18\" fill=\"#d9e2ef\" font-family=\"sans-serif\" font-size=\"12\">{}</text>\n",
            escape_xml(title)
        ));
        svg.push_str(&format!(
            "<line x1=\"{PAD:.1}\" y1=\"{middle:.1}\" x2=\"{:.1}\" y2=\"{middle:.1}\" stroke=\"#334155\" stroke-width=\"1\"/>\n",
            WIDTH - PAD
        ));
        if self.peaks.is_empty() {
            svg.push_str(&format!(
                "<text x=\"{:.1}\" y=\"{:.1}\" fill=\"#94a3b8\" font-family=\"sans-serif\" font-size=\"14\" text-anchor=\"middle\">no waveform evidence</text>\n",
                WIDTH * 0.5,
                HEIGHT * 0.5
            ));
        } else {
            let step = plot_width / self.peaks.len().max(1) as f64;
            for (index, peak) in self.peaks.iter().enumerate() {
                let x = PAD + step * index as f64 + step * 0.5;
                let y1 = middle - f64::from(peak.max) * plot_height * 0.48;
                let y2 = middle - f64::from(peak.min) * plot_height * 0.48;
                svg.push_str(&format!(
                    "<line x1=\"{x:.2}\" y1=\"{y1:.2}\" x2=\"{x:.2}\" y2=\"{y2:.2}\" stroke=\"#38bdf8\" stroke-width=\"{:.2}\"/>\n",
                    (step * 0.8).clamp(0.4, 2.0)
                ));
            }
        }
        if self.duration_s > 0.0 {
            for marker in &self.markers {
                let fraction = (marker.seconds / self.duration_s).clamp(0.0, 1.0);
                let x = PAD + plot_width * fraction;
                svg.push_str(&format!(
                    "<line x1=\"{x:.1}\" y1=\"{PAD:.1}\" x2=\"{x:.1}\" y2=\"{:.1}\" stroke=\"#f59e0b\" stroke-width=\"1.5\" stroke-dasharray=\"4 3\"/>\n",
                    HEIGHT - PAD
                ));
                let label = marker
                    .label
                    .clone()
                    .unwrap_or_else(|| format_clock_time(marker.seconds));
                svg.push_str(&format!(
                    "<text x=\"{x:.1}\" y=\"{:.1}\" fill=\"#fbbf24\" font-family=\"sans-serif\" font-size=\"10\" text-anchor=\"middle\">{}</text>\n",
                    HEIGHT - 8.0,
                    escape_xml(&label)
                ));
            }
        }
        svg.push_str("</svg>\n");
        svg
    }

    pub fn write_json(&self, path: &Path) -> Result<()> {
        let mut file = std::fs::File::create(path).map_err(|source| CoreError::Write {
            path: path.to_path_buf(),
            source,
        })?;
        serde_json::to_writer_pretty(&mut file, self)?;
        file.write_all(b"\n").map_err(|source| CoreError::Write {
            path: path.to_path_buf(),
            source,
        })
    }

    pub fn write_svg(&self, path: &Path, title: &str) -> Result<()> {
        std::fs::write(path, self.to_svg(title)).map_err(|source| CoreError::Write {
            path: path.to_path_buf(),
            source,
        })
    }
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_seconds_to_peak_index() {
        let waveform = WaveformEvidence::new(8_000, 10.0, 2.0, vec![Peak::silence(); 200]);
        assert_eq!(waveform.peak_index(1.0), 100);
        assert_eq!(waveform.peak_index(99.0), 199);
    }

    #[test]
    fn renders_svg_without_invalid_numbers() {
        let mut waveform = WaveformEvidence::new(8_000, 10.0, 1.0, vec![Peak::new(-0.5, 0.5, 0.2)]);
        waveform.add_marker(0.5, Some("mid".to_string()));
        let svg = waveform.to_svg("Credits & roll");
        assert!(svg.contains("Credits &amp; roll"));
        assert!(!svg.contains("NaN"));
    }
}
