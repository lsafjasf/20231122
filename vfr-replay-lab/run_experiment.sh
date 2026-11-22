#!/usr/bin/env bash
# Reproducible end-to-end experiment runner.
#
# 1. Builds every layer (pure core, GStreamer media, GTK4 shell).
# 2. Runs the recorded scenario (journal + media evidence).
# 3. Replays the recovered journal and proves tamper detection.
# 4. Renders the GTK4 view headlessly through Broadway.
#
# Outputs land in ../evidence/run/.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
EVIDENCE="$HERE/../evidence/run"
mkdir -p "$EVIDENCE"

# Local sysroot + cargo (no system install / sudo required).
source /home/hj/local/env.sh
unset http_proxy https_proxy HTTP_PROXY HTTPS_PROXY all_proxy ALL_PROXY

cd "$HERE"

echo "== [1/5] build all features =="
cargo build --features gtk

echo "== [2/5] test suite (core + integration + GStreamer) =="
cargo test --features gtk

echo "== [3/5] recorded scenario =="
./target/debug/vfr-lab run-scenario "$EVIDENCE"

echo "== [4/5] recovered replay + tamper control =="
./target/debug/vfr-lab replay "$EVIDENCE/journal.jsonl"
./target/debug/vfr-lab tamper "$EVIDENCE/journal.jsonl"
if ./target/debug/vfr-lab replay "$EVIDENCE/journal.tampered.jsonl"; then
  echo "ERROR: tampered journal was accepted" >&2
  exit 1
else
  echo "tamper detection confirmed (non-zero exit as expected)"
fi

echo "== [5/5] GTK4 headless render (Broadway) =="
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
# Pick a display number/port unlikely to collide with stale daemons. The
# broadway socket is broadway<N+1>.socket, so :7 uses broadway8.socket.
DISPLAY_NUM=7
HTTP_PORT=8097
SOCKET="$XDG_RUNTIME_DIR/broadway$((DISPLAY_NUM + 1)).socket"
setsid gtk4-broadwayd -p "$HTTP_PORT" ":$DISPLAY_NUM" \
  >"$EVIDENCE/broadwayd.log" 2>&1 < /dev/null &
BROADWAY_PID=$!
for _ in $(seq 1 20); do
  [ -S "$SOCKET" ] && break
  sleep 0.5
done
GDK_BACKEND=broadway BROADWAY_DISPLAY=":$DISPLAY_NUM" \
  ./target/debug/vfr-gtk "$EVIDENCE/journal.jsonl" "$EVIDENCE/gtk_snapshot.png"
kill "$BROADWAY_PID" 2>/dev/null || true

echo
echo "Experiment complete. Evidence in: $EVIDENCE"
ls -la "$EVIDENCE"
