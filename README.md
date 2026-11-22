# 变速不变调试听（Rust + GStreamer + GTK4）

这是一个可复现、可独立复核的变速/变调试听实现。输入为原始 WAV 音频、速度曲线和音高轨迹；输出为经过真实 GStreamer 处理的 WAV、逐窗时间/帧映射、异常证据和哈希清单。

固定技术栈：

- Rust：数据模型、流式校验、时间映射、审计与重载
- GStreamer：`wavparse` 解码、`appsrc` 分段、`pitch(tempo,pitch)` 处理、`appsink` 取回波形样本
- GTK4：输入选择、运行、异常高亮波形和 GStreamer 试听

## 快速运行

系统依赖（Ubuntu/Debian）：

```bash
sudo apt install cargo rustc pkg-config libgstreamer1.0-dev \
  libgstreamer-plugins-base1.0-dev gstreamer1.0-plugins-good \
  gstreamer1.0-plugins-bad libgtk-4-dev
```

运行内置异常场景：

```bash
cargo run --release -- scenario \
  --config fixtures/missing_segment/scenario.json
```

空输入和超限样例：

```bash
cargo run --release -- scenario \
  --config fixtures/empty_input/scenario.json
cargo run --release -- scenario \
  --config fixtures/limit_rejected/scenario.json
```

独立复核：

```bash
cargo run --release -- verify \
  --artifact artifacts/missing-segment

cargo run --release -- reload \
  --artifact artifacts/missing-segment
```

对应样例产物位于 `artifacts/missing-segment`、`artifacts/empty-input`、
`artifacts/limit-rejected`，均可单独执行 `verify`。

启动 GTK4 调听：

```bash
cargo run --release --bin tempo-pitch-gui
```

重新生成 fixtures：

```bash
cargo run --example make_fixtures --no-default-features
```

## 输入格式

音频必须是无损 RIFF/WAVE（PCM 8/16/24/32-bit 或 IEEE float 32/64-bit）。解码后统一为 48 kHz、F32LE、原声道数，再分段进入 GStreamer。

曲线为两列 CSV，第一行可以是表头：

```csv
time_us,rate
0,1.00
700000,1.10
1250000,1.45
```

音高轨迹：

```csv
time_us,semitones
0,0.0
20000,0.02
40000,0.04
```

支持的表头时间单位：`time_us`、`time_ms`、`time_s`。时间必须严格递增，数值必须有限。

- 速度：`0.1..=4.0`，表示相对原音频的播放倍率；`2.0` 表示两倍速，输出时长约为源时长的一半。
- 音高：`-24..=24` 半音；应用比例为 `2^(semitones/12)`。
- 处理段中点用于固定该段的 `pitch` 参数；记录同时保留“曲线中点值”和“实际应用值”。

## 缺失片段场景

`fixtures/missing_segment/pitch_trajectory.csv` 为 20 ms 密集轨迹，但在源时间
`1,250,000..1,500,000 us` 完全没有采样。`scenario.json` 同时声明该区间，防止“稀疏控制点”和“真实丢帧”混淆。

记录会把以下内容关联到同一个异常：

- 源时间区间与输出时间区间
- 输入轨道 `audio/channel-1` 与输入帧区间
- 输出帧区间（来自实际 GStreamer 输出样本数）
- 曲线速度/实际应用速度、曲线半音/实际应用半音、实际音高比例
- 音高证据状态 `held_last_known`
- 对应 `timeline.csv` / `waveform.csv` 行号
- 异常窗口的 min/max/RMS/peak dBFS

处理区间缺失音高时沿用最后已知值，但不伪造“已观测”证据；`record.json` 与两个 CSV 均可看到 `held_last_known`。

## 空输入与超大输入

空音频（0 帧）不会崩溃：

- 输出结构有效的 44 字节空 WAV
- 状态为 `empty_input`
- 写入 `EmptyAudio` 异常
- 仍生成完整 `record.json`、清单和复核报告

空速度曲线/空音高轨迹分别使用 `1.0x`/`0 st` 标称值，并写入显式异常。

超大输入采用“先限制、后处理”：

- 音频、曲线文件大小和曲线点数在解析前检查
- 解码 PCM 字节数在流式拷贝时检查
- 渲染段数、时间窗数量有上限；长音频会自动放大窗口而不是无限分配
- 超限时状态为 `rejected`，仍输出空 WAV、输入摘要、限制值、异常和可复核清单
- 默认限制见 `src/model.rs::Limits`，CLI 可逐个覆盖

## 时间与帧映射

对每个输出窗口，记录：

```text
source_start_us/source_end_us     输入时间
source_frame_start/end            输入帧（floor(us * input_rate / 1e6)）
output_start_us/output_end_us     实际输出时间
output_frame_start/end            实际 GStreamer 输出帧
speed_rate_curve                  曲线中点速度
speed_rate_applied                该渲染段实际传入 pitch.tempo
pitch_semitones_curve             曲线中点半音
pitch_semitones_applied           该渲染段实际传入 pitch.pitch
pitch_ratio_applied               2^(semitones/12)
pitch_evidence                    observed / held_last_known / ...
waveform                          min/max/RMS/peak dBFS
```

输出帧来自 `appsink` 实际收到的 F32LE 样本，而不是仅按公式推算。逐窗输出帧由该渲染段的实际输出帧按源时长比例分配，最后一个窗口吸收整数舍入余数。

## 产物

```text
artifacts/missing-segment/
  record.json       权威记录，字段顺序固定
  timeline.csv      全量逐窗映射，顺序与 record.timeline 一致
  waveform.csv      全量逐窗波形证据
  output.wav        实际 GStreamer 输出
  REVIEW.md         面向同事的人工复核摘要
  manifest.json     记录与旁文件的大小/SHA-256
```

`record.json` 顶层字段顺序固定为：

```text
schema, status, job_id, created_utc, request, inputs, processing,
timeline, anomalies, output, verification, recording_policy
```

`verify` 会：

1. 校验清单与文件大小/SHA-256；
2. 重新载入 `record.json` 并要求规范化序列化与原文件逐字节一致；
3. 校验 WAV 头、帧数、采样率、声道数；
4. 从 `output.wav` 重新计算每个窗口波形并与记录比较；
5. 校验 CSV 表头和行数。

详细字段契约见 `docs/ARTIFACT_FORMAT.md`。

## 测试

核心测试不需要 GStreamer/GTK：

```bash
cargo test --no-default-features --test core
```

真实 GStreamer 端到端测试：

```bash
cargo test --features gst --test gst_e2e
```

测试覆盖完整异常映射、空音频、超限拒绝、字段顺序、重载一致性、长音频窗口自适应和真实 `pitch` 管线。
