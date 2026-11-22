use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::timecode::{format_clock_time, FramePosition, DEFAULT_FPS};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectFormat {
    LegacyV1,
    RollV2,
    BareMedia,
}

impl ProjectFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LegacyV1 => "legacy_v1",
            Self::RollV2 => "roll_v2",
            Self::BareMedia => "bare_media",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TypesetParameters {
    pub font_family: String,
    pub font_size: f64,
    pub color: String,
    pub shadow: bool,
    pub scroll_speed_px_s: f64,
    pub roll_start_s: f64,
    pub horizontal_margin: f64,
    pub line_spacing: f64,
    pub opacity: f64,
}

impl Default for TypesetParameters {
    fn default() -> Self {
        Self {
            font_family: "Sans".to_string(),
            font_size: 34.0,
            color: "#FFFFFFFF".to_string(),
            shadow: true,
            scroll_speed_px_s: 72.0,
            roll_start_s: 0.0,
            horizontal_margin: 0.08,
            line_spacing: 1.25,
            opacity: 1.0,
        }
    }
}

impl TypesetParameters {
    pub fn normalized(mut self) -> Self {
        self.font_size = self.font_size.clamp(6.0, 240.0);
        self.scroll_speed_px_s = self.scroll_speed_px_s.clamp(0.0, 4000.0);
        self.roll_start_s = self.roll_start_s.max(0.0);
        self.horizontal_margin = self.horizontal_margin.clamp(0.0, 0.45);
        self.line_spacing = self.line_spacing.clamp(0.8, 4.0);
        self.opacity = self.opacity.clamp(0.0, 1.0);
        self.color = normalize_color(&self.color);
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Checkpoint {
    pub seconds: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_frame: Option<u64>,
}

impl Checkpoint {
    pub fn new(seconds: f64) -> Self {
        Self {
            seconds,
            requested: None,
            label: None,
            track: None,
            requested_frame: None,
        }
    }

    pub fn display_time(&self) -> String {
        format_clock_time(self.seconds)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MediaObject {
    pub id: String,
    pub path: PathBuf,
    pub track: String,
    pub start_s: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_s: Option<f64>,
    #[serde(default)]
    pub checkpoints: Vec<Checkpoint>,
    #[serde(default)]
    pub parameters: TypesetParameters,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credits_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_note: Option<String>,
}

impl MediaObject {
    pub fn resolved_checkpoints(&self, fps: f64) -> Vec<Checkpoint> {
        if !self.checkpoints.is_empty() {
            return self
                .checkpoints
                .iter()
                .map(|checkpoint| {
                    let mut checkpoint = checkpoint.clone();
                    if let Some(frame) = checkpoint.requested_frame {
                        if checkpoint.seconds <= 0.0 {
                            checkpoint.seconds = frame as f64 / fps.max(1.0);
                        }
                    }
                    checkpoint.seconds = checkpoint.seconds.max(0.0);
                    checkpoint
                })
                .collect();
        }

        let duration = self.duration_s.unwrap_or(4.0).max(0.04);
        let midpoint = self.start_s + duration * 0.5;
        vec![Checkpoint {
            seconds: midpoint,
            requested: Some("auto:midpoint".to_string()),
            label: Some("midpoint".to_string()),
            track: None,
            requested_frame: None,
        }]
    }

    pub fn effective_text(&self) -> String {
        self.credits_text
            .clone()
            .filter(|text| !text.trim().is_empty())
            .unwrap_or_else(|| format!("{}\n\nDirected by\nRolling Credits", self.id))
    }

    pub fn frame_at(&self, seconds: f64, fps: f64) -> FramePosition {
        FramePosition::from_seconds(seconds, fps).unwrap_or(FramePosition {
            frame: 0,
            fps_num: fps.max(1.0).round() as u32,
            fps_den: 1,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Project {
    pub schema: String,
    pub title: String,
    pub format: ProjectFormat,
    pub fps: f64,
    pub defaults: TypesetParameters,
    pub objects: Vec<MediaObject>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

impl Project {
    pub fn new(format: ProjectFormat, title: impl Into<String>, fps: f64) -> Self {
        Self {
            schema: "credits-review/normalized-project-v1".to_string(),
            title: title.into(),
            format,
            fps: sane_fps(fps),
            defaults: TypesetParameters::default(),
            objects: Vec::new(),
            warnings: Vec::new(),
        }
    }
}

pub fn sane_fps(fps: f64) -> f64 {
    if fps.is_finite() && (1.0..=240.0).contains(&fps) {
        fps
    } else {
        DEFAULT_FPS
    }
}

pub fn is_known_media_extension(extension: &str) -> bool {
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "mp4"
            | "m4v"
            | "mov"
            | "mkv"
            | "webm"
            | "avi"
            | "mpg"
            | "mpeg"
            | "ts"
            | "m2ts"
            | "mts"
            | "wav"
            | "mp3"
            | "flac"
            | "ogg"
            | "oga"
            | "opus"
            | "aac"
            | "m4a"
            | "wma"
            | "aiff"
            | "y4m"
    )
}

pub fn normalize_color(raw: &str) -> String {
    let value = raw.trim();
    let without_hash = value.strip_prefix('#').unwrap_or(value);
    match without_hash.len() {
        3 => {
            let mut expanded = String::from("#");
            for byte in without_hash.chars() {
                expanded.push(byte);
                expanded.push(byte);
            }
            expanded.push_str("FF");
            expanded.to_ascii_uppercase()
        }
        6 => format!("#{}FF", without_hash.to_ascii_uppercase()),
        8 => format!("#{}", without_hash.to_ascii_uppercase()),
        _ => "#FFFFFFFF".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_colors() {
        assert_eq!(normalize_color("#abc"), "#AABBCCFF");
        assert_eq!(normalize_color("112233"), "#112233FF");
        assert_eq!(normalize_color("#11223344"), "#11223344");
    }

    #[test]
    fn unknown_extensions_are_rejected() {
        assert!(!is_known_media_extension("rollunk"));
        assert!(is_known_media_extension("MKV"));
    }
}
