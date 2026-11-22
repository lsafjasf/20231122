# 视频截图色彩取样 · 可暂停处理流程

固定技术栈：**Rust + GStreamer 1.x + GTK4**。Rust 负责计划、检查点、
降级策略、哈希与验收；GStreamer 负责按精确 PTS 取帧和缩放；GTK4 提供
开始、暂停、恢复、参考跑的复核界面。

## 场景与固定锚点

取样对象是一段 320×240、10 fps、6 s 的 PNG-in-Matroska 视频。计划由
**5 个时间点 × 5 个坐标 = 25 个重复取样项**组成：

- 时间点：`0 / 100 / 3000 / 5800 / 5900 ms`
- 坐标：`(0,0) / (319,0) / (0,239) / (319,239) / (160,120)`
- 首锚点：`timestamp_ms=0`，`raw_offset=0`
- 末锚点：`timestamp_ms=5900`，`raw_offset=115680`（中心像素 RGB24 偏移）
- 暂停点：第 8 项之前，即 `next_index=8`；此时 100 ms 组只完成 3/5 项，
  恢复会重新 seek 到 100 ms，但只补第 4、5 项。

`raw_offset` 始终按原始 RGB24 帧计算：
`(y * 320 + x) * 3`。降采样后另记 `effective_x/effective_y` 和
`effective_offset`，后者按实际 RGBA 缓冲计算。

## 极限参数与降级

`ExtremeParams` 是本场景的固定极限参数组：

- `memory_budget_bytes=100000`：原始 RGBA 帧需要 307200 字节，因此触发降级。
- 自动选择 `25%` 缩放，得到 `80×60` RGBA 帧（19200 字节，预算内预留后仍有余量）。
- 末组使用 `final_seek_timeout_ms=u64::MAX`，表示不设 seek 超时上限。
- `pause_after=8` 故意落在组中间，验证帧重取和检查点续跑。
- 预算连最小帧都放不下时，继续以 `1%` 最小帧处理，并在每一项上标记
  `degraded_reason`，同时在证据中写 `resource_shortage`。

解码器优先 `avdec_png`；缺失时写入 `fallback` 证据并退回 `pngdec`。
所有事件追加到 `artifacts/evidence.jsonl`。

## 运行

依赖：Rust stable、GTK4/GStreamer 开发库、`ffmpeg`（仅首次生成原始素材）。

```bash
source scripts/env.sh
cargo build

# 一键：生成素材 -> 暂停 8/25 -> 恢复 -> 全量参考跑 -> 比对 -> 验收
cargo run -- demo

# 不重新处理，仅从现有产物重跑验收
cargo run -- verify

# GTK4 复核界面
cargo run -- ui

# 自定义单次运行
cargo run -- run --out artifacts/manual --pause-after 8 --mem-budget 100000
cargo run -- run --out artifacts/manual --resume

cargo test --all-targets
```

无 GL 的桌面环境可加 `GSK_RENDERER=cairo cargo run -- ui`。

## 原始材料完整性

- 原始文件固定在 `originals/source.mkv`，生成后设为只读 `0444`。
- 已存在时 `ensure_original` 只校验、不重写。
- `originals/SOURCE.json` 记录 SHA-256、字节数、尺寸、时长和确定性生成配方。
- `original_integrity` 验收会同时比较当前文件、计划内来源哈希和清单哈希。

当前素材 SHA-256：
`6edaca1f234675eac87f1b50db43cb5505f29d6af52f80e1e3aed603eb469b6c`。

## 关键证据

- `artifacts/checkpoint.paused.json`：暂停时 8/25 的原子中间状态。
- `artifacts/checkpoint.json`：恢复完成后的 25/25 状态。
- `artifacts/results.paused.json`：暂停前 8 项结果。
- `artifacts/results.json`：暂停/恢复后的完整结果。
- `artifacts/results.fresh.json`：不中断参考跑的完整结果。
- `artifacts/comparison.json`：暂停前缀前后一致性，以及恢复结果 vs 新跑结果。
- `artifacts/evidence.jsonl`：`run_started / fallback / degraded / pipeline_ready /
  sample / paused / resumed / run_finished` 全事件链。
- `artifacts/acceptance.json`：原始完整性、锚点、暂停恢复等价性和证据检查。
- `reference-artifacts/`：工作区原有参考产物，原样保留。

检查点保存时先用临时文件再原子替换；恢复时会校验来源 SHA、计划 SHA、
总数和游标。素材、参数或计划任一变化都会拒绝错误恢复。
