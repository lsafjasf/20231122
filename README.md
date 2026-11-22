# 转码队列 · 局部修正

面向“同一重复对象、两台设备时钟不同步”场景的本地桌面工具。界面固定采用
**Rust + GStreamer + GTK4**，每个媒体片段可以单独重跑、比较和导出；会话在中途
退出后重新载入，片段字段、顺序、运行记录和关键证据都会保留。

## 功能

- 为设备 A/B 分别设置 `clock_offset_ms`，实时计算修正后的时间线。
- 只对选中片段设置优先级覆盖值，不修改原始优先级，也不重排片段列表。
- 队列按“生效优先级降序、修正后起点升序、原始顺序升序”计算，结果稳定可复算。
- 单片段重跑使用 GStreamer/VP8/WebM，按 `in_ms`/`out_ms` 解码后裁剪。
- 每次运行保留输入/输出 SHA-256、字节数、耗时、GStreamer 版本、完整管线、错误和首帧。
- 最新运行与更早运行并排比较；失败运行不会伪造成功记录。
- 每次交互后原子保存会话，并保留上一版 `.bak`；进程中断后会把残留的 `running`
  片段恢复为 `pending`，同时追加“中断恢复”证据。
- 导出可自包含复核包，包含固定列顺序的 CSV、JSONL 证据、媒体副本和校验清单。

## 运行

系统依赖：

```bash
sudo apt install libgtk-4-dev libgstreamer1.0-dev \
  gstreamer1.0-plugins-base gstreamer1.0-plugins-good \
  gstreamer1.0-plugins-bad gstreamer1.0-libav
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

启动桌面界面：

```bash
cargo run --release
```

首次启动会在 `artifacts/` 创建示例会话和两段本地示例媒体，可直接点击“重跑此片段”
体验完整链路。默认会话路径是 `artifacts/queue_session.json`，可用
`QP_SESSION=/path/session.json` 或 `--session` 覆盖。

```bash
cargo run --release -- --session /path/session.json
```

## 操作约定

### 时钟

```text
offset_ms = 设备时钟读数 - 共享基准时钟
corrected_start_ms = raw_capture_start_ms - offset_ms + in_ms
```

正值表示设备时钟偏快。界面顶部显示 `A−B` 的当前差值；修改偏移会把旧值、新值、
符号约定和备注追加到证据日志。

### 局部优先级

`base_priority` 是原始值，`priority_override` 是可选的局部覆盖值，
`effective_priority = priority_override ?? base_priority`。修改覆盖值只影响该片段，
证据日志记录修改前后的生效值。列表本身始终按 `segment.ordinal` 展示，队列名次用
`Qn` 标记。

### 单段重跑与比较

一次只运行一个片段。运行开始前先持久化 `running` 状态和 `RunStarted` 证据；成功
后写入 `RunRecord`、输出哈希、首帧和 `RunFinished` 证据。若进程在运行中退出，
下次载入会追加“中断恢复”事件，而不是补写成功结果。

比较区并排显示同一片段最近两次运行；没有上一版时明确显示为空。媒体输出固定为
640×360 VP8/WebM，`in_ms`/`out_ms` 通过 GStreamer buffer probe 在解码后裁剪。

## 复核包

界面“导出复核包”导出整个会话，详情区“单段导出”可只导出一个片段。目录内容：

```text
session.json          完整持久化快照，字段与数组顺序原样保留
manifest.json         会话摘要、队列、文件哈希、媒体副本索引
segments.csv          固定列顺序；行顺序严格等于 session.json 的 segments 数组顺序
runs.csv              固定列顺序；行顺序严格等于运行发生顺序
evidence.jsonl        每行一个追加事件；fields 数组顺序即字段顺序
media_index.csv       原始路径、包内路径、SHA-256、字节数
REVIEW.md             时钟公式、字段契约、复核步骤和精确重放命令
media/                成功产物的自包含副本
previews/             首帧副本
checksums.sha256      除自身外全部证据与媒体副本的 SHA-256
```

复核：

```bash
queue-priority-corrector --verify <复核包目录>
# 或
cd <复核包目录> && sha256sum -c checksums.sha256
```

精确重放某个片段（使用会话中该次运行保存的区间）：

```bash
queue-priority-corrector \
  --session session.json \
  --replay-run run-0001 \
  --output replay.webm
```

容器元数据可能使重放文件哈希与原件不同。复核应同时核对输入哈希、裁剪区间、
GStreamer 版本、完整管线以及包内媒体副本的哈希；`REVIEW.md` 已写明这些步骤。

无图形界面的导出：

```bash
queue-priority-corrector \
  --session artifacts/queue_session.json \
  --export artifacts/exports/review-manual
```

导出目录必须不存在或为空，工具不会覆盖已有复核包。

## 数据与实现

- `src/model.rs`：稳定领域模型、队列规则、证据事件和会话恢复。
- `src/store.rs`：临时文件 + `fsync` + 原子替换，损坏时回退 `.bak`。
- `src/transcode.rs`：GStreamer 管线、裁剪探针、VP8/WebM 编码和首帧提取。
- `src/export.rs`：固定顺序 CSV/JSONL、媒体副本、清单和校验。
- `src/ui.rs`：GTK4 界面、异步单片段任务、自动保存和比较视图。
- `src/demo.rs`：首次运行示例，不依赖外部网络或素材。

## 验证

```bash
cargo fmt --check
cargo test
cargo clippy --all-targets
```

测试覆盖会话顺序往返、队列稳定性、局部覆盖、偏移计算、中断恢复、GStreamer
裁剪/首帧、复核包字段顺序、媒体副本与篡改检测。

当前实现聚焦视频片段：音频流不会被混入产物。一次只运行一个转码任务，避免多个
GStreamer 管线争用同一会话证据；需要批量处理时可按片段逐个触发。
