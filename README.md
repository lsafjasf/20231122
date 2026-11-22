# 环绕声对象下混 · 可复核流程

把“环绕声对象下混”做成一条可复核的流水线：对每个音频对象记录两个版本快照
（旧配方 / 现行配方），把时间点、帧序号、处理参数与输出波形逐项对应，
同时兼容旧格式与新格式清单，并完整保留关键证据。

技术栈固定为 **Rust + GStreamer + GTK4**。

## 工作区结构

- `crates/downmix-core`：核心库（发现、嗅探、下混管线、双版本清单、复核、清理）
- `crates/downmix-cli`：命令行工具 `downmix`
- `crates/downmix-gtk`：GTK4 图形界面 `downmix-gtk`

## 可复核性设计

- **未知扩展记录**：扩展名未知或缺失的文件通过文件头魔数（RIFF/Ogg/FLAC/ID3 等）
  做内容嗅探，标记为 `sniffed_content`，与常规扩展名对象区分。
- **两个版本快照**：每个对象分别用
  - 旧配方 `legacy-broadcast-v1`（旧版扁平清单 `manifest.legacy.json`）
  - 现行配方 `itu-bs775-stereo-v2`（版本化 envelope 清单 `manifest.v2.json`）
  渲染一次，输出 `downmix.legacy.wav` 与 `downmix.current.wav`。
- **参数与波形对应**：清单记录输入/输出声道布局、完整下混系数矩阵、采样率、
  采样格式、管线描述；关联表按固定帧间隔（1024 帧）给出 `pts_ns`、`time_ms`、
  `frame_index`、峰值/RMS 以及该帧原始字节的 SHA-256。
- **关键证据**：源文件与输出文件均留存大小与 SHA-256；波形汇总含总帧数、
  时长、全局峰值/RMS/直流偏置。
- **格式兼容**：读取侧 `SnapshotManifest::load_any` 同时接受旧版扁平文档与
  新版版本化 envelope，复核流程不依赖写入时的格式。
- **异常后继续**：单个对象失败只记录失败清单，不中断批处理；
  `run-summary.json` 汇总成功/失败计数。
- **临时状态清理**：所有中间产物放在 `TempDir` 守卫下，正常与异常路径都会删除；
  `clean` 子命令可清理中断后残留的 `downmix-tmp-*` 目录，证据目录不受影响。

## 命令行用法

```bash
# 发现对象（含未知扩展的嗅探结果）
downmix discover --input /path/to/objects

# 对每个对象渲染两个版本快照并留存证据
downmix run --input /path/to/objects --evidence /path/to/evidence

# 重新计算输出哈希并校验关联表（接受旧/新两种清单格式）
downmix verify --input /path/to/objects --evidence /path/to/evidence

# 清理中断残留的临时目录
downmix clean --evidence /path/to/evidence
```

`run` 在有对象失败时仍会处理完其余对象，并以非零退出码提示；
`verify` 发现任何哈希或关联表不一致时以非零退出码结束。

## 证据目录布局

```text
evidence/
├── run-summary.json              # 批处理汇总（成功/失败/跳过）
└── <object-id>/                  # 每个对象一个稳定 ID 目录
    ├── manifest.legacy.json      # 旧格式：扁平清单
    ├── manifest.v2.json          # 新格式：版本化 envelope
    ├── downmix.legacy.wav        # 旧配方输出
    └── downmix.current.wav       # 现行配方输出
```

## GStreamer 管线

```text
filesrc ! decodebin ! queue ! audioconvert ! audioresample
  ! capsfilter(F32LE, N 声道)
  ! audiomixmatrix(mode=manual, in-channels=N, out-channels=2, matrix=<配方矩阵>)
  ! tee
      ├─ queue ! wavenc ! filesink        # 证据 WAV
      └─ queue ! appsink                  # 原始 F32 样本，用于波形关联表
```

支持 1/2/4/6/8 声道输入布局（单声道、立体声、四声道、5.1、7.1），
未知布局会显式报错而不是静默处理。

## GTK4 界面

`downmix-gtk` 提供目录选择、运行双版本下混、证据复核、临时状态清理四个动作，
对象结果以表格展示（对象路径、是否未知扩展、状态、说明），耗时任务在独立
线程执行并通过主线程轮询回传，不阻塞 UI。

## 测试

```bash
cargo test --workspace
```

覆盖：旧/新清单格式解析与回环、两套配方矩阵差异、未知扩展嗅探、非音频跳过。
