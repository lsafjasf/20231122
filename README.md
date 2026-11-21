# plvcmp — 可暂停的播放列表版本比较

围绕一个**缺失片段**（missing segment）组织一组极限参数，用 GStreamer
逐条处理播放列表条目，支持**暂停 / 保存中间状态 / 恢复续跑**，并比较
两个播放列表版本（v1 → v2）的处理结果。技术栈：Rust + GStreamer + GTK4。

## 场景设计

- 播放列表 7~8 个条目，`seg-003` 的媒体文件**不存在**（缺失片段，索引 3）。
- 缺失片段前后各挂一组**极限参数**（`EdgeParams`）：
  - `seg-002`（边界前）：`chunk_bytes = u64::MAX` → 引擎钳制并记录；
  - `seg-003`（缺失）：`chunk_bytes = 0`、`timeout_ms = 0`、`expect_duration_ns = i64::MAX`；
  - `seg-004`（边界后）：`max_entry_bytes = 4096` → 触发降级路径；
  - `seg-005`：`timeout_ms = u64::MAX`（无超时上限）。
- **资源不足**：全局读取预算（`--budget-bytes`）故意调小，尾部条目
  （`seg-006` 等）预算耗尽时走**降级路径**（按预算截断读取，保证
  `bytes_read <= budget`），结果标记 `Degraded(reason)`。
- **边界采样**：以缺失片段为边界，前后各取一个样本（`seg-002` /
  `seg-004`），记录其在两个版本间的字段级变化。

## 处理流程

每个条目经 `filesrc ! fakesink` 管线处理，pad probe 统计字节数、
buffer 数并计算 SHA-256。每处理完一条就把 `Checkpoint` 原子写入
`checkpoint.json`；暂停（`--pause-after N` 或 UI 暂停按钮）后可用
`--resume` 从检查点继续。演示场景还会做**一致性校验**：暂停+恢复拼出的
结果必须与不间断参考运行完全一致。

## 使用

```sh
source scripts/env.sh          # 本地 sysroot 工具链（无需 root）
cargo build

# 一键完整场景：fixtures → 暂停 → 恢复 → 参考运行 → v2 → diff → 证据
./target/debug/plvcmp demo --root artifacts/demo

# 分步命令
./target/debug/plvcmp fixtures artifacts/demo
./target/debug/plvcmp run --playlist artifacts/demo/playlist_v1.json \
    --out /tmp/run_v1 --pause-after 3
./target/debug/plvcmp run --playlist artifacts/demo/playlist_v1.json \
    --out /tmp/run_v1 --resume
./target/debug/plvcmp compare --run-a /tmp/run_v1 --run-b artifacts/demo/run_v2 \
    --playlist-a artifacts/demo/playlist_v1.json \
    --playlist-b artifacts/demo/playlist_v2.json --out /tmp/diff.json

# GTK4 界面（需要显示服务器；本环境 GL 不可用时用 GSK_RENDERER=cairo）
./target/debug/plvcmp ui --playlist artifacts/demo/playlist_v1.json

cargo test
```

## 关键证据（artifacts/demo/evidence/）

- `checkpoint_v1_paused.json` / `checkpoint_v1_final.json` — 暂停时与结束时的中间状态；
- `results_v1.json` / `results_v2.json` — 两个版本的逐条处理结果（含降级原因与参数钳制记录）；
- `consistency_report.json` — 暂停/恢复 vs 不间断参考运行的一致性；
- `diff_report.json` — 版本间 Added/Removed/Changed/Unchanged 全量 diff；
- `boundary_samples.json` — 边界前后各一个样本的变化记录；
- `run.log` — 全部引擎事件的追加日志。

## 环境说明

沙箱无 root 权限，工具链为本地搭建：rustup 安装 Rust 到 `~/.cargo`，
GTK4/GStreamer 开发包用 `apt download` + `dpkg-deb -x` 解压到
`~/sysroot/root`，由 `scripts/env.sh` 注入
`PKG_CONFIG_PATH` / `PKG_CONFIG_SYSROOT_DIR` / `LD_LIBRARY_PATH` /
`GST_PLUGIN_PATH`。注意：环境变量里的代理 `127.0.0.1:7897` 已失效，
`env.sh` 会清除代理变量走直连。
