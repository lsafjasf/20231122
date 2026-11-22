//! 核心数据模型：快照、字幕条目、合并结论与审计记录。

use std::path::PathBuf;

/// 一条字幕记录在原始输入中的定位信息，保证结论可回溯。
#[derive(Debug, Clone)]
pub struct SourceLoc {
    /// 来源快照标识（`snapshot_a` / `snapshot_b`）。
    pub snapshot_id: String,
    /// 原始文件中的 1 起始行号区间（含）。
    pub line_start: usize,
    pub line_end: usize,
    /// 记录在解析结果中的序号（0 起始，含解析失败的记录）。
    pub record_index: usize,
}

/// 单条字幕（cue）。
#[derive(Debug, Clone)]
pub struct Cue {
    pub seq: Option<u32>,
    /// 纳秒时间戳；解析失败时为 `None`。
    pub start_ns: Option<u64>,
    pub end_ns: Option<u64>,
    pub text: String,
    pub loc: SourceLoc,
    /// 原始记录的完整字节（截断/非法编码时仍保留）。
    pub raw_record: Vec<u8>,
    /// 解析阶段发现的问题，空数组表示结构完整。
    pub defects: Vec<String>,
}

/// 一份版本快照（一个 SRT 输入）的完整处理结果。
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub id: String,
    pub label: String,
    pub lang: String,
    /// 原始字节（未经任何转码），用于哈希与证据展示。
    pub raw_bytes: Vec<u8>,
    pub sha256: String,
    pub byte_len: usize,
    pub line_count: usize,
    /// 临时工作区里的落盘路径（工作区销毁后失效，仅记录历史）。
    pub staged_path: PathBuf,
    pub cues: Vec<Cue>,
}

/// 合并判定的五种结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// 时间轴 1:1 对齐，可合并。
    Aligned,
    /// 时间部分重叠，需要人工确认。
    Overlap,
    /// 边界两侧重叠度相同，无法自动判断。
    Ambiguous,
    /// 来源记录被截断，证据不足。
    Truncated,
    /// 来源结构损坏，无法参与合并。
    Malformed,
}

impl Verdict {
    pub fn as_str(&self) -> &'static str {
        match self {
            Verdict::Aligned => "已对齐",
            Verdict::Overlap => "部分重叠",
            Verdict::Ambiguous => "无法判断",
            Verdict::Truncated => "来源截断",
            Verdict::Malformed => "结构损坏",
        }
    }
}

/// 一条合并结论，携带回溯所需的全部证据指针。
#[derive(Debug, Clone)]
pub struct MergeFinding {
    pub index: usize,
    pub verdict: Verdict,
    pub reason: String,
    /// 涉及的 A 侧记录序号（可能为空）。
    pub a_records: Vec<usize>,
    /// 涉及的 B 侧记录序号（可能为空）。
    pub b_records: Vec<usize>,
    /// 关键证据（原始行内容、十六进制字节、时间区间等）。
    pub evidence: Vec<String>,
}

/// GStreamer 对单个快照的独立核验结果。
#[derive(Debug, Clone, Default)]
pub struct GstCheck {
    pub snapshot_id: String,
    pub pipeline_desc: String,
    pub emitted_cues: usize,
    pub first_start_ns: Option<u64>,
    pub last_end_ns: Option<u64>,
    pub bus_message: String,
    pub success: bool,
    pub error: Option<String>,
}
