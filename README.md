# 字幕安全区审查 · 局部修正工具（subtitle-safe-review）

针对“缺失片段”级别的字幕安全区审查工具：为单个片段设置一组**极限参数**，
每个媒体片段都可**单独重跑、比较、导出**；支持**中途退出与重新进入**，
关键证据（边界快照、运行日志）持久保留。技术栈：**Rust + GStreamer + GTK4**。

## 功能映射

- **极限参数**：每片段独立的 `ExtremeParams`（四向安全区内缩、亮度阈值、
  违规占比、边界采样偏移），内置「极限收紧 / 极限放宽」两档预设。
- **单独重跑**：`重跑本段` 仅对选中片段执行 GStreamer 逐帧分析。
- **比较**：保留最近两次运行结果，`比较上次` 输出违规率/墨迹/边界变化量。
- **导出**：`导出本段` 生成 `report.json` + 全部边界证据 PNG。
- **中途退出/重进**：每次改动即原子写 `session.json`；关窗自动保存；
  重开项目目录即恢复媒体、片段、参数与历史结果。
- **边界前后各取一个样本**：入点/出点各按 `boundary_epsilon_ms` 前后抓帧，
  记录墨迹占比变化量（Δ），快照存入 `evidence/<片段id>/`。
- **关键证据**：`evidence/runs.jsonl` 只增不删，逐次记录每次重跑。

## 项目结构

```
src/model.rs     数据模型（片段 / 极限参数 / 边界样本 / 运行结果）
src/metrics.rs   纯 Rust 帧墨迹指标（安全区外高亮像素统计，可单测）
src/session.rs   会话持久化、证据目录、导出
src/analysis.rs  GStreamer 分析引擎（uridecodebin → appsink 逐帧 + 边界采样）
src/ui.rs        GTK4 界面（预览 + 安全区叠加 + 片段面板）
tests/           单元测试 + 真实媒体端到端测试
examples/        grab（单帧抓取调试）、snap（手动证据留存）
```

## 构建与运行

正常环境（有 sudo）：

```bash
sudo apt install cargo pkg-config libgtk-4-dev libgstreamer1.0-dev \
  libgstreamer-plugins-base1.0-dev gstreamer1.0-plugins-{base,good} \
  gstreamer1.0-gtk4 gstreamer1.0-x
cargo run
```

本仓库环境（无 sudo）已用本地 sysroot 解决依赖：

```bash
source env.sh          # 指向 ~/sysroot 的 pkg-config / 插件路径
cargo test             # 单元 + 端到端测试
cargo run              # 启动 GUI（需要显示服务器）
```

端到端测试依赖测试片，先生成：`bash scripts/make-test-media.sh`

## 使用流程

1. `打开/新建项目` 选择工作目录（自动创建 `session.json` 与 `evidence/`）。
2. `打开媒体` 载入视频；预览上出现安全区叠加框。
3. `新增缺失片段`，用 `设为入点/出点` 以当前播放位置划定范围。
4. 选预设（极限收紧/放宽）或手动微调参数，参数实时写回并保存。
5. `重跑本段` → 查看结果与边界证据；调整后再次重跑，`比较上次` 看变化。
6. `导出本段` 交付审查包；随时可退出，重进项目目录即恢复现场。
