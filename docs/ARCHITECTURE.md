# 架构与设计说明

## 目标

针对“同一重复对象、两台时钟不同步设备”的素材，提供可对**每个媒体片段单独重跑、比较、导出**
的关键帧索引重建与局部修正工具。支持中途退出与重新进入，全程保留关键证据，导出物可由
不接触本程序的同事独立复核。

技术栈固定为 **Rust + GStreamer + GTK4**，采用 Cargo 工作区：

```text
crates/
  vkri-core/   领域模型、持久化、证据、GStreamer 提取、比对、自校验导出（无 UI 依赖）
  vkri-cli/    终端界面 `vkri`，薄封装
  vkri-gui/    GTK4 图形界面 `vkri-gui`，薄封装
```

CLI 与 GUI 共享同一个 `ProjectService`，因此状态流转、审计记录与磁盘产物完全一致。

## 时间与时钟模型

所有绝对时间存为 Unix 纳秒 `i64`。每个片段记录 PTS 零点对应的设备墙钟读数
`clock_origin_unix_ns`，每台设备带一个时钟偏移 `ClockOffset`：

```text
设备时间   = clock_origin_unix_ns + pts_ns
参考时间   = 设备时间 + device.clock_offset.offset_ns
```

参考时间把两台不同步的设备时钟校正到同一时间轴，比对即基于该时间轴进行。

## 持久化与字段顺序

- 项目文档固定为 `<project>/project.json`，serde 按结构体字段声明顺序序列化，
  配合 `preserve_order`，**重新载入后字段与数组顺序逐字节保持**（有测试覆盖）。
- 写入走 `project.json.tmp` → fsync → 原子 rename，崩溃只会留下旧版或新版，不会出现半写文件。
- 校验在保存/加载时执行：schema 版本、设备/片段 id 唯一性、外键、关键帧索引连续性。

## 证据与审计

```text
evidence/
  _global/audit.jsonl
  <segment_id>/
    audit.jsonl                 追加式、哈希链
    run-<run_id>/
      kf-000001.jpg ...         缩略图（内容以 SHA-256 命名引用）
      manifest.json             本次运行的帧清单（含中断标记）
```

- 审计为 JSONL，每条记录含 `prev_sha256` 与自身 `record_sha256`，形成可独立校验的哈希链。
- 只追加、从不改写或删除。重跑产生**新的 run 目录**，旧证据保留。
- 关键帧只在一次运行**完整成功**后才替换项目中的当前索引；中断时保留部分产物并标记
  `interrupted`，可随时重新进入续跑（生成新的完整 run）。

## GStreamer 提取管线

```text
filesrc ! decodebin3 ! queue ! videoconvertscale(RGB caps) ! appsink
```

- `decodebin3` 动态 pad 在 pad-added 时接入（caps 未协商时也尝试连接，兼容早期事件）。
- 抽帧策略在 Rust 侧 `FrameSelector` 决定：
  - `interval`：距上一个保留帧达到 `interval_ns` 即保留，确定性均匀采样；
  - `keyframe_only`：仅保留非 DELTA_UNIT 的解码器关键帧。
- 缩略图缩放在 Rust 内做确定性最近邻缩放，再经可复用的
  `appsrc ! jpegenc ! appsink` 编码为 JPEG，落盘后计算 SHA-256 内容地址。
- 取消通过 `CancellationToken`：主循环每轮检查，触发后干净拆除管线、保留部分证据、
  写入 `run.cancel_signalled` / `run.interrupted` 审计。

## 比对

对同组（同一重复对象）两个已提取片段，在参考时间轴上做贪心最近邻匹配，受
`match_window_ns` 约束；逐对输出时差，取中位数并给出建议的设备 B 偏移
`suggested_offset_b_ns = -median(delta)`。算法确定性强，可对照导出 CSV 手工复核。

## 导出与独立复核

导出包包含 `project.snapshot.json`、`keyframes.csv`、`comparisons.csv`、完整 `evidence/`、
`SHA256SUMS`、`MANIFEST.json`（含自身摘要）与中文 `REVIEW.md`。

复核方式不依赖本程序：

```sh
sha256sum -c SHA256SUMS
```

也可用 `vkri verify-export` 复核摘要清单、MANIFEST 自哈希与每条审计哈希链。

## GTK4 界面

- 三页：片段与关键帧 / 设备时钟 / 比对与偏移；底部状态条带进度与“停止”按钮。
- 提取在独立 worker 线程运行，经 `std::sync::mpsc` + 100ms `timeout_add_local` 回传进度，
  界面不卡顿；关窗或点停止都会翻转取消令牌。
