# B 帧寻址可视化实验

一个可回放、可审计的 B 帧寻址实验。工程固定使用 Rust、GStreamer 1.x 和 GTK4：

- Rust 负责确定性寻址、逐对象异常隔离、JSONL 日志和回放校验。
- GStreamer 通过 `appsrc ! fakesink` 实际传递带 PTS/DTS 的缓冲，并校验时间戳未改变。
- GTK4 提供场景选择、逐步/自动回放，以及 DTS 行 × PTS 轴的 B 帧地址可视化。

## 两个版本

| 快照 | 策略 | 地址 | 顺序改变时 |
| --- | --- | --- | --- |
| `fixtures/snapshots/v1.json` | `decode-ordinal` | `D000`, `D001`, ... | B 帧地址随提交顺序移动 |
| `fixtures/snapshots/v2.json` | `pts-stable` | `P0000000000` 等 | B 帧地址保持稳定 |

两个快照使用同一组 8 个帧对象（两个 GOP，四个 B 帧），但策略不同。每个有效记录的载荷都有 SHA-256 校验；快照文件本身也有摘要写入证据。

## 实验场景

1. `baseline-v1`：v1 按解码顺序提交。
2. `baseline-v2`：v2 按解码顺序提交。
3. `reordered-v1`：同一批对象改按展示顺序提交，验证地址漂移。
4. `reordered-v2`：展示顺序下地址仍稳定。
5. `duplicates-v2`：重复提交一次 B 帧，再提交同一 `object_id` 的冲突内容，之后继续处理剩余对象。
6. `truncated-v2`：在有效记录之间插入一条真实截断的 JSON；解析失败后继续接收后续 B 帧。

每一步日志都同时保存：

- 原始输入及输入摘要；
- 提交前状态及状态摘要；
- 结构化结果（accepted / duplicate / conflict / invalid / finalized）；
- 提交后状态及状态摘要。

## 运行

Ubuntu/Debian 需要：

```bash
sudo apt install build-essential pkg-config libgtk-4-dev \
  libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
  gstreamer1.0-plugins-base
```

执行测试并重新生成证据：

```bash
cargo test --all-targets
cargo run --bin bframe-evidence -- evidence
cargo run --bin bframe-evidence -- --replay evidence
```

启动 GTK4 回放界面：

```bash
cargo run --bin bframe-lab
```

无 OpenGL 的远程桌面或 WSL 环境可使用：

```bash
GSK_RENDERER=cairo cargo run --bin bframe-lab
```

只跑不依赖 GUI/GStreamer 运行时的核心测试：

```bash
cargo test --no-default-features --lib --test experiment
```

也可直接运行 `scripts/run_all.sh` 完成测试、生成证据和二次回放。

## 证据

`evidence/experiment.json` 是完整实验包，包含所有场景的逐步日志、摘要、检查项、清理报告和 GStreamer 报告。每个场景另有独立 JSONL：

```text
evidence/
  experiment.json
  key-evidence.json
  baseline-v1.jsonl
  baseline-v2.jsonl
  reordered-v1.jsonl
  reordered-v2.jsonl
  duplicates-v2.jsonl
  truncated-v2.jsonl
```

`--replay` 会重新解析这些 JSONL，重新执行每个输入，并逐项比较结果与前后状态摘要。任何不一致都会返回非零退出码。

## 已固化结果

- v1 的四个 B 帧全部因顺序改变而换址，例如 `gop0-b1: D002 -> D001`。
- v2 的 B 帧地址在两个顺序下完全相同。
- 重复提交和冲突提交均未改变地址表；冲突后后续对象继续被接收。
- 截断对照记录归类为 `truncated_json`，之后 `gop0-b1` 与 `gop0-b2` 仍成功入表。
- 6 个场景共 59 条日志均可从磁盘 JSONL 精确回放。
- GStreamer 1.28.2 实测传递 8/8 个缓冲，PTS/DTS 全部保持不变。
- 临时目录中的 6 个日志文件被回放后删除，残留条目为 0。

## 实现结构

```text
src/model.rs      帧、快照、地址项和状态视图
src/engine.rs     地址分配、去重/冲突和逐对象异常处理
src/journal.rs    输入/状态/结果的 JSONL 结构
src/scenario.rs   六个场景、断言、临时暂存与证据包
src/replay.rs     从日志重建引擎并逐项校验
src/gst_probe.rs  appsrc/fakesink 时间戳往返探测
src/bin/bframe_gui.rs       GTK4 可视化与自动回放
src/bin/bframe_evidence.rs  证据生成与独立回放 CLI
```

该实验只验证帧元数据寻址与提交语义，不对压缩视频做解码；B 帧通过帧类型、DTS、PTS 和参考关系描述。
