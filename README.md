# 音效素材批量变体 · 可回放实验

技术栈固定为 **Rust + GStreamer + GTK4**，无外部音频输入：所有素材由
GStreamer `audiotestsrc` 在进程内合成，保证同一 `JobSpec` 在任何机器、
任何时间重放都产出字节一致（SHA256 相同）的 WAV。

## 工作区结构

- `svbatch/` — UI 无关的核心库：GStreamer 引擎、取消令牌、批处理注册表、场景定义。
- `svbatch-cli/`（二进制 `svcli`）— 无头实验运行器与自动判定。
- `svbatch-gtk/`（二进制 `svgui`）— GTK4 控制面板，调用同一个核心库。
- `env.sh` — 工具链环境（Rust 1.100、conda 内 GStreamer 1.28 / GTK 4.24、绕过失效代理）。
- `run-experiment.sh` — 一键回放：构建 → 六场景实验 → 人工清单 → GTK4 冒烟。

## 实验场景（每次运行固定执行）

| 场景 | 目的 | 取消 | 作业数 |
| --- | --- | --- | --- |
| `S1-baseline` | 基线全量批次，建立哈希基准 | 无 | 9 |
| `S2-cancel-mid` | **实验组：中途取消**（350ms 定时触发） | 有 | 9 |
| `S3-control-no-cancel` | **对照组：无取消**，同批作业全部跑完 | 无 | 9 |
| `S4-duplicate-submit` | 重复提交：第 10 个作业与第 1 个同 idempotency key | 无 | 10（1 拒绝） |
| `S5-reordered` | 顺序改变：变体优先排序，作业集合不变 | 无 | 9 |
| `S6-replay` | 重放：与 S1 逐字节比对 | 无 | 9 |

## 记录的输入、状态与结果

每个场景在 `evidence/runs/<场景>.json` 留下三层证据：

- `steps[]`：有序步骤（`batch_start` / `submit_accepted` / `submit_rejected` /
  `job_start` / `job_end` / `batch_end`），每步记录 `input`（含完整 GStreamer
  管线描述）、`state`（取消令牌与作业状态）、`result`（文件、SHA256、字节数）。
- `snapshots[]`：带序号与相对毫秒时间戳的全量作业状态快照。
- `jobs[]`：每个作业的终态与产物哈希。

产物 WAV 位于 `evidence/outputs/<场景>/`，GUI 冒烟证据位于 `evidence/gui/`。

## 自动结论与人工确认（分开呈现）

- 自动判定：`evidence/reports/auto-conclusion.json`，七项检查 C1–C7，
  每项含 `claim`、`passed`、`evidence`、`remediation`。
- 人工确认：`evidence/reports/manual-review.md`，由 `svcli render-review`
  基于自动结论生成勾选清单，人工填写复核人与日期；两者文件分离、职责分离。

## 如何回放

```bash
source ./env.sh
./run-experiment.sh                 # 全量回放（含 GTK4 冒烟）
svcli inspect-jobs                  # 只看两种提交顺序与 idempotency key
svgui                               # 手动操作 GTK4 面板
svgui --smoke                       # 无头自动：提交→120ms 取消→900ms 退出
```

## 失败时的可执行后续操作

每条自动检查都内嵌 `remediation`，通用排障顺序：

1. 读 `evidence/runs/<失败场景>.json` 中最后一个 `job_end` 的 `detail`。
2. 复制该步 `input.pipeline`，用 `gst-launch-1.0` 手工复现（`source env.sh` 后可直接运行）。
3. 哈希漂移：`sha256sum evidence/outputs/S1-baseline/*.wav evidence/outputs/S6-replay/*.wav` 定位差异文件。
4. 取消异常：检查 `svbatch/src/audio.rs` 的 50ms bus 轮询与 20ms 睡眠粒度。
5. 归档当前证据后重跑：`mv evidence evidence.bak.$(date +%s) && ./run-experiment.sh`。
6. 依赖问题：`source env.sh` 确认 `PKG_CONFIG_PATH` 指向 `/home/hj/env-media`；
   crates 走直连（脚本会自动 unset 失效的 `*_proxy` 变量）。
