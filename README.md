# 视频帧丢失补偿复核流程

这是一个固定采用 **Rust + GStreamer + GTK4** 的逐帧复核工具。它不会把结果压缩成一个总分，而是在第一个非法状态处立即停止，保留该步之前和之后的状态、输入字节、处理参数、输出波形及证据哈希。

## 已实现内容

- 新增一条物理截断的 JSONL 记录：`fixtures/truncated-record.jsonl`。前 7 条完整，第 8 条保留 269 字节后被切断，末尾没有换行。
- 对恢复后的完整输入执行控制重放：`fixtures/recovered-record.jsonl`，11 条记录全部接受，3 帧丢失均由前后参考帧补偿。
- 每一帧同时记录 `time_ms`、`pts_ns`、轨道、帧号、源状态、参数指纹、补偿模式、计算 luma、GStreamer 实测 luma、记录 ID 和链式证据哈希。
- 兼容旧 `frame-loss-review/v1` 与新 `frame-loss-review/session-v2` / `record-v2`。旧字段迁移后，原始文件、原始 SHA-256 和迁移警告仍保留在证据包中。
- 首错不是统计分数：报告同时给出 `state_before`、拒绝后的 `state_after`、违规规则、期望值、实际值与本步证据。
- GStreamer 管线为 `appsrc(I420) → videoconvert → video/x-raw,format=GRAY8 → appsink`。CSV 中的 `measured_luma` 来自真实 GStreamer 输出，不是直接复制输入值。
- GTK4 窗口可切换恢复、截断、旧格式和参数篡改场景，查看每步状态并绘制输出波形。

## 快速运行

```bash
# 生成 fixtures、执行 4 次回放、输出完整证据包
cargo run -- demo

# 校验证据包中所有文件的字节数与 SHA-256
cargo run -- verify-manifest artifacts/experiment/manifest.sha256.json

# 单独重算一条 trace 的状态衔接和 evidence 链
cargo run -- verify-trace artifacts/experiment/truncated-prefix.trace.jsonl

# 打开 GTK4 复核界面
cargo run -- desktop artifacts/experiment

# 单独重放新格式
cargo run -- replay fixtures/recovered-record.jsonl --trace /tmp/replay.trace.jsonl

# 迁移并重放旧格式
cargo run -- replay-legacy fixtures/legacy-v1.json
```

系统需要 Rust 工具链、GStreamer 1.x 开发包和 GTK4 开发包。当前验证环境为 GStreamer 1.28 / GTK 4.22。

## 复核输出

`cargo run -- demo` 会写入 `artifacts/experiment/`：

| 文件 | 用途 |
|---|---|
| `recovered-record.jsonl` | 恢复后的完整输入，也是控制重放输入 |
| `truncated-record.jsonl` | 未修复的物理截断证据 |
| `recovered.trace.jsonl` | 恢复重放每一步的输入、前后状态、结果和证据链 |
| `truncated-prefix.trace.jsonl` | 截断输入在第 7 步被拒绝的完整现场 |
| `legacy-v1.json` | 旧格式原始字节 |
| `legacy-import-v2.json` | 旧格式迁移后的新格式会话 |
| `legacy-v1-import.trace.jsonl` | 迁移后重放证据 |
| `tampered-params.trace.jsonl` | 参数指纹篡改在第 8 步被拒绝的证据 |
| `waveform-recovered.csv/.svg` | 时间、轨道、帧、参数与输出波形的对应 |
| `waveform-legacy.csv/.svg` | 旧格式重建波形，明确标注为非解码证据 |
| `summary.json` | 结构化首错、截断统计、兼容信息与场景摘要 |
| `key-evidence.json` | 原始输入、波形和证据链末端的关键哈希 |
| `report.md` | 人类可读复核报告 |
| `manifest.sha256.json` | 除自身外全部证据文件的 SHA-256 清单；校验时同时重算 trace 证据链 |

`summary.json` 固定包含：

```json
{
  "has_total_score": false,
  "no_score_reason": "A total score would hide the first invalid state; this report preserves each step and its evidence chain."
}
```

## 时间点、轨道、帧、参数和波形的对应

每个接受步骤产生一行 `WavePoint`。恢复重放的核心对应关系如下：

| 语义 | 字段 |
|---|---|
| 时间点 | `time_ms`, `pts_ns` |
| 轨道 | `track_id`, `track_index` |
| 帧 | `frame`, `source_state` |
| 处理参数 | `params_fingerprint`, `compensation_mode`, `confidence` |
| 计算输出 | `output_luma`，由记录中的 `reference_before/after` 与 `blend` 重算并校验 |
| GStreamer 输出 | `measured_luma`，由 GRAY8 appsink 统计 |
| 可追溯性 | `record_id`, `evidence_sha256` |

`evidence_sha256` 的连接关系为：

```text
previous_evidence_sha256
  + input_sha256
  + state_before_sha256
  + state_after_sha256
  + result_sha256
  → evidence_sha256
```

因此任何一步输入、参数、状态或结果被改写，后续链都会断裂。`verify-manifest` 会重新计算每条 trace，而不是只相信文件级哈希。

## 格式兼容

旧 v1 字段映射：

| v1 | v2 |
|---|---|
| `before` | `before_frames` |
| `after` | `after_frames` |
| `mix` | `blend` |
| `confidence` | `min_confidence` |
| `max_gap` | `max_conceal_frames` |
| `frame_number`, `timestamp_ms`, `present/lost` | `frame.frame`, `frame.time_ms`, `frame.state` |

迁移时为每条记录补算 `pts_ns` 和参数指纹，并写入兼容警告。旧波形来自 v1 luma 包络重建，报告和 SVG 都会明确标注它不等同于解码波形证据。

## 首个错误状态

状态机按固定顺序检查：记录 schema、序号、重复 ID、轨道元数据、参数合法性、参数指纹、轨道内帧号、PTS、PTS 与毫秒时间一致性、luma、补偿边界、信心阈值、参考帧、补偿输出和 GStreamer 测量。

首个失败立即将状态改为 `Faulted`，并冻结：

- `step_index`、`record_id`、`sequence`
- `rule`、`message`、`expected`、`observed`
- `state_before`（仍为 `Streaming` 且 `first_error=null`）
- `state_after`（`Faulted`，`first_error` 已设置）
- 本步 `evidence_sha256`

演示数据中的最早首错是 `truncated-prefix` 第 7 步的 `input_truncated`；参数篡改场景则在第 8 步以 `params_fingerprint_mismatch` 停止。

## 测试

```bash
cargo test
```

回归测试覆盖恢复重放、截断尾部、首错前后状态、参数指纹篡改、旧格式迁移、证据链连接和 manifest 校验。
