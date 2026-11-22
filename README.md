# 素材代理失效恢复：可回放实验

技术栈固定为 **Rust + GStreamer + GTK4**。实验把“素材代理失效 → 恢复 →
恢复后重放”做成可审计状态机，并加入未知扩展对照、恢复顺序扰动和重复提交。

## 实验设计

- 源素材：GStreamer `videotestsrc` 生成两张确定性 PNG，分别为 `smpte` 和 `ball`。
- 代理：GStreamer `pngdec → videoscale → pngenc` 生成 320x180 RGB PNG。
- 失效注入：保留代理元数据，替换代理前缀并删除原文件前 24 字节，使 SHA-256
  与解码同时失效。
- 恢复事务：校验源 SHA-256 → 写临时代理 → GStreamer 解码并计算原始帧指纹 →
  原子重命名 → 更新 generation 和代理哈希。
- 恢复后重放：对 `clip-a` 用同一输入再请求一次，必须命中缓存且不重写文件。
- 未知扩展对照：复制 `clip-a` 的相同字节为 `control-unknown.weird`；扩展检查
  发生在 GStreamer 解码之前，应隔离且不创建代理。
- 顺序改变：先失效 A/B，再按 A→B 恢复；再次失效后按 B→A 恢复，代理哈希与
  解码指纹必须一致。
- 重复提交：恢复后连续提交 `clip-a` 两次，必须均为 cache hit，generation 与
  文件修改时间不变。

状态机：

```text
missing -> recovering -> ready -> invalid -> recovering -> ready
                                  \-> quarantined（未知扩展）
```

每一步都写入 `journal.jsonl`，字段包括 `input`、`state_before`、
`state_after`、`result`、`evidence` 和追加时间。

## 运行

```bash
source scripts/env.sh

# 完整实验：生成 artifacts/latest/{journal.jsonl,REPORT.md,evidence/}
cargo run -- run

# 从日志重建完整场景，逐条比较输入、状态和结果
cargo run -- replay artifacts/latest/journal.jsonl

# 独立校验证据 SHA-256 清单
cargo run -- verify-artifacts artifacts/latest/evidence/manifest.json

# GTK4 界面
cargo run -- gui
```

无头环境可设 `GSK_RENDERER=cairo`。界面包含“自动结论 / 人工确认 / 失败后续
操作”三个分页，并提供运行、恢复记录回放和证据清单校验按钮。

## 回放与证据

`replay` 会读取日志中的 `experiment-config`，在 `artifacts/latest/replay/`
重建实验并比较每步语义。时间戳和证据路径不参与比较，以下内容必须一致：

- 输入参数、`state_before`、`state_after`、`result`；
- 源文件、代理文件、解码指纹；
- 缓存命中、generation、反序恢复检查和重复提交检查。

关键证据：

```text
artifacts/latest/
  journal.jsonl                         每步输入/状态/结果
  REPORT.md                             自动结论、人工确认、失败操作
  evidence/
    gst-env.txt                         GStreamer 版本与元素
    source-clip-*.png                   源素材副本
    proxy-clip-*-initial.png            初始代理
    proxy-clip-*-invalid-*.bin          失效样本
    proxy-clip-*-recover-*.png          各次恢复产物
    meta-clip-*.json                    各阶段元数据快照
    meta-control-unknown.json           未知扩展隔离状态
    manifest.json                       证据文件 SHA-256 清单
  replay/
    journal.jsonl                       回放重建日志
    REPLAY.md                           回放比较结果
    replay-comparison.json              机器可读匹配结果
```

## 自动检查

| 检查项 | 约束 |
| --- | --- |
| `recovery-restores-valid-proxy` | 恢复后代理哈希等于初始代理，且 GStreamer 解码尺寸正确 |
| `post-recovery-replay-no-regeneration` | 恢复后重放 cache hit，generation 与文件均不变 |
| `unknown-extension-control-quarantined` | 相同内容换未知扩展后隔离，未调用解码、未创建代理 |
| `recovery-order-independent` | A→B 与 B→A 的代理哈希及原始帧指纹一致 |
| `duplicate-submission-idempotent` | 两次重复提交均 cache hit，generation/文件不变 |
| `journal-and-key-evidence-retained` | 必需步骤、输入/状态/结果、失效哈希和证据文件齐全 |

本次执行的自动结论为 **6/6 通过**，回放为 **23/23 步一致**，证据清单
35 个文件校验通过。具体哈希见 `artifacts/latest/REPORT.md`。

自动结论只覆盖可计算约束，不替代以下人工判断：

- 代理画面是否业务正确；
- 未知扩展隔离是否符合产品策略；
- 真实素材格式、色彩和缩放算法是否可接受；
- 界面分区与失败操作是否可执行。

## 失败处置

`REPORT.md` 的自动结论、人工确认和失败后续操作分栏输出。任一自动检查失败时，
报告会列出该检查的复现命令和观察项。通用处理顺序：

```bash
source scripts/env.sh
cargo test
cargo run -- replay artifacts/latest/journal.jsonl
GST_DEBUG=3 cargo run -- run --out /tmp/proxyrecover-debug \
  2>/tmp/proxyrecover-gst.log
gst-inspect-1.0 pngdec pngenc videoscale
```

如果证据清单校验失败，不要继续使用该证据包；从新目录重跑实验。

## 构建说明

当前容器没有系统级开发包，`.cargo/config.toml` 指向
`/home/hj/sysroot/root` 的本地 sysroot。部分运行时库由宿主提供，
`build.rs` 会在 target 目录创建链接目录补齐 pkg-config 所需的未版本化
`.so`，不会修改 sysroot。依赖版本与已验证环境：

- Rust 1.100
- GStreamer 1.28.2
- GTK 4.22

测试：

```bash
cargo test
```

测试会运行完整场景、独立回放、证据清单校验，并验证篡改证据会被拒绝。
