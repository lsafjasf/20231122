# 响度分段校准 · 可暂停流程

用 **Rust + GStreamer + GTK4** 实现的三段锚点响度校准工作台。每个 5 秒段
独立分析、独立渲染、独立落盘断点；暂停只发生在锚点边界。恢复时先校验原件、
只读副本、已完成段输出和断点摘要，再从下一段继续。连续运行与“暂停 A → 恢复
暂停 B → 恢复完成”的最终 WAV 字节一致。

## 验收锚点

内置原件是 15.000 秒、48 kHz、16-bit、单声道 PCM WAV，数据块从文件偏移
`44` 开始。三段边界同时是验收锚点：

| 锚点 | 时间 | 采样帧 | data 偏移 | 文件绝对偏移 | 段范围（文件字节） |
| --- | ---: | ---: | ---: | ---: | --- |
| A | `5.000000000 s` | `240000` | `480000` | `480044` | `44 .. 480044` |
| B | `10.000000000 s` | `480000` | `960000` | `960044` | `480044 .. 960044` |
| C | `15.000000000 s` | `720000` | `1440000` | `1440044` | `960044 .. 1440044` |

`data 偏移` 是相对原始 WAV 数据区的偏移，`文件绝对偏移` 已加 WAV 头长度。
每个锚点验收行同时打印二者，并附该段输出 SHA-256：

```text
PASS A t=5.000000000s frame=240000 data_offset=480000 file_offset=480044 ...
PASS B t=10.000000000s frame=480000 data_offset=960000 file_offset=960044 ...
PASS C t=15.000000000s frame=720000 data_offset=1440000 file_offset=1440044 ...
```

## 原始材料

| 项目 | 值 |
| --- | --- |
| 原件 | `assets/original/loudness_original.wav` |
| 大小 | `1440044` bytes |
| 权限 | `0444`（只读） |
| SHA-256 | `6ae0c846d20bb1dec3590d39e326ecbb097359729b2c991c6f4c32c9a936d80e` |

原件由 GStreamer 一次性生成：三段各 5 秒、440 Hz 正弦，振幅依次为
`0.08 / 0.25 / 0.015`。处理只读取原件；每次运行还会复制一份
`source/original.wav` 到证据目录并设为只读。开始前和验收时均重新计算原件
哈希，任何变化都会使验收失败。

可从零复现（已有原件时只校验、不覆盖）：

```bash
chmod +x scripts/make-original.sh
./scripts/make-original.sh
```

## 快速开始

系统依赖（Rust 1.85+，验证环境为 Rust 1.100 / GStreamer 1.28 / GTK4 4.22）：

```bash
sudo apt install rustc cargo pkg-config \
  libgtk-4-dev libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
  gstreamer1.0-plugins-base gstreamer1.0-plugins-good
```

如 Rust 安装在用户目录，先补齐环境：

```bash
. ./env.sh
```

分步演示暂停与恢复：

```bash
cargo build --release
BIN=target/release/loudness-segment-calibrator

# 处理 A，保存断点后退出
$BIN run --source assets/original/loudness_original.wav \
  --run artifacts/evidence/paused --pause-after A --restart

# 校验 A，处理 B，保存断点后退出
$BIN resume --run artifacts/evidence/paused --pause-after B

# 校验 A/B，处理 C，拼接最终 WAV
$BIN resume --run artifacts/evidence/paused

# 重算全部哈希并输出验收行
$BIN verify --run artifacts/evidence/paused
```

一键生成连续、暂停恢复、降级与对比证据：

```bash
chmod +x scripts/run-acceptance.sh
./scripts/run-acceptance.sh
```

打开 GTK4 工作台：

```bash
cargo run --release -- gui
# 或载入已有断点
cargo run --release -- gui --run artifacts/evidence/paused
```

界面按钮“请求暂停”不会截断正在写出的段；它在当前锚点完成并原子保存断点后
退出处理线程。“开始 / 恢复处理”会从 `state.json` 的 `next_segment` 继续。

![GTK4 工作台](artifacts/gui-evidence.png)

## 处理与校准

每段执行两次 GStreamer 管线：

1. 分析：`filesrc ! wavparse ! audioconvert ! audioresample`，在锚点窗口内
   seek，`appsink` 接收 `F32LE` 样本，累计均方值和峰值。
2. 渲染：同一锚点窗口经 `volume volume=<linear> ! wavenc ! filesink` 输出
   16-bit PCM 段文件。

目标响度默认 `-20 dBFS RMS`。增益先限制在 `[-20 dB, +20 dB]`，再受
`-1 dBFS` 峰值上限约束。渲染完成后重新解码该段输出，记录实际
`after_rms_dbfs`，并给出：

