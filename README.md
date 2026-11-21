# dub-switch:可暂停的配音语言切换处理流程

技术栈:Rust + GStreamer + GTK4。

## 概念映射

- **重复对象**:`DubJob`(`crates/dub-core/src/model.rs`)。整个流程围绕同一个
  job 反复运行:创建 → 分段处理 → 暂停 → 恢复 → 再恢复,直到完成。
- **两台设备时钟不同步**:`clocks.rs`。设备 A(处理节点)与设备 B(证据记录器)
  各有独立的 epoch 偏移与漂移;每条证据记录同时盖上两个时钟的时间戳,
  `ClockSync` 采样表支持两个时钟域互相换算,并随状态与报告一起导出。
- **保存中间状态 / 恢复后比较前后结果**:`state.rs` 每个分段完成后原子写入
  `checkpoint.json`;`pipeline.rs` 在恢复时先重算暂停前产物的 SHA-256 并与
  checkpoint 比对(`resume-compare` 证据),一致才继续处理。
- **资源不足与降级**:`degrade.rs`。每个分段前探测磁盘余量与所需 GStreamer
  元素;不足时降级为 8 kHz 单声道 WAV 直通路径,并记录 `degrade` 证据。
- **关键证据**:`evidence.rs`。仅追加的 JSONL 哈希链(`prev_hash` 链接),
  篡改任何一条记录都会导致 `verify` 失败。
- **重载产物保持字段和顺序**:`state.rs` 固定顶层字段顺序
  (`STATE_FIELD_ORDER`),加载时逐字段校验顺序,重序列化后字节一致。
- **同事独立复核**:`report.rs` 的 `verify_workdir` 不信任任何应用状态,
  仅从磁盘重算:checkpoint 字段顺序、证据哈希链、全部分段哈希、时钟同步表、
  分段覆盖完整性。

## 布局

- `crates/dub-core` — 流程核心(模型、双时钟、证据链、状态、降级、GStreamer 管线)
- `crates/dub-cli` — 命令行:`make-sample / start / resume / report / verify`
- `crates/dub-gui` — GTK4 界面(Start / Pause / Resume / Verify,进度条与日志)

## 运行

```sh
cargo run -p dub-cli -- make-sample --out /tmp/demo/sample.mkv
cargo run -p dub-cli -- start --input /tmp/demo/sample.mkv --workdir /tmp/demo/work \
    --languages eng,zho,eng,zho --pause-after 2   # 处理 2 段后暂停并存档
cargo run -p dub-cli -- resume --workdir /tmp/demo/work   # 比对前后结果后继续
cargo run -p dub-cli -- report --workdir /tmp/demo/work   # 导出 report.json
cargo run -p dub-cli -- verify --workdir /tmp/demo/work   # 独立复核
```

降级路径可用 `--simulate-low-disk` / `--simulate-missing-encoder` 触发。

GUI:`cargo run -p dub-gui`(需要显示服务器;本容器无显示,仅验证编译链接)。

## 本机工具链说明

容器无 root,Rust 经 rustup 装在用户目录;GStreamer 1.28 / GTK4 4.22 的
Ubuntu 包解包在 `~/sysroot/root`,`.cargo/config.toml` 通过 pkg-config 包装
脚本与 RPATH(`--disable-new-dtags`)指向该 sysroot,因此 `cargo build/test/run`
无需额外环境变量。运行产物到 cargo 之外时,先 `source scripts/env.sh`。
