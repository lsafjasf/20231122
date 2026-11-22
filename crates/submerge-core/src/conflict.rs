use crate::event::Language;
use crate::fmt;
use std::fmt as stdfmt;

/// Category of a timeline problem attached to one or two events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConflictKind {
    /// Two cues occupy the same unified time range.
    Overlap,
    /// Suspiciously long silence between consecutive cues (possible missing cue).
    Gap,
    /// Cue falls outside the video's [0, duration) range.
    OutOfRange,
}

impl ConflictKind {
    pub fn label(&self) -> &'static str {
        match self {
            ConflictKind::Overlap => "重叠",
            ConflictKind::Gap => "缺失",
            ConflictKind::OutOfRange => "超出范围",
        }
    }
}

/// A detected problem on the unified timeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub kind: ConflictKind,
    /// Ids of the events involved (1 for Gap/OutOfRange, 2 for Overlap).
    pub event_ids: Vec<usize>,
    /// Human-readable reason, e.g. "zh#2 与 zh#3 重叠 0.917s".
    pub reason: String,
}

impl Conflict {
    pub fn overlap(
        a_id: usize,
        a_lang: Language,
        b_id: usize,
        b_lang: Language,
        overlap_ns: u64,
    ) -> Self {
        let scope = if a_lang == b_lang {
            "同语言"
        } else {
            "跨语言"
        };
        Conflict {
            kind: ConflictKind::Overlap,
            event_ids: vec![a_id, b_id],
            reason: format!(
                "{}#{} 与 {}#{} {}重叠 {}",
                a_lang.code(),
                a_id,
                b_lang.code(),
                b_id,
                scope,
                fmt::ns_to_duration(overlap_ns),
            ),
        }
    }

    pub fn gap(after_id: usize, lang: Language, gap_ns: u64, threshold_ns: u64) -> Self {
        Conflict {
            kind: ConflictKind::Gap,
            event_ids: vec![after_id],
            reason: format!(
                "{}#{} 之前空档 {} 超过阈值 {}，可能缺失字幕",
                lang.code(),
                after_id,
                fmt::ns_to_duration(gap_ns),
                fmt::ns_to_duration(threshold_ns),
            ),
        }
    }

    pub fn out_of_range(id: usize, lang: Language, end_ns: u64, video_ns: u64) -> Self {
        Conflict {
            kind: ConflictKind::OutOfRange,
            event_ids: vec![id],
            reason: format!(
                "{}#{} 结束于 {}，超出视频时长 {}",
                lang.code(),
                id,
                fmt::ns_to_timecode(end_ns),
                fmt::ns_to_timecode(video_ns),
            ),
        }
    }
}

impl stdfmt::Display for Conflict {
    fn fmt(&self, f: &mut stdfmt::Formatter<'_>) -> stdfmt::Result {
        write!(f, "[{}] {}", self.kind.label(), self.reason)
    }
}
