# 音频导出抖动诊断(可回放实验)

技术栈: **Rust + GStreamer + GTK4**。

诊断问题:音频导出(float32 -> S16 WAV)过程中抖动(dither)与噪声整形是否生效,
以及中途取消导出的行为是否可控、可复现。

## 实验设计

信号链: 确定性 440 Hz f32le 单声道音源(48 kHz, 2 s)
-> `audioconvert`(dithering / noise-shaping)-> `capsfilter`(S16LE)-> `wavenc` -> 文件。

步骤(每步的输入、前后状态、结果均写入 `journal.jsonl`):

1. `gst-env` — 记录 GStreamer 版本与关键元素,作为环境证据
2. `generate-source` — 生成确定性音源,记录 SHA-256
3. `export-full-dither` — TPDF 抖动 + medium 噪声整形完整导出
4. `export-full-nodither` — 禁用抖动完整导出(分析对照)
5. `export-full-tpdf` — 仅 TPDF 抖动(隔离抖动与噪声整形)
6. `export-cancel-dither` — **中途取消(测试组)**: 1.0 s 处直接置 NULL(不发 EOS)
7. `export-cancel-control` — **中途取消(对照组)**: 同一取消点,禁用抖动
8. `verify-order` — **顺序改变验证**: 颠倒提交顺序重跑,哈希必须一致
9. `verify-repeat` — **重复提交验证**: 相同导出再提交两次,哈希必须一致
10. `analyze` — 计算量化误差指标并给出自动结论
11. `report` — 生成 `REPORT.md`(自动结论 / 人工确认 / 失败后续操作 分离)

管线中插入 `identity sync=true` 实时定速,使中途取消落在确定性的流位置上。

## 运行

```sh
source ~/sysroot/env.sh          # 本地工具链环境(rustup + 解压的 GStreamer/GTK4 deb)
cargo run -- run                 # 执行实验 -> artifacts/latest/{journal.jsonl,REPORT.md,evidence/}
cargo run -- replay artifacts/latest/journal.jsonl   # 回放: 重放每步并比对记录结果
cargo run -- gui                 # GTK4 界面(运行实验 / 回放日志 / 手动中途取消)
./scripts/gui-evidence.sh        # 无头环境(cage + grim)采集 GUI 截图证据
```

回放规则: 取消步骤对时序敏感,按结构属性比对(cancelled 标志、WAV 头完整性、
字节数容差);其余产物按 SHA-256 精确比对。

## 关键证据(`artifacts/latest/`)

- `journal.jsonl` — 每步输入/状态/结果的可回放日志
- `REPORT.md` — 自动结论与人工确认分离的报告,含失败后续操作表
- `evidence/*.wav` — 各导出产物(完整 / 中途取消的部分文件)
- `evidence/gst-env.txt` — GStreamer 环境记录
- `evidence/gui-evidence.png` — GTK4 界面运行截图(cage headless + grim 采集)

## 自动结论检查项

| 检查项 | 含义 |
| --- | --- |
| `dither-applied` | TPDF 误差 RMS ≈ q/2 LSB,噪声整形进一步提高总 RMS,与理论一致 |
| `repeat-submission-deterministic` | 重复提交产物哈希一致(幂等) |
| `order-independent` | 提交顺序不影响产物 |
| `midway-cancel-partial-output` | 取消产生部分文件、头部未收尾、测试/对照取消点一致 |
| `state-recovery-after-cancel` | 取消后管线回到 NULL,后续导出仍确定 |
