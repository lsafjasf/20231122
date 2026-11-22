# 产物字段契约

本文件描述 `record.json` 的稳定顺序和证据含义。所有时间单位为整数微秒（`_us`），帧为从 0 开始计数的采样帧，声道号从 0 开始，轨道名从 1 开始。

## 顶层字段

| 顺序 | 字段 | 含义 |
|---:|---|---|
| 1 | `schema` | 固定为 `tempo-pitch-debug/1` |
| 2 | `status` | `complete` / `complete_with_anomalies` / `empty_input` / `rejected` |
| 3 | `job_id` | 三个输入 SHA-256 派生的稳定作业号 |
| 4 | `created_utc` | RFC3339 UTC 创建时间 |
| 5 | `request` | 路径、场景、窗口、声明缺失区间和限制 |
| 6 | `inputs` | 音频与两条曲线的摘要、SHA-256、点、内部间隔 |
| 7 | `processing` | 后端、插件链、采样率、声道、参数范围、渲染段数 |
| 8 | `timeline` | 逐窗源/输出时间与帧、参数、波形 |
| 9 | `anomalies` | 缺失片段、空输入、超限等异常 |
| 10 | `output` | 实际输出 WAV 的格式、帧数、大小、SHA-256、峰值 |
| 11 | `verification` | 生成期检查、重载结果、复核命令 |
| 12 | `recording_policy` | 行数、是否自适应、字段顺序契约 |

Serde 按结构体声明顺序输出字段；`verify` 将文件重新解析后再次规范化，要求字节完全相同。

## `timeline` 行

- `index`：连续递增，从 0 开始。
- `render_segment_index`：对应 GStreamer 渲染段。
- `track_id` / `channel`：分析轨道与声道。
- `source_start_us` / `source_end_us`：源窗口。
- `source_frame_start` / `source_frame_end`：源帧，按输入采样率换算。
- `output_start_us` / `output_end_us`：实际输出窗口。
- `output_frame_start` / `output_frame_end`：实际输出帧，按 `output.wav` 的 48 kHz 换算。
- `speed_rate_curve`：窗口中点的速度曲线值。
- `speed_rate_applied`：该渲染段实际应用的 `pitch.tempo`。
- `pitch_semitones_curve`：窗口中点的音高曲线值。
- `pitch_semitones_applied`：该渲染段实际应用的半音。
- `pitch_ratio_applied`：实际 `pitch.pitch` 比例。
- `pitch_evidence`：`observed`、`held_last_known`、`default_nominal` 等。
- `waveform`：该输出窗口选中声道的样本帧数、min、max、RMS、peak dBFS。

`timeline.csv` 与 `waveform.csv` 表头固定；第 1 行为表头，数据行与 `timeline` 索引一一对应。

## 异常

每个异常均包含：

- `source_span` / `output_span`
- `source_frames` / `output_frames`
- `parameters`（曲线值、应用值、比例、证据状态）
- `timeline_index_span`（闭区间）
- `waveform_at_gap`（异常覆盖窗口的聚合波形）
- `evidence_refs`（artifact + 行号/时间/帧定位）

`missing_pitch_segment` 表示声明区间或超过阈值的内部间隔。处理参数仍完整记录，不能用“输出听感正常”替代证据。

## 边界语义

- 0 帧音频：`output.wav` 是有效空 WAV，`timeline` 为空，状态 `empty_input`。
- 超限：`status=rejected`，输出空 WAV；异常记录实际值和限制值。
- 长音频：若时间窗会超过上限，`processing.window_us` 自动放大；`recording_policy.adaptive_window_applied=true`。
- 曲线点过多时，`CurveEvidence.points` 只保留首尾样本作为摘要，但 `points_count`、SHA-256 和 `points_sampled` 说明完整性；超点数上限时直接拒绝，不静默截断处理。
