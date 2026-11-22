# Seamless Audio Loop — 可暂停的三锚点处理流程

Rust + GStreamer + GTK4 实现的“音频循环无缝连接”处理流水线。
处理过程以三段锚点为阶段检查点，可暂停、可恢复，恢复后产物与连续运行
逐字节一致；资源不足时自动降级或干净地失败，并保留全部关键证据。

## 环境

```bash
source /home/hj/local/env.sh   # Rust 1.100 / GStreamer 1.28.2 / GTK4 4.22.4
cargo build
```

## 三段锚点（阶段检查点）

| 锚点 | 阶段 | 产物（检查点） |
|------|------|----------------|
| `A1_DECODE`  | GStreamer 解码为 48 kHz mono f32 PCM | `checkpoints/a1_pcm.f32le` |
| `A2_ANALYZE` | 搜索最佳循环拼接点 + 交叉淡化参数 | `checkpoints/a2_plan.json` |
| `A3_RENDER`  | 等功率交叉淡化渲染无缝循环 WAV | `output/loop.wav` + `output/a3_report.json` |

每个锚点完成后原子写入 `state.json`（样本数、字节偏移、流时间 ns、
SHA-256、UTC 完成时间），进程可在任意锚点退出，之后从检查点恢复。

## 使用

```bash
# 一键验收（连续 vs 暂停/恢复等价性、接缝质量、降级路径、原始材料完整性）
scripts/acceptance.sh

# GTK4 界面（broadway 后端，浏览器打开 http://127.0.0.1:8092 查看）
scripts/acceptance.sh ui

# 手动：分阶段暂停/恢复
./target/debug/seamless-loop run --run-id demo --pause-after A1
./target/debug/seamless-loop run --run-id demo --pause-after A2
./target/debug/seamless-loop run --run-id demo            # 恢复到完成
./target/debug/seamless-loop compare accept-continuous accept-paused

# 强制降级路径（模拟可用内存 200MB）/ 资源不足（64MB）
./target/debug/seamless-loop run --run-id deg --simulate mem=200
./target/debug/seamless-loop run --run-id low --simulate mem=64
```

## 验收（落到具体时间点与原始偏移）

`seamless-loop verify` 输出 `artifacts/acceptance/summary.json`，当前结果：

- **pause_resume_equivalence**: 连续运行与“暂停→恢复×2”运行的三个锚点
  产物 SHA-256 全部一致（A1 PCM `8ff9ee12…`、A2 plan `2e4e0934…`、
  A3 WAV `002b70ca…`）。
- **seamless_join**: 循环起点 1.500000 s（样本 72000）、终点 4.866688 s
  （样本 233601），循环长 3.366687 s（161601 样本），交叉淡化 4096 样本；
  回绕接缝步进 0.4238 ≤ 内部最大步进 1.8733 × 1.5，判定无缝。
- **degradation_path**: 模拟低内存进入降级模式（分块解码、stride=4 粗分析、
  淡化降至 1024 样本），证据见 `artifacts/runs/accept-degraded/evidence.jsonl`。
- **insufficient_resource_handling**: 模拟 64MB 内存时干净失败并记录
  `resource_insufficient` 事件，见 `artifacts/runs/accept-insufficient/`。
- **original_unmodified**: `materials/original.wav` 运行前后 SHA-256 恒为
  `2d6bd5fe…`；只读原件 `materials/original.pristine.wav`（chmod 444）。

## 证据与状态

- 每次运行一个目录 `artifacts/runs/<run-id>/`：`state.json`（检查点状态）、
  `evidence.jsonl`（带 UTC 毫秒时间戳的结构化证据）、`checkpoints/`、`output/`。
- 关键事件：`run_start`、`resource_probe`、`degradation_applied`、
  `anchor_start/anchor_done`（含样本偏移、秒、ns、SHA-256）、`paused`、
  `resume`、`resource_insufficient`、`run_complete`。

## 原始材料

`materials/original.wav`（6.000000 s，48 kHz mono S16，由
`audiotestsrc wave=5 freq=220` 生成）。引擎只读打开；每次运行前校验其
SHA-256 与 `materials/ORIGINAL.sha256` 中登记的只读原件哈希一致，
否则拒绝启动/恢复。
