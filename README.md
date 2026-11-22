# 音频噪声频谱标注 · 来源检查页

Rust + GStreamer + GTK4 实现的音频噪声频谱标注来源检查工具。
每条结论都可回溯到原始输入（SHA-256 指纹）与处理步骤链；
自动结论与人工确认分开呈现；中途取消与失败均保留证据并给出可执行后续操作。

## 功能

- **来源检查页**：原始输入（路径 / SHA-256 / 大小）+ 处理步骤链（S1 输入校验 → S2 解码 → S3 格式转换 → S4 频谱测量 → S5 噪声判定），每条频段结论标注其溯源步骤。
- **中途取消记录**：分析可随时取消，取消时的进度与已测得的部分频谱证据完整保留（演示数据含两次中途取消：35% 与 70%）。
- **无法判断的边界**：幅度落在判定阈值 ±2dB 边界带内的频段标记为“无法判断”，保留平均幅度、本底、阈值、距阈值、测量消息数等关键证据。
- **报告分栏**：自动结论（干净 / 噪声 / 无法判断）与人工确认（待确认 / 已确认 / 已驳回，含确认人与时间戳）分两栏呈现。
- **失败后续操作**：输入缺失、解码失败、插件缺失等失败均附可执行的后续操作清单。

## 构建与运行

```bash
source env.sh          # 本地工具链环境（rustup + ~/sysroot 中的 GTK4/GStreamer）
cargo build
./target/debug/noise-provenance          # 启动界面
./target/debug/noise-provenance --demo   # 重新生成演示数据（约 12 秒）
./target/debug/noise-provenance --shot=/tmp/ui.png     # 截图自检（可选 :序号 指定记录）
```

数据持久化在 `artifacts/session.json`，演示音频为 `artifacts/demo_noise.wav`。

## 环境说明

本机无 root 权限，工具链装在用户目录：

- Rust：`~/.cargo`（rustup，stable）
- GTK4 4.22 / GStreamer 1.28 开发文件：`~/sysroot`（apt 下载 .deb 解压，`env.sh` 注入 `PKG_CONFIG_PATH` / `LD_LIBRARY_PATH` / `GST_PLUGIN_PATH`）
- 注意：本环境 D-Bus 会话注册异常，应用不能使用 `application_id`（会立即退出）

