# PEAKGUARD · 音频峰值预警局部修正台

一个用 **Rust + GStreamer + GTK4** 实现的桌面工具：把媒体切成独立片段，对缺失片段设置一组极限参数，逐段分析、限幅、比较和导出，并在退出/重进后恢复任务与关键证据。

## 功能

- **缺失片段定位**：演示项目内置 4 秒静音缺口；导入媒体时自动把最低能量的片段标记为“疑似缺失片段”。
- **极限参数组**：预警阈值、峰值上限、补偿增益、启动/释放时间、边界窗口均随每次运行冻结记录。
- **逐段独立运行**：每个片段有自己的 GStreamer 分析与渲染任务、状态、输出 WAV、运行历史和错误信息。
- **边界双样本**：每个片段入口/出口边界前后各取 1 个 `level` 样本，修正后再测一次并写入变化量。
- **局部修正手柄**：输出 WAV 在核心片段前后各保留一个边界窗口作为修复手柄，方便比较边界过渡。
- **退出恢复**：处理前把 `running` 状态落盘；异常退出后重新打开会自动恢复为 `interrupted`，已完成的片段和历史证据不丢。
- **比较与导出**：对比原始/修正峰值、RMS、预警数、截断样本和四次边界变化；逐段或全部导出 WAV + JSON 证据 + Markdown 报告。
- **自动保存**：会话 JSON 使用临时文件原子替换，所有关键动作都追加到证据轨迹。

## 运行

系统依赖（Ubuntu/WSL）：

```bash
sudo apt install \
  libgtk-4-dev libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
  libgstreamer-plugins-good1.0-dev gstreamer1.0-plugins-good \
  gstreamer1.0-plugins-base gstreamer1.0-libav pkg-config
```

Rust 工具链安装后：

```bash
cargo run
```

首次启动会在 `artifacts/peakguard/` 生成演示会话，并生成带静音缺口的 24 秒演示 WAV。点击“运行全部”即可看到六个片段分别完成。

数据目录可以用环境变量覆盖：

```bash
PEAKGUARD_DATA_DIR=/path/to/data cargo run
```

## 证据与恢复

```text
artifacts/peakguard/
├── session.json                 # 项目、参数、片段状态、最近运行和证据轨迹
├── demo/site-c07.wav            # 内置演示媒体
├── segments/                    # 每段独立修正输出（含边界手柄）
│   └── segment-01-corrected.wav
└── exports/                     # 显式导出的 WAV / JSON / Markdown
```

运行链路：

1. GTK 主线程把片段任务提交给后台线程。
2. GStreamer `level` 以 20 ms 窗口采集原始峰值与 RMS。
3. `volume` 应用补偿增益，`identity` pad probe 执行带启动/释放包络的硬限幅，并统计截断样本。
4. 修正结果重新经过 `level` 分析，入口/出口各取边界前后样本。
5. 每完成一段立即原子保存会话；中途关闭后重新进入会恢复为可重跑状态。

## 验证

```bash
cargo test
```

测试覆盖真实 GStreamer 峰值采集、片段渲染、边界样本，以及 `running → interrupted` 的会话恢复。GUI 还支持用 `PEAKGUARD_SCREENSHOT=/tmp/ui.png` 将渲染结果保存为 PNG，便于无头环境回归检查。
