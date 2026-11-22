# llsm-replay — 低延迟推流监控可回放实验

技术栈：Rust + GStreamer（低延迟管线与探针）+ GTK4（监控窗口）。

## 实验设计

- **两个版本快照对照**：`snapshots/v1.json`（30fps 基线）与 `snapshots/v2.json`
  （60fps 候选），差异写入证据 `snapshot-diff.json`，两条管线分别运行并对比延迟统计。
- **未知扩展**：`objects/stream_gamma.xyzq` 使用未注册的扩展名，加载器抛出可恢复
  错误，记入日志后**继续处理其余对象**。
- **每步记录**：`journal.jsonl` 每行一条步骤记录，包含输入、状态快照、结果，
  以及输入/结果的 SHA-256 摘要，是回放的唯一事实来源。
- **重复提交验证**：`verify-duplicates` 重复提交首个对象，日志将其标记为
  `duplicate` 而非重复执行。
- **顺序改变验证**：`verify-order` 正序与逆序处理对象列表，比较最终状态摘要，
  结果写入 `order-verification.json`。
- **证据保留**：`artifacts/<run-id>/` 下保留日志、报告、快照差异、逐缓冲延迟
  CSV（`latency-v1.csv` / `latency-v2.csv`）。
- **临时状态清理**：实验结束删除 `tmp/<run-id>/`，证据目录保留。

## 运行

```sh
cargo run -- run                    # 完整实验：记录日志、验证、保留证据、清理临时状态
cargo run -- replay artifacts/<run-id>/journal.jsonl   # 回放并校验摘要
cargo run -- monitor                # GTK4 监控窗口（无显示环境自动跳过）
cargo run -- clean                  # 清理遗留临时状态
```

## 环境说明

本机无系统级 GStreamer/GTK4 开发包时，可将发行版 deb 解到 `~/sysroot`
并 `source ~/sysroot/env.sh`（本仓库实验即在此环境下运行：
GStreamer 1.28.2 / GTK4 4.22.4 / rustc 1.100）。
