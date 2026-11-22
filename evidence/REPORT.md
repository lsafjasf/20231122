# 实验报告：变帧率剪辑预览的可回放实验

- 技术栈（固定）：Rust 1.100 / GStreamer 1.28.2 / GTK 4.22.4
- 运行时间：见 `run/SHA256SUMS` 同目录文件时间戳
- 证据目录：`evidence/run/`

本报告把**机器自动得出的结论**与**需要人工确认的事项**严格分开。自动结论
全部可由 `./run_experiment.sh` 重新执行复现；人工确认项无法仅由程序断言，需
要人来判断。

---

## A. 自动结论（程序已验证，可重复执行）

以下每条均由自动化测试或运行日志直接支撑，不包含主观判断。

1. **可回放性成立**：从磁盘恢复的 `journal.jsonl` 经 genesis 状态重放 10 步，
   `chain_ok=true`，`state_divergences=[]`，`outcome_divergences=[]`，最终状态
   与首次运行逐字段相等。
   - 复现：`./target/debug/vfr-lab replay evidence/run/journal.jsonl`
2. **输入/状态/结果全量留痕**：每条 journal 记录含 `command`（输入）、
   `state_before`/`state_after`（状态）、`outcome`（结果）、`prev_hash`、
   `entry_hash`。
3. **顺序改变被正确处理**：`Swap(0,1)` 后时间线由 `[vfr-1, bumper]` 变为
   `[bumper, vfr-1]`，重放后顺序一致（见 `tests/replay_integration.rs`）。
4. **重复提交具备幂等性**：相同幂等键 `idem-remove` 第二次提交返回
   `Duplicate{original_seq:7}`，`revision` 不再增长，时间线不发生二次变化。
5. **未知扩展作为对照被记录但不生效**：`x-experimental-twist` 返回
   `UnknownIgnored`，库与时间线内容不变，仅 `revision` 计数前进。
6. **VFR 为真实变帧率而非伪装**：合成帧 PTS 为
   `[0, 33, 66, 133, 166, 199, 266] ms`，相邻间隔 33/33/67ms 不等；经
   `vp8enc + webmmux` 编码并由 `decodebin` 解码后，探测到的 PTS 与写入值完全
   一致（`evidence.json` 的 `probed_pts_ns == fixture_pts_ns`）。
7. **帧率归一化由 GStreamer 实际完成**：
   - 24fps 预览：7 个 VFR 输入帧 → 8 个输出帧，输出 PTS 间隔恒定
     41,666,666±1 ns；
   - 60fps 预览：7 个输入帧 → 17 个输出帧，输出 PTS 间隔恒定
     16,666,666±1 ns（±1ns 为整数纳秒累计舍入）。
8. **防篡改证据链有效**：篡改 `journal.jsonl` 首条记录后重放，立即报
   `CHAIN FAILURE: seq 0 content hash mismatch` 并以非零码退出。
9. **测试与静态检查全绿**：13 个测试通过（5 纯核心 + 5 集成 + 3 GStreamer
   真实管线），`cargo clippy --all-targets` 0 警告，`cargo fmt --check` 通过。
10. **GTK4 真实渲染成功**：Broadway 无头后端产出 736×457 PNG
    （`gtk_snapshot.png`），界面逐行展示 10 个步骤的输入→结果与最终时间线。

---

## B. 人工确认（程序无法替代的判断）

1. **画面语义是否符合剪辑意图**：快照文字与状态正确，但 VFR 片段的实际画面
   节奏/观感需要人在真实显示器上播放 `vfr_sample.webm` 主观确认。
2. **未知扩展的业务定义**：程序只能证明“未识别即忽略且留痕”；该扩展未来应
   如何被解释、是否升级为正式能力，属于产品/人工决策。
3. **证据链的信任根**：SHA-256 链能检测事后篡改，但首个 genesis 哈希本身的
   信任需要人工/外部公证（如签名或写入只读存储）来确立。
4. **时间线交互体验**：当前 GTK 界面为只读回放视图；拖拽排序、撤销等交互的
   易用性需人工可用性评估。
5. **跨环境一致性**：本次结论来自本机 user-local sysroot；在目标发行版/硬件
   上（尤其 GPU、不同 GStreamer 插件版本）需人工再跑一次确认。

---

## C. 失败时的可执行后续操作

| 失败现象 | 直接可执行的排查/修复命令 |
| --- | --- |
| `cargo: command not found` | `source /home/hj/local/env.sh`（注入 `~/.cargo/bin`） |
| 网络报错 `Failed to connect to 127.0.0.1:7897` | `unset http_proxy https_proxy HTTP_PROXY HTTPS_PROXY all_proxy ALL_PROXY` 后重试（本机代理已失效，直连可用） |
| `pkg-config` 找不到 gstreamer/gtk4 | 确认 `source /home/hj/local/env.sh`；检查 `echo $PKG_CONFIG_PATH` 含 `/home/hj/local/usr/lib/x86_64-linux-gnu/pkgconfig` |
| GStreamer `no element "fakesink"` | 确认 `GST_PLUGIN_SYSTEM_PATH` 同时含本地与系统插件目录（env.sh 已设置）；用 `gst-inspect-1.0 fakesink` 验证 |
| 管线 `negotiation problem` | `GST_DEBUG=2 ./target/debug/vfr-lab fixture /tmp/t.webm` 查看协商细节；确认 appsrc caps 不含固定 framerate 字段 |
| 重放报 `CHAIN FAILURE` | 说明 journal 被改动或损坏；用 `./target/debug/vfr-lab tamper` 对照，或从可信备份恢复 `journal.jsonl` 后重新 `replay` |
| GTK `Failed to open display` | 先启动守护进程 `gtk4-broadwayd -p 8097 :7`，确认 socket `/run/user/$UID/broadway8.socket` 存在，再带 `GDK_BACKEND=broadway BROADWAY_DISPLAY=:7` 运行 |
| Broadway 端口被占 `Address already in use` | 更换 `run_experiment.sh` 中的 `HTTP_PORT`/`DISPLAY_NUM`，或 `pkill -f broadwayd` 后重试 |
| Wayland 下 `failed to create dri2 screen` | 本机无 GPU，改用 Broadway 后端（脚本默认路径），不要用 `GDK_BACKEND=wayland` |
| 测试出现 1ns PTS 误差失败 | 属整数纳秒舍入，断言已用 `abs_diff(step)<=1` 容忍；若误差变大需检查 `videorate`/caps 配置 |

---

## D. 证据清单与校验

```bash
cd evidence/run
sha256sum -c SHA256SUMS      # 应全部 OK
```

- `journal.jsonl`：10 条哈希链式记录（输入/前后状态/结果）。
- `journal.tampered.jsonl`：被故意篡改的副本，重放必失败。
- `evidence.json`：媒体 PTS、两次预览结果、重放报告、最终状态。
- `vfr_sample.webm`：真实 VFR WebM 夹具。
- `gtk_snapshot.png`：GTK4 无头渲染快照。
- `SHA256SUMS`：上述文件的 SHA-256 指纹。
