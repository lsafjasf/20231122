# 处理过程

## 1. 数据进入

GStreamer 管线由 `videotestsrc` 生成实时视频，`x264enc` 编码，`identity` 按概率丢缓冲，`appsink` 把编码缓冲交给 Rust：

```text
videotestsrc -> capsfilter -> x264enc -> h264parse -> identity
             -> appsink(max-buffers=1, leaky-type=downstream)
```

`PipelineMeter` 对每个缓冲提取：

- `timestamp_ns`：PTS。
- `frame`：相对首个 PTS、按标称帧周期换算的帧序号。
- `frame_rate`：相邻 PTS 间隔换算的瞬时帧率。
- `bitrate_bps`：最近 1 秒缓冲字节数的滑窗码率。
- `dropped_frames`：帧序号缺口累计；`appsink` 和 `identity` 丢掉的缓冲都表现为 PTS 缺口。
- `encoded_bytes`：累计编码字节。

合成场景直接产生同一 `EncodeSample` 结构，因此 GUI、异常场景和测试共享完全相同的处理代码。

## 2. 清洗与派生值

处理参数先经过 `ProcessingParams::sanitized()`：

- 窗口容量限制在 1 到 65,536。
- EMA alpha 限制在 0.001 到 0.999，非有限值回落为 0.35。
- FPS 阈值满足 `reference >= warn >= critical`。
- 码率阈值满足 `reference <= warn <= critical`。
- 丢帧阈值满足 `warn <= critical`。
- 轨道数、证据事件数均有上限。

样本清洗规则：

- `NaN` / `inf` / 负 FPS 归零并记录 `non_finite_fps` 或 `clamped_fps`。
- 码率超过 10 Gbps 时截断并记录 `clamped_bitrate`。
- 累计丢帧超过 10^10 时截断并记录 `clamped_dropped`。
- 时间戳、帧号或编码字节回退只记录标记，不用无符号减法制造回绕。
- 丢帧计数回退视为计数器重置，增量为 0。
- 帧号跨度大于 1 记录 `frame_gap`。

每个点同时保存输入值、EMA、三项等级和归一化波形值。码率归一化分母取 `max(reference_bitrate, bitrate_critical)`，因此 warning/critical 线始终落在 0 到 1 的可视范围内。

## 3. 等级和边界

```text
FPS:     value < critical => critical
         value < warn     => warning
         其他              => ok

Bitrate: value > critical => critical
         value > warn     => warning
         其他              => ok

Dropped: delta >= critical => critical
         delta >= warn     => warning
         其他               => ok
```

样本的整体等级取三项最大值。对同一轨道，任一指标等级变化就产生一条 `BoundaryEvent`：

```text
event_sequence
metric
from_level -> to_level
before: 前一个完整 ProcessedPoint
after:  当前完整 ProcessedPoint
changes:
  timestamp_delta_ns
  frame_delta
  fps_delta
  bitrate_delta_bps
  dropped_delta_change
  level_change
```

首样本若直接异常，`before` 为 `null`；其余边界都成对保留前后样本。证据事件缓冲区满后，只累计 `events_total` 和 `events_dropped`，内存保持有界。

## 4. 波形映射

每个 `WaveformPoint` 与一个处理点一一对应：

```text
x 轴       = 窗口内样本顺序
FPS 轨     = fps / reference_fps
码率轨     = bitrate / max(reference_bitrate, bitrate_critical)
丢帧轨     = dropped_delta / drop_critical
点颜色     = 整体等级
点 title   = 时间、轨道、帧、原始值、增量、等级
```

GTK4 的 `DrawingArea` 用 Cairo 绘制同样的三项归一化数据；证据目录的 `waveform.svg` 提供无需 GUI 的稳定输出。SVG 最多渲染 512 个点，超过时均匀降采样并强制保留末点。

## 5. 场景与边界证据

| 场景 | 参数 | 目的 |
| --- | --- | --- |
| `empty` | 默认 | 无输入、无样本、SVG 显示 `NO DATA` |
| `boundary` | 默认 | FPS/码率/丢帧阈值前后各一个样本 |
| `abnormal` | 窗口 8、alpha 0.999、240 FPS、9 Mbps/10 Mbps、丢帧阈值 1/2 | `NaN`、无穷、负数、`u64::MAX`、时间戳/帧/计数回退 |
| `huge` | 极限参数、1,000,000 样本、8 轨道 | 有界内存、计数饱和、证据截断 |

`boundary` 中 FPS 的明确证据是：

```text
before: timestamp=1033333333 track=7 frame=101 fps=25.0001 level=ok
after:  timestamp=1066666666 track=7 frame=102 fps=24.9999 level=warning
change: fps_delta=-0.0002 timestamp_delta_ns=33333333 frame_delta=1
```

同类证据也覆盖码率 warning/critical 和丢帧 warning/critical。

## 6. 空输入与超大输入

空输入不会向处理器注入任何合成零点。快照保持：

```text
has_data=false
samples=0
waveform=[]
current_level=ok
```

百万输入策略：

- 处理点窗口固定为 8。
- 边界证据最多保留 32 条，其余只累加计数。
- 轨道数上限为 8，超过上限的样本计入 `rejected_samples`。
- 峰值、样本数、总丢帧增量等使用饱和计数。
- 证据只保存首尾输入、首尾处理点、有限边界事件和固定长度波形。

## 7. 证据文件

每个场景目录包含：

```text
evidence.json    完整摘要与参数
boundaries.json  前后样本及变化
trace.jsonl      每个波形点一行
waveform.svg     三轨波形
manifest.json    文件大小和 SHA-256
```

`artifacts/evidence/gstreamer/` 使用同一格式记录实际 GStreamer 管线。`artifacts/evidence/index.json` 汇总所有场景、截图、测试输出和全局文件校验和。
