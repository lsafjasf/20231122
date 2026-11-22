#!/usr/bin/env bash
# Replayable entry point for the sound-effect batch variant experiment.
# Fixed stack: Rust + GStreamer + GTK4.
set -euo pipefail

cd "$(dirname "$0")"
# shellcheck source=/dev/null
source ./env.sh

EVIDENCE_ROOT="${EVIDENCE_ROOT:-./evidence}"
WORK_DELAY_MS="${WORK_DELAY_MS:-120}"

echo "==> toolchain"
rustc --version
pkg-config --modversion gstreamer-1.0 gtk4 | sed 's/^/    native: /'

echo "==> build"
cargo build --workspace

echo "==> headless experiment (six scenarios, seven auto checks)"
./target/debug/svcli --evidence "$EVIDENCE_ROOT" experiment --work-delay-ms "$WORK_DELAY_MS"

echo "==> render separate manual review document"
./target/debug/svcli --evidence "$EVIDENCE_ROOT" render-review

echo "==> GTK4 smoke (auto-submit, mid-run cancel, auto-quit)"
mkdir -p "$EVIDENCE_ROOT/gui"
if [ -d /tmp/svbatch-gtk-output ]; then
  mv /tmp/svbatch-gtk-output "/tmp/svbatch-gtk-output.bak.$(date +%s)"
fi
(setsid ./target/debug/svgui --smoke >"$EVIDENCE_ROOT/gui/smoke-live.log" 2>&1 < /dev/null &)
sleep 3
if pgrep -x svgui >/dev/null; then
  echo "    WARNING: svgui still running after smoke window" >&2
  pkill -x svgui || true
fi
grep -E '\[smoke\]' "$EVIDENCE_ROOT/gui/smoke-live.log" || true

echo "==> evidence tree"
find "$EVIDENCE_ROOT" -maxdepth 2 -type d | sort
echo "done. Read:"
echo "  - $EVIDENCE_ROOT/reports/auto-conclusion.json   (machine verdict)"
echo "  - $EVIDENCE_ROOT/reports/manual-review.md        (human checklist)"
