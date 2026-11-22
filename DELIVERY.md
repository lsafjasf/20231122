# 交付说明：片段内容指纹可暂停处理流程

本目录交付一套真实可运行的 Rust + GStreamer + GTK4 实现，以及围绕
「双设备时钟不同步导致缺失片段」场景生成的可复核产物。

## 代码

- `segment-fingerprint/`：Cargo workspace，含核心库、CLI、GTK4 界面三个 crate。
- 构建方式见 `segment-fingerprint/README.md`。本地无 root 环境使用
  `.tools/env.sh` 注入用户态 GStreamer/GTK4 前缀与 Rust 工具链。

## 场景与三个运行

输入案例 `artifacts/clock-skew-demo/input/case.json`：设备 B 本地时钟领先
1350ms，并在本地 9.85..11.85s 存在采集缺口；第 5 个片段（统一时钟
8.5..10.25s）完全落入该缺口。

- `artifacts/clock-skew-pipeline/`：主运行。先暂停于第 4 段后，强制降级恢复，
  最终 9 匹配 + 1 缺失，80 项 `--deep` 复核全部通过。
- `artifacts/manual-resume/`：用独立的 `run` 与 `resume` 命令分步完成，
  验证命令行暂停/恢复契约，结果摘要与主运行一致。
- `artifacts/resource-pressure/`：先以极端磁盘阈值触发资源暂停（游标 seg-001，
  0 片段完成），放宽策略后恢复完成，验证资源不足与降级路径。

`artifacts/clock-skew-demo/` 是任务提供的参考产物，保留作为对照基线，
本实现未修改其中任何文件。

## 暂停前后结果如何比较

每个运行的 `run-record.json` 内 `resume_checks` 记录：

- `before_resume_digest` 与 `after_reload_digest`：检查点落盘摘要与重新载入后
  重新序列化摘要，二者相等证明字段与顺序在往返后无漂移。
- `anchor_before_digest` / `anchor_after_digest`：锚点片段（seg-001）内容摘要，
  证明已完成片段在恢复前后逐字节一致。
- `checkpoint_segment_count`：恢复时已载入的完成片段数。

## 关键证据

- 时钟对齐：`evidence/alignment.json`，观测偏移 1350000µs、置信度与比较帧数。
- 缺失判定：`run-record.json` 中 seg-005 的 `missing` 字段携带采集元数据缺口
  证据（gap_id、源时钟区间、统一时钟区间、重叠时长），避免把静音误判为内容缺失。
- 降级事件：`degradation_events` 记录 strict → reduced 的原因、RSS、磁盘与证据保留标记。
- 完整性：`checksums.sha256` 可直接用 `sha256sum -c` 校验；`verification.json`
  是独立复核器的机器可读结论。

## 一键复现（在已 source .tools/env.sh 的环境）

```bash
cd /home/hj/userfeedback/uid4-0017
BIN=segment-fingerprint/target/debug/segment-fingerprint
$BIN demo --case artifacts/clock-skew-demo/input/case.json --out /tmp/demo-out --pause-after 4
$BIN verify --record /tmp/demo-out/run-record.json --deep
(cd /tmp/demo-out && sha256sum -c checksums.sha256)
```
