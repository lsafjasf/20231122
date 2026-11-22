use crate::fps::FrameRate;
use std::fmt;

/// Subtitle language track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Language {
    Zh,
    En,
    Ja,
}

impl Language {
    pub fn code(&self) -> &'static str {
        match self {
            Language::Zh => "zh",
            Language::En => "en",
            Language::Ja => "ja",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Language::Zh => "中文",
            Language::En => "English",
            Language::Ja => "日本語",
        }
    }
}

impl fmt::Display for Language {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}({})", self.code(), self.label())
    }
}

/// One subtitle cue, timed in frames of its own source frame rate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubtitleEvent {
    pub id: usize,
    pub language: Language,
    pub text: String,
    /// Inclusive start frame in `source_fps` units.
    pub start_frame: u64,
    /// Exclusive end frame in `source_fps` units.
    pub end_frame: u64,
    pub source_fps: FrameRate,
}

impl SubtitleEvent {
    pub fn new(
        id: usize,
        language: Language,
        text: impl Into<String>,
        start_frame: u64,
        end_frame: u64,
        source_fps: FrameRate,
    ) -> Self {
        assert!(start_frame < end_frame, "event must have positive duration");
        Self {
            id,
            language,
            text: text.into(),
            start_frame,
            end_frame,
            source_fps,
        }
    }

    /// Start on the unified nanosecond timeline.
    pub fn unified_start_ns(&self) -> u64 {
        self.source_fps.frame_to_ns(self.start_frame)
    }

    /// End on the unified nanosecond timeline.
    pub fn unified_end_ns(&self) -> u64 {
        self.source_fps.frame_to_ns(self.end_frame)
    }

    pub fn unified_duration_ns(&self) -> u64 {
        self.unified_end_ns() - self.unified_start_ns()
    }

    /// Original timing, human readable: `frames [a, b) @ 25fps (TC .. TC)`.
    pub fn original_timing(&self) -> String {
        format!(
            "frames [{}, {}) @ {} ({} - {})",
            self.start_frame,
            self.end_frame,
            self.source_fps,
            self.source_fps.frame_to_timecode(self.start_frame),
            self.source_fps.frame_to_timecode(self.end_frame),
        )
    }

    /// Converted timing on the unified timeline.
    pub fn unified_timing(&self) -> String {
        format!(
            "{} - {}",
            crate::fmt::ns_to_timecode(self.unified_start_ns()),
            crate::fmt::ns_to_timecode(self.unified_end_ns()),
        )
    }
}
