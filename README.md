# source-ledger — 媒体项目打包清单来源检查

一个可审计的 GTK4 桌面检查页：对媒体交付包逐项记录原始输入、GStreamer
处理步骤、恢复重放、边界证据、自动结论与人工确认。技术栈固定为
**Rust + GStreamer + GTK4**。

## 场景

示例包 `demo_project/project.json` 包含 6 个来源：

| 来源 | 状态 | 说明 |
| --- | --- | --- |
| `intro_master.mp4` | 自动通过 | GStreamer 解析为 ISO BMFF / MP4 |
| `recovered_take_07.mp4` | 恢复重放 | 扫描中断后从检查点恢复，并重放同一输入 |
| `roomtone_48k.wav` | 自动通过 | GStreamer 解析为 RIFF / WAV |
| `swing_notes.capx` | 待人工 | 未知扩展，不按同类 MIME 或文本外观猜测 |
| `orphan_tail.bin` | 边界待定 | 头部有 ISO BMFF 签名，但没有 moov，完整性边界不可判定 |
| `camera_b_missing.mp4` | 失败 | 清单声明存在，磁盘缺失 |

示例媒体由 Rust 调用 GStreamer 管线生成；扫描由
`gstreamer-pbutils::Discoverer` 执行，不使用 ffmpeg 参与检查。

## 运行

```sh
source scripts/env.sh
cargo test
cargo run -- demo      # 生成示例媒体、中断扫描、恢复重放并导出报告
cargo run -- ui        # 启动 GTK4 来源检查页
```

如果 `demo` 已经生成报告，UI 会直接读取 `artifacts/report.json`。
`SOURCE_LEDGER_SELECT=src-recovered cargo run -- ui` 可指定启动时选中的来源；
`SOURCE_LEDGER_SCREENSHOT=/tmp/ui.png cargo run -- ui` 可在窗口完成布局后
离屏保存 PNG，便于无桌面会话验收。

补齐缺失文件后可执行一次完整重扫：

```sh
cargo run -- scan
```

## 证据链

每个来源都有两条独立线索：

1. **原始输入**：清单声明、实际路径、大小、修改时间、完整 SHA-256、
   首尾字节窗口、GStreamer 探测快照。
2. **处理步骤**：解析清单 → 读取字节并计算指纹 → GStreamer 探测 →
   应用判定策略；每一步记录输入、输出、状态与证据引用。

恢复流程使用真实检查点：在恢复镜头之后暂停，写入
`artifacts/runtime/checkpoint.json`；恢复时重新探测同一路径，比较
`sha256 / byte_size / detected_mime / container / duration_ms /
video_streams / audio_streams` 七个字段，并把比对写入
`artifacts/evidence/src-recovered/replay_after_recovery.json`。

未知扩展与截断边界会保留：

- `probe.json`：清单声明 + GStreamer 实际结果；
- `head.hex` / `tail.hex`：首尾原始字节窗口；
- `original.bin`：未知附件与残片的原始副本；
- `boundary-window.hex`：边界窗口与待回答的问题。

注意：签名字节只证明“头部像什么”，不证明文件完整或可交付。

## 报告

报告分别落盘为：

- `artifacts/report.json`：结构化报告；
- `artifacts/report.md`：人类可读报告；
- `artifacts/report.sha256`：报告哈希；
- `artifacts/runtime/events.jsonl`：只追加的处理与恢复事件日志。

JSON 与 Markdown 都显式分节：

- **自动结论**：机器可验证的字节、媒体解析和重放一致性；
- **人工确认**：未知扩展、不可判定边界、恢复对象的语义确认；
- **失败处置**：失败原因与可执行后续操作。

UI 右侧把自动结论与人工确认拆成两个独立滚动区；人工项可直接记录
“确认保留”或“记录排除”，决定人、时间和说明写回 `report.json`。

失败条目不会生成占位哈希。缺失来源的处置包括：放回原始文件、只重试扫描、
保留清单声明与缺失诊断；命令会带当前工作目录，可直接复制执行。

## 代码结构

```text
src/model.rs    报告数据契约
src/scanner.rs  清单扫描、GStreamer 探测、检查点、恢复重放、失败处置
src/fixtures.rs 用 GStreamer 生成示例媒体
src/export.rs   自动/人工分节的 Markdown 报告
src/ui.rs       GTK4 三栏来源检查页与人工确认交互
tests/          端到端报告契约测试
```

页面默认选中未知扩展，左侧同时展示恢复镜头和边界残片；右侧固定可见
“AUTO / 自动结论”与“HUMAN / 人工确认”两段，失败处置位于人工区下方。
