# HLS 断点续播诊断 · 可回放实验

一个完全可复现的实验工程，用于诊断 HLS 断点续播记录的处理逻辑。
技术栈固定为 **Rust + GStreamer + GTK4**，无需 root 即可在本机构建运行。

## 实验目标与覆盖场景

| 要求 | 实现方式 |
| --- | --- |
| 两个版本快照 | `fixtures/snapshots/v1_*.json`（整秒精度）与 `v2_*.json`（毫秒精度 + 完整性哈希） |
| 一条被截断的记录作对照 | `fixtures/snapshots/v2_truncated_control.json`，JSON 在字符串中途被截断 |
| 记录每一步的输入、状态、结果 | 追加式 JSONL 日志 `evidence/<run>.journal.jsonl`，每行含 `step/phase/input/state/result` |
| 验证顺序改变 | 每个快照先按提交顺序处理，再用全新去重状态逆序处理，并写入 `order_change` 对比证据 |
| 验证重复提交 | `run-config.json` 中的 `duplicate_ids` 被二次提交，引擎判定为 `duplicate` 并忽略 |
| 异常后继续处理其余对象 | 截断快照仅产生一条失败报告，后续快照与记录继续处理，运行不中断 |
| 保留关键证据 | `evidence/` 下持久化日志、报告与 GTK4 截图；临时探针文件放在系统临时目录 |
| 结束时清理临时状态 | CLI 运行结束自动删除 scratch 目录，durable 证据保留；另提供 `cleanup` 子命令 |

## 目录结构

```
hls-resume-lab/
├── Cargo.toml
├── run-config.json              # 实验声明式配置
├── fixtures/
│   ├── snapshots/               # v1、v2、截断对照三个快照
│   └── media/channel-{a,b,c}/   # 本地 HLS VOD 清单与分片（ffmpeg 生成）
├── src/
│   ├── core/                    # 纯引擎：模型、日志、去重、顺序、编排
│   ├── gst_backend.rs           # GStreamer 管线探针（uridecodebin + appsink，PAUSED 预卷）
│   ├── gtk_app.rs               # GTK4 界面（feature = "gtk"）
│   ├── cleanup.rs               # 临时状态生命周期
│   └── main.rs                  # CLI：run / replay / cleanup / gui
├── scripts/
│   ├── env.sh                   # 用户态 sysroot 环境变量
│   ├── run-experiment.sh        # 一键端到端实验
│   ├── start-xvfb.sh            # 无 root 启动 Xvfb（bwrap 用户命名空间）
│   └── verify-gui.sh            # 无头运行 GTK4 并截图
├── tests/experiment_flow.rs     # 端到端集成测试
└── evidence/                    # 运行后生成的持久证据
```

## 无 root 依赖前缀

本机没有 sudo 权限，开发库通过 `apt-get download` + `dpkg-deb -x` 解包到仓库外的
`../.sysroot`。构建/运行前 `source scripts/env.sh` 即可注入 `PKG_CONFIG_PATH`、
`LD_LIBRARY_PATH`、GStreamer 插件路径与 GTK schema 路径。

## 常用命令

```bash
source scripts/env.sh

cargo test                      # 9 个单元/集成测试
cargo build --features gtk      # 含 GTK4 界面的构建

./target/debug/hls-lab run run-config.json demo-001   # 运行实验
./target/debug/hls-lab replay evidence/demo-001.journal.jsonl
./target/debug/hls-lab cleanup demo-001

bash scripts/run-experiment.sh final-001   # 一键完整流程
bash scripts/verify-gui.sh                 # 无头 GUI 截图
```

## 关键证据解读

- `evidence/<run>.journal.jsonl`：按步追加、崩溃也安全；可被 `replay` 子命令逐条回放。
- `evidence/<run>.report.json`：结构化汇总，含每个快照的诊断结果与 GStreamer 探针结论。
- `evidence/screenshots/gtk4-gui.png`：GTK4 界面在 Xvfb 上的实际渲染截图。

截断对照在日志中以 `error_kind: "truncated"` 明确标记（基于 serde 的 EOF 判定），
与普通 JSON 语法错误区分。GStreamer 探针将每条记录的 HLS 清单通过
`uridecodebin` 构建管线并预卷到 `PAUSED`，使用 `hlsdemux2` 工厂与 `appsink`，
验证通过后回到 `NULL` 并清理。
