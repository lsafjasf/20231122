# submerge — 字幕时间轴合并工具

把同一项目里中文、英文、日文字幕（素材帧率在 24 / 25 / 30 fps 之间切换）
换算到统一的纳秒时间轴，标出**重叠**、**缺失**（空档超阈值）和
**超出视频范围**的事件，并支持对选中的字幕区间做**局部重排**。

技术栈：Rust + GStreamer + GTK4。

## 项目结构

- `crates/submerge-core` — 核心库：帧率换算（`FrameRate`）、字幕事件
  （`SubtitleEvent`）、冲突检测与局部重排（`Timeline`）
- `crates/submerge-cli` — 无界面验证程序，输出语言 / 原始时间 / 换算时间 / 冲突原因
- `crates/submerge-gui` — GTK4 界面：GStreamer Discoverer 读取视频时长与帧率，
  列表展示冲突，支持调空档阈值与局部重排
- `scripts/env.sh` — 本地 sysroot（GTK4/GStreamer 开发库）环境配置

## 构建与运行

```bash
source scripts/env.sh          # 配置 cargo、pkg-config、本地 sysroot
cargo test -p submerge-core    # 核心逻辑单元测试（5 个）
cargo run -p submerge-cli      # 三种帧率 + 两段重叠字幕的验证报告
cargo run -p submerge-gui      # GTK4 界面（需要显示服务器）
```

## 设计要点

- 每条字幕以**源帧率下的帧号**存储（`start_frame`/`end_frame` + `FrameRate`），
  换算到统一时间轴时先乘后除（`frame * 1e9 * den / num` 纳秒），整数帧率下无精度损失。
- 冲突检测按统一时间轴排序后扫描相邻事件：
  - **重叠**：后一条开始早于前一条结束（区分同语言/跨语言）；
  - **缺失**：相邻空档超过阈值（默认 2 秒，GUI 可调）；
  - **超出范围**：结束时间超过视频时长。
- 局部重排 `Timeline::reorder_range(selection, new_order)`：把选中事件按新顺序
  从选区最早起点开始顺序重排，保留各自时长；写回帧号时起点向下取整、终点向上取整，
  保证覆盖不缩水。

## 验证结果（cargo run -p submerge-cli）

示例工程：120 秒视频，zh@24fps / en@25fps / ja@30fps 共 7 条字幕。

- 重叠：zh#2（4.000–7.000s）与 zh#3（6.250–9.000s）同语言重叠 0.750s；
- 缺失：ja#6 前空档 12.000s、ja#7 前空档 94.000s，超过 2s 阈值；
- 超出范围：ja#7 结束于 00:02:13.000，超出视频时长 00:02:00.000；
- 对 zh#2/zh#3 执行局部重排（交换顺序）后，重叠消除，其余事件不受影响。
