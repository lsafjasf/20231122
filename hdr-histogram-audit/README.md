# HDR 亮度直方图来源检查页

技术栈固定为 **Rust + GStreamer + GTK4**。页面把 HDR 亮度样本从原始输入到
最终分桶的每一步都记录成可回溯的证据，支持一次截断记录、恢复后的确定性重放、
一个无法判断的边界样本，并且明确指出**第一个错误状态**（具体到记录与步骤），
而不是只给出一个总分。

## 工作区结构

* `crates/audit-core`：纯 Rust、无 GUI 的规则引擎。拥有数据模型、确定性处理
  管线、截断/恢复重放和审计报告，可在无显示服务器的环境里单测。
* `crates/gst-source`：GStreamer 采集层。构建
  `videotestsrc → videoconvert → appsink` 管线，按帧计算 Rec.709 亮度并映射到
  nits，保留来源元素名与缓冲区 PTS 作为来源证据。
* `crates/gtk4-ui`：GTK4 页面。左侧为控制按钮、首个错误状态、计分（仅作上下文）
  和样本记录列表；右上为 cairo 绘制的直方图；右下为逐步处理日志与关键证据。

## 可追溯设计

每条 `SampleRecord` 都保存：

* `raw`：未修改的原始输入（样本 id、PTS、来源元素、nits、像素坐标）。
* `steps`：有序处理日志，每步记录 `input_value`、`output_value`、运行参数和
  `Evidence`（观察描述、数值事实、回溯用的 `source_sample_id`）。
* `verdict`：`Binned` / `Indeterminate` / `Truncated` / `Error`。

处理管线固定为 5 步：`validate → normalize → clip_headroom → edge_guard →
assign_bin`。

### 四类结论的固定场景（`scenario.rs`）

* `s-03`：1400 nits 超过 1000 nits headroom，在 `clip_headroom` 被截断，是全批次
  的**第一个错误状态**。
* `s-04`：归一化后正好落在内部 bin 边界 `0.0625` 上，距离为 0、落在容差带内，
  `edge_guard` 判定为 `Indeterminate`，不强行归桶并保留左右 bin 等证据。
* `s-05`：`NaN` 读数在 `validate` 直接报错。
* `s-01/s-02/s-06`：正常分桶。

### 截断与重放

`capture_truncation` 保存中断原因、已完成记录、待处理原始输入和当时配置；
`recover_and_replay` 用相同输入与配置重跑，并与独立计算的参考运行逐字段比对
（状态、输入、输出、步骤名、结论、记录数）。任何不一致都会定位到具体样本和步骤。

## 构建与运行

本环境没有 root，依赖以 deb 解包到 `~/local/sysroot`，Rust 通过 rustup 安装在
`~/.cargo`。先准备环境变量（脚本内已设置 `PKG_CONFIG_*`、`LD_LIBRARY_PATH`、
`GST_PLUGIN_SYSTEM_PATH`）：

```bash
source ~/local/env.sh
cd hdr-histogram-audit
cargo test --workspace      # 7 个测试：核心规则 4 + GStreamer 2 + 无头冒烟 1
cargo run -p gtk4-ui        # 启动 GTK4 页面（需要 DISPLAY/Wayland）
```

页面按钮：

* `运行审计`：对固定场景跑完整管线并渲染。
* `记录截断`：记录前 3 条已完成样本和剩余待处理输入。
* `恢复并重放`：重放待处理输入并与参考运行比对。
* `从 GStreamer 采集`：实时跑 GStreamer 管线，把采集样本送入同一审计引擎。
* `导出证据 JSON`：把配置、运行、报告、截断记录、重放结果写到
  `hdr_audit_evidence.json`。

一份已生成的证据样例位于 `artifacts/hdr_audit_evidence.json`。

## 无头验证

GStreamer 集成测试在无显示环境下真实拉帧并断言 PTS 与来源元素被保留；
`gtk4-ui/tests/headless.rs` 在不启动 GTK 的情况下装配完整场景并验证 JSON
可序列化、首个错误与重放结论。
