# srcrepair — 音频采样率转换局部修正工具

针对两台设备时钟不同步造成的**缺失片段（gap）**做局部修正：每个媒体片段
（fragment）都能单独重跑、比较和导出；支持中途退出与重新进入，并为每次处理
保留可独立复核的关键证据。

技术栈固定为 **Rust + GStreamer + GTK4**。

## 构建

需要 GStreamer（含 `gstreamer-audio`、`gstreamer-pbutils`）与 GTK4 开发库：

```sh
cargo build --release
```

## 运行

图形界面（默认）：

```sh
./target/release/srcrepair
```

无显示器/脚本化场景使用同一套引擎的 headless 子命令：

```sh
srcrepair cli init    <项目目录> [目标采样率，默认48000]
srcrepair cli add     <目录> <设备标签> <源文件> <源采样率> <gap起点ns> <gap时长ns> <原因>
srcrepair cli run     <目录> [片段id]   # 不传 id 则顺序处理全部；传 id 即单独重跑
srcrepair cli verify  <目录>           # 用记录的 SHA-256 复核磁盘产物
srcrepair cli export  <目录>           # 生成自包含复核 bundle
```

## 工作流与关键性质

- **独立片段**：片段在表单中登记源采样率、目标采样率（项目统一）、缺失片段的
  起点/时长（纳秒）与原因；每行有独立 Run/Rerun 按钮，互不影响。
- **确定性处理**：GStreamer 负责解码与重采样；缺失片段由 Rust 按精确 PCM 帧数
  合成静音并在帧边界拼接。相同输入重跑得到字节一致、SHA-256 相同的输出。
- **可中断/可重入**：进入 `running` 立即落盘；每次状态转换都原子写回
  `project.srcrepair.json`。崩溃或退出后重开，残留的 `running` 自动转为
  `failed` 并提示重跑，不会留下被当作有效的半成品证据。
- **字段与顺序保持**：serde 按结构体声明顺序序列化，片段存于有序 `Vec`，
  重新载入后字段名、字段顺序、片段顺序完全一致。
- **证据**：每次尝试（含失败后的重跑）追加一条 `RunEvidence`，记录流水线描述、
  GStreamer 版本、输入/输出哈希、帧数、期望帧数、插入静音帧数、字节数与时间戳。

## 复核导出

`export` 生成 `exports/review-<时间戳>/`，同事无需本工具、无需 Rust/GStreamer，
仅凭 POSIX `sh` + `sha256sum` 即可独立复核：

```text
manifest.json     完整项目状态与逐条证据（含流水线字符串）
comparison.csv    固定列顺序的对比表（帧数是否匹配等）
SHA256SUMS        标准 sha256sum 格式的输出哈希清单
verify.sh         独立校验脚本，./verify.sh 即逐条比对
fragments/<id>/output.wav
```

详见 [DESIGN.md](DESIGN.md)。
