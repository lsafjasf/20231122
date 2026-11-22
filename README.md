# 音频修复版本管理

用 Rust + GStreamer + GTK4 重建可复核的音频修复版本处理链。输入为原始录音、修复区间和版本清单；输出为规范化 WAV、波形 CSV、结构化报告与可独立校验的证据包。

## 能力

- 双设备时钟统一：按 `offset_ms` 与 `drift_ppm` 将设备读数反算到参考时间轴。
- 时间点/轨道/帧对应：保留请求时间与帧、统一时间与帧、偏差和处置结果。
- 版本化处理：严格按版本清单顺序处理；版本内严格按 `interval_ids` 顺序应用。
- 修复算子：`gain`、`mute`、`declick`、`normalize`、`replace_silence`，均带可追溯参数。
- 输出波形：固定上限的峰值/RMS 桶，CSV 与 JSON 摘要双份保留。
- 边界处理：空区间、空版本、空文件、缺失文件、超大 JSON/音频、区间过多、样本过多均留下明确证据。
- 独立复核：SHA-256 清单、稳定 JSON 字段/数组顺序、Markdown/CSV 记录与 `verify` 命令。
- GTK4 界面：选择三份输入、生成失步演示、异步处理、版本下拉、输出波形、产物校验。

## 构建

系统依赖：

```sh
sudo apt install build-essential pkg-config \
  libgtk-4-dev libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
  gstreamer1.0-tools gstreamer1.0-plugins-base \
  gstreamer1.0-plugins-good gstreamer1.0-libav
```

构建与测试：

```sh
cargo build --release
cargo test
```

仅验证与系统库无关的核心逻辑：

```sh
cargo test --no-default-features
```

## 快速演示

```sh
cargo run --release --bin audio-repair-cli -- demo \
  --work demo-input --out demo-output

cargo run --release --bin audio-repair-cli -- verify --dir demo-output

cargo run --release --bin audio-repair-gui
```

边界场景可直接复现：

```sh
audio-repair-cli empty --work empty-input --out empty-output
audio-repair-cli oversized --work oversized-input --out oversized-output
```

`empty` 生成 pass-through 版本；`oversized` 生成 `Rejected` 报告且不进入媒体渲染。产物目录必须不存在旧的 `report.json`，防止覆盖已复核证据。

演示固定构造两台失步设备：

- 设备 A：参考时钟，`offset_ms=0`，`drift_ppm=0`。
- 设备 B：读数约晚 7.312 秒，且每分钟慢 140 ppm。
- B 轨区间 `12311.3..12811.23 ms` 经反算后落在统一时间轴 `5000..5500 ms`。
- 版本按 `v1-reference -> v2-draft -> v3-final` 输出；`v3` 的两个区间重叠，用于验证应用顺序和告警。

## 输入契约

### 修复区间

```json
{
  "schema_version": 1,
  "reference_device_id": "device-a",
  "tolerance_ms": 40.0,
  "devices": [
    {"id": "device-a", "label": "A", "offset_ms": 0.0, "drift_ppm": 0.0},
    {"id": "device-b", "label": "B", "offset_ms": 7312.0, "drift_ppm": -140.0}
  ],
  "tracks": [
    {"id": 1, "device_id": "device-a", "sample_rate": 48000, "label": "A main"},
    {"id": 2, "device_id": "device-b", "sample_rate": 48000, "label": "B main"}
  ],
  "intervals": [
    {
      "id": "repair-01",
      "track_id": 2,
      "device_id": "device-b",
      "start": {"device_ms": 12311.3, "frame": 590942},
      "end": {"device_ms": 12811.23, "frame": 614939},
      "processor": {"kind": "gain", "gain_db": -9.0, "fade_ms": 20.0},
      "note": "reduce burst"
    }
  ]
}
```

`frame` 可选。时间是可执行锚点；若同时给出帧，则用 `tolerance_ms` 交叉检查
`frame - round(device_ms * track.sample_rate / 1000)`，偏差写入证据但不会静默丢弃。

### 版本清单

