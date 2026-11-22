# 多声道监听路由 · 来源检查

Rust + GStreamer + GTK4 桌面检查页，用于复核一次恢复后的重放、一个未知 RIFF 扩展，以及一个无法自动归因的恢复边界。

## 运行

系统需要 GTK4、GStreamer 及 `wavparse` / `audioconvert` / `audioresample` / `appsink` 插件。

```bash
sudo apt install libgtk-4-dev libgstreamer1.0-dev \
  libgstreamer-plugins-base1.0-dev gstreamer1.0-plugins-base \
  gstreamer1.0-plugins-good
./scripts/run.sh
```

首次启动会在 `artifacts/demo-source.wav` 生成确定性的 8 声道问题来源。检查结果与证据写入 `artifacts/reports/<run-id>/`，不会修改原始来源。

## 检查内容

- 自动结论 A-01：GStreamer 解码后的 8 声道 F32LE 样本跨窗匹配，确认恢复后存在 `1.470 s` 重放。
- 自动结论 A-02：发现未知 `XMTA` 扩展；其记录表声明长度越界，解析失败后按不透明字节隔离。
- 自动结论 A-03：`312,960 ± 41` 帧内无法唯一确定恢复边界，保留为待人工裁定。
- 人工确认 H-01/H-02/H-03：与自动结论分栏呈现、独立保存，不覆盖自动结果。
- 失败处置：展示错误码、已保留资料，并提供导出原始扩展、只读重试、保持隔离等后续动作。

## 追溯链

每个结论都带 `INPUT` 定位、处理链引用和 `E-xx` 证据编号。证据包包含：

- `pipeline.txt`：GStreamer 处理链、协商 caps、解码耗时与输入摘要。
- `replay-window.txt`：匹配源/重放帧区间、规则和置信度。
- `boundary-raw-pcm24.bin`：41 帧原始 24-bit PCM 边界证据。
- `boundary-f32le.bin`：同一区域的 GStreamer F32LE 解码证据。
- `xmta-payload.bin`：未知扩展的完整原始负载。
- `xmta-probe.log`：可复现的解析失败原因。
- `report.json`：自动结论与人工确认状态分别导出。

## 验证

```bash
cargo test
cargo fmt --check
```

测试覆盖确定性问题来源、RIFF 扩展解析、时间格式和跨窗重放检测。
