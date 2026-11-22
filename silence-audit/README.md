# 音轨静音段识别 · 可复核流程

固定技术栈：**Rust + GStreamer + GTK4**。把“音轨静音段识别”做成一条输入可追溯、
输出可复核、异常可隔离、临时状态可清理的批处理流程。

## 复核链路

1. **快照输入**：同一批对象支持两种快照格式（见 `examples/`）：
   - 旧格式 `legacy`：扁平 `records` 数组，字段简短；缺少 `closed:true` 或 `end_time`
     的记录判定为**截断记录**。
   - 新格式 `v2`：显式 `version: 2`，对象分版组织；`state:"truncated"` 或缺少
     `end_frame` 判定为截断。
2. **GStreamer 分析**（`src/pipeline/`）：
   `filesrc ! decodebin ! audioconvert ! audioresample ! capsfilter(F32LE) ! tee`
   - 一支经 `level` 元素按固定间隔上报每声道 RMS/峰值（dB），用带迟滞的状态机
     （`on_millis`/`off_millis`）判定静音段；
   - 另一支通过 pad probe 收集原始 PCM，聚合为固定桶数的峰值/RMS 波形。
3. **四要素对应**：每个静音段同时记录时间点（秒）、轨道、帧序号（帧 ≈ 时间 × 采样率）、
   处理参数（阈值/迟滞/间隔/重采样率），并与输出波形共享同一时间轴。
4. **GTK4 复核**（`src/ui/`）：
   - `review` 子命令打开窗口，左侧对象列表、右侧波形（红色遮罩高亮静音段）与明细；
   - 批处理用 cairo 离屏渲染每个对象的波形 PNG 作为长期证据，无需显示环境。
5. **证据链**（`src/report/`）：`report.json` 记录输入快照 SHA256、每个对象的参数/
   静音段/波形、全部证据文件的 SHA256 与大小，并附带 `report.json.sha256`。

## 健壮性与清理

- **异常隔离**：单个对象失败（文件缺失、解码错误等）只产生 `failed` 结果，
  其余对象继续处理；运行结束汇总成功/失败计数。
- **截断语义**：截断记录末尾仍在持续的静音段保持开放（`end_secs/end_frame = null`、
  `truncated = true`），不伪造结束点。
- **临时状态清理**：所有中间文件先落在运行目录的 `tmp/`，运行结束统一删除；
  证据与报告保留在 `evidence/` 与运行目录根部。

## 用法

```bash
# 批处理（参数均可调，以下为默认值）
cargo run -- batch \
  --snapshot examples/snapshot.legacy.json \
  --media-base ./media \
  --output runs \
  --threshold-db -50 --on-millis 200 --off-millis 300 \
  --interval-millis 50 --resample-rate 48000 --waveform-buckets 512

# 打开 GTK4 复核窗口（无物理显示时用 broadway：
#   gtk4-broadwayd :5 后设置 GDK_BACKEND=broadway BROADWAY_DISPLAY=:5）
cargo run -- review --run-dir runs/run-<uuid>
```

每次运行的目录结构：

```
runs/run-<uuid>/
├── report.json            # 完整复核报告
├── report.json.sha256     # 报告自检值
└── evidence/
    ├── <id>.waveform.png  # 波形+静音段遮罩证据
    └── <id>.input.<ext>   # 输入媒体留存（哈希入报告）
```

## 构建环境

- Rust 1.100（离线 cargo 缓存含 gstreamer-rs 0.24 / gtk4-rs 0.10）。
- GStreamer 1.28（含 `level`、`decodebin`、`wavparse`、coreelements 插件）与 GTK 4.22
  开发包；本环境通过把预置 deb 解包到本地前缀并配置 `PKG_CONFIG_PATH`、
  `LD_LIBRARY_PATH`、`GST_PLUGIN_SYSTEM_PATH` 完成无 root 构建。
- 验证：`cargo test`（快照兼容、截断开放语义、端到端批处理、失败隔离、证据哈希、
  临时目录清理共 5 个测试）。
