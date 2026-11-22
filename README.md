# hdr-meta-fix

相机导出 HDR 帧的元数据修复工具。逐帧检查 HDR10/HLG 标签、传递函数、
色彩矩阵、量化范围、母版亮度、MaxCLL/MaxFALL 与曝光值之间的矛盾，
给出逐字段修复建议（含修复依据），支持逐字段应用/撤销，
并导出修复前后对照清单（Markdown + CSV）。

技术栈：Rust + GStreamer（帧元数据提取）+ GTK4（图形界面）。

## 构建与运行

```sh
cargo build
cargo run                          # 启动 GTK4 界面
cargo run -- --selftest            # 用合成帧（正常/缺字段/冲突）自检并导出报告
cargo run -- --analyze video.mp4   # 无头分析真实视频逐帧元数据
cargo test                         # 规则引擎 + GStreamer 集成测试
```

自检报告输出到 `artifacts/report.md` 与 `artifacts/report.csv`，
列为：帧号 | 字段 | 修复前 | 修复后 | 修复依据。

## 界面

- 左侧：帧列表，标注每帧冲突/缺字段数量。
- 右侧：选中帧的字段明细（原始值、当前值、冲突关系描述、建议值与依据）。
- 每个字段行内"应用"/"撤销"按钮；顶栏支持全局撤销一步、导出对照清单。
- "加载示例帧"一键载入正常/缺字段/冲突三类合成帧，便于无素材验证。

## 冲突检测规则（src/rules.rs）

1. HDR10 标签必须搭配 PQ（ST 2084）传递函数。
2. HLG 标签必须搭配 HLG（ARIB STD-B67）传递函数。
3. SDR 标签不得搭配 PQ/HLG 传递函数。
4. HDR 传递函数必须搭配 BT.2020 基色与矩阵（BT.2100）。
5. BT.2020 恒定亮度矩阵仅允许 limited range（BT.2020 表 4）。
6. MaxFALL 不得超过 MaxCLL（CTA-861.3）。
7. MaxCLL 不得超过母版峰值亮度（ST 2086）。
8. EV 隐含场景亮度（EV=log2(L·S/K)，ISO 100，K=12.5）不得超过母版峰值。
9. 声明格式要求的字段缺失时按规范默认值补齐。

## 代码结构

- `src/meta.rs`：帧元数据模型（原始值/当前值双快照）。
- `src/rules.rs`：冲突检测与修复建议。
- `src/fix.rs`：修复会话，逐字段应用/撤销，修改历史。
- `src/report.rs`：对照清单导出（Markdown/CSV）。
- `src/gst.rs`：GStreamer 管线逐帧提取 caps/静态 HDR 元数据。
- `src/synth.rs`：正常/缺字段/冲突三类合成验证帧。
- `src/ui.rs`：GTK4 界面。
- `tests/`：规则引擎测试与 GStreamer 集成测试（含真实编码文件往返）。
