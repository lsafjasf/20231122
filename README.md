# ClockBridge

面向双机时钟不同步场景的“转码错误重试策略”局部修正工具。技术栈固定为 **Rust + GStreamer + GTK4**。

## 它解决什么

- 以设备 A 为主时间线，设备 B 通过 `clock_offset_ms` 对齐。
- 在双机录像中划出一个明确的缺失片段，分别计算两机的实际出入点。
- 每个片段、每台设备都能单独重跑，不覆盖旧尝试。
- 任意两次尝试可生成比较记录；成功尝试可选为修复基准。
- 工作区每次状态变化都原子落盘，证据日志追加写入。
- 窗口中途退出后重新进入，未完成任务会保留并标记为“中断”。
- 可导出整工作区或单个片段的复核包，同事可用校验和、CSV、JSONL 和可重放管线字段独立复核。

## 环境

- Rust 1.85+（edition 2024）
- GTK 4.10+
- GStreamer 1.20+，至少包含 `base`、`good`、`libav` 插件
- Linux 桌面环境

Ubuntu/Debian 参考：

```bash
sudo apt install build-essential pkg-config libgtk-4-dev \
  libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
  gstreamer1.0-tools gstreamer1.0-plugins-base \
  gstreamer1.0-plugins-good gstreamer1.0-libav
```

## 运行

首次直接启动会在当前目录创建 `retry-workspace/`，并载入 `examples/media` 中的双机演示素材：

```bash
cargo run --bin clockbridge
```

打开已有工作区：

```bash
cargo run --bin clockbridge -- --workspace /path/to/retry-workspace/workspace.json
```

也可以先通过命令行创建、重跑、比较和导出：

```bash
cargo run --bin retryctl -- demo --root retry-workspace \
  --a examples/media/device-a.mp4 --b examples/media/device-b.mp4
cargo run --bin retryctl -- run --workspace retry-workspace/workspace.json \
  --segment GAP-001 --device device-a
cargo run --bin retryctl -- compare --workspace retry-workspace/workspace.json
cargo run --bin retryctl -- export --workspace retry-workspace/workspace.json \
  --out retry-review
cargo run --bin retryctl -- verify retry-review
```

## 界面操作

1. 在左侧为设备 A、B 选择媒体文件。
2. 输入设备 B 的时钟偏移。方向为 `设备时间 = 主时间线 + 偏移`；正值表示 B 机时码更快。
3. 输入缺失片段的主时间线起点和时长，点击“应用时钟与缺失范围”。
4. 在右侧对 A/B 单独重跑，或点击“重跑双机”。
5. 用两次尝试生成比较记录；需要时把成功尝试设为基准。
6. 点击“导出本段证据”或顶栏“导出复核包”。

## 持久化与证据

工作区目录结构：

```text
retry-workspace/
  workspace.json                 # 原子替换写入的权威快照
  evidence/
    events.jsonl                 # 只追加的证据事件，seq 严格递增
    attempts/
      attempt-<id>.json          # 单次尝试的结构化证据
      attempt-<id>.result.json   # 后台线程先落盘的结果证据
      attempt-<id>.dot           # GStreamer 管线图（GUI 任务）
  runs/
    segment-001/
      attempt-001-device-a.mp4
```

- `workspace.json` 先写同目录临时文件、`fsync`，再原子 `rename`。
- 每次重跑开始前先保存 `Running` 尝试；进程退出后，下次载入会自动转为 `Interrupted` 并追加证据事件。
- 若后台管线已完成但界面尚未来得及回写，重进时从 `.result.json` 恢复成功/失败状态、输出指纹和实际管线。
- 证据日志损坏时工具拒绝静默跳过，以免破坏复核链。
- 尝试记录包含源文件整文件 SHA-256、精确时间范围、实际 GStreamer 管线、输出大小与 SHA-256、耗时和错误。

## 复核包

导出目录固定包含：

```text
README.md
manifest.json
segments.csv
attempts.csv
comparisons.csv
evidence/events.jsonl
outputs/...
SHA256SUMS
```

独立复核：

```bash
cd retry-review
sha256sum -c SHA256SUMS
retryctl verify .
retryctl inspect-bundle .
```

`manifest.json` 由固定字段顺序的 Rust 结构体生成。片段、尝试、比较、证据事件和文件清单都是数组，重新载入时不会按 map 键重排。仓库测试会逐项检查该契约。

## 验证

```bash
cargo test --all-targets
```

测试覆盖双机偏移解析、工作区重载字段/顺序、证据追加顺序、未完成任务恢复、后台结果证据恢复、导出回载、SHA-256 校验和篡改检测。

## 实现说明

- GTK4 只负责界面与工作区交互；重跑在后台线程执行。
- GStreamer 管线由 Rust API 装配，不使用 shell 调 `ffmpeg`。
- 片段裁剪在编码前使用 buffer pad probe：按 PTS 丢弃入点前帧，到达出点后向下游发送 EOS，避免 flush seek 重置封装器。
- 输出会检查顶层 MP4 `ftyp`/`moov` box 或 WebM EBML 头，空文件和未完成封装不会标记为成功。
