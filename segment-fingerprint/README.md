# 片段内容指纹可暂停处理流程

固定技术栈：**Rust + GStreamer + GTK4**。围绕两台时钟不同步设备的音频采集，
对划定片段计算内容指纹，在时钟对齐后逐段比较，识别「内容匹配 / 分歧 / 缺失」，
并支持任意时刻暂停、检查点落盘、恢复后继续，且全部产物可由同事独立复核。

## 工作区结构

```
segment-fingerprint/
├── crates/
│   ├── core/      # 无 UI 的核心库：解码、指纹、对齐、比较、检查点、复核
│   ├── cli/       # segment-fingerprint 命令行（run/resume/verify/demo）
│   └── gtk-app/   # segment-fingerprint-gtk GTK4 控制界面
└── rust-toolchain.toml
```

核心库模块（`crates/core/src/`）：

- `model.rs`：全部 JSON schema，字段顺序固定，配合 `serde_json/preserve_order`
  保证重新载入后字段与片段顺序不漂移。
- `decode.rs`：GStreamer `uridecodebin ! audioconvert ! audioresample ! appsink`
  解码出 mono S16LE PCM；WAV 直读作为资源受限时的降级后端。
- `fingerprint.rs`：确定性的 `band-energy-v1` 指纹（10 个频带 + RMS，无 RNG）。
- `alignment.rs`：在搜索半径内滑动相关，估计设备 B 相对 A 的时钟偏移与置信度。
- `compare.rs`：逐帧余弦相似度、非静音覆盖率、声明缺口的缺失误判保护与决策。
- `resources.rs`：读取 RSS（`/proc`）与剩余磁盘（`statvfs`），决定继续/降级/暂停。
- `pipeline.rs`：运行、暂停、原子检查点、恢复往返校验、模式切换与定稿。
- `store.rs`：证据清单、校验和、复核报告、原始清单保全。
- `verify.rs`：独立复核，含字节级往返、摘要重算与 `--deep` 重解码比对。

## 构建

需要 Rust（1.92+）、GStreamer 1.28（含 app/base 开发库与插件）、GTK4 4.22 开发库。

```bash
cargo build --release
```

本仓库的运行环境无 root 且系统代理不可用，开发依赖被下载到仓库外的
`../.tools/prefix`（用户态前缀），Rust 通过 rustup 安装在 `~/.cargo`。
构建/运行前 `source ../.tools/env.sh` 即可注入 `PKG_CONFIG_PATH`、`LD_LIBRARY_PATH`、
`GST_PLUGIN_SYSTEM_PATH` 与 `RUSTFLAGS`，不修改任何系统目录。

## 命令行用法

```bash
# 新建运行，处理完前 4 个片段后暂停并落盘 checkpoint.json
segment-fingerprint run --case input/case.json --out out/ --pause-after 4

# 从检查点恢复；--force-degrade 切换到低采样率/大帧长的 reduced 模式
segment-fingerprint resume --out out/ --force-degrade

# 独立复核；--deep 会重新走 GStreamer 解码并重算 PCM/指纹哈希
segment-fingerprint verify --record out/run-record.json --deep

# 一键演示：暂停 4 段 -> 强制降级恢复 -> 定稿
segment-fingerprint demo --case input/case.json --out out/ --pause-after 4
```

## 暂停与恢复的正确性

- 每个片段处理完即原子重写 `checkpoint.json`（先写 `.tmp` 再 rename）。
- 恢复时先对检查点做反序列化→重新序列化的**字节级往返比对**，并对锚点片段
  单独计算内容摘要，结果写入 `resume_checks`。
- 资源/对齐策略可在恢复时从 `input/case.json` 热刷新，但 `case_id` 不一致会拒绝恢复。
- 定稿后 `checkpoint.json` 与 `run-record.json` 内容字节一致。

## 资源不足与降级

`case.json` 的 `resource_policy` 定义阈值：

- 剩余磁盘低于 `min_free_disk_mb` 或 RSS 达到 `pause_rss_mb`：暂停并记录游标。
- RSS 达到 `degrade_rss_mb`：从 strict（8kHz/100ms 帧）降到 reduced（4kHz/160ms 帧）。
- GStreamer 不可用且输入为 WAV：切换 `wav-file-fallback`，在证据中标记 `used_fallback`。

每次降级都生成 `degradation_events`，已完成片段的证据保持不变。

## 产物与独立复核

一次完整运行产出：

- `case.original.json`：原始清单的规范副本（SHA-256 入记录）。
- `checkpoint.json` / `run-record.json`：完整状态与最终记录（字段顺序固定）。
- `evidence/alignment.json`：时钟对齐证据；`evidence/fingerprints/*.json`：逐片段逐设备指纹。
- `evidence/manifest.json`：证据清单与复核步骤；`checksums.sha256`：全部证据校验和。
- `review.md`：人工可读复核报告；`verification.json`：机器复核结果。

同事无需信任产生过程，按以下顺序即可独立复核：

```bash
sha256sum -c checksums.sha256
segment-fingerprint verify --record run-record.json
segment-fingerprint verify --record run-record.json --deep   # 有原始媒体时
```

## GTK4 控制界面

`segment-fingerprint-gtk` 提供案例/输出目录选择、运行、暂停、恢复（降级）、
独立复核按钮，以及进度条和逐片段决策表。重活在工作线程执行，界面通过重新读取
检查点文件刷新状态，与 CLI 共用同一套核心库与产物。无头环境可用
`SF_GTK_SELFTEST=1` 触发控件装配自检（仍需可用的显示后端才能真正开窗）。

## 测试

```bash
cargo test
```

核心库含 10 个单元测试，覆盖指纹确定性、静音语义、对齐相关、缺口决策、
资源阈值边界、WAV 解析与规范哈希。
