# 视频遮罩边缘审查

本地桌面工具，用 Rust、GStreamer 和 GTK4 实现。围绕三个锚点建立三个可独立重跑、比较和导出的媒体片段，并把每次运行落到具体时间点、原始偏移和可校验证据。

## 核心行为

- 三个片段各自保存一个锚点、入点偏移、出点偏移和遮罩参数。
- 绝对范围由 `anchor_ns + in_offset_ns`、`anchor_ns + out_offset_ns` 计算；界面同时显示毫秒内时间码、纳秒值、原始偏移和越界钳制状态。
- 每个片段可独立重跑，生成新的 VP8/WebM 候选视频；最近两次成功运行可比较参数、输出哈希和证据哈希。
- 每次重跑先落盘三张证据帧：入点、锚点、出点前一帧。证据写入 `evidence-manifest.json` 和追加式 `journal.jsonl`，随后才开始编码。
- 自动保存会话和播放位置。应用中断后重新进入，未完成运行会标记为“已中断”，已落盘证据仍被保留和校验。
- 导入时把原片按字节复制到 `original/`，计算 SHA-256 并设为 `0444`。每次重跑、导出和打开项目都重新校验；原片被篡改或变成可写时禁止继续渲染。

## 构建

Ubuntu/Debian 依赖：

```bash
sudo apt install \
  build-essential pkg-config libgtk-4-dev \
  libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
  gstreamer1.0-plugins-base gstreamer1.0-plugins-good \
  gstreamer1.0-libav gstreamer1.0-tools
```

`gstreamer1.0-plugins-good` 提供 VP8 编解码、PNG 编码和 `cairooverlay`。输入格式还需要对应的 GStreamer 解码插件，`gstreamer1.0-libav` 可覆盖大多数常见容器和编码。

```bash
cargo build --release
./target/release/mask-edge-review
```

无参数启动 GTK4 工作台并自动恢复最近项目；也可指定项目：

```bash
./target/release/mask-edge-review --project /path/to/project
```

## 使用流程

1. 点击“新建”，选择原片。工具先探测媒体，再创建项目并保全只读原始副本。
2. 在三个“片段”页签分别设置锚点：
   - 直接输入 `HH:MM:SS.mmm`；
   - 或把播放头移到目标帧后点击“设为播放头”。
3. 调整入/出偏移（毫秒，支持负数），界面立即显示绝对时间码、纳秒值、原始偏移和钳制状态。
4. 调整当前片段遮罩的形状、中心、半径、羽化、边缘位移和外区压暗。
5. 点击“重跑本段”。完成后可“比较最近两次”或“导出 WebM”。
6. 直接关闭窗口即可中途退出；下次启动会恢复项目、播放位置、片段选择、锚点、偏移和遮罩。

## 项目数据

```text
project/
  session.json                         # 原子替换的会话与运行记录
  journal.jsonl                        # 追加式关键证据日志
  original/<hash>-<name>               # 只读原片副本，0444
  runs/segment-01/run-0001/
    segment.webm                       # 该次重跑的候选输出
    evidence-manifest.json             # 先于编码落盘的证据清单
  evidence/segment-01/run-0001/
    start-<timestamp_ns>.png
    anchor-<timestamp_ns>.png
    end-<timestamp_ns>.png
  exports/
```

所有会话写入先写临时文件、`fsync`，再原子重命名。`journal.jsonl` 每行独立事件并立即 `sync_data`。运行开始时先记录 `run_started`；若进程被终止，下次打开项目会把对应运行改为 `interrupted` 并追加 `runs_interrupted`。

## 验收命令

`--inspect` 只读打开项目，不改 journal，输出机器可读 JSON：

```bash
./target/release/mask-edge-review --inspect /path/to/project
```

每个片段包含：

- `anchor_ns` / `anchor_timecode`
- `in_offset_ns` / `in_offset`
- `out_offset_ns` / `out_offset`
- `range.start_ns` / `range.start_timecode`
- `range.end_ns` / `range.end_timecode`
- `range.start_clamped` / `range.end_clamped`
- `runs[]` 中的请求范围、实测范围、输出 SHA-256 和证据帧
- `evidence_checks[]` 中的文件存在性、预期/实际 SHA-256 与 `valid`

验收判断建议：

```text
original.integrity.verified == true
len(segments) == 3
range.start_ns == anchor_ns + in_offset_ns
range.end_ns   == anchor_ns + out_offset_ns
所有 evidence_checks[].valid == true
每个已完成的 run 都有 output_sha256 和具体 range
```

原片完整性失败时 `--inspect` 返回退出码 `2`，正常返回 `0`。

也可无界面重跑一个片段，并保留项目内证据：

```bash
./target/release/mask-edge-review \
  --render /path/to/project --segment 2 \
  --output /tmp/segment-2-masked.webm
```

`--import` 用于自动化建立项目：

```bash
./target/release/mask-edge-review \
  --import /path/to/source.mp4 \
  --project-dir /path/to/new-project \
  --title "镜头 A 遮罩审查"
```

## 验证

```bash
cargo test
```

真实 GStreamer 渲染冒烟测试：

```bash
MASK_EDGE_TEST_VIDEO=/path/to/source.webm \
  cargo test --test render_smoke -- --ignored --nocapture
```

该测试会导入原片、重跑片段 1、生成证据与 WebM，并逐项校验输出和证据 SHA-256。

## 当前边界

- 审查与导出针对视频轨；音频不写入分段 WebM。
- 导出固定为 VP8/WebM，便于在常见 GStreamer 安装上稳定运行。
- 遮罩主要用于边缘审查和局部修正可视化；输出会烧录遮罩边缘、羽化带和外区压暗。
