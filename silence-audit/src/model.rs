//! 核心领域模型：静音段、检测参数与单对象处理结果。

use serde::{Deserialize, Serialize};

/// 静音检测的全部处理参数，完整写入报告以便复核。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SilenceParams {
    /// level 元素判定静音的幅度阈值（dB，负值；低于该值视为静音）。
    pub threshold_db: f64,
    /// 连续多少毫秒低于阈值才确认进入静音段。
    pub on_millis: u64,
    /// 连续多少毫秒高于阈值才确认离开静音段（迟滞，避免抖动）。
    pub off_millis: u64,
    /// level 元素消息间隔（毫秒），决定时间分辨率。
    pub interval_millis: u64,
    /// 分析前统一重采样的采样率（Hz）。
    pub resample_rate: u32,
    /// 波形证据横向采样点数。
    pub waveform_buckets: usize,
}

impl Default for SilenceParams {
    fn default() -> Self {
        Self {
            threshold_db: -50.0,
            on_millis: 200,
            off_millis: 300,
            interval_millis: 50,
            resample_rate: 48_000,
            waveform_buckets: 512,
        }
    }
}

/// 一段静音区间，同时给出时间点与帧位置、所在轨道。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SilenceSegment {
    /// 静音段序号（从 0 开始）。
    pub index: usize,
    /// 所在轨道标识（多轨道文件中对应 GStreamer 流序号）。
    pub track: String,
    /// 起始时间（秒）。
    pub start_secs: f64,
    /// 结束时间（秒）；截断对象可能为 `None`，表示静音段延伸到已知数据末尾。
    pub end_secs: Option<f64>,
    /// 起始帧序号（按重采样后的采样率计）。
    pub start_frame: u64,
    /// 结束帧序号；截断时为 `None`。
    pub end_frame: Option<u64>,
    /// 时长（秒）；截断时按已知数据估算。
    pub duration_secs: f64,
    /// 该区间内观测到的最小 RMS（dB）。
    pub min_rms_db: f64,
    /// 该区间内观测到的最大峰值（dB）。
    pub max_peak_db: f64,
    /// 该区间是否因记录截断而未观察到明确结束。
    pub truncated: bool,
}

/// 波形证据：每个桶记录该时间窗内的峰值/RMS，供 UI 与 PNG 绘制。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Waveform {
    /// 每个桶的起始时间（秒）。
    pub bucket_start_secs: Vec<f64>,
    /// 每个桶的峰值幅度（0.0..1.0）。
    pub peaks: Vec<f32>,
    /// 每个桶的 RMS 幅度（0.0..1.0）。
    pub rms: Vec<f32>,
    /// 音频总时长（秒）。
    pub duration_secs: f64,
    /// 实际分析用的采样率。
    pub sample_rate: u32,
    /// 声道数。
    pub channels: u32,
}

/// 单个待处理对象在一次运行中的处理状态。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ObjectStatus {
    /// 正常完成。
    Ok,
    /// 完成但输入记录被截断（静音段可能延伸到数据末尾之外）。
    Truncated,
    /// 处理失败，已记录错误并继续处理其余对象。
    Failed,
}

/// 单个对象的处理结果（成功时携带静音段与波形，失败时携带错误信息）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ObjectResult {
    /// 快照中对象的稳定标识。
    pub object_id: String,
    /// 记录来源格式版本：`legacy` 或 `v2`。
    pub source_format: String,
    /// 输入媒体路径。
    pub media_path: String,
    /// 轨道/帧引用信息（直接保留快照原文，便于回溯）。
    pub track_ref: String,
    /// 处理状态。
    pub status: ObjectStatus,
    /// 失败时的错误描述。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// 检测到的静音段。
    #[serde(default)]
    pub segments: Vec<SilenceSegment>,
    /// 波形证据（失败时可能缺失）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waveform: Option<Waveform>,
    /// 实际使用的处理参数。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<SilenceParams>,
    /// 证据文件相对路径（波形 PNG 等）。
    #[serde(default)]
    pub evidence_files: Vec<String>,
}

impl ObjectResult {
    pub fn failed(
        object_id: impl Into<String>,
        source_format: impl Into<String>,
        err: impl std::fmt::Display,
    ) -> Self {
        Self {
            object_id: object_id.into(),
            source_format: source_format.into(),
            media_path: String::new(),
            track_ref: String::new(),
            status: ObjectStatus::Failed,
            error: Some(err.to_string()),
            segments: Vec::new(),
            waveform: None,
            params: None,
            evidence_files: Vec::new(),
        }
    }
}
