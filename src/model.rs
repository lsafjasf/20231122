use std::fmt;

use anyhow::{Context, Result, bail};
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const SCHEMA_VERSION: u32 = 1;
pub const TOOL_NAME: &str = "ClockBridge";
pub const TOOL_VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn new_id(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4().simple())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClockOffset {
    pub milliseconds: i64,
    pub source: String,
}

impl ClockOffset {
    pub fn manual(milliseconds: i64) -> Self {
        Self {
            milliseconds,
            source: "manual".to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub role: String,
    pub media_path: String,
    pub clock_offset: ClockOffset,
    pub notes: String,
}

impl Device {
    pub fn new(id: &str, name: &str, role: &str, media_path: &str, offset_ms: i64) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            role: role.to_string(),
            media_path: media_path.to_string(),
            clock_offset: ClockOffset::manual(offset_ms),
            notes: String::new(),
        }
    }

    pub fn timeline_to_device_ms(&self, timeline_ms: i64) -> i64 {
        timeline_ms.saturating_add(self.clock_offset.milliseconds)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissingSegment {
    pub label: String,
    pub timeline_start_ms: i64,
    pub duration_ms: i64,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceBinding {
    pub device_id: String,
    pub device_name: String,
    pub media_path: String,
    pub clock_offset_ms: i64,
    pub resolved_start_ms: i64,
    pub resolved_end_ms: i64,
}

impl SourceBinding {
    pub fn from_device(device: &Device, segment: &MissingSegment) -> Self {
        let resolved_start_ms = device.timeline_to_device_ms(segment.timeline_start_ms);
        let resolved_end_ms = resolved_start_ms.saturating_add(segment.duration_ms);
        Self {
            device_id: device.id.clone(),
            device_name: device.name.clone(),
            media_path: device.media_path.clone(),
            clock_offset_ms: device.clock_offset.milliseconds,
            resolved_start_ms,
            resolved_end_ms,
        }
    }

    pub fn clamped_start_ms(&self) -> i64 {
        self.resolved_start_ms.max(0)
    }

    pub fn clamped_duration_ms(&self) -> i64 {
        (self.resolved_end_ms - self.clamped_start_ms()).max(0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptStatus {
    Running,
    Succeeded,
    Failed,
    Interrupted,
}

impl AttemptStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Running => "运行中",
            Self::Succeeded => "成功",
            Self::Failed => "失败",
            Self::Interrupted => "中断",
        }
    }

    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Running)
    }
}

impl fmt::Display for AttemptStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttemptRecord {
    pub id: String,
    pub attempt_no: u32,
    pub segment_id: String,
    pub device_id: String,
    pub device_name: String,
    pub started_at: String,
    pub finished_at: String,
    pub status: AttemptStatus,
    pub source_path: String,
    pub resolved_start_ms: i64,
    pub duration_ms: i64,
    pub pipeline: String,
    pub output_path: String,
    pub output_size_bytes: Option<u64>,
    pub output_sha256: Option<String>,
    pub source_size_bytes: Option<u64>,
    pub source_sha256: Option<String>,
    pub wall_time_ms: u64,
    pub error: Option<String>,
    pub evidence_file: String,
}

impl AttemptRecord {
    pub fn new_pending(
        segment: &Segment,
        binding: &SourceBinding,
        output_path: &str,
        evidence_file: &str,
    ) -> Self {
        Self {
            id: new_id("attempt"),
            attempt_no: segment.next_attempt_no(),
            segment_id: segment.id.clone(),
            device_id: binding.device_id.clone(),
            device_name: binding.device_name.clone(),
            started_at: now_rfc3339(),
            finished_at: String::new(),
            status: AttemptStatus::Running,
            source_path: binding.media_path.clone(),
            resolved_start_ms: binding.clamped_start_ms(),
            duration_ms: binding.clamped_duration_ms(),
            pipeline: String::new(),
            output_path: output_path.to_string(),
            output_size_bytes: None,
            output_sha256: None,
            source_size_bytes: None,
            source_sha256: None,
            wall_time_ms: 0,
            error: None,
            evidence_file: evidence_file.to_string(),
        }
    }

