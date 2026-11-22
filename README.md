# 视频旋转元数据修复 · 来源检查页

对一批视频对象执行旋转元数据检查/修复决策，并为每个对象生成可回溯的
来源链：**结论 → 处理记录 → 两个版本快照 → 关键证据 → 原始输入**。
技术栈：Rust + GStreamer + GTK4。

## 功能与设计

- **两个版本快照**：每个对象在 `ingest`（v1）与 `normalize`（v2）阶段各留
  一份状态快照，记录原始旋转信息与 GStreamer 规范化结果。
- **被截断的记录**：处理记录超过 32 步即截断（保留头部、截断标记与末步），
  同时保留原始步数与完整记录的 FNV-1a 哈希，截断可核验。
- **无法判断的边界**：旋转标签与显示矩阵矛盾时（如 `rotate-90` ↔ `180°`）
  结论为「无法判断」，双方原值作为关键证据保留，不做猜测性修复。
- **异常后继续**：单个对象失败（如损坏的 sidecar）记为「处理异常」并保留
  异常现场证据，批处理继续处理其余对象。
- **结束时清理临时状态**：规范化中间产物写入临时工作区，运行结束自动删除
  并在报告中核验 `temp_cleaned`。

## GStreamer 的使用

- `filesrc ! typefind ! fakesink` 真实管线探测容器类型（证据 `GstProbe`）。
- `GstTagList` + `image-orientation` 标签做旋转值的规范化读写。

## 构建与运行

```sh
source scripts/devenv.sh        # 本地 sysroot（GTK4/GStreamer 开发文件）
cargo test -p rotprov-core      # 核心管线测试（含真实 GStreamer 探测）
ROTFIX_ROOT=$PWD cargo run -p rotprov-core --bin rotfix-run   # headless 报告
ROTFIX_ROOT=$PWD cargo run -p rotprov-gtk                     # GTK4 来源检查页
```

报告输出到 `artifacts/provenance/report.json`。
UI 中 `ROTFIX_SELECT=<行号>` 可预选对象（无头测试用）。

## 布局

- `crates/rotprov-core` — 来源引擎：模型、管线、GStreamer 探测、临时工作区。
- `crates/rotprov-gtk` — GTK4 检查页：对象列表 + 来源链详情。
- `fixtures/` — 5 个示例对象（修复/跳过/存疑/损坏/超长记录）。
