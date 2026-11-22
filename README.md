# Encode Pulse

实时编码统计面板重建：用 Rust 处理编码统计、帧率、码率和丢帧数，用 GStreamer 采集实时编码缓冲，用 GTK4 展示三轨波形与统计卡片。

## 能做什么

- 输入统一为 `EncodeSample { timestamp_ns, track, frame, encoded_bytes, frame_rate, bitrate_bps, dropped_frames }`。
- 对每个样本计算丢帧增量、FPS/码率/丢帧等级、EMA 和归一化波形值。
- 阈值跨越时保存完整的前后样本及变化量，输出 `BoundaryEvent`。
- 空输入显示明确的 `NO DATA`，不伪造零值样本。
- 百万级输入使用固定长度窗口和有界证据区，内存不随样本数增长。
- 输出 JSON、JSONL、SVG 波形、PNG 界面截图及 SHA-256 清单。

## 快速运行

```bash
source scripts/env.sh

# GTK4 实时面板（默认 640x360@30，1 Mbps，3% 丢帧）
cargo run

# 生成空输入、边界、极限异常、百万输入证据
cargo run -- --headless --out artifacts/evidence

# 真实 GStreamer 极限管线：1080p@240、32 kbps、25% drop、1 缓冲 appsink
cargo run -- --headless --gstreamer --extreme-pipeline \
  --duration-ms 4000 --out artifacts/evidence

# 一键跑测试、全部场景、实时管线和界面截图
scripts/reproduce.sh
```

常用参数：

```text
--scenario empty|boundary|abnormal|huge
--huge-samples 1m
--duration-ms 4000
--extreme-pipeline
--ui-smoke-ms 3500 --screenshot artifacts/evidence/ui-smoke.png
```

## 处理链路

1. `videotestsrc -> x264enc -> h264parse -> identity(drop) -> appsink` 产生实时编码缓冲。
2. `PipelineMeter` 从 PTS、缓冲大小和 GOP 间隔得到时间点、帧号、估算 FPS、1 秒滑窗码率和累计丢帧。
3. `StatisticsProcessor` 清洗 `NaN`、负数、无穷和 `u64::MAX`，计算丢帧增量与阈值等级。
4. 每个处理点生成 `WaveformPoint`，包含时间偏移、轨道、帧号、输入值、归一化值和处理等级。
5. 等级变化时生成 `BoundaryEvent`，保留变化前和变化后的完整 `ProcessedPoint`。
6. GTK4 每 80 ms 拉取样本，更新卡片、边界计数和三轨 Cairo 波形。

阈值语义：

- FPS 低于 `fps_warn` 进入 warning，低于 `fps_critical` 进入 critical。
- 码率高于 `bitrate_warn_bps` 进入 warning，高于 `bitrate_critical_bps` 进入 critical。
- 单样本丢帧增量达到 `drop_warn` 进入 warning，达到 `drop_critical` 进入 critical。

详细设计见 `docs/processing.md`。

## 证据

入口文件为 `artifacts/evidence/index.json`，包含工具链、场景摘要、实时管线摘要和全部文件校验和。

| 证据 | 内容 |
| --- | --- |
| `empty/evidence.json` | 空输入状态、无数据波形 |
| `boundary/boundaries.json` | 每个阈值变化的前样本、后样本、变化量 |
| `abnormal/evidence.json` | 极限参数、清洗标记、有界波形 |
| `huge/evidence.json` | 1,000,000 样本计数、有界窗口、丢弃证据计数 |
| `gstreamer/evidence.json` | 实际 GStreamer 版本、管线描述、缓冲数与 PTS 丢帧 |
| `ui-smoke.png` | GTK4 面板实际渲染截图 |
| `tests.txt` | 单元/集成测试输出 |

边界示例：帧率从 `25.0001`（track 7, frame 101）跌到 `24.9999`（track 7, frame 102），事件记录 `fps_delta=-0.0002`、`timestamp_delta_ns=33333333`、`frame_delta=1`。

## 测试

```bash
source scripts/env.sh
cargo test --all-targets
cargo test --test live_gst -- --ignored
```

当前结果：12 个默认测试通过，1 个需要 GStreamer/x264 的实时集成测试通过。百万输入测试约 0.5 秒完成，波形固定为 8 点，边界证据固定为 32 条。

## 环境

本项目固定使用 Rust、GStreamer 1.x 和 GTK4。`scripts/env.sh` 默认复用 `/home/hj/env-media`，可用 `ENCODE_PANEL_MEDIA_PREFIX` 指向其他前缀。系统级安装时只需保证 `pkg-config` 能找到 `gtk4`、`gstreamer-1.0` 和 `gstreamer-app-1.0`。

实时丢帧数是根据编码缓冲 PTS 间隔推断的“已检测缺失帧”，不是编码器内部所有未交付帧的绝对计数；这一限制在证据中保留为管线描述和检测口径。
