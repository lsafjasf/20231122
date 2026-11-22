//! Durable domain model for local queue-priority corrections.

use crate::util::now_rfc3339;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const SCHEMA_VERSION: u32 = 1;
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceId {
    A,
    B,
}

impl DeviceId {
    pub fn label(self) -> &'static str {
        match self {
            Self::A => "设备 A",
            Self::B => "设备 B",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    pub id: DeviceId,
    pub name: String,
    /// Device clock reading minus shared reference clock. Positive means fast.
    pub clock_offset_ms: i64,
    pub clock_note: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DuplicateObject {
    pub object_id: String,
    pub label: String,
    pub note: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentStatus {
    Pending,
    Running,
    Ready,
    Failed,
}

impl SegmentStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pending => "待处理",
            Self::Running => "运行中",
            Self::Ready => "已生成",
            Self::Failed => "失败",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    /// Stable, one-based display order. Never re-sorted when priorities change.
    pub ordinal: u32,
    pub id: String,
    pub label: String,
    pub device: DeviceId,
    pub source_path: String,
    pub source_sha256: Option<String>,
    pub capture_start_ms: i64,
    pub in_ms: i64,
    pub out_ms: i64,
    pub base_priority: i32,
    pub priority_override: Option<i32>,
    pub status: SegmentStatus,
    pub latest_run_id: Option<String>,
    pub correction_note: String,
}

impl Segment {
    pub fn effective_priority(&self) -> i32 {
        self.priority_override.unwrap_or(self.base_priority)
    }

    pub fn normalized_start_ms(&self, offset_ms: i64) -> i64 {
        self.capture_start_ms - offset_ms + self.in_ms
    }

    pub fn duration_ms(&self) -> i64 {
        (self.out_ms - self.in_ms).max(0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Succeeded,
    Failed,
    Interrupted,
}

impl RunStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Succeeded => "成功",
            Self::Failed => "失败",
            Self::Interrupted => "中断",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunRecord {
    pub run_id: String,
    pub segment_id: String,
    pub attempt: u32,
    #[serde(default)]
    pub in_ms: i64,
    #[serde(default)]
    pub out_ms: i64,
    #[serde(default)]
    pub device_offset_ms: i64,
    #[serde(default)]
    pub effective_priority: i32,
    pub status: RunStatus,
    pub started_at: String,
    pub finished_at: String,
    pub duration_ms: u64,
    pub input_path: String,
    pub input_sha256: Option<String>,
    pub output_path: Option<String>,
    pub output_sha256: Option<String>,
    pub output_bytes: Option<u64>,
    pub preview_path: Option<String>,
    pub pipeline: String,
    pub gst_version: String,
    pub tool_version: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    SessionCreated,
    SessionLoaded,
    RunInterrupted,
    DeviceOffsetChanged,
    PriorityCorrected,
    RunStarted,
    RunFinished,
    ExportCreated,
}

impl EvidenceKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::SessionCreated => "会话创建",
            Self::SessionLoaded => "会话载入",
            Self::RunInterrupted => "中断恢复",
            Self::DeviceOffsetChanged => "时钟修正",
            Self::PriorityCorrected => "优先级修正",
            Self::RunStarted => "片段开跑",
            Self::RunFinished => "片段结束",
            Self::ExportCreated => "导出复核包",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceField {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceEvent {
    pub seq: u64,
    pub at: String,
    pub kind: EvidenceKind,
    pub segment_id: Option<String>,
    pub run_id: Option<String>,
    pub summary: String,
    /// Ordered key/value pairs. Do not replace with a map: field order is evidence.
    pub fields: Vec<EvidenceField>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueueItem {
    pub rank: u32,
    pub segment_id: String,
    pub effective_priority: i32,
    pub normalized_start_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub schema_version: u32,
    pub session_id: String,
    pub app_version: String,
    pub created_at: String,
    pub updated_at: String,
    pub duplicate_object: DuplicateObject,
    pub devices: [Device; 2],
    pub segments: Vec<Segment>,
    pub runs: Vec<RunRecord>,
    pub evidence: Vec<EvidenceEvent>,
    pub next_evidence_seq: u64,
    pub next_run_seq: u64,
}

impl Session {
    pub fn device(&self, id: DeviceId) -> &Device {
        self.devices
            .iter()
            .find(|device| device.id == id)
            .expect("both device slots are always present")
    }

    pub fn device_mut(&mut self, id: DeviceId) -> &mut Device {
        self.devices
            .iter_mut()
            .find(|device| device.id == id)
            .expect("both device slots are always present")
    }

    pub fn offset_for(&self, id: DeviceId) -> i64 {
        self.device(id).clock_offset_ms
    }

    pub fn normalized_start_ms(&self, segment: &Segment) -> i64 {
        segment.normalized_start_ms(self.offset_for(segment.device))
    }

    /// Higher priority first; corrected start time and original ordinal break ties.
    /// The ordinal is the final key so repeated recomputation is stable.
    pub fn queue(&self) -> Vec<QueueItem> {
        let mut items: Vec<QueueItem> = self
            .segments
            .iter()
            .map(|segment| QueueItem {
                rank: 0,
                segment_id: segment.id.clone(),
                effective_priority: segment.effective_priority(),
                normalized_start_ms: self.normalized_start_ms(segment),
            })
            .collect();
        items.sort_by(|left, right| {
            let left_segment = self
                .segments
                .iter()
                .find(|segment| segment.id == left.segment_id)
                .expect("queue items originate from segments");
            let right_segment = self
                .segments
                .iter()
                .find(|segment| segment.id == right.segment_id)
                .expect("queue items originate from segments");
            right
                .effective_priority
                .cmp(&left.effective_priority)
                .then_with(|| left.normalized_start_ms.cmp(&right.normalized_start_ms))
                .then_with(|| left_segment.ordinal.cmp(&right_segment.ordinal))
        });
        for (index, item) in items.iter_mut().enumerate() {
            item.rank = index as u32 + 1;
        }
        items
    }

    pub fn queue_ranks(&self) -> HashMap<String, u32> {
        self.queue()
            .into_iter()
            .map(|item| (item.segment_id, item.rank))
            .collect()
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

    pub fn runs_for(&self, segment_id: &str) -> Vec<&RunRecord> {
        self.runs
            .iter()
            .filter(|run| run.segment_id == segment_id)
            .collect()
    }

    pub fn latest_run(&self, segment_id: &str) -> Option<&RunRecord> {
        self.runs
            .iter()
            .rev()
            .find(|run| run.segment_id == segment_id)
    }

    pub fn set_device_offset(&mut self, id: DeviceId, offset_ms: i64, note: &str) {
        let previous = self.device(id).clock_offset_ms;
        self.device_mut(id).clock_offset_ms = offset_ms;
        self.touch();
        self.append_evidence(
            EvidenceKind::DeviceOffsetChanged,
            None,
            None,
            format!("{} 时钟偏移 {} ms → {} ms", id.label(), previous, offset_ms),
            vec![
                field("device", id.label()),
                field("previous_offset_ms", previous),
                field("new_offset_ms", offset_ms),
                field("sign_convention", "offset = device clock - shared clock"),
                field("note", note),
            ],
        );
    }

    pub fn set_priority_override(
        &mut self,
        segment_id: &str,
        priority: Option<i32>,
        note: &str,
    ) -> Result<(), String> {
        let Some(segment) = self.segment_mut(segment_id) else {
            return Err(format!("未知片段：{segment_id}"));
        };
        let previous = segment.effective_priority();
        segment.priority_override = priority;
        segment.correction_note = note.to_owned();
        let effective = segment.effective_priority();
        self.touch();
        self.append_evidence(
            EvidenceKind::PriorityCorrected,
            Some(segment_id.to_owned()),
            None,
            format!("优先级 {previous} → {effective}"),
            vec![
                field("previous_effective_priority", previous),
                field(
                    "priority_override",
                    priority.map_or_else(|| "none".to_owned(), |value| value.to_string()),
                ),
                field("effective_priority", effective),
                field("note", note),
            ],
        );
        Ok(())
    }

    pub fn append_evidence(
        &mut self,
        kind: EvidenceKind,
        segment_id: Option<String>,
        run_id: Option<String>,
        summary: impl Into<String>,
        fields: Vec<EvidenceField>,
    ) {
        let seq = self.next_evidence_seq;
        self.next_evidence_seq += 1;
        self.evidence.push(EvidenceEvent {
            seq,
            at: now_rfc3339(),
            kind,
            segment_id,
            run_id,
            summary: summary.into(),
            fields,
        });
    }

    pub fn touch(&mut self) {
        self.updated_at = now_rfc3339();
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(format!(
                "会话 schema {} 不受支持（当前 {}）",
                self.schema_version, SCHEMA_VERSION
            ));
        }
        if self.devices[0].id != DeviceId::A || self.devices[1].id != DeviceId::B {
            return Err("设备槽位必须是 A、B 且顺序固定".to_owned());
        }
        let mut ids = std::collections::HashSet::new();
        let mut ordinals = std::collections::HashSet::new();
        for segment in &self.segments {
            if !ids.insert(segment.id.as_str()) {
                return Err(format!("重复的片段 ID：{}", segment.id));
            }
            if !ordinals.insert(segment.ordinal) {
                return Err(format!("重复的片段顺序号：{}", segment.ordinal));
            }
            if segment.out_ms < segment.in_ms {
                return Err(format!("片段 {} 的出点早于入点", segment.id));
            }
        }
        let mut run_ids = std::collections::HashSet::new();
        for run in &self.runs {
            if !run_ids.insert(run.run_id.as_str()) {
                return Err(format!("重复的运行 ID：{}", run.run_id));
            }
            if !ids.contains(run.segment_id.as_str()) {
                return Err(format!(
                    "运行 {} 引用了不存在的片段 {}",
                    run.run_id, run.segment_id
                ));
            }
            if run.out_ms < run.in_ms && run.out_ms != 0 {
                return Err(format!("运行 {} 的区间无效", run.run_id));
            }
        }
        let mut previous_seq = None;
        for event in &self.evidence {
            if let Some(previous) = previous_seq
                && event.seq <= previous
            {
                return Err(format!(
                    "证据序号必须严格递增：{} 位于 {} 之后",
                    event.seq, previous
                ));
            }
            previous_seq = Some(event.seq);
        }
        if let Some(maximum) = self.evidence.iter().map(|event| event.seq).max()
            && self.next_evidence_seq <= maximum
        {
            return Err("next_evidence_seq 未超过现有证据序号".to_owned());
        }
        let next_run_id = format!("run-{:04}", self.next_run_seq);
        if self.runs.iter().any(|run| run.run_id == next_run_id) {
            return Err(format!("next_run_seq 会生成重复的运行 ID：{next_run_id}"));
        }
        Ok(())
    }

    /// A process can die while a segment is marked as running. The next launch
    /// records that fact without fabricating a successful run.
    pub fn recover_interrupted(&mut self) {
        let interrupted: Vec<String> = self
            .segments
            .iter()
            .filter(|segment| segment.status == SegmentStatus::Running)
            .map(|segment| segment.id.clone())
            .collect();
        if interrupted.is_empty() {
            return;
        }
        for segment_id in &interrupted {
            if let Some(segment) = self.segment_mut(segment_id) {
                segment.status = SegmentStatus::Pending;
            }
        }
        self.touch();
        self.append_evidence(
            EvidenceKind::RunInterrupted,
            None,
            None,
            format!("检测到 {} 个运行中片段，已恢复为待处理", interrupted.len()),
            vec![
                field("segments", interrupted.join(",")),
                field("recovery", "running → pending; no success record created"),
            ],
        );
    }
}

pub fn field(name: impl Into<String>, value: impl ToString) -> EvidenceField {
    EvidenceField {
        name: name.into(),
        value: value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_is_stable_and_override_is_local() {
        let mut session = crate::demo::seed_session(std::path::Path::new("/tmp/qp-test"));
        let initial: Vec<String> = session
            .queue()
            .into_iter()
            .map(|item| item.segment_id)
            .collect();
        assert_eq!(
            initial,
            vec!["seg-01", "seg-02", "seg-03", "seg-04", "seg-05", "seg-06"]
        );
        let ordinal_before: Vec<u32> = session
            .segments
            .iter()
            .map(|segment| segment.ordinal)
            .collect();
        session
            .set_priority_override("seg-04", Some(100), "test")
            .unwrap();
        let after = session.queue();
        assert_eq!(after[0].segment_id, "seg-04");
        assert_eq!(after[0].effective_priority, 100);
        let ordinal_after: Vec<u32> = session
            .segments
            .iter()
            .map(|segment| segment.ordinal)
            .collect();
        assert_eq!(ordinal_before, ordinal_after);
        assert_eq!(session.segment("seg-04").unwrap().base_priority, 25);
    }

    #[test]
    fn corrected_time_uses_signed_device_offset() {
        let session = crate::demo::seed_session(std::path::Path::new("/tmp/qp-test"));
        let seg_01 = session.segment("seg-01").unwrap();
        let seg_02 = session.segment("seg-02").unwrap();
        assert_eq!(session.normalized_start_ms(seg_01), 500);
        assert_eq!(session.normalized_start_ms(seg_02), 1_700);
    }

    #[test]
    fn interrupted_segment_is_recovered_without_success() {
        let mut session = crate::demo::seed_session(std::path::Path::new("/tmp/qp-test"));
        session.segment_mut("seg-02").unwrap().status = SegmentStatus::Running;
        let runs_before = session.runs.len();
        session.recover_interrupted();
        assert_eq!(
            session.segment("seg-02").unwrap().status,
            SegmentStatus::Pending
        );
        assert_eq!(session.runs.len(), runs_before);
        assert_eq!(
            session.evidence.last().unwrap().kind,
            EvidenceKind::RunInterrupted
        );
    }

    #[test]
    fn json_roundtrip_keeps_segment_and_evidence_field_order() {
        let mut session = crate::demo::seed_session(std::path::Path::new("/tmp/qp-test"));
        session.set_device_offset(DeviceId::A, 1_250, "roundtrip");
        let json = serde_json::to_string(&session).unwrap();
        let reloaded: Session = serde_json::from_str(&json).unwrap();
        assert_eq!(session.segments, reloaded.segments);
        assert_eq!(session.evidence, reloaded.evidence);
        let event = reloaded.evidence.last().unwrap();
        let names: Vec<&str> = event
            .fields
            .iter()
            .map(|field| field.name.as_str())
            .collect();
        assert_eq!(
            names,
            vec![
                "device",
                "previous_offset_ms",
                "new_offset_ms",
                "sign_convention",
                "note"
            ]
        );
    }
}