- `delta_db = after_rms - before_rms`
- `target_error_db = after_rms - target`

三段完成后，`concat` 管线把段 WAV 拼成 `output/calibrated.wav`，并校验总时长
与原件相差不超过 1 ms。内置素材三段实测均落在目标 ±0.005 dB。

## 暂停、断点与恢复

安全暂停点是 A/B/C 锚点。每段完成后按以下顺序落盘：

1. `segments/segment-N-X.wav`
2. `checkpoints/anchor-X.json`
3. `state.json`

所有 JSON 使用临时文件 + `fsync` + 原子重命名。断点文件内保存锚点时间、
帧号、文件偏移、段前/段后响度、增益、段输出哈希和下一段游标。
`checkpoint_sha256` 是对断点自身（摘要字段置空后规范化 JSON）的 SHA-256，
恢复前会重算。

恢复前还会检查：

- 原件哈希等于 `state.source.sha256`；
- 只读原件副本哈希未变；
- 每个已完成段输出哈希与断点一致；
- 断点摘要、锚点顺序、时间、帧号和原始偏移一致；
- `next_segment == completed.len()`。

任一失败会写 `evidence/resume-preflight-N.json`、保留所有断点，并拒绝继续。

## 资源不足与降级

启动时读取 `/proc/meminfo` 的 `MemAvailable`、输出目录可用磁盘和 CPU 数：

- 可用内存低于 `256 MiB` 或估算需求的 2 倍；
- 可用磁盘低于 `512 MiB` 或估算需求的 2 倍；
- 可用 CPU 少于 2；
- 或显式 `--force-degrade` / `LOUDNESS_FORCE_DEGRADE=1`。

满足任一条进入 `degraded`：

- 输出从 48 kHz 降到 24 kHz，降低磁盘与处理压力；
- 锚点原文证据块从 4096 bytes 缩为 512 bytes；
- 仍使用流式 GStreamer 管线，不保留整段 PCM；
- 暂停/恢复/复测/最终拼接流程不变，验收仍落到相同时间点和原始偏移。

如果可用内存或磁盘低到安全下限以下，流程拒绝启动，并在证据目录写
`resource-refusal.json`，不覆盖已有断点。

## 证据目录

```text
<run>/
├── source/original.wav                  # 只读原件副本
├── state.json                           # 下一段、锚点、恢复次数、事件
├── checkpoints/anchor-{A,B,C}.json      # 自摘要断点
├── segments/segment-*.wav               # 三段校准输出
├── output/calibrated.wav                # 最终 15 秒 WAV
└── evidence/
    ├── original.json / original.sha256  # 原件与只读副本证据
    ├── resource.json                    # 资源探测
    ├── degradation.json                 # 降级策略（如有）
    ├── raw/{A,B,C}-<offset>.bin         # 锚点前原始字节
    ├── resume-preflight-*.json          # 每次恢复前校验
    ├── comparison.json                  # 段前/段后与目标误差
    ├── acceptance.txt                   # 时间、帧、偏移、增益、哈希
    ├── verification.json                # 重算后的逐项验收
    ├── report.json                      # 汇总报告
    ├── events.jsonl                     # 流程事件
    ├── manifest.json                    # 运行目录内文件哈希清单
    └── failure.json                     # 失败时保留的错误证据
```

`raw/A-480044.bin` 保存锚点 A 前 4096 字节；JSON 中记录其实际起止偏移。
降级模式保存 512 字节。GTK4 界面也会显示每个锚点的断点摘要和输出摘要。

## 连续 / 暂停恢复对比

```bash
$BIN compare \
  --a artifacts/evidence/continuous \
  --b artifacts/evidence/paused \
  --out artifacts/evidence/compare-equivalence.json
```

验收条件：

- 两次运行的最终 WAV SHA-256 完全相同；
- A/B/C 段前 RMS、段后 RMS、增益和段输出 SHA-256 分别相同；
- `verify` 对原件、断点、原始锚点块、段输出和最终输出逐项通过。

## 验证

```bash
cargo test
```

测试覆盖：

- A/B 两次暂停、两次恢复，最终与连续运行字节一致；
- 锚点 A/B/C 的纳秒时间、采样帧和文件绝对偏移；
- 强制降级后 24 kHz、360000 帧、15.000 秒；
- 篡改段输出后拒绝恢复，并保留 `resume-preflight-1.json`；
- 原始材料哈希前后不变。

界面截图可无头生成：

```bash
GDK_BACKEND=x11 LOUDNESS_GUI_SCREENSHOT=artifacts/gui-evidence.png \
  $BIN gui --run artifacts/evidence/paused
```
