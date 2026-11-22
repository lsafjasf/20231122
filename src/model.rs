use serde::{Deserialize, Serialize};

/// 原始输入事件：逐行写入 JSONL 日志，行号即证据定位。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RawEvent {
    SessionStart {
        session_id: String,
        channels: u32,
        tick_ms: u64,
        started_ms: u64,
    },
    Frame {
        channel: u32,
        seq: u64,
        pts_ms: u64,
        wall_ms: u64,
    },
    Gap {
        channel: u32,
        from_ms: u64,
        to_ms: u64,
        reason: String,
    },
    Checkpoint {
        channel: u32,
        last_seq: u64,
        wall_ms: u64,
    },
    Recovered {
        session_id: String,
        resumed_from: String,
    },
    SessionEnd {
        session_id: String,
        wall_ms: u64,
    },
}

/// 日志中的一行：行号 + 解析结果。截断行作为证据保留。
#[derive(Debug, Clone)]
pub struct JournalLine {
    pub line_no: usize,
    pub raw: String,
    pub event: Option<RawEvent>,
    pub parse_error: Option<String>,
}

/// 结论严重度。Indeterminate 用于无法判断的边界。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Verdict {
    Ok,
    Indeterminate,
    Error,
}

impl Verdict {
    pub fn label(&self) -> &'static str {
        match self {
            Verdict::Ok => "正常",
            Verdict::Indeterminate => "无法判断",
            Verdict::Error => "错误",
        }
    }
}

/// 证据引用：指向某个日志文件的某一行原始输入。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceRef {
    pub source_file: String,
    pub line_no: usize,
    pub raw_excerpt: String,
    pub note: String,
}

/// 一条处理步骤，让结论可回溯到处理管线。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessingStep {
    pub index: usize,
    pub name: String,
    pub detail: String,
}

/// 分析结论：每个结论都带证据链与产生它的处理步骤。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub id: usize,
    pub verdict: Verdict,
    pub kind: String,
    pub channel: Option<u32>,
    pub at_ms: Option<u64>,
    pub summary: String,
    pub step_index: usize,
    pub evidence: Vec<EvidenceRef>,
}
