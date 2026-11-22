//! 领域模型：输入对象、版本快照、检查结论与证据链。
//!
//! 设计约束：每一条 [`Conclusion`] 都必须能回溯到
//! 1. 原始输入（[`SourceObject`] 的路径、大小、内容指纹）；
//! 2. 两个版本快照（[`Snapshot`]，含指纹与处理步骤）；
//! 3. 产生结论的具体证据（[`Evidence`]）。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 单个待检查对象的来源类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceKind {
    /// 可被 GStreamer 解码的音频文件。
    Audio,
    /// 扩展名未知、无法确认媒体类型的文件。
    UnknownExtension,
    /// 路径不存在或不可读。
    Missing,
}

/// 原始输入对象的登记信息（不复制文件内容，只记录可复核的指纹）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceObject {
    /// 编号，在同一批次内唯一，用于证据交叉引用。
    pub id: u32,
    /// 用户提供的原始路径，保持原样记录。
    pub raw_path: String,
    /// 规范化后的绝对路径；规范化失败时回退为原始路径。
    pub absolute_path: PathBuf,
    pub kind: SourceKind,
    /// 文件字节数；不可读取时为 None。
    pub size_bytes: Option<u64>,
    /// SHA-256 十六进制摘要；不可读取时为 None。
    pub sha256: Option<String>,
    /// 扩展名（不含点，小写）；无扩展名为 None。
    pub extension: Option<String>,
}

/// 处理管线的单个步骤，按执行顺序留痕。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineStep {
    /// 步骤序号，从 1 开始。
    pub order: u32,
    /// GStreamer 元素或内部处理环节的名称。
    pub stage: String,
    /// 人类可读的说明。
    pub detail: String,
    /// 该步骤观测到的关键数据（如采样率、声道数、帧数）。
    pub observation: Option<String>,
}

/// 一个版本的快照：对同一原始输入的两次独立分析结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    /// 快照版本标签（如 `v1-decoded` / `v2-resampled`）。
    pub label: String,
    /// 快照产生时刻（UTC RFC3339）。
    pub taken_at: String,
    /// 该快照所分析字节流的指纹，用于证明确实来自原始输入。
    pub input_sha256: String,
    /// 采样率；解码失败时为 None。
    pub sample_rate: Option<u32>,
    /// 声道数；解码失败时为 None。
    pub channels: Option<u32>,
    /// 单声道化后的样本帧数。
    pub frame_count: Option<u64>,
    /// 本快照使用的处理步骤。
    pub steps: Vec<PipelineStep>,
}

/// 淡化判定结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verdict {
    /// 淡入与淡出均存在。
    FadeInAndOut,
    /// 仅淡入。
    FadeInOnly,
    /// 仅淡出。
    FadeOutOnly,
    /// 未检测到淡化。
    NoFade,
    /// 证据处于判定阈值的边界带内，无法做出确定结论。
    Inconclusive,
    /// 输入不可分析（未知扩展、解码失败等）。
    NotAnalyzable,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::FadeInAndOut => "淡入+淡出",
            Verdict::FadeInOnly => "仅淡入",
            Verdict::FadeOutOnly => "仅淡出",
            Verdict::NoFade => "无淡化",
            Verdict::Inconclusive => "无法判断",
            Verdict::NotAnalyzable => "不可分析",
        }
    }
}

/// 支撑结论的关键证据，直接引用样本窗口的测量值。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evidence {
    /// 证据名称（如 `head_rms_ratio`）。
    pub metric: String,
    /// 测量值。
    pub value: f64,
    /// 判定所用阈值；边界情形同时给出上下沿。
    pub threshold: String,
    /// 对应的样本窗口（起始帧..结束帧）。
    pub window_frames: Option<(u64, u64)>,
    /// 来自哪个快照版本。
    pub snapshot: String,
    /// 人类可读的解释。
    pub note: String,
}

/// 单个对象的最终检查结论及其完整溯源链。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conclusion {
    pub object_id: u32,
    pub path: String,
    pub verdict: Verdict,
    /// 两个版本快照；不可分析对象可能为空或仅含登记快照。
    pub snapshots: Vec<Snapshot>,
    /// 关键证据列表，结论为“无法判断”时至少保留一条边界证据。
    pub evidence: Vec<Evidence>,
    /// 处理过程中发生但已恢复的异常记录。
    pub warnings: Vec<String>,
}

/// 整个批次的检查报告，可序列化为 JSON 留档。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub generated_at: String,
    pub total_objects: usize,
    pub conclusions: Vec<Conclusion>,
}
