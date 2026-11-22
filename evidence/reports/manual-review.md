# 人工确认清单（Manual Review）

> 本文件只收集人工判断；自动判定见 `reports/auto-conclusion.json`，二者分开呈现。

## C1-mid-cancel
- 自动结论：PASS
- 断言：S2 中途取消：至少一个作业被取消，且后续作业保持 pending，批次 outcome=cancelled
- 人工复核（请勾选）：
  - [ ] 已核对对应 `runs/*.json` 的 steps/snapshots
  - [ ] 已试听/检查 `outputs/` 下相关 WAV
  - [ ] 人工结论：[ ] 同意自动结论  [ ] 不同意（说明理由）
- 复核人：____________ 日期：____________

## C2-control
- 自动结论：PASS
- 断言：S3 无取消对照：9 个作业全部 succeeded，outcome=completed
- 人工复核（请勾选）：
  - [ ] 已核对对应 `runs/*.json` 的 steps/snapshots
  - [ ] 已试听/检查 `outputs/` 下相关 WAV
  - [ ] 人工结论：[ ] 同意自动结论  [ ] 不同意（说明理由）
- 复核人：____________ 日期：____________

## C3-no-partial-output
- 自动结论：PASS
- 断言：S2 被取消/未开始的作业没有残留 WAV 文件
- 人工复核（请勾选）：
  - [ ] 已核对对应 `runs/*.json` 的 steps/snapshots
  - [ ] 已试听/检查 `outputs/` 下相关 WAV
  - [ ] 人工结论：[ ] 同意自动结论  [ ] 不同意（说明理由）
- 复核人：____________ 日期：____________

## C4-duplicate-blocked
- 自动结论：PASS
- 断言：S4 重复提交：10 个提交中 9 个唯一作业正常完成，重复的第 10 个（同 idempotency key）被拒绝且无文件
- 人工复核（请勾选）：
  - [ ] 已核对对应 `runs/*.json` 的 steps/snapshots
  - [ ] 已试听/检查 `outputs/` 下相关 WAV
  - [ ] 人工结论：[ ] 同意自动结论  [ ] 不同意（说明理由）
- 复核人：____________ 日期：____________

## C5-order-invariance
- 自动结论：PASS
- 断言：S5 改变提交顺序后，每个作业的输出 SHA256 与 S1 完全一致
- 人工复核（请勾选）：
  - [ ] 已核对对应 `runs/*.json` 的 steps/snapshots
  - [ ] 已试听/检查 `outputs/` 下相关 WAV
  - [ ] 人工结论：[ ] 同意自动结论  [ ] 不同意（说明理由）
- 复核人：____________ 日期：____________

## C6-replay-determinism
- 自动结论：PASS
- 断言：S6 重放：9 个作业的 SHA256 与 S1 基线逐一相等
- 人工复核（请勾选）：
  - [ ] 已核对对应 `runs/*.json` 的 steps/snapshots
  - [ ] 已试听/检查 `outputs/` 下相关 WAV
  - [ ] 人工结论：[ ] 同意自动结论  [ ] 不同意（说明理由）
- 复核人：____________ 日期：____________

## C7-evidence-integrity
- 自动结论：PASS
- 断言：报告中记录的全部 SHA256 与磁盘 WAV 文件实际哈希一致
- 人工复核（请勾选）：
  - [ ] 已核对对应 `runs/*.json` 的 steps/snapshots
  - [ ] 已试听/检查 `outputs/` 下相关 WAV
  - [ ] 人工结论：[ ] 同意自动结论  [ ] 不同意（说明理由）
- 复核人：____________ 日期：____________

