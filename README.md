# BWF 时间标记校验 · 可回放实验

使用 Rust、GStreamer 和 GTK4 构建的确定性实验：先对一条被截断的 BWF
记录做结构探针，再用“按声明长度补零”的恢复副本完成一次全时长重放对照，
最后验证 Cue 标记的顺序改变与重复提交。

实验不只给出 PASS/FAIL 总分。每条轨迹都保留逐步的输入、状态迁移和结果，
并在第一次拒绝处冻结状态机，输出可独立读取的首个错误。

![GTK4 审计界面](artifacts/bwf-replay-lab/ui.png)

## 实验结果

本次生成的证据位于
`artifacts/bwf-replay-lab/evidence/report.json`。

| 轨迹 | 结果 | 首个错误 |
| --- | --- | --- |
| `truncated_probe` | FAIL | step 02 `data_truncated`，期望 576000 字节，实际 403200 字节 |
| `recovered_control` | PASS | 无；GStreamer EOS，解码 288000 帧 |
| `order_changed` | FAIL | step 03 `marker_order_violation`，期望 `cue-2`，收到 `cue-3` |
| `duplicate_submission` | FAIL | step 04 `duplicate_marker`，重复提交 `cue-2` |

全局首个错误状态为：

```text
truncated_probe / step 02 / data_truncated
asset_read -> rejected
expected 576000 data bytes, actual 403200 data bytes
```

错误发生后的后续输入仍写入记录，但结果标为 `OBSERVED`，状态保持不变。
因此报告能区分“第一个失败点”和“错误之后的观测流量”，不会用一个总分
掩盖真正的故障位置。

## 实验设计

BWF 夹具是确定性的 6 秒、48 kHz、单声道、16-bit PCM WAVE：

- BEXT `TimeReference` 为 `01:00:00:00`。
- Cue/LABL 标记为 `cue-1`、`cue-2`、`cue-3`、`cue-4`，样本偏移分别是
  `48000`、`96000`、`168000`、`240000`，对应 25 fps 的
  `01:00:01:00`、`01:00:02:00`、`01:00:03:12`、`01:00:05:00`。
- 完整 data 段声明 576000 字节；夹具在 70% 处截断，实际留下 403200 字节。
- 恢复副本保留原 PCM 前缀，并按 data 声明长度补 172800 个零字节。这是一次
  “容器可重放”恢复，不声称还原丢失的音频内容。

GStreamer 对照管线：

```text
filesrc ! wavparse ! audioconvert
  ! capsfilter(S16LE, 48 kHz, mono) ! fakesink
```

证据记录 EOS、总线消息、buffer 数、解码字节/帧数、PTS 范围和 sink caps。
本次对照重放得到 288000 帧、576000 字节、EOS，sink caps 为
`audio/x-raw, format=S16LE, channels=1, rate=48000`。

## 复现

环境已有 `$HOME/env-media` 时，脚本会自动设置 GTK4/GStreamer 的
`PKG_CONFIG_PATH`、`LD_LIBRARY_PATH` 和插件路径。

```bash
# 运行测试并检查格式
./scripts/test.sh

# 无头运行，生成证据并打印摘要
./scripts/headless.sh

# 启动 GTK4 审计界面
./scripts/run.sh

# 重新生成界面截图
./scripts/screenshot.sh

# 直接查看任一轨迹的截图
BWF_SELECT_RUN=recovered_control ./scripts/screenshot.sh artifacts/bwf-replay-lab/ui-control.png
BWF_SELECT_RUN=order_changed ./scripts/screenshot.sh artifacts/bwf-replay-lab/ui-order.png
```

若系统工具链而非 `$HOME/env-media` 提供依赖，直接运行：

```bash
cargo test
cargo run -- --headless
cargo run
```

## 证据

`artifacts/bwf-replay-lab/` 保留原始输入、恢复结果和结构化审计记录：

- `fixtures/bwf_truncated.wav`：被截断的输入记录。
- `fixtures/bwf_recovered.wav`：补零到声明长度后的对照记录。
- `evidence/report.json`：完整实验报告，含每条轨迹的逐步输入、前后状态、结果和证据。
- `evidence/steps.jsonl`：逐行审计流，一行一个 step，便于外部工具回放。
- `evidence/first-error.json`：全局首个错误状态。
- `evidence/gst-control.json`：GStreamer 对照重放证据。
- `evidence/summary.txt`：人可直接阅读的逐步摘要。
- `ui.png`、`ui-control.png`、`ui-order.png`：GTK4 界面截图。

报告同时记录截断文件的 SHA-256、恢复文件的 SHA-256、截断点、补零字节数，
以及 BEXT/Cue 元数据。恢复前后哈希不同，但恢复后的 data 实际长度与声明长度
一致，这是对照重放能够完成 EOS 的容器层依据。

## 代码结构

- `src/bwf.rs`：RIFF/BWF 解析、BEXT/Cue/LABL 读取、夹具生成与恢复。
- `src/engine.rs`：标记提交状态机、首错冻结、顺序与重复校验。
- `src/replay.rs`：GStreamer 管线、pad probe 统计和 EOS/错误采集。
- `src/experiment.rs`：四条轨迹编排、JSON/JSONL 证据落盘。
- `src/ui.rs`：GTK4 审计界面，逐步展开输入、状态、结果和证据。
- `tests/experiment.rs`：端到端断言，包括首错定位、对照重放和证据可回读。
