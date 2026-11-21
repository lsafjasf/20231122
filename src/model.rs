use serde::{Deserialize, Serialize};

/// 安全区极限参数：针对一个（缺失）片段单独设置，
/// 用收紧/放宽两档极限值审查字幕是否越出安全区。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExtremeParams {
    /// 安全区内缩（占画面比例 0.0..0.5）
    pub inset_left: f64,
    pub inset_right: f64,
    pub inset_top: f64,
    pub inset_bottom: f64,
    /// 字幕“墨迹”亮度阈值 (0..255)
    pub luma_threshold: u8,
    /// 判定违规的墨迹像素占比阈值 (0.0..1.0)
    pub violation_ratio: f64,
    /// 边界采样偏移（毫秒）：边界前后各取一个样本
    pub boundary_epsilon_ms: u64,
}

impl Default for ExtremeParams {
    fn default() -> Self {
        Self::tightened()
    }
}

impl ExtremeParams {
    /// 极限收紧：安全区最小、阈值最敏感
    pub fn tightened() -> Self {
        Self {
            inset_left: 0.10,
            inset_right: 0.10,
            inset_top: 0.10,
            inset_bottom: 0.20,
            luma_threshold: 160,
            violation_ratio: 0.001,
            boundary_epsilon_ms: 40,
        }
    }

    /// 极限放宽：安全区最大、阈值最迟钝
    pub fn loosened() -> Self {
        Self {
            inset_left: 0.02,
            inset_right: 0.02,
            inset_top: 0.02,
            inset_bottom: 0.05,
            luma_threshold: 220,
            violation_ratio: 0.02,
            boundary_epsilon_ms: 40,
        }
    }
}

/// 一个待审查的媒体片段（例如一个缺失片段的时间范围）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Segment {
    pub id: String,
    pub label: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub params: ExtremeParams,
    /// 历史运行结果，最新在最后；保留最近两次用于比较
    #[serde(default)]
    pub runs: Vec<RunResult>,
}

impl Segment {
    pub fn new(id: String, label: String, start_ms: u64, end_ms: u64) -> Self {
        Self {
            id,
            label,
            start_ms,
            end_ms,
            params: ExtremeParams::default(),
            runs: Vec::new(),
        }
    }

    pub fn latest_run(&self) -> Option<&RunResult> {
        self.runs.last()
    }

    pub fn previous_run(&self) -> Option<&RunResult> {
        self.runs.len().checked_sub(2).map(|i| &self.runs[i])
    }

    pub fn push_run(&mut self, run: RunResult) {
        self.runs.push(run);
        if self.runs.len() > 2 {
            self.runs.remove(0);
        }
    }
}

/// 边界样本：边界前/后各一帧的墨迹指标与证据文件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundarySample {
    pub timestamp_ms: u64,
    pub ink_pixels: u64,
    pub ink_ratio: f64,
    /// 证据 PNG（相对证据目录）
    pub snapshot: String,
}

/// 一条边界（入点或出点）的前后对照
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundaryPair {
    pub boundary_ms: u64,
    pub before: BoundarySample,
    pub after: BoundarySample,
    /// after.ink_ratio - before.ink_ratio
    pub ink_ratio_delta: f64,
}

/// 单次重跑结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunResult {
    pub run_at_unix: u64,
    pub params: ExtremeParams,
    pub frames_scanned: u64,
    pub violating_frames: u64,
    pub max_ink_ratio: f64,
    pub mean_ink_ratio: f64,
    pub entry_boundary: Option<BoundaryPair>,
    pub exit_boundary: Option<BoundaryPair>,
}

impl RunResult {
    pub fn violation_ratio(&self) -> f64 {
        if self.frames_scanned == 0 {
            0.0
        } else {
            self.violating_frames as f64 / self.frames_scanned as f64
        }
    }
}
