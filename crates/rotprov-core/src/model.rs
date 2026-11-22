use serde::{Deserialize, Serialize};
use std::fmt;

/// 旋转角度（顺时针，度）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rotation {
    R0,
    R90,
    R180,
    R270,
}

impl Rotation {
    pub fn degrees(self) -> u16 {
        match self {
            Rotation::R0 => 0,
            Rotation::R90 => 90,
            Rotation::R180 => 180,
            Rotation::R270 => 270,
        }
    }

    /// 从 GStreamer image-orientation 标签解析。
    pub fn from_orientation_tag(tag: &str) -> Option<Self> {
        match tag {
            "rotate-0" => Some(Rotation::R0),
            "rotate-90" => Some(Rotation::R90),
            "rotate-180" => Some(Rotation::R180),
            "rotate-270" => Some(Rotation::R270),
            _ => None,
        }
    }

    pub fn to_orientation_tag(self) -> &'static str {
        match self {
            Rotation::R0 => "rotate-0",
            Rotation::R90 => "rotate-90",
            Rotation::R180 => "rotate-180",
            Rotation::R270 => "rotate-270",
        }
    }
}

impl fmt::Display for Rotation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}°", self.degrees())
    }
}

/// 原始输入：sidecar 元数据 + 媒体文件路径。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputManifest {
    pub id: String,
    pub name: String,
    pub media_path: String,
    pub sidecar_path: String,
}

/// sidecar 文件中的原始旋转信息（可能自相矛盾）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RawRotationInfo {
    /// 容器旋转标签，如 "rotate-90"。
    pub orientation_tag: Option<String>,
    /// 显示矩阵推导出的角度（度）。
    pub display_matrix_degrees: Option<i64>,
    /// 采集设备声明的方向（自由文本，仅作证据）。
    pub device_orientation: Option<String>,
}

/// 关键证据：任何结论都可回溯到的原始材料。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evidence {
    pub id: String,
    pub kind: EvidenceKind,
    pub summary: String,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EvidenceKind {
    /// 原始输入文件内容（或其摘要）。
    RawInput,
    /// GStreamer 探测结果（容器类型等）。
    GstProbe,
    /// 规范化后的 GStreamer 标签列表。
    GstTagList,
    /// 冲突双方的原值（用于无法判断的边界）。
    ConflictingValues,
    /// 异常现场。
    ErrorContext,
}

/// 处理步骤（处理记录中的一行）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessingStep {
    pub seq: u32,
    pub name: String,
    pub detail: String,
    pub evidence_ids: Vec<String>,
}

/// 版本快照：某一阶段的对象状态。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub version: u32,
    pub stage: String,
    pub state: ObjectState,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ObjectState {
    pub raw: Option<RawRotationInfo>,
    pub container_type: Option<String>,
    pub normalized_rotation: Option<Rotation>,
    pub matrix_rotation: Option<Rotation>,
}

/// 处理记录。可能被截断；截断时保留原始长度与哈希以便核验。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessRecord {
    pub object_id: String,
    pub steps: Vec<ProcessingStep>,
    pub truncated: bool,
    /// 截断前的完整步骤数。
    pub original_step_count: usize,
    /// 截断前完整记录（JSON 序列化）的 FNV-1a 哈希，十六进制。
    pub original_hash: Option<String>,
}

/// 单个对象的最终结论。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Conclusion {
    /// 需要修复：从 from 归一到 R0。
    Fixed { from: Rotation },
    /// 无需修复。
    NoFixNeeded,
    /// 无法判断的边界：证据冲突，保留双方原值。
    Undecidable { reason: String },
    /// 处理异常（不阻断其余对象）。
    Failed { error: String },
}

impl Conclusion {
    pub fn label(&self) -> String {
        match self {
            Conclusion::Fixed { from } => format!("已修复（{from} → 0°）"),
            Conclusion::NoFixNeeded => "无需修复".to_string(),
            Conclusion::Undecidable { .. } => "无法判断".to_string(),
            Conclusion::Failed { .. } => "处理异常".to_string(),
        }
    }
}

/// 一个对象的完整来源链：结论 → 记录 → 快照 → 证据。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvenanceEntry {
    pub manifest: InputManifest,
    pub conclusion: Conclusion,
    pub record: ProcessRecord,
    pub snapshots: Vec<Snapshot>,
    pub evidence: Vec<Evidence>,
}

/// 整批运行的报告。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunReport {
    pub entries: Vec<ProvenanceEntry>,
    /// 临时工作目录（运行结束后应已清理）。
    pub temp_dir: String,
    /// 临时状态是否已清理。
    pub temp_cleaned: bool,
}
