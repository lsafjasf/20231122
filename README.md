# 音频剪辑撤销历史可回放实验

这是一个固定使用 **Rust + GStreamer + GTK4** 的小型实验：把音频剪辑编辑记录成带请求 ID 的事件日志，用两个版本快照作对照，并验证编辑顺序变化、重复提交、撤销/重做和异常后的批处理恢复。

## 实验对象与扩展

- 场景文件使用自定义扩展 **`.undoexp`**。它故意不注册 MIME 类型，也不依赖 GStreamer typefind；程序只通过显式路径读取 JSON。
- 场景文件：`experiments/audio-undo.undoexp`
- 两个对照快照：
  - `snapshot_v1`：`trim 0..2000ms`，`gain 1000‰`
  - `snapshot_v2`：`trim 200..1800ms`，`gain 700‰`
- 快照同时校验规范化状态 JSON 的 SHA-256，以及 GStreamer 实际渲染出的 PCM WAV 的 SHA-256。

## 运行

```bash
./scripts/run-experiment.sh
./scripts/verify.sh
```

也可直接运行：

```bash
cargo run -- run --scenario experiments/audio-undo.undoexp --evidence artifacts/evidence
cargo run -- replay --evidence artifacts/evidence
cargo run -- ui --scenario experiments/audio-undo.undoexp
cargo run -- ui --selftest
```

`scripts/env.sh` 会启用容器内预置的 `/home/hj/env-media` GTK4/GStreamer 前缀和 Rust 工具链。UI 无头验证使用 Cairo 渲染后端。
直接执行 `cargo` 命令前需先 `source scripts/env.sh`，或使用上面的封装脚本。

## 历史与幂等语义

每次提交包含 `request_id`、可选 `base_revision` 和命令：

- `trim` / `gain`：编辑命令
- `undo` / `redo`：历史栈操作

历史栈保存每一步的 `before`、`after`、序列号、命令指纹和状态哈希。处理规则：

1. 相同 `request_id` 且命令指纹相同时，返回 `duplicate_ignored`；状态、修订号和历史深度不变。
2. 相同 `request_id` 但命令不同，返回 `rejected` / `request_id_reuse`，状态不变。
3. 非法裁剪范围或增益返回 `rejected`，不会写入历史。
4. `base_revision` 作为证据记录，但有意不作为顺序门禁；因此可以比较同一组可交换编辑的提交顺序。
5. 每个对象从 `snapshot_v1` 新建独立历史，异常不会污染后续对象。

## GStreamer 渲染

`src/render.rs` 按快照生成确定性的 440 Hz 单声道样本，裁剪到精确样本区间后送入真实 GStreamer 管线：

```text
appsrc -> audioconvert -> volume -> audioconvert
       -> audio/x-raw,format=S16LE,rate=16000,channels=1
       -> wavenc -> filesink
```

裁剪是样本级精确的，增益和 WAV 编码由 GStreamer 完成。输出 PCM WAV 再做 SHA-256；相同状态在不同提交顺序下应得到相同媒体哈希。

## 场景覆盖

| 对象 | 验证点 |
| --- | --- |
| `canonical-trim-then-gain` | 基线顺序：先裁剪、后增益 |
| `reordered-gain-then-trim` | 反序提交，最终状态和 WAV 均等于 `snapshot_v2` |
| `duplicate-submissions` | 两个请求 ID 各重复提交一次，均 `duplicate_ignored` |
| `undo-redo-roundtrip` | 撤销后再重做，回到 `snapshot_v2` |
| `conflict-then-recover` | 请求 ID 冲突被拒绝，随后新请求恢复并完成对象 |
| `after-exception-object` | 前一个对象发生拒绝后，批处理继续处理该对象 |

## 证据

实验结束后保留在 `artifacts/evidence/`：

- `events.jsonl`：逐步记录输入、`state_before`、结果、`state_after` 和该步媒体信息。
- `summary.json`：对象结果、快照哈希、回放结果、临时状态清理结果。
- `replay.json`：仅凭事件日志从 `snapshot_v1` 重放并与每步状态哈希比较。
- `controls/snapshot_v1.json`、`controls/snapshot_v2.json`：两个快照的状态与媒体哈希。
- `controls/snapshot_v1.wav`、`controls/snapshot_v2.wav`：两个版本对照的实际音频。
- `media/<object>/step-NN.wav`：每个对象的每一步渲染结果，包括重复提交和拒绝步骤。
- `scenario.undoexp`：本次运行所用的场景副本。

临时渲染文件先写入 `artifacts/tmp/audio-undo-*`，再移入证据目录。运行结束时关闭并删除该临时目录，`summary.json` 的 `cleanup` 字段记录删除前后状态；空的 `artifacts/tmp` 也会被移除。

`scripts/verify.sh` 额外把格式化、测试、实验、回放、GTK4 启动和断言日志保存到 `artifacts/verification/`。

## GTK4 回放界面

界面可从快照 v1 开始手动提交裁剪/增益、撤销、重做、重复最后一个请求，或一键回放 `canonical-trim-then-gain`。右侧显示撤销栈、重做栈，主区域显示当前修订号和状态哈希。
