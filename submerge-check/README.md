# 多语言字幕时间轴合并 — 来源检查页

技术栈固定为 **Rust + GStreamer + GTK4**：

- Rust 负责全部业务逻辑、审计链与临时状态生命周期；
- GStreamer 通过 `filesrc ! subparse ! appsink` 对每个快照做独立解析核验；
- GTK4 渲染来源检查页，展示快照、证据链、结论与处理步骤。

## 可回溯性设计

每个结论都能回到原始输入与处理步骤：

- 两份版本快照（英文 V1 / 中文 V2）原样落盘并记录 SHA-256、字节数、行数；
- 每条字幕保留 `raw_record` 原始字节、原始行号区间与记录序号；
- 合并结论携带证据指针（A/B 侧记录序号）与完整证据列表；
- 16 步处理日志记录每个阶段的对象、动作与结果。

## 内置场景

| 结论 | 场景 |
| --- | --- |
| 已对齐 | B1 与 A1 起止时间完全一致 |
| 部分重叠 | B0 与 A0 起点不同但区间相交 |
| 无法判断 | B3 与 A2/A3 等权重叠（各 1500 ms），无打破平局依据 |
| 结构损坏 | B4 时间轴行缺少 `-->`，单对象异常被隔离 |
| 来源截断 | B5 缺少结束时间且末尾多字节 UTF-8 被截断，保留末尾字节证据 |

异常隔离：解析与 GStreamer 核验均按对象独立 `catch_unwind`，任一对象失败后其余对象继续处理。

临时状态：`TempWorkspace` 使用 RAII，流水线结束（含 panic 路径）自动删除临时目录，并在报告中记录清理前后状态。

## 运行

```bash
cargo build --release

# GTK4 界面（需要显示服务）
./target/release/submerge_check

# 无界面自检（CI / 无显示环境），退出码 0 表示硬性约束全部通过
./target/release/submerge_check --selftest

# 指定报告目录
./target/release/submerge_check --selftest --report-dir ../artifacts
```

## 测试

```bash
cargo test      # 12 个单元/集成测试，含真实 GStreamer pipeline
cargo clippy --all-targets
cargo fmt --check
```

## 模块

- `src/model.rs`：快照、字幕、结论、GStreamer 核验结果数据模型
- `src/sample.rs`：两份内置快照输入（含截断与损坏记录）
- `src/parser.rs`：容错 SRT 解析器，按原始行号保留证据
- `src/gst_check.rs`：GStreamer pipeline 核验（异常收敛为结果字段）
- `src/merger.rs`：时间轴合并判定与证据组装
- `src/audit.rs`：流水线编排、异常隔离、处理步骤与文本报告
- `src/workspace.rs`：RAII 临时工作区
- `src/ui.rs`：GTK4 来源检查页与 `--selftest` 摘要
