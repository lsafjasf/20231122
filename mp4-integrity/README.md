# MP4 容器完整性检查工具

技术栈固定为 **Rust + GStreamer + GTK4**。工具直接按 ISO-BMFF（MP4/QuickTime）
规范遍历 box 树，逐项输出块类型、字节偏移、轨道归属、错误状态与可修复性，
并用 GStreamer `qtdemux` 独立重载容器来核对轨道数量。

## 构建（离线 sysroot 环境）

```bash
source env.sh
cargo build --offline
```

`env.sh` 指向 `/home/hj/sysroot` 中的 GTK 4.22 / GStreamer 1.28 开发库以及
`~/.cargo` 中的离线 crate 缓存。

## CLI 用法

```bash
# 检查容器：box 明细表 + 第一个错误偏移 + 修复建议 + GStreamer 重载轨道数
cargo run -- check fixtures/normal.mp4

# 修复：始终写入新文件，原文件永不覆盖；目标存在或与输入相同都会被拒绝
cargo run -- repair fixtures/unknown.mp4 --out /tmp/fixed.mp4 --reload

# GTK4 图形界面
cargo run -- gui
```

## 检查项说明

- 每个 box 输出：字节偏移、四字符类型、声明大小、嵌套层级、轨道归属
  （`track <id> (video|audio|...)`，来自 `tkhd`/`hdlr`）、状态、可修复性。
- 错误状态：`OK` / `UNKNOWN`（私有/未知块）/ `TRUNCATED`（尺寸越过文件尾）/
  `INVALID-SIZE` / `MALFORMED`。
- 可修复性：
  - `droppable`：顶层未知块，可在修复时剔除；
  - `trim-tail`：被截断的非必需顶层块，可裁掉损坏尾部；
  - `required`：`ftyp`/`moov`/`trak` 等关键块损坏，拒绝自动修复，提示重新混流。

## 安全策略：拒绝覆盖原文件

修复路径满足任一条件即报错退出（退出码 2）：

1. 输出路径规范化后与输入是同一文件 → `SameAsInput`；
2. 输出文件已存在 → `OutputExists`；
3. 容器本身没有可修复问题 → `NothingToRepair`；
4. 关键块损坏无法安全修复 → `RequiredBoxDamaged`。

写入采用同目录 `.part` 临时文件再 `rename`，避免半成品出现在目标路径。

## 测试夹具与验证

```bash
bash tests/make_fixtures.sh   # ffmpeg 生成 normal / truncated / unknown 三份样本
cargo test --offline
```

三种容器的实测结果：

| 容器 | 第一个错误偏移 | 修复建议 | 重载后轨道数 |
| --- | --- | --- | --- |
| `normal.mp4` | 无 | 无需修复 | 2（video + audio） |
| `truncated.mp4` | 2443（`mdat` 截断） | trim-tail，裁掉尾部 | 2（video + audio） |
| `unknown.mp4` | 32（私有 `zzzz` 块） | droppable，剔除未知块 | 2（video + audio） |

## 模块布局

- `src/isobmff.rs` — box 遍历器、轨道归属、错误分类、修复判定
- `src/repair.rs` — 保守修复与覆盖保护
- `src/gst_check.rs` — GStreamer `filesrc ! qtdemux ! fakesink` 重载计数
- `src/report_view.rs` — 文本报告（CLI/GUI 共用）
- `src/gui.rs` — GTK4 界面（明细表、错误面板、修复/重载按钮）
