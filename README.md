# 摄像机 HDR 元数据修复台

一个固定采用 **Rust + GStreamer + GTK4** 的可暂停修复流程。它处理“相机原始灰度视频 + 逐帧 HDR 元数据”夹具，把异常的极限 CLL/FALL、白点和 mastering 参数重建为内容相关值，并在每个安全边界原子保存中间状态。

构建依赖：Rust 1.92+、GTK 4.12+ 开发包、GStreamer 1.24+ 及 `app`/`videoconvert` 插件。验收环境使用 GTK 4.22 与 GStreamer 1.28。

## 验收点

确定性夹具位于 `fixtures/camera-extreme/`：

- `video.gray16`：128×72、30 fps、150 帧的原始视频，18,432 字节/帧。
- `metadata.bin`：每帧 64 字节的相机 HDR 元数据，共 9,600 字节。
- `camera.json`：相机极限参数、重复对象定义和源视频 SHA-256。

目标验收点为第 72 帧：

| 项目 | 值 |
| --- | --- |
| 时间 | `2.400000000 s` |
| PTS | `2,400,000,000 ns` |
| 原始视频偏移 | `0x144000`（1,327,104） |
| 原始元数据偏移 | `0x1200`（4,608） |
| 修复前 | CLL/FALL `65535`、白点 `0,0`、mastering `65535/0` |
| 修复后 | CLL `9961`、FALL `2542`、白点 `15635,16450`、mastering `10000/1` |
| 重复对象 | `calibration-patch-extreme`，帧 `42/72/102/132` |

原始视频在暂停、恢复和修复前后的 SHA-256 均为：

```text
4d5a5f0c6e4ba9a0cbd5e7188c9fd21ac32ebc449c829687f94ff760735733d6
```

## 流程

1. 导入夹具并校验大小、格式和源哈希。
2. 将 `video.gray16`、`metadata.bin`、`camera.json` 复制到任务目录的 `original/`，权限设为 `0444`。原始材料只读，修复只写 `output/metadata.repaired.bin`。
3. GStreamer 管线为 `appsrc ! videoconvert ! video/x-raw,format=RGBA ! appsink`，逐帧生成内容亮度统计和重复对象命中结果。
4. Rust 侧重写该帧的 64 字节元数据记录，CRC32 随记录更新。
5. 每块结束写入 `state/checkpoint.json`：下一帧、PTS、视频偏移、元数据偏移、已处理前缀 SHA-256、资源模式和恢复次数。写入方式为临时文件 + `fsync` + 原子 rename。
6. 暂停后重新打开同一工作区，从检查点记录的帧和双偏移继续；恢复前校验原始件哈希和已处理前缀哈希。
7. 完成后生成 `acceptance.json`，并在关键帧目录保存原始视频帧、修复前记录、修复后记录及各自 SHA-256。

## 资源不足与降级

资源模型以 RGBA 工作帧和固定 32 KiB 开销估算：

| 预算 | 路径 |
| --- | --- |
| `>= 176 KiB` | 全质量，最多 8 帧/块 |
| `>= 104 KiB` | 全质量，降为 2 帧/块 |
| `>= 68 KiB` | 采样步长 2、单帧处理，记录 `FLAG_RESOURCE_DEGRADED` |
| `< 68 KiB` | 不开始新帧，停在当前安全边界，状态写为 `ResourcePaused` |

UI 提供 `80 KiB 降级` 和 `40 KiB 不足` 两个演练入口。资源不足不会损坏已写结果；提高预算后从同一检查点继续。

仓库中的 `artifacts/resource-low/` 展示了 `pixel_step=2` 与 `flags=14`（已修复 + 重复对象 + 资源降级）；`artifacts/resource-blocked/` 展示了在创建处理管线前以 `40960` 字节预算安全停在帧 0，再由提高后的预算继续。

## 运行

```bash
# GTK4 工作台
cargo run --bin hdr-repair

# 无头演示：2.400s 暂停，自动恢复，生成完整验收证据
cargo run --no-default-features -- headless \
  --fixture fixtures/camera-extreme \
  --workspace artifacts/demo-run \
  --pause-ms 2400 \
  --memory-kib 1024 \
  --auto-resume --reset

# 回归测试
cargo test --all-features
```

GTK4 窗口左侧显示极限参数、重复对象、块大小/内存预算和验收卡；右侧显示 GStreamer 帧预览、处理进度和事件证据。`在 2.400s 暂停` 会精确停在帧 72 的检查点，`继续` 从 `pts=2,400,000,000` 恢复。

## 证据文件

一次已完成任务的目录：

```text
artifacts/demo-run/
├── original/                    # 0444，只读原始材料
├── output/metadata.repaired.bin
├── state/checkpoint.json        # 最终检查点
├── state/paused.json            # 暂停时的检查点快照
├── evidence/events.jsonl        # 事件、记录十六进制、偏移
├── evidence/key_frames/00072/
│   ├── source.gray16
│   ├── metadata.before.bin
│   ├── metadata.after.bin
│   └── manifest.json
└── acceptance.json
```

`acceptance.json` 的 `pass` 同时要求：修复前记录带相机故障标志、修复后带已修复标志、帧号/PTS/源偏移不变、CLL 发生变化。`resume_comparison.prefix_verified` 证明暂停前已写元数据前缀在恢复后未改变。集成测试还会把“暂停再恢复”的最终输出哈希与不中断运行逐字节比较。
