# 多路画面同步墙 · 来源检查页

技术栈：Rust + GStreamer + GTK4。

## 功能

- **截断记录**：采集端把每路帧事件逐行写入 JSONL 日志，可模拟崩溃导致末行只写了一半（`--truncate-at`），截断的半行原样保留为关键证据。
- **恢复重放**：从原始日志的 checkpoint 恢复，续采剩余帧写入重放日志（`--replay-of`），以 `recovered` 事件标记重放起点。
- **证据链**：每条结论都携带 `证据(文件:行号 + 原始行)` 与产生它的处理步骤，可逐条回溯到原始输入。
- **无法判断的边界**：截断点与重放起点之间的窗口标记为 `Indeterminate`，边界两侧的关键证据（截断前最后完整帧、截断半行、重放标记、重放首帧）全部保留。
- **第一个错误状态**：报告按时间排序定位第一个错误（丢帧 / 漂移 / 截断），而不是只给总分。

## 构建与运行

```bash
cargo build
# 生成演示数据：3 路、40 帧，通道1丢 seq12，seq18 处卡顿 150ms，seq25 处截断
./target/debug/syncwall-recorder artifacts/session-truncated.jsonl \
    --channels 3 --frames 40 --drop 12 --stall-at 18 --stall-ms 150 --truncate-at 25
# 恢复重放
./target/debug/syncwall-recorder artifacts/session-replay.jsonl \
    --channels 3 --frames 40 --replay-of artifacts/session-truncated.jsonl
# GTK4 检查页（默认加载 artifacts 下两份日志）
./target/debug/syncwall-inspector
# 无显示环境下的文本报告
cargo run --example cli_report
```

## 代码结构

- `src/model.rs` — 原始事件、结论、证据引用、处理步骤
- `src/journal.rs` — JSONL 日志读写，容错截断行
- `src/analyze.rs` — 分析管线：截断扫描 → 恢复拼接 → 序列检查 → 漂移检查 → 首个错误定位
- `src/gst_src.rs` — GStreamer 多路 videotestsrc 采集与重放
- `src/main.rs` — GTK4 来源检查页
- `tests/analyze_test.rs` — 截断+重放、干净会话两组端到端测试
