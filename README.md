# 滚动字幕安全区工作台

面向片尾滚动字幕的实时排版与安全区检查工具。技术栈固定为 **Rust + GTK4 + GStreamer**：Pango/GTK4 负责文本整形、折行与界面，GStreamer 负责播放时钟、视频帧管线和 H.264 导出。

## 功能

- 实时播放滚动字幕，并逐行绘制文字可视框、基线、行 ID 和安全区。
- 显示实时滚动速度、可见行数、安全区尺寸、基线坐标和越界统计。
- 中英文混排、双向文本、表情符号和超长无空格单行均由 Pango 原生整形与 `WordChar` 折行处理。
- 字体、字号、行距、画幅、滚动速度改变时，以当前可视锚点行重算播放头，不重置时间轴、不跳行。
- 内置三组验证内容：`mixed-cjk-english`、`emoji-heavy`、`super-long-line`。
- 可编辑字幕文本；“应用文本并重排”会把每个换行保留为硬换行。
- 一键导出排版参数、逐行时间窗、越界区间、调试图和 GStreamer MP4。

## 构建依赖

- Rust 1.85+
- GTK4 4.10+ 开发包（`pkg-config gtk4`）
- GStreamer 1.x 开发包及插件：`appsrc`、`videoconvert`、`openh264enc`、`h264parse`、`mp4mux`
- 推荐字体：`Noto Sans CJK SC`、`Noto Color Emoji`

Ubuntu/Debian 参考：

```bash
sudo apt install libgtk-4-dev libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
  gstreamer1.0-tools gstreamer1.0-plugins-good gstreamer1.0-plugins-bad \
  fonts-noto-cjk fonts-noto-color-emoji
```

## 运行

```bash
cargo run --release -- ui             # GTK4 实时工作台
cargo run --release -- report artifacts
cargo run --release -- demo artifacts # 报告 + 三张调试图 + stability audit + MP4
```

`report` 只生成 JSON；`demo` 会重新生成全部证据文件。默认窗口启动时定位到首行字幕中心。

## 导出内容

每个 `<sample>.report.json` 包含：

- `config`：画幅、字体、字号、行距、安全区 inset、滚动速度、片头/片尾留量。
- `canvas` 与 `safe_area`：逻辑像素坐标。
- `natural_widths`：每个硬换行的未折行宽度、安全宽度、来源溢出量和生成视觉行数。
- `lines[]`：每个视觉行的稳定 ID、硬换行索引、折行索引、字体大小、可视框、内容坐标基线、安全区进入/完全可见/离开/完全离开时间。
- `records[]`：逐行越界记录，含 `SafeAreaBelow`、`VerticalClip`、`SafeAreaAbove`、`HorizontalOverflow`、`TallerThanSafeArea`、`SourceWrapOverflow`。
- `records_by_line`：按 `line_id` 分组的越界记录。
- `summary`：横向溢出、来源折行、纵向裁切、安全区违规和涉及行数汇总。

`stability-audit.json` 在 4:3、21:9、125% 字号和 80% 字号下重排，并记录锚点行、重算时间和基线跳变量；容差为 1 px。

UI 的“导出参数 + 越界记录 + 预览”写入 `exports/`：

```text
<sample>-<aspect>-font<scale>.report.json
<sample>-<aspect>-font<scale>.params.json
<sample>-<aspect>-font<scale>.png
<sample>-<aspect>-font<scale>.mp4
```

## 实现说明

- `src/layout.rs`：安全区、Pango 布局、硬换行/视觉行模型、时间窗和锚点求解。
- `src/report.rs`：JSON 报表、逐行越界区间和稳定性审计。
- `src/render.rs`：Cairo/Pango 逐帧绘制，支持实时预览、PNG 与 GStreamer 帧输入。
- `src/gst.rs`：GStreamer 系统时钟和 `appsrc → openh264enc → h264parse → mp4mux` 导出。
- `src/ui.rs`：GTK4 工作台、控件、实时检查表和导出入口。

防跳行采用“硬换行 + 视觉行稳定 ID + 锚点重算”。布局变化时先记录视口中心附近行的基线，再用新布局求同一硬换行的播放时间，使该基线保持在原屏幕坐标；只有当前没有可锚定行时才保留时间比例。

## 验证

```bash
cargo test
```

测试覆盖三组样例的视觉行唯一性、基线顺序、超长单行字符级折行、比例/字号变化后的锚点稳定性，以及 JSON 中每行均有越界记录。
