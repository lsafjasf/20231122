#!/usr/bin/env bash
# 生成 4 秒带底部字幕的测试片 /tmp/ssr-test.ogg
set -euo pipefail
cd "$(dirname "$0")/.."
source env.sh 2>/dev/null || true
gst-launch-1.0 videotestsrc num-buffers=100 pattern=smpte \
  ! video/x-raw,width=640,height=360,framerate=25/1 \
  ! textoverlay text="TEST SUB 测试字幕" valignment=bottom halignment=center font-desc="Sans 28" \
  ! theoraenc ! oggmux ! filesink location=/tmp/ssr-test.ogg
echo "已生成 /tmp/ssr-test.ogg"
