# 黑场复核台

把“镜头黑场检测”做成一次可复核、可恢复的重放流程。技术栈固定为 **Rust + GStreamer + GTK4**。

界面以“恢复后的重放”为主线：左边锁定源文件与处理参数，中间用亮度波形串联时间点、轨道、帧和关键帧，右边把**自动结论**与**人工确认**严格分栏。每次分析都会自动封存证据包。

## 复核流程

1. `载入媒体`：接受任意扩展名。未知扩展（例如 `.recx`）不会被拒绝，而是写入 `source.extension`，再以 GStreamer `decodebin` 的内容探测结果为准。
2. `开始恢复后重放`：`filesrc → decodebin → videoconvert/videoscale/videorate → GRAY8 appsink`。只连接选定视频轨，逐采样计算亮度、黑像素比例和黑/非黑判定。
3. 自动检测：按亮度阈值、最短持续时间生成黑场区间；每个区间保存起点帧与最暗帧 PNG，并写入波形 CSV 和事件 JSONL。
4. 人工确认：复核人、状态、备注和逐区间判定独立存储，不修改自动结论。
5. `封存当前复核`：生成新的证据包，不覆盖旧证据。`manifest.json` 对报告、波形、关键帧、事件和源指纹做 SHA-256 封存。

## 构建与运行

依赖：

```sh
sudo apt install libgtk-4-dev libgstreamer1.0-dev \
  libgstreamer-plugins-base1.0-dev pkg-config \
  gstreamer1.0-tools gstreamer1.0-plugins-base gstreamer1.0-plugins-good jq
```

GTK 需 4.10+、GStreamer 需 1.20+。H.264/H.265 等素材还需要对应的解码插件（通常为 `gstreamer1.0-libav`）。

```sh
cargo run -- --ui
```

首次启动会载入一份“恢复后重放”演示数据，其中包含未知扩展 `.recx`、两段黑场和对应关键帧；点“载入媒体”即可替换为真实素材。

无界面分析与证据封存：

```sh
cargo run -- --analyze /path/to/recovered.recx \
  --output review-evidence \
  --threshold 0.10 --min-duration 500 --fps 5 --track 0 \
  --label "恢复盘 A / 交接批次 0040"
```

检查旧格式并迁移为统一 v2：

```sh
cargo run -- --inspect samples/legacy-v1.json
```

生成可独立检查的演示证据包：

```sh
cargo run -- --demo-bundle /tmp/demo-evidence
```

## 格式兼容

- **v1 旧报告**：支持 `version: 1`、`file`、`threshold`、`minimum_duration`、`black_segments`、`reviewed` 等字段。导入时自动转为 v2，并在 `compatibility.warnings` 明示“波形为区间重建示意，不是原始波形”。
- **v2 新报告**：使用 `schema_version: "2.0"`，逐采样保存 `time_ms / pts_ns / frame / track_id / params_fingerprint / y_mean / is_black`。
- 旧报告没有原始波形或逐帧证据时不会伪造：`waveform.source` 会标记为 legacy rasterization，报告建议重新执行恢复后重放。
- 示例见 `samples/legacy-v1.json`。

核心对应关系：

| 维度 | 字段 |
|---|---|
| 时间点 | `WaveSample.time_ms`, `BlackInterval.start_ms/end_ms` |
| 轨道 | `TrackRecord.id`, `WaveSample.track_id` |
| 帧 | `WaveSample.frame`, `BlackInterval.start_frame/end_frame` |
| 处理参数 | `DetectorParams`, `params_fingerprint` |
| 输出波形 | `waveform.csv`, `Waveform.samples`, GTK 波形图 |
| 关键证据 | `frames/*.png`, `events.jsonl`, `source.sha256`, `manifest.json` |

## 证据包

每次分析/人工封存生成：

```text
review-YYYYMMDD-HHMMSS-mmm-<report-suffix>/
  report.json          # v2 完整报告
  review.md            # 自动结论与人工确认分开呈现
  waveform.csv         # 输出波形与逐采样对应关系
  frames/*.png         # 区间起点 / 最暗帧
  events.jsonl         # 恢复后重放时间线
  source.sha256        # 源文件指纹、大小、caps
  README.txt
  manifest.json        # 全部关键文件的 SHA-256
```

校验：

```sh
./scripts/verify-manifest.sh review-evidence/review-...
```

## 失败与后续操作

失败不会只返回一条错误信息。报告中的 `failure.actions` 和 GTK 失败卡片会给出可执行步骤，例如：

- 源文件不可访问：重新选择文件、检查恢复副本。
- 缺插件：`gst-inspect-1.0 decodebin`、`gst-discoverer-1.0 -v <file>`，安装对应插件后重试。
- 无视频轨：切换轨道索引，并保留已写入的失败证据。
- 无 GRAY8 样本：降低采样帧率、检查输出像素格式。
- 证据目录不可写：更换目录并检查空间/权限。
- 源文件 SHA-256 不一致：从恢复副本取回原文件，禁止沿用旧报告指纹。

失败报告仍会封存源指纹、参数、失败码、事件和操作清单（若证据目录可写）。

## 验证

```sh
cargo test --lib --no-default-features
cargo test
```

单测覆盖旧格式迁移、v2 往返、自动/人工分离、未知扩展、亮度区间、关键帧 PNG、证据 manifest 哈希和失败操作。

界面内置按项目字符集子集化的 Noto Sans CJK SC，离线环境也能显示中文；字体许可见 `assets/fonts/OFL.txt`。
