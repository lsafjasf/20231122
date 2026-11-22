# Review Evidence Format

`review.json` 使用 `credits-review/v2` schema。它的目标不是替代项目文件，而是提供可独立复核的关联快照。

## 顶层字段

| 字段 | 含义 |
| --- | --- |
| `schema` | 固定为 `credits-review/v2` |
| `generated_at_unix_ms` | 报告生成时间 |
| `tool` | Rust、GStreamer、GTK4 版本及工具版本 |
| `source_project` | 输入路径、检测格式、扩展名、未知扩展标记、大小、SHA-256、前 64 字节十六进制 |
| `project_format` | `legacy_v1`、`roll_v2` 或 `bare_media` |
| `summary` | 对象、成功/部分/失败、关联与快照计数 |
| `objects[]` | 成功处理的对象 |
| `failures[]` | 未成功对象的阶段、消息、证据目录 |
| `cleanup` | 临时状态清理结果 |
| `normalized_project` | 规范化项目文件的哈希证据 |
| `evidence_files[]` | 所有保留证据的相对路径、SHA-256、大小 |

## 对象与关联

`objects[].media` 记录实际媒体路径、扩展名、`unknown_extension`、大小、SHA-256 和魔数。`probe` 记录 GStreamer Discoverer 的容器、时长、编解码器、尺寸、帧率和流列表。

`objects[].correlations[]` 是复核核心：

```json
{
  "index": 0,
  "time": {
    "seconds": 1.0,
    "clock": "00:00:01.000",
    "timecode": "00:00:01:00",
    "frame": 24,
    "pts_ns": 1000000000,
    "fps": 24.0,
    "track": "video-main:video",
    "requested": "00:00:01.000",
    "label": "first card"
  },
  "versions": [
    {
      "version": "before",
      "file": "evidence/0000-roll-open/checkpoint-000-frame-00000024-before.png",
      "sha256": "...",
      "frame": 24,
      "timecode": "00:00:01:00",
      "track": "video-main:video",
      "parameters_applied": false
    },
    {
      "version": "after",
      "file": "evidence/0000-roll-open/checkpoint-000-frame-00000024-after.png",
      "sha256": "...",
      "frame": 24,
      "timecode": "00:00:01:00",
      "track": "video-main:video",
      "parameters_applied": true
    }
  ],
  "parameters": {
    "font_family": "Sans Bold",
    "font_size": 30.0,
    "color": "#FFF7E6FF",
    "scroll_speed_px_s": 96.0
  },
  "waveform_file": "evidence/0000-roll-open/waveform.json",
  "waveform_peak_index": 100,
  "waveform_window": [{"min": -0.2, "max": 0.3, "rms": 0.1}],
  "pixel_diff": {"mean_abs": 2.05, "changed_ratio": 0.02, "compared_pixels": 57600}
}
```

`before` 是 GStreamer 解码后的原始帧，`after` 是在同一帧上按 `parameters` 排版后的结果。`parameters_applied=false` 表示该版本不应用排版，但仍记录当前参数，便于对比。

## 波形

`waveform.json` 同时嵌入 `objects[].waveform`：

- `sample_rate`：统一为 8000 Hz 单声道，便于稳定比较。
- `bucket_ms`：峰值分桶宽度；长素材会自动增大以限制峰值数量。
- `peaks[]`：每桶的 `min`、`max`、`rms`，范围归一化到 `[-1, 1]`。
- `markers[]`：检查点秒数、对应峰值索引和标签。
- `available`/`reason`：无音频或提取失败时的显式状态。

`waveform.svg` 是可人工打开的同一数据的包络视图，并绘制检查点竖线。对象级 `waveform_json` / `waveform_svg` 指向两份输出文件，关联记录中的 `waveform_file`、`waveform_peak_index` 和 `waveform_window` 可直接定位到该时间点。

## 完整性

`review.json.sha256` 使用 `<sha256>  review.json` 格式。`verify` 会：

1. 校验报告文件本身的 sidecar。
2. 校验 `evidence_files[]` 中每个相对路径的 SHA-256。
3. 校验 `normalized_project`。
4. 拒绝绝对路径与 `..` 路径穿越。

失败对象只保留已安全落盘的证据和 `failure.json`；这些中间证据也会被扫描并纳入 `evidence_files[]` 哈希清单。它们不会因为后续对象成功而删除，也不会把关键证据放入临时目录。

## 兼容策略

旧版 v1 与新格式 v2 在解析后统一为 `Project` 模型，因此时间、轨道、参数和波形证据的字段完全一致。旧字段只作为输入别名存在，不泄漏到输出契约。
