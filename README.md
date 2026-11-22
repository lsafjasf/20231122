# VKRI · 视频关键帧索引重建局部修正工具

针对**同一重复对象、两台时钟不同步设备**的媒体片段，提供可对每个片段单独重跑、比较与导出
的关键帧索引重建工具。支持中途退出与重新进入、全程保留关键证据，导出物可由同事独立复核。

技术栈固定为 **Rust + GStreamer + GTK4**。

## 能力与需求对应

- 两台设备时钟不同步：每设备独立 `clock_offset`，按 `参考时间 = 设备时钟起点 + PTS + 偏移`
  校正到统一时间轴；比对给出中位时差与偏移建议。
- 每个媒体片段单独重跑：片段是独立工作单元，有独立状态、配置、关键帧与审计；`run` 只作用于
  选定片段，历史 `run-*` 证据目录永久保留。
- 中途退出 / 重新进入：`Ctrl-C`（CLI）或停止按钮 / 关窗（GUI）触发取消令牌，管线干净拆除、
  部分证据落盘并标记 `interrupted`；再次运行即恢复，重新载入后字段与顺序逐字节保持。
- 保留关键证据：源文件 SHA-256、JPEG 缩略图内容哈希、每次运行的 `manifest.json`、
  追加式哈希链 `audit.jsonl`。
- 同事独立复核：导出包含 `SHA256SUMS`（可直接 `sha256sum -c`）、自哈希 `MANIFEST.json`、
  平面 `keyframes.csv` / `comparisons.csv` 与中文 `REVIEW.md`，无需本程序即可核验。

## 仓库结构

```text
Cargo.toml                 工作区清单
crates/vkri-core/          核心库：模型/存储/审计/GStreamer/比对/导出
crates/vkri-cli/           终端工具 vkri
crates/vkri-gui/           GTK4 图形工具 vkri-gui
docs/ARCHITECTURE.md       架构与设计细节
docs/CLI.md                CLI 命令手册
scripts/demo.sh            一键可复现演示（合成素材→全流程→复核）
```

## 前置依赖（系统库）

- Rust（stable，含 cargo）
- GStreamer 1.24+ 开发库与插件：`gstreamer-1.0`、`gstreamer-app-1.0`、
  `gstreamer-pbutils-1.0`，以及 `decodebin3`、`videoconvertscale`、`jpegenc` 等插件
- GTK4 开发库（`gtk4`，本项目在 4.22 上验证；Rust 绑定为 `gtk4` 0.11 / `glib` 0.22）

Ubuntu 示例：

```sh
sudo apt install build-essential pkg-config libgstreamer1.0-dev \
  libgstreamer-plugins-base1.0-dev libgstreamer-plugins-bad1.0-dev gstreamer1.0-plugins-* \
  libgtk-4-dev
```

## 构建与测试

```sh
cargo build --workspace
cargo test --workspace
```

产物：`target/debug/vkri`（CLI）、`target/debug/vkri-gui`（GUI）。

## 快速上手

一键演示（自动用 GStreamer 合成两段视频并跑完整流程）：

```sh
bash scripts/demo.sh
```

手动流程见 `docs/CLI.md`。常用命令：

```sh
vkri init <project_dir> --name case7
vkri add-device <project_dir> --label camera-A
vkri register <project_dir> ...      # 注册片段
vkri run <project_dir> --segment-id <seg>
vkri compare <project_dir> --segment-a <a> --segment-b <b>
vkri export <project_dir> --export-root <dir> --bundle <name>
vkri verify-export <dir>/<name>
```

GUI：

```sh
vkri-gui          # 有显示环境直接启动；无头可用 xvfb-run -a vkri-gui
```

## 项目磁盘布局

```text
<project_dir>/
  project.json                 原子写入，字段/顺序固定
  evidence/
    _global/audit.jsonl
    <segment_id>/
      audit.jsonl              追加式哈希链
      run-<run_id>/
        kf-000001.jpg ...
        manifest.json
```

导出包额外包含 `project.snapshot.json`、`keyframes.csv`、`comparisons.csv`、`SHA256SUMS`、
`MANIFEST.json` 与 `REVIEW.md`。