    pub fn finish(&mut self, status: AttemptStatus, error: Option<String>) {
        self.status = status;
        self.finished_at = now_rfc3339();
        self.error = error;
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComparisonRecord {
    pub id: String,
    pub created_at: String,
    pub left_attempt_id: String,
    pub right_attempt_id: String,
    pub left_status: AttemptStatus,
    pub right_status: AttemptStatus,
    pub left_size_bytes: Option<u64>,
    pub right_size_bytes: Option<u64>,
    pub size_delta_bytes: Option<i64>,
    pub left_duration_ms: i64,
    pub right_duration_ms: i64,
    pub checksum_equal: bool,
    pub same_source: bool,
    pub verdict: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Segment {
    pub id: String,
    pub ordinal: u32,
    pub label: String,
    pub timeline_start_ms: i64,
    pub duration_ms: i64,
    pub reason: String,
    pub sources: Vec<SourceBinding>,
    pub attempts: Vec<AttemptRecord>,
    pub comparisons: Vec<ComparisonRecord>,
    pub selected_attempt_id: Option<String>,
}

impl Segment {
    pub fn from_missing(ordinal: u32, missing: &MissingSegment, devices: &[Device]) -> Self {
        let sources = devices
            .iter()
            .map(|device| SourceBinding::from_device(device, missing))
            .collect();
        Self {
            id: new_id("segment"),
            ordinal,
            label: missing.label.clone(),
            timeline_start_ms: missing.timeline_start_ms,
            duration_ms: missing.duration_ms,
            reason: missing.reason.clone(),
            sources,
            attempts: Vec::new(),
            comparisons: Vec::new(),
            selected_attempt_id: None,
        }
    }

    pub fn next_attempt_no(&self) -> u32 {
        self.attempts
            .iter()
            .map(|attempt| attempt.attempt_no)
            .max()
            .unwrap_or(0)
            + 1
    }

    pub fn selected_attempt(&self) -> Option<&AttemptRecord> {
        let selected = self.selected_attempt_id.as_deref()?;
        self.attempts.iter().find(|attempt| attempt.id == selected)
    }

    pub fn latest_attempts(&self) -> Vec<&AttemptRecord> {
        let mut latest = Vec::new();
        for source in &self.sources {
            if let Some(attempt) = self
                .attempts
                .iter()
                .rev()
                .find(|attempt| attempt.device_id == source.device_id)
            {
                latest.push(attempt);
            }
        }
        latest
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceEvent {
    pub seq: u64,
    pub at: String,
    pub kind: String,
    pub segment_id: Option<String>,
    pub attempt_id: Option<String>,
    pub summary: String,
    pub details: Vec<String>,
}

impl EvidenceEvent {
    pub fn new(
        seq: u64,
        kind: &str,
        segment_id: Option<String>,
        attempt_id: Option<String>,
        summary: &str,
        details: Vec<String>,
    ) -> Self {
        Self {
            seq,
            at: now_rfc3339(),
            kind: kind.to_string(),
            segment_id,
            attempt_id,
            summary: summary.to_string(),
            details,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    pub schema_version: u32,
    pub workspace_id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    pub revision: u64,
    pub devices: Vec<Device>,
    pub missing_segment: Option<MissingSegment>,
    pub segments: Vec<Segment>,
}

impl Workspace {
    pub fn new(title: &str, devices: Vec<Device>) -> Self {
        let now = now_rfc3339();
        Self {
            schema_version: SCHEMA_VERSION,
            workspace_id: new_id("workspace"),
            title: title.to_string(),
            created_at: now.clone(),
            updated_at: now,
            revision: 1,
            devices,
            missing_segment: None,
            segments: Vec::new(),
        }
    }

    pub fn demo(device_a_path: &str, device_b_path: &str) -> Self {
        let mut workspace = Self::new(
            "双机时钟缺失片段 · 现场修正",
            vec![
                Device::new("device-a", "设备 A · 主机", "主时钟", device_a_path, 0),
                Device::new("device-b", "设备 B · 辅机", "跟随", device_b_path, 1_250),
            ],
        );
        let missing = MissingSegment {
            label: "GAP-001".to_string(),
            timeline_start_ms: 2_800,
            duration_ms: 1_600,
            reason:
                "B 机时钟快 1.25 s；切点后主轨出现 1.6 s 缺失，需要 Align 后单独重跑并留存证据。"
                    .to_string(),
        };
        workspace.set_missing_segment(missing);
        workspace
    }

    pub fn device(&self, device_id: &str) -> Option<&Device> {
        self.devices.iter().find(|device| device.id == device_id)
    }

    pub fn segment(&self, segment_id: &str) -> Option<&Segment> {
        self.segments
            .iter()
            .find(|segment| segment.id == segment_id)
    }

    pub fn segment_mut(&mut self, segment_id: &str) -> Option<&mut Segment> {
        self.segments
            .iter_mut()
            .find(|segment| segment.id == segment_id)
    }

    pub fn set_missing_segment(&mut self, missing: MissingSegment) {
        self.missing_segment = Some(missing.clone());
        if let Some(segment) = self.segments.first_mut() {
            segment.label = missing.label.clone();
            segment.timeline_start_ms = missing.timeline_start_ms;
            segment.duration_ms = missing.duration_ms;
            segment.reason = missing.reason.clone();
            segment.sources = self
                .devices
                .iter()
                .map(|device| SourceBinding::from_device(device, &missing))
                .collect();
        } else {
            self.segments
                .push(Segment::from_missing(1, &missing, &self.devices));
        }
        self.touch();
    }

    pub fn refresh_sources(&mut self) {
        let Some(missing) = self.missing_segment.clone() else {
            return;
        };
        for segment in &mut self.segments {
            segment.sources = self
                .devices
                .iter()
                .map(|device| SourceBinding::from_device(device, &missing))
                .collect();
        }
        self.touch();
    }

    pub fn touch(&mut self) {
        self.revision = self.revision.saturating_add(1);
        self.updated_at = now_rfc3339();
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != SCHEMA_VERSION {
            bail!(
                "unsupported workspace schema {} (expected {})",
                self.schema_version,
                SCHEMA_VERSION
            );
        }
        if self.devices.len() != 2 {
            bail!("workspace must contain exactly two devices");
        }
        let mut device_ids = std::collections::HashSet::new();
        for device in &self.devices {
            if device.id.trim().is_empty() || !device_ids.insert(&device.id) {
                bail!("device ids must be non-empty and unique");
            }
        }
        let mut segment_ids = std::collections::HashSet::new();
        let mut ordinals = std::collections::HashSet::new();
        for segment in &self.segments {
            if segment.id.trim().is_empty() || !segment_ids.insert(&segment.id) {
                bail!("segment ids must be non-empty and unique");
            }
            if !ordinals.insert(segment.ordinal) {
                bail!("segment ordinals must be unique");
            }
            if segment.duration_ms <= 0 || segment.timeline_start_ms < 0 {
                bail!("segment {} has an invalid time range", segment.label);
            }
            if segment.sources.len() != 2 {
                bail!("segment {} must bind both devices", segment.label);
            }
            let mut attempt_ids = std::collections::HashSet::new();
            for attempt in &segment.attempts {
                if !attempt_ids.insert(&attempt.id) {
                    bail!("duplicate attempt id in segment {}", segment.label);
                }
                if attempt.segment_id != segment.id {
                    bail!("attempt {} belongs to another segment", attempt.id);
                }
            }
            if let Some(selected) = &segment.selected_attempt_id
                && !attempt_ids.contains(selected)
            {
                bail!("selected attempt is missing from segment {}", segment.label);
            }
        }
        Ok(())
    }
}

impl Default for Workspace {
    fn default() -> Self {
        Self::new("未命名修正工作区", Vec::new())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewManifest {
    pub schema_version: u32,
    pub tool_name: String,
    pub tool_version: String,
    pub exported_at: String,
    pub bundle_id: String,
    pub workspace: Workspace,
    pub evidence_events: Vec<EvidenceEvent>,
    pub exported_files: Vec<ExportedFile>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportedFile {
    pub order: u32,
    pub path: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub purpose: String,
}

pub fn count_attempts(workspace: &Workspace) -> usize {
    workspace
        .segments
        .iter()
        .map(|segment| segment.attempts.len())
        .sum()
}

pub fn validate_media_path(path: &str) -> Result<()> {
    if path.trim().is_empty() {
        bail!("未选择媒体文件");
    }
    let metadata = std::fs::metadata(path).with_context(|| format!("无法读取媒体文件 {path}"))?;
    if !metadata.is_file() {
        bail!("媒体路径不是文件: {path}");
    }
    Ok(())
}

pub fn format_time_ms(ms: i64) -> String {
    let sign = if ms < 0 { "-" } else { "" };
    let absolute = ms.unsigned_abs();
    let hours = absolute / 3_600_000;
    let minutes = (absolute % 3_600_000) / 60_000;
    let seconds = (absolute % 60_000) / 1_000;
    let millis = absolute % 1_000;
    format!("{sign}{hours:02}:{minutes:02}:{seconds:02}.{millis:03}")
}
