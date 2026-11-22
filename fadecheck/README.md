# 音频片段淡化检查 — 来源检查页

固定技术栈：**Rust + GStreamer + GTK4**。

对一批音频片段做淡入/淡出检查，并让每条结论都能完整回溯到
原始输入、两个版本快照与关键证据。

## 来源检查的溯源链

每个对象的报告按三层组织，任一层都可独立复核：

1. **原始输入登记**：原始路径、规范化绝对路径、大小、扩展名、SHA-256 指纹。
2. **两个版本快照**：
   - `v1-decoded`：`filesrc ! decodebin ! audioconvert ! audioresample ! appsink`，保留原始采样率；
   - `v2-resampled`：同一管线重采样到 16 kHz，作为第二个独立版本。
   两个快照都记录输入指纹与逐步处理环节（`PipelineStep`）。
3. **关键证据**：首尾 5% 窗口相对中部基准的 RMS 比值、分块能量单调性、样本窗口帧范围、所用阈值。

## 无法判断的边界处理

判定阈值之间显式保留边界带（默认 `(0.65, 0.85)`）：

- 比值 `<= 0.65` 且单调性达标 → 存在淡化；
- 比值 `>= 0.85` → 无淡化（单调性不再否决）；
- 落入边界带，或单调性不足 → `Inconclusive`（无法判断），
  此时**保留全部边界证据**，报告中明确记录测量值与阈值上下沿，不做强行归类。

## 异常隔离与临时状态

- 单对象解码失败只记录到该对象的 `warnings`，批次继续处理其余对象。
- 未知扩展名（如 `.xyz`）不猜测媒体类型，登记为 `NotAnalyzable` 并保留大小与指纹。
- 解码 PCM 落在 `TempState` 管理的临时目录，批次结束显式 `cleanup()`，
  `Drop` 再兜底，保证无残留。

## 运行

本仓库在无 root 环境下把 GTK4/GStreamer 开发库解压到 `~/local/sys`，
所有路径由 `env.sh` 统一注入：

```bash
source env.sh
cargo test          # 4 个集成测试
./smoke.sh          # 端到端冒烟（无头批处理 + 清理校验）
cargo run           # GTK4 来源检查页
```

无头批处理（JSON 报告写到 stdout，进度日志走 stderr）：

```bash
./target/debug/fadecheck --batch a.wav b.xyz ... > report.json
```

无显示环境可用 Broadway 离屏验证界面：

```bash
gtk4-broadwayd :99 &
GDK_BACKEND=broadway BROADWAY_DISPLAY=:99 cargo run
```

## 目录结构

- `src/model.rs`：输入对象、快照、结论、证据的领域模型（可序列化）。
- `src/decoder.rs`：GStreamer 解码/重采样管线与步骤留痕。
- `src/analyzer.rs`：RMS/单调性测量、阈值边界带与三态判定。
- `src/engine.rs`：登记、双快照编排、异常隔离、临时状态清理。
- `src/ui.rs`：GTK4 三栏来源检查页（登记 / 结论 / 溯源详情）。
- `src/util.rs`：SHA-256、UTC 时间戳、`TempState`。
- `tests/`：分析器与引擎端到端测试，以及 WAV 夹具生成脚本。
