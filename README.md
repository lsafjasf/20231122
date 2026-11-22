# 实时转码背压监控 · 来源检查

一个用 **Rust + GStreamer + GTK4** 实现的桌面来源检查页。页面把一次实时转码检查拆成可追溯的证据链：原始输入、处理步骤、背压采样、中途取消、自动结论和人工确认都能互相回跳；无法判断的窗口不会被自动升级成故障结论。

![来源检查首屏](artifacts/screenshots/source-inspection.png)

处理步骤、关键证据与自动/人工分栏：

![证据链与分栏报告](artifacts/screenshots/source-inspection-trace.png)

证据缺失时的恢复动作：

![失败恢复动作](artifacts/screenshots/source-inspection-actions.png)

## 页面包含

- 两次明确记录的中途取消：`00:18.420` 的 RUN-01 和 `00:41.227` 的 RUN-02。
- 每个自动结论都引用 `IN-01` 原始输入、`STEP-xx` 处理步骤和 `E-xx` 证据。
- 单独的 `B-01` 不可判定边界：阈值越界后的取消遮蔽了排空观察，保留 380 ms 缺口。
- 自动结论与人工确认使用不同区域、颜色和标识，不混在同一判断中。
- 证据包不完整时提供四个可执行动作：重试来源探测、复制诊断命令、导出证据包、创建复核任务。
- 证据详情可查看原始载荷、来源引用、步骤引用，并可复制或回到来源上下文。

## 运行

需要 Rust 1.92+、GTK 4.22+、GStreamer 1.28+ 及开发头文件。当前环境使用预置媒体前缀 `/home/hj/env-media`：

```bash
./scripts/run.sh
```

如果依赖安装在别处：

```bash
MEDIA_PREFIX=/path/to/media-prefix ./scripts/run.sh
```

手动构建时，脚本会设置 `PKG_CONFIG_PATH`、`LD_LIBRARY_PATH` 和字体配置。直接构建可参考：

```bash
export PKG_CONFIG_PATH="$MEDIA_PREFIX/lib/pkgconfig"
export LD_LIBRARY_PATH="$MEDIA_PREFIX/lib"
cargo build --release
```

### 截图模式

应用内置离屏截图，便于在无窗口管理器的 WSLg 环境中检查页面：

```bash
BPI_HEIGHT=1000 \
./scripts/run.sh --screenshot artifacts/screenshots/source-inspection.png
```

`BPI_SCROLL=<px>` 可把内容滚动到指定位置，用于检查证据链或失败恢复区域。

## 交互

- `重新检查`：运行真实的本地 GStreamer 背压探针（`queue` 后接慢消费者），展示峰值、样本数和耗时。
- `重试来源探测`：通过 `uridecodebin` 在 3 秒内尝试 preroll；失败时展示原因并给出可复制的 `gst-launch-1.0` 命令。
- `复制诊断命令`：把 `data/session.json` 中保留的原始命令写入剪贴板。
- `导出证据包`：把原始输入、步骤、取消审计、自动结论和人工确认写入 `artifacts/exports/SRC-20261122-042.json`。
- `E-xx` / `STEP-xx` / `IN-01`：打开证据或来源上下文；证据详情中始终显示关联输入和步骤。

## 代码结构

```text
src/main.rs           GTK Application、CSS 载入、离屏截图
src/model.rs          会话、证据、结论与人工确认的数据模型
src/gst_probe.rs      GStreamer 背压探针与来源 preroll 检查
src/ui/shell.rs       顶栏、侧栏、页面框架和分区布局
src/ui/panels.rs      指标、曲线、取消记录、报告、边界和失败动作
src/ui/chart.rs       Cairo 背压曲线与 sparkline
src/ui/dialogs.rs     证据详情、来源上下文和动作结果
data/session.json     本次检查的只读样本与证据载荷
```

`data/session.json` 是可替换的检查记录；UI 和测试都从同一份记录读取，避免报告与证据脱节。页面中的 GStreamer 操作是实时执行的，不依赖预生成结果。

## 验证

```bash
./scripts/test.sh
```

测试会校验两次取消、自动/人工分栏数量、所有结论的步骤与证据引用、边界关键证据的完整性，并实际运行一次 GStreamer 探针。
