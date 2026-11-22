#!/usr/bin/env bash
# One-command acceptance for the seamless-loop pipeline.
# Usage: scripts/acceptance.sh [ui]
set -euo pipefail
cd "$(dirname "$0")/.."
source /home/hj/local/env.sh
export LOOP_WORKSPACE="$PWD"
cargo build
if [ "${1:-}" = "ui" ]; then
  pgrep -f 'gtk4-broadwayd :12' >/dev/null || setsid gtk4-broadwayd :12 >/tmp/broadwayd12.log 2>&1 &
  sleep 1
  GDK_BACKEND=broadway BROADWAY_DISPLAY=:12 exec ./target/debug/seamless-loop ui
else
  exec ./target/debug/seamless-loop verify
fi
