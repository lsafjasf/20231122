use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::mask::MaskConfig;
use crate::timecode::{format_signed_offset, format_timecode, NANOS_PER_SECOND};

pub const SCHEMA_VERSION: u32 = 1;
pub const SEGMENT_COUNT: usize = 3;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OriginalMaterial {
    pub source_path: String,
    pub stored_relative_path: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub imported_at: String,
    pub read_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MediaInfo {
    pub duration_ns: u64,
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    pub container: String,
    pub video_codec: String,
    pub probe_engine: String,
}

impl MediaInfo {
    pub fn frame_label(&self, ns: u64) -> String {
        crate::timecode::frame_number(ns, self.fps_num, self.fps_den)
            .map(|frame| format!("帧 {frame}"))
            .unwrap_or_else(|| "帧率未知".to_string())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    Succeeded,
    Failed,
    Interrupted,
}

impl RunStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Running => "运行中",
            Self::Succeeded => "已完成",
            Self::Failed => "失败",
            Self::Interrupted => "已中断",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvidenceFrame {
    pub kind: String,
    pub timestamp_ns: u64,
    pub timestamp_timecode: String,
    pub source_offset_ns: i64,
    pub source_offset: String,
    pub relative_path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RunRecord {
    pub id: String,
    pub number: u32,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub status: RunStatus,
    pub anchor_ns: u64,
    pub in_offset_ns: i64,
    pub out_offset_ns: i64,
    pub range_start_ns: u64,
    pub range_end_ns: u64,
    #[serde(default)]
    pub measured_start_ns: Option<u64>,
    #[serde(default)]
    pub measured_end_ns: Option<u64>,
    pub mask: MaskConfig,
    pub source_sha256: String,
    pub output_relative_path: Option<String>,
    pub output_sha256: Option<String>,
    pub evidence: Vec<EvidenceFrame>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExportRecord {
    pub id: String,
    pub at: String,
    pub segment_index: usize,
    pub run_id: Option<String>,
    pub output_path: String,
    pub output_sha256: String,
    pub start_ns: u64,
    pub end_ns: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Segment {
    pub index: usize,
    pub label: String,
    pub anchor_ns: u64,
    pub in_offset_ns: i64,
    pub out_offset_ns: i64,
    pub mask: MaskConfig,
    pub notes: String,
    pub runs: Vec<RunRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SegmentRange {
    pub anchor_ns: u64,
    pub in_offset_ns: i64,
    pub out_offset_ns: i64,
    pub requested_start_ns: i64,
    pub requested_end_ns: i64,
    pub start_ns: u64,
    pub end_ns: u64,
    pub start_clamped: bool,
    pub end_clamped: bool,
}

impl SegmentRange {
    pub fn duration_ns(&self) -> u64 {
        self.end_ns.saturating_sub(self.start_ns)
    }

    pub fn start_timecode(&self) -> String {
        format_timecode(self.start_ns)
    }

    pub fn end_timecode(&self) -> String {
        format_timecode(self.end_ns)
    }

    pub fn start_offset_label(&self) -> String {
        format_signed_offset(self.in_offset_ns)
    }

    pub fn end_offset_label(&self) -> String {
        format_signed_offset(self.out_offset_ns)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectSession {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    pub project_id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    pub original: OriginalMaterial,
    pub media: MediaInfo,
    pub segments: Vec<Segment>,
    pub review_position_ns: u64,
    pub selected_segment: usize,
    #[serde(default)]
    pub exports: Vec<ExportRecord>,
    #[serde(skip, default)]
    pub project_dir: PathBuf,
}

fn default_schema_version() -> u32 {
    SCHEMA_VERSION
}

impl ProjectSession {
    pub fn new(
        title: String,
        original: OriginalMaterial,
        media: MediaInfo,
        project_dir: PathBuf,
    ) -> Self {
        let now = crate::evidence::now_rfc3339();
        let segments = default_segments(media.duration_ns);
        let review_position_ns = segments[0].anchor_ns;
        Self {
            schema_version: SCHEMA_VERSION,
            project_id: Uuid::new_v4().to_string(),
            title,
            created_at: now.clone(),
            updated_at: now,
            original,
            media,
            segments,
            review_position_ns,
            selected_segment: 0,
            exports: Vec::new(),
            project_dir,
        }
    }

    pub fn segment(&self, index: usize) -> Result<&Segment, String> {
        self.segments
            .get(index)
            .ok_or_else(|| format!("片段 {} 不存在", index + 1))
    }

    pub fn segment_mut(&mut self, index: usize) -> Result<&mut Segment, String> {
        self.segments
            .get_mut(index)
            .ok_or_else(|| format!("片段 {} 不存在", index + 1))
    }

    pub fn range_for(&self, segment_index: usize) -> Result<SegmentRange, String> {
        let segment = self.segment(segment_index)?;
        segment.range(self.media.duration_ns)
    }

    pub fn last_successful_run(&self, segment_index: usize) -> Option<&RunRecord> {
        self.segments
            .get(segment_index)?
            .runs
            .iter()
            .rev()
            .find(|run| run.status == RunStatus::Succeeded)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.segments.len() != SEGMENT_COUNT {
            return Err(format!(
                "项目必须恰好包含 {SEGMENT_COUNT} 个片段，当前为 {}",
                self.segments.len()
            ));
        }
        if self.media.duration_ns == 0 {
            return Err("媒体时长不能为 0".to_string());
        }
        for (index, segment) in self.segments.iter().enumerate() {
            if segment.index != index {
                return Err(format!(
                    "片段顺序损坏：索引 {} 不等于 {index}",
                    segment.index
                ));
            }
            let range = segment.range(self.media.duration_ns)?;
            if range.end_ns <= range.start_ns {
                return Err(format!(
                    "片段 {} 的入点必须早于出点（{} → {}）",
                    index + 1,
                    range.start_timecode(),
                    range.end_timecode()
                ));
            }
        }
        Ok(())
    }

    pub fn mark_abandoned_runs_interrupted(&mut self) -> usize {
        let mut count = 0;
        for segment in &mut self.segments {
            for run in &mut segment.runs {
                if run.status == RunStatus::Running {
                    run.status = RunStatus::Interrupted;
                    run.finished_at = Some(crate::evidence::now_rfc3339());
                    run.error = Some("应用在任务结束前退出；已保留该次运行记录和现有证据".into());
                    count += 1;
                }
            }
        }
        count
    }
}

impl Segment {
    pub fn range(&self, duration_ns: u64) -> Result<SegmentRange, String> {
        let requested_start = self.anchor_ns as i128 + self.in_offset_ns as i128;
        let requested_end = self.anchor_ns as i128 + self.out_offset_ns as i128;
        if requested_end <= requested_start {
            return Err(format!(
                "片段 {} 的偏移无效：{} → {}",
                self.index + 1,
                format_signed_offset(self.in_offset_ns),
                format_signed_offset(self.out_offset_ns)
            ));
        }
        let start = requested_start.clamp(0, duration_ns as i128) as u64;
        let end = requested_end.clamp(0, duration_ns as i128) as u64;
        if end <= start {
            return Err(format!("片段 {} 的有效范围为空", self.index + 1));
        }
        Ok(SegmentRange {
            anchor_ns: self.anchor_ns,
            in_offset_ns: self.in_offset_ns,
            out_offset_ns: self.out_offset_ns,
            requested_start_ns: requested_start as i64,
            requested_end_ns: requested_end as i64,
            start_ns: start,
            end_ns: end,
            start_clamped: requested_start != start as i128,
            end_clamped: requested_end != end as i128,
        })
    }
}

fn default_segments(duration_ns: u64) -> Vec<Segment> {
    let span = (duration_ns / 10).clamp(1, NANOS_PER_SECOND);
    (0..SEGMENT_COUNT)
        .map(|index| {
            let numerator = (index as u64 * 2 + 1) as u128;
            let anchor_ns = (duration_ns as u128 * numerator / (SEGMENT_COUNT as u128 * 2)) as u64;
            Segment {
                index,
                label: format!("片段 {}", index + 1),
                anchor_ns,
                in_offset_ns: -(span.min(anchor_ns) as i64),
                out_offset_ns: span.min(duration_ns.saturating_sub(anchor_ns)) as i64,
                mask: MaskConfig::default(),
                notes: String::new(),
                runs: Vec::new(),
            }
        })
        .collect()
}
