# 设计说明

## 模块划分

| 模块 | 职责 |
| --- | --- |
| `model.rs` | 项目/片段/证据的数据结构，字段声明顺序即磁盘顺序 |
| `storage.rs` | 项目文件的加载与原子写入（先写 `.tmp` 再 rename） |
| `media.rs` | GStreamer 初始化、Discoverer 探测、重采样流水线、证据采集 |
| `wav.rs` | 精确静音 PCM 合成、RIFF/WAVE 头重建、帧级确定性拼接 |
| `engine.rs` | 片段状态机、崩溃恢复、证据组装、哈希复核 |
| `review.rs` | 自包含复核 bundle、SHA256SUMS、独立 `verify.sh`、确定性 CSV |
| `ui.rs` | GTK4 界面；处理在 worker 线程，主线程用定时器轮询通道 |
| `cli.rs` | 与 GUI 共用同一引擎的 headless 入口 |
| `util.rs` | 流式 SHA-256、WAV 头解析、RFC3339 时间戳 |

## 状态机

```text
pending ──run──▶ running ──成功──▶ done
                    │
                    └──失败/中断──▶ failed ──rerun──▶ running …
```

进入 `running` 时 `attempts += 1` 并立即落盘；成功则追加证据并置 `done`，
失败则记录 `last_error` 并置 `failed`。重跑只追加证据，从不覆盖历史。
应用启动加载项目时，`engine::recover_interrupted` 把残留的 `running`
标记为 `failed`，因此中断现场可被明确识别。

## 缺失片段的确定性处理

早期方案尝试用 `valve` 在播放位置上实时切换静音源，但这种方式依赖运行时
时钟，重跑无法保证字节一致。最终采用三段式、可复现的做法：

1. GStreamer：`uridecodebin ! audioconvert ! audioresample !
   audio/x-raw,format=S16LE,rate=<目标> ! wavenc` 得到重采样 WAV。
2. Rust：按 `gap_duration_ns × 目标采样率 / 1e9` 计算**精确帧数**，合成
   零填充 16-bit PCM 静音 WAV（头部大小即实际大小）。
3. Rust：在 `gap_start_ns` 对应的 PCM 帧处切片，重建 RIFF/WAVE 头并拷贝
   精确帧区间，产出 `output.wav`。

因此输出帧数恒等于「重采样源帧数 + 声明静音帧数」，证据中的
`expected_frames` 据此计算，`comparison.csv` 的 `frame_count_matches`
可直接核对。

## 磁盘布局

```text
<项目目录>/
├── project.srcrepair.json      # 唯一事实来源，原子写入
├── fragments/
│   └── <id>/
│       ├── resampled.wav       # GStreamer 重采样中间产物
│       ├── gap.wav             # 精确静音（无 gap 时不存在）
│       └── output.wav          # 最终交付与哈希对象
└── exports/review-<时间戳>/    # 自包含复核 bundle
```

## 线程模型

GTK 主线程只负责渲染；点击 Run 后启动独立 worker 线程，worker 自己加载
项目文件、驱动 GStreamer 并写回所有状态。线程间通过 `std::sync::mpsc`
传递完成消息，主线程以 `glib::timeout_add_local` 非阻塞轮询。处理期间
可以直接关窗：状态已在磁盘上，重开后从项目文件恢复。

## 复核者视角

复核者不需要信任本程序：bundle 内的 `verify.sh` 只依赖 `sha256sum`，
`manifest.json` 记录了完整流水线字符串与全部输入/输出哈希，可用
`gst-launch-1.0` 按描述手工复现重采样步骤，再按文档中的帧拼接规则核对。
