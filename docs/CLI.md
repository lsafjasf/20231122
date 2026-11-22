# CLI 使用指南

所有命令都作用于一个项目目录，命令之间可随时退出、稍后重新进入；状态持久化在
`project.json` 与 `evidence/` 中。

## 典型流程

```sh
# 1. 建项目、注册两台设备
vkri init ./case7 --name case7 --description "object 7, two cameras"
vkri add-device ./case7 --label camera-A
vkri add-device ./case7 --label camera-B
vkri set-offset ./case7 --device-id <devB> --offset-ns 1500000000 --uncertainty-ns 50000000

# 2. 注册片段（哈希+探测，不解码）
vkri register ./case7 --device-id <devA> --label frag-A \
  --source /media/a.webm --clock-origin-unix-ns 1700000000000000000 \
  --group object_7 --mode interval --interval-ns 1000000000
# 对设备 B 重复 register（同一 --group object_7）

# 3. 每个片段单独提取；Ctrl-C 可安全中断，之后再次 run 即可
vkri run ./case7 --segment-id <segA>
vkri run ./case7 --segment-id <segB>

# 4. 局部修正：改配置会把已提取片段标记为 stale，旧证据保留
vkri configure ./case7 --segment-id <segA> --mode interval \
  --interval-ns 500000000 --thumbnail-max-edge 320 --jpeg-quality 90
vkri request-rerun ./case7 --segment-id <segA>
vkri run ./case7 --segment-id <segA>

# 5. 比对两台设备并获得时钟偏移建议
vkri compare ./case7 --segment-a <segA> --segment-b <segB> --match-window-ns 500000000

# 6. 导出并独立复核
vkri export ./case7 --export-root ./exports --bundle case7
vkri verify-export ./exports/case7
(cd ./exports/case7 && sha256sum -c SHA256SUMS)
```

## 状态语义

- `pending` 已注册未提取；`extracted` 最近一次运行成功；`interrupted` 上次被中断/失败
  （部分证据保留）；`stale` 配置变化或显式要求重跑，等待下一次运行替换索引。
- 每次运行都生成独立 `run-<run_id>/`，历史 run 不删除；当前关键帧索引仅在完整成功后切换。
