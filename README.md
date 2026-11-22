# safe-frame-audit · 画面安全框检测（可复核流程）

技术栈：Rust + GStreamer（逐帧分析）+ GTK4（复核界面）。

核心思想：检测不是给一个总分就结束，而是产出一条**可复核的记录**——
每一帧的时间点（pts）、轨道、帧号、处理参数指纹和输出波形（metric 序列）
一一对应；记录可能被截断，可以恢复，恢复后可以重放比对，
并且任何时候都能直接指出**第一个错误状态**。

## 流程

```bash
source /home/hj/sysroot/env.sh        # 工具链/库环境（见下文）
cargo build

# 1. 生成确定性测试媒体（无损 raw/matroska，白块漂移越过字幕安全框）
./target/debug/safe-frame-audit generate --out artifacts/sample.mkv --frames 100

# 2. 分析并写记录（v2 新格式；--format v1 写旧格式）
./target/debug/safe-frame-audit analyze --media artifacts/sample.mkv \
    --record artifacts/sample.sfar2.jsonl --evidence-dir artifacts/evidence

# 3. 模拟崩溃：记录被截断
./target/debug/safe-frame-audit truncate --record artifacts/sample.sfar2.jsonl --bytes 40

# 4. 恢复：截掉垃圾尾、保留 .truncated.bak 证据、追加 recovered 标记
./target/debug/safe-frame-audit recover --record artifacts/sample.sfar2.jsonl

# 5. 恢复后的重放：按记录里的参数重新分析，逐条比对
./target/debug/safe-frame-audit replay --record artifacts/sample.sfar2.jsonl \
    --report artifacts/replay_report.json

# 6. GTK4 复核界面（波形 + 首错标记 + 条目列表 + 证据帧 + 一键重放）
./target/debug/safe-frame-audit ui --record artifacts/sample.sfar2.jsonl
```

## 记录格式（新旧兼容）

- **v1（旧）**：每行一条 `{"v":1,"pts_ms":..,"frame":..,"track":..,"metric":..,"violation":..}`，
  无参数头。读取时自动识别；重放需 `--media` 指定媒体，参数用默认值。
- **v2（新）**：首行 `header`（媒体路径、处理参数、`params_hash`），
  随后每条 `entry` 带 `seq/pts_ms/frame/track/metric/params_hash`，
  末尾 `eof` 行带输出波形（metric 序列）的 CRC32。
  恢复后追加 `recovered` 标记行（v1 用 `{"v":1,"recovered":true,..}`）。

加载器对两种格式都宽容：遇到无法解析的尾部（截断）即停，并报告垃圾字节数。

## 第一个错误状态

- 记录层面：`Record::first_error()` = 第一条 `violation=true` 的 entry
  （帧号、pts、轨道、metric 全部给出）。
- 重放层面：逐条比对记录与重算结果，报告**第一个不一致**
  （`metric_diverged` / `missing_in_record` / `missing_in_replay`），
  而不是只给总分。退出码 0=OK，3=有问题。

## 关键证据（全部落盘）

- `artifacts/evidence/first_error_frame_*.png`：首错帧 + 安全框叠加（自研 PNG 编码器，零依赖）
- `artifacts/evidence/waveform.png`：输出波形图（阈值线 + 首错竖线标记）
- `*.truncated.bak`：截断原件，恢复时不覆盖
- `artifacts/replay_report.json`：重放报告（首错、首个不一致、波形 CRC、参数指纹）
- `fixtures/`：仓库内置的被截断记录（v1/v2 各一条），可直接 `recover` + `replay` 复现

## 环境

系统只有 GStreamer 运行时，无 sudo。所有开发库解包在 `/home/hj/sysroot`，
`.cargo/config.toml` 已固化 `PKG_CONFIG*` / `LD_LIBRARY_PATH` / `GST_PLUGIN_PATH`，
因此 `cargo build` / `cargo test` 开箱即用；直接运行二进制前建议
`source /home/hj/sysroot/env.sh`（或改用 `cargo run`）。

## 测试

```bash
cargo test    # 7 个单元测试 + 1 个端到端测试（生成→分析→截断→恢复→重放，v1/v2）
```
