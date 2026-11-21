# plc-eval — 网络录音丢包掩蔽（PLC）离线评估工具

针对网络录音中同时存在的三类丢包——**孤立丢包**、**连续丢包（突发）**、
**单侧缺包**——离线评估三种掩蔽策略：**静音填充**、**重复前帧**、
**插值**。工具输出每个缺口的时长、所选策略、波形能量差（dB）和
可听伪影标记，并支持试听原始 / 降级 / 恢复片段。

技术栈：Rust + GStreamer（播放）+ GTK4（图形界面）。

## 组成

- `src/` — `plc-eval` 核心库 + CLI（纯 Rust，无系统依赖）
  - `loss.rs` 丢包模型：孤立 / 连续 / 单侧，丢包清单（manifest）读写
  - `plc.rs` 三种掩蔽策略；插值在单侧缺包时自动改用跨声道电平匹配拷贝
  - `metrics.rs` 能量差（dB，下限 -60 dB）与边界突变，合成伪影标记
  - `eval.rs` 逐缺口跑三种策略、选最优、计算相对静音基线的削减量
- `gui/` — GTK4 + GStreamer 前端（独立 crate，需系统开发库）
- `tests/core.rs` — 端到端测试（三类丢包的策略选择与削减量核对）

## CLI 用法

```sh
cargo build --release

# 1. 生成三类丢包样例（干净 + 降级 WAV + 丢包清单）
./target/release/plc-eval gen-samples -o samples

# 2. 逐缺口评估（表格：时长/策略/能量差/突变/伪影），并导出最优恢复
./target/release/plc-eval analyze --clean samples/clean.wav \
    --degraded samples/burst.wav --manifest samples/burst.manifest.txt \
    --recovered samples/burst.recovered.wav
# 加 --scores 可查看每个缺口三种策略的完整得分

# 3. 用指定策略恢复（便于 A/B 试听）
./target/release/plc-eval recover --degraded samples/isolated.wav \
    --manifest samples/isolated.manifest.txt --strategy repeat -o out.wav

# 4. 用三类样例核对策略选择与削减量（退出码反映通过/失败）
./target/release/plc-eval check -d samples --min-reduction 3.0
```

## GUI 用法

```sh
# 需要系统依赖：libgtk-4-dev libgstreamer1.0-dev
#   libgstreamer-plugins-base1.0-dev（Debian/Ubuntu）
cd gui && cargo run --release
```

界面中填入 clean / degraded / manifest 三个路径，点击“加载并评估”
后列出每个缺口的时长、策略、能量差与伪影标记；可试听原始、降级与
恢复片段（恢复策略可选 best / silence / repeat / interpolate），
播放经 GStreamer `playbin` 输出。

## 策略选择结论（样例核对结果）

- **孤立丢包**（20 ms）：插值胜出（双侧上下文邻近，能量差≈0 dB）。
- **连续丢包**（80–180 ms）：离线场景下插值仍最优（可利用未来上
  下文）；但**录音末尾的尾部丢包没有未来上下文**，重复前帧（基
  音周期对齐 + 缓衰减）胜出。
- **单侧缺包**：插值自动降级为跨声道电平匹配拷贝，利用完好一侧，
  远优于时间域插值。
- 静音填充仅在原始信号本身为停顿时间隙时最优；伪影标记
  （|能量差| > 3 dB 或边界突变超限）能正确识别其余情况。

`check` 子命令将以上结论固化为断言：三类样例的最优策略胜率
（按有声缺口计）需过半、平均削减量需超过阈值，连续丢包的尾部
缺口必须由重复前帧胜出。
