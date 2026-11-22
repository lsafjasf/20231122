# Credits Review

把“片尾滚动字幕排版”做成可复核流程：对项目中的每个媒体对象记录 **before / after 两个版本快照**，并用统一的关联记录绑定时间点、轨道或帧号、处理参数、波形证据与文件哈希。未知扩展名不会参与判断，实际格式由 GStreamer 内容探测决定。

技术实现固定为 **Rust + GStreamer + GTK4**：

- Rust 负责项目解析、证据模型、哈希、异常隔离与 CLI。
- GStreamer 负责内容探测、解码抓帧、音频波形提取。
- GTK4 / Pango / Cairo 负责滚动字幕排版与双版本复核界面。

## 快速开始

系统依赖（Ubuntu/Debian）：

```bash
sudo apt install rustc cargo \
  libgtk-4-dev libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
  gstreamer1.0-tools gstreamer1.0-plugins-base gstreamer1.0-plugins-good
```

运行新格式示例（其中包含一个有效未知扩展对象、一个缺失对象和一个随后继续处理的对象）：

```bash
cargo run -p credits-app -- review examples/project-v2.json --out review-out
cargo run -p credits-app -- verify review-out
```

旧格式：

```bash
cargo run -p credits-app -- review examples/project-v1.json --out review-v1
```

直接传入扩展名未知的媒体文件：

```bash
cargo run -p credits-app -- review fixtures/tail-roll.rollunk --out review-bare \
  --checkpoint 00:00:00.750 --checkpoint frame:36
```

GTK4 复核界面：

```bash
cargo run -p credits-app -- gui review-out
# 也可以直接传项目或媒体文件，先分析再打开：
cargo run -p credits-app -- gui examples/project-v2.json --out review-gui
```

## 可复核契约

每次运行产生：

```text
review-out/
  review.json                 # 关联总报告
  review.json.sha256          # 报告自身校验和
  normalized-project.json     # 规范化输入，保留兼容层结果
  evidence/
    0000-<object-id>/
      probe.json              # GStreamer 探测结果
      checkpoint-000-frame-00000024-before.png
      checkpoint-000-frame-00000024-after.png
      waveform.json           # 峰值、时长、标定点
      waveform.svg            # 可人工查看的波形
      failure.json            # 仅失败对象，保留已产生的证据
```

`review.json` 中每个 `correlations[]` 同时包含：

- `time`：请求时间、时钟时间、帧时间码、帧号、PTS、fps、轨道。
- `versions[]`：`before` 原始帧与 `after` 排版帧，各自带 PNG 路径、SHA-256、尺寸、PTS、帧号、时间码、轨道、处理参数和管线描述。
- `parameters`：字体、字号、颜色、阴影、滚动速度、起始时间、边距、行距、透明度。
- `waveform_file` / `waveform_peak_index` / `waveform_window`：对应波形文件、该时间点的峰值索引与局部包络。
- `pixel_diff`：两个版本的平均通道差与变化像素比例。

顶层 `cleanup` 记录临时目录、是否删除、残留列表和完成时间；关键证据始终在临时目录之外。顶层 `evidence_files[]` 保存每个证据文件的 SHA-256 与大小，`verify` 子命令会逐项复核。

## 输入兼容

支持三种入口：

1. `credits-roll/v2`：`schema` 为 `credits-roll/v2`，使用 `timeline.tracks[].objects[]`、`review_points`、`typeset`。
2. 旧版 v1：`items[]` 或 `clips[]`，接受 `file/path/src`、`start/in/begin`、`end/out/finish`、`track`、`snapshot`、`font`、`speed` 等别名。
3. 裸媒体：输入不是 JSON 时，自动视为单个媒体对象；扩展名只写入证据，不参与格式判断。

时间值支持秒数、`HH:MM:SS.mmm` 和 `frame:N`。新格式还支持 `{"frame": 48}` 形式的检查点。

## 异常隔离与清理

- 每个对象在独立 `catch_unwind` + `Result` 边界内处理；对象失败只写 `failure.json` 和 `failures[]`，已落盘的 probe/波形/快照仍纳入哈希清单，后续对象继续。
- 没有音频流时输出 `waveform.available=false`；有音频但提取失败时对象标记为 `partial`，快照仍保留。
- `WorkArea` 用 RAII 管理 `.credits-review-work-*`；正常结束、提前返回和 panic 展开都会尝试删除。`--strict` 在存在失败或 partial 对象时返回退出码 3。

详细字段见 [docs/review-format.md](docs/review-format.md)。

## 验证

```bash
cargo test --workspace
cargo run -p credits-app -- verify review-out
```

集成测试会生成包含未知扩展、缺失对象和后续对象的项目，验证双版本快照、帧/时间码、波形、失败隔离、临时目录清理与哈希复核。若当前环境缺少 GStreamer 插件，测试会明确跳过端到端部分。
