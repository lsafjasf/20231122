# 时间码漂移回放（可暂停流程）

固定技术栈：**Rust + GStreamer + GTK4**。项目围绕 4.000s–5.500s 的缺失片段构造极限参数，演示连续回放与“暂停 → 保存断点 → 恢复 → 继续处理 → 前后对比”的完整路径。恢复后的逐帧 CRC、漂移统计、资源池状态和最终链哈希必须与不中断运行完全一致。

## 核心行为

- **缺失片段**：30 fps、300 帧源，`(4.0s, 5.5s]` 的 45 帧不产生输出；边界样本取缺失前 `#119` 与缺失后 `#166`。
- **极限参数**：缺失窗口前后各 1s、±20ms 注入抖动、100ms 漂移阈值、队列仅 2 个 buffer / 65536 B、资源池 409600 B、降级时保留 1/4 帧。
- **可暂停状态**：断点包含下一帧号、最后 PTS、处理/丢弃计数、漂移事件、滚动 CRC、资源池字节、降级状态和参数指纹；使用临时文件 + `fsync` + 原子重命名写入。
- **恢复前校验**：重新计算已有 `frames.jsonl` 的帧范围和滚动 CRC，与 checkpoint 比对；参数指纹不一致、帧记录不连续或链哈希不一致时拒绝恢复并保留证据。
- **资源不足与降级**：窗口内队列压力超过预算时进入 `degraded`，按 `degrade_keep_divisor` 丢弃帧并写 `crc=0`；离开窗口后扩大排空量，低于低水位时恢复全质量。
- **关键证据**：每次运行生成 JSONL 哈希链事件、逐帧记录、降级记录、边界 PPM/JSON、GStreamer 管线报告，以及包含全部文件 SHA-256 的 `manifest.json`。

## 运行

```sh
# 1. 连续运行 + 在缺口处暂停 + 恢复 + 对比，生成全部证据
cargo run -- demo --out artifacts/evidence --pause-at 120

# 2. 分步执行
cargo run -- run --out artifacts/evidence/continuous [--config configs/extreme.json]
cargo run -- run --out artifacts/evidence/pausable --pause-at 120
cargo run -- resume --out artifacts/evidence/pausable
cargo run -- compare \
  --a artifacts/evidence/continuous \
  --b artifacts/evidence/pausable \
  --report artifacts/evidence/compare/compare_report.json

# 3. GStreamer 边界帧往返校验
cargo run -- gst-probe --out artifacts/evidence/continuous

# 4. GTK4 控制界面
cargo run -- gui
```

`demo` 的退出码为 0 仅当前后两组 255 条帧记录逐字段一致、最终 CRC 链一致；否则退出码为 1。

## 证据文件

每个运行目录（`continuous/`、`pausable/`）包含：

- `params.json`：本次极限参数；紧凑 JSON 的 FNV-1a 指纹为 `8174572386738060988`。
- `frames.jsonl`：每个非缺失帧的 PTS、抖动、连续漂移、CRC、降级与窗口标记。
- `checkpoint.json` / `final_state.json`：暂停状态与最终状态（checkpoint 仅暂停运行生成）。
- `resume_compare.json`：恢复前对参数、帧范围与滚动 CRC 的复核结果，正常为 `"action":"continue"`，拒绝时为 `"reject"` 并附原因。
- `degradation.jsonl`：资源不足的进入/退出事件及预算。
- `boundary_before.ppm`、`boundary_after.ppm`：边界前后原始画面样本；对应 JSON 记录原始画面 CRC。
- `boundary_delta.json`：PTS 差、标称步长、缺失跨度和 CRC 是否变化。
- `events.jsonl`：`prev`/`hash` 连接的只追加事件链。
- `pipeline.json`：真实 GStreamer `appsrc → queue → videoconvert → appsink` 的 caps、元素和逐帧输入/输出 CRC。
- `manifest.json`：除自身外全部证据文件的字节数与 SHA-256。

顶层 `compare/compare_report.json` 比较两组帧序列与最终 CRC 链；`summary.json` 汇总暂停位置、边界时间码和最终结论。

## 代码布局

- `src/model.rs`：参数、时间码、状态与报告模型；参数校验和指纹。
- `src/source.rs`：确定性抖动、缺失帧、可目视检查的 RGB/PPM 和 CRC32。
- `src/engine.rs`：暂停/恢复状态机、资源预算、降级和前后比较。
- `src/evidence.rs`：原子状态写入、JSONL 哈希链、边界样本和证据 manifest。
- `src/gst_bridge.rs`：GStreamer 边界帧真实管线与 CRC 往返验证。
- `src/gui.rs`：GTK4 控制面板（开始、暂停、恢复、比较、状态与日志）。
- `tests/acceptance.rs`：等价性、降级恢复、断点防篡改和 GStreamer 往返测试。

## 工具链

当前容器的 GStreamer 1.28 / GTK4 4.24 来自 `/home/hj/env-media`，`.cargo/config.toml` 已设置 pkg-config、插件路径和 RPATH。标准 Ubuntu 环境可改用系统包：

```sh
sudo apt install rustc cargo libgstreamer1.0-dev \
  libgstreamer-plugins-base1.0-dev libgtk-4-dev gstreamer1.0-plugins-base
cargo test
```

GTK 界面需要显示服务器；CLI、GStreamer appsrc/appsink 和测试不需要显示服务器。