```json
{
  "schema_version": 1,
  "versions": [
    {"id": "v1", "label": "reference", "interval_ids": [], "note": "passthrough"},
    {"id": "v2", "label": "draft", "interval_ids": ["repair-01"], "note": "gain only"},
    {"id": "v3", "label": "final", "interval_ids": ["repair-01", "repair-02"], "note": "all"}
  ]
}
```

版本顺序和 `interval_ids` 顺序均按文件原序保存、处理和输出；重复引用、未知引用和重复 ID 会被拒绝。

## 时钟与帧

统一时间轴公式：

```text
canonical_ms = (device_ms - offset_ms) / (1 + drift_ppm / 1_000_000)
canonical_frame = round(canonical_ms * 48_000 / 1000)
```

所有版本先经 GStreamer 解码为 `F32LE / mono / 48000 Hz`，再按统一帧坐标切片。原始请求值、反算值、帧偏差、时钟偏差、裁剪量均写入报告。

## 边界与限制

默认限制定义在 `src/limits.rs`：

| 限制 | 默认值 | 行为 |
|---|---:|---|
| JSON 文件 | 8 MiB | 元数据预检后拒绝，保留路径/大小 |
| 原始录音 | 2 GiB | 预检后拒绝，不解码 |
| 音频时长 | 1800 s | 解码时按样本数硬限制 |
| 输出总时长 | 10800 s | 按“音频时长 × 版本数”在渲染前拒绝 |
| 区间数量 | 5000 | 解码前拒绝 |
| 版本数量 | 200 | 拒绝 |
| 单版本引用 | 5000 | 拒绝 |
| 波形桶 | 4000 | 桶宽自动放大，覆盖完整输出 |
| 证据条数 | 20000 | 截断时写入 `EVIDENCE_TRUNCATED` |

空修复集合不是错误：产生 pass-through 版本并记录 `EMPTY_REPAIR_SET`。空版本清单、零字节 JSON、零样本音频均被拒绝或失败，但仍尽量生成报告与复核记录。

## 产物

```text
<output>/
  report.json             # 完整字段化报告，数组顺序即处理顺序
  artifact_manifest.json  # 稳定顺序的产物索引
  review.md               # 人读复核记录
  review.csv              # 机读逐版本/逐区间记录
  SHA256SUMS              # 与 artifact_manifest 同序
  media/0001-v1.wav
  waveforms/0001-v1.csv
  ...
```

`report.json` 使用 `deny_unknown_fields` 与显式结构体字段顺序。重新载入后重新序列化的字节必须一致；元数据、区间、版本、应用区间均使用 `Vec`，不经过哈希表重排。

## 独立复核

```sh
sha256sum -c SHA256SUMS
audio-repair-cli verify --dir <output>
```

校验器会：

1. 重新计算 `artifact_manifest.json` 中每个产物的 SHA-256 与字节数。
2. 核对 `SHA256SUMS` 的行内容和顺序。
3. 核对版本 `manifest_index` 连续且等于数组位置。
4. 核对版本引用、实际应用区间、帧范围、输出与波形哈希。
5. 原始输入可访问时重算输入哈希，并核对区间/版本顺序。

输入不在本机时会明确报告缺失，不会视为“已通过输入复核”。

## 代码结构

- `src/model.rs`：版本化字段契约与稳定顺序。
- `src/clock.rs`：双设备映射、时间/帧交叉校验、越界裁剪、重叠检测。
- `src/media.rs`：GStreamer 解码、样本上限、WAV 编码。
- `src/waveform.rs`：修复算子与有界波形摘要。
- `src/engine.rs`：预检、版本编排、证据流水、产物收尾。
- `src/artifact.rs`：重载、哈希清单、独立校验。
- `src/review.rs`：Markdown/CSV 复核记录。
- `src/ui.rs`：GTK4 工作台与波形绘制。
- `src/bin/cli.rs`、`src/bin/gui.rs`：两个入口。

## 已知取舍

- 处理为内存内单声道分析，受 `max_audio_seconds` 硬限制；这是为了保证帧级证据与版本一致性，而不是无限流式缓存。
- `wavenc` 的输出编码由 GStreamer 决定；报告保留运行时 GStreamer 版本和输出哈希。
- 时钟模型使用一阶偏移 + 线性漂移；非线性时钟或多次重同步需要扩展模型并提升 schema 版本。
