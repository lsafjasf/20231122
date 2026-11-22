# 媒体元数据对照：可回放实验

这是一个固定采用 **Rust + GStreamer + GTK4** 的媒体元数据实验。它不把结果压缩成一个总分，而是保留每次回放的输入、进入前状态、结果和哈希链，直接指出**第一个错误状态**。

实验使用一条在 `submit` 行中间被截断的 JSONL 记录，并用恢复后的完整重放作为对照，再验证两类污染：交换提交顺序、重复提交相同 `submission_id`。

## 已固化结果

- 恢复控制：7 步全部接受，最终 `Verified`。
- 截断前缀：5 个完整输入；第 6 个输入 `submit` 缺失尾部 190 bytes，因此首次分歧是索引 5 的 `input_missing`，不伪造规则错误。
- 顺序改变：索引 2 输入 `evt-stage-0004`，状态机期望 sequence `2`、收到 `3`，首个错误为 `sequence_order`。进入该步骤前状态为 `phase=Probed, cursor=2, revision=0`。
- 重复提交：索引 6 再次收到 `sub-commit-0006`。该输入 sequence 已推进到 `6`，排除了乱序干扰，首个错误为 `duplicate_submission`。进入该步骤前状态为 `phase=Committed, cursor=6, revision=3`。
- 跨场景最早错误：`reordered` 的索引 2。

完整结果见 `artifacts/experiment/report.md`、`summary.json` 和各场景 `.trace.jsonl`。

## 运行

核心回放不需要 GUI 依赖：

```bash
cargo run
cargo test
```

输出默认写入 `artifacts/experiment/`。重新生成固定夹具：

```bash
cargo run -- init-fixtures fixtures
```

桌面版本需要 GTK4 与 GStreamer 开发包：

```bash
sudo apt-get install -y build-essential pkg-config \
  libgtk-4-dev libgstreamer1.0-dev \
  libgstreamer-plugins-base1.0-dev gstreamer1.0-plugins-good
```

```bash
cargo run --features desktop --bin media-meta-ui
```

界面左侧选择恢复控制、截断前缀、乱序、重复提交；右侧逐步显示输入、进入前状态、结果和证据哈希。右上角“探测示例媒体”会调用真实 GStreamer `Discoverer`，并显示探针 JSON。

命令行也可直接执行探针：

```bash
cargo run --features desktop --bin media-meta-replay -- \
  probe fixtures/media/take-001.wav fixtures/gstreamer-probe.json
```

## 技术结构

- `src/model.rs`：记录、动作、状态、结果、追踪和证据结构。
- `src/replay.rs`：确定性状态机。校验 `sequence`、重复事件/提交、revision、payload 哈希和状态转换；遇到首个拒绝立即冻结并停止。
- `src/fixture.rs`：真实 WAV 生成、恢复记录、物理截断 JSONL 的写入与读取。
- `src/experiment.rs`：恢复对照、截断前缀、乱序、重复提交四组回放，以及产物和报告生成。
- `src/evidence.rs`：规范 JSON 哈希和逐步证据链。
- `src/media.rs`：GStreamer `Discoverer` 探针，提取容器、编解码器、时长、采样率、声道和标签。
- `src/ui.rs`：GTK4 场景侧栏、逐步轨迹、首错卡、JSON 详情和现场探针。

`desktop` feature 才引入 GTK4/GStreamer；默认构建可独立测试回放语义。

## 状态机与首错

正常路径：

```text
Idle -> Opened -> Probed -> Staged -> Committed -> Verified
```

每步都记录：

- `input`：事件 ID、sequence、submission ID、动作参数。
- `state_before` / `state_after`：最后有效状态与首个错误状态，均含 phase、cursor、revision、媒体、元数据摘要、已见 ID、首错。
- `outcome`：`accepted` 或带规则名的 `rejected`。
- `evidence`：输入、进入前状态、结果、上一条证据哈希及本条链式哈希。

校验顺序是：`duplicate_event_id` → `duplicate_submission` → `sequence_order` → 动作和阶段规则。因此重复提交场景即使 sequence 正确，也会在重复 ID 处失败。

首个拒绝发生后，不继续应用后续输入。`summary.json` 的 `first_error_across_scenarios` 和每个 case 的 `first_error` 都同时包含：

- 场景、步骤索引、步骤 ID、规则、期望和实际值；
- 进入该步骤前的完整 `state_before`（最后有效状态）和拒绝后的 `state_after`（首个错误状态）；
- 该步骤 `evidence_sha256`。

## 证据文件

`artifacts/experiment/` 中：

- `truncated-record.jsonl`：保留部分最后一行且无换行的原始截断证据。
- `recovered-record.json`：恢复后的完整输入序列。
- `gstreamer-probe.json`：GStreamer 1.28.2 对 9734-byte WAV 的真实探针快照。
- `control.trace.jsonl`：恢复控制逐步轨迹。
- `truncated-prefix.trace.jsonl`、`reordered.trace.jsonl`、`duplicate-submission.trace.jsonl`：对照场景。
- `summary.json`：无总分的结构化结论、首错状态和对照分歧。
- `key-evidence.json`：源媒体、GStreamer 探针、截断前缀、恢复记录、控制轨迹和首错证据的 SHA-256。
- `report.md`：面向人的实验报告。
- `manifest.sha256.json`：产物清单及每个文件的 SHA-256。
- `media/take-001.wav`：实验媒体副本。

逐步证据链按以下材料计算：

```text
sha256(canonical_json(index, previous_evidence, input_hash,
                      before_state_hash, after_state_hash, result_hash))
```

任何顺序调整或重复输入都会改变对应步骤的 `input_sha256` 和后续链，便于回放取证。

## GStreamer 探针事实

已记录的探针结果：

```text
container_format = WAV
audio_codec      = Uncompressed 16-bit PCM audio
duration         = 600 ms
sample_rate      = 8000 Hz
channels         = 1
title            = Harbor Lights
artist           = Lin & The Night Shift
album            = Recovery Sessions
discoverer       = Ok
media_sha256     = 82d98b537a4d2172cd07d65cd17be1cee930768285260cec77e2bc6e41e530c5
```

`gstreamer-probe.json` 编译时嵌入实验产物；更换探针输入后重新运行 `probe` 和 `cargo run` 即可刷新证据。
