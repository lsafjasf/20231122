#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT/scripts/env.sh"

mkdir -p artifacts/evidence

cargo test --all-targets 2>&1 | tee artifacts/evidence/tests.txt
cargo test --test live_gst -- --ignored 2>&1 | tee artifacts/evidence/gstreamer-test.txt
cargo run --quiet -- --headless --out artifacts/evidence
cargo run --quiet -- --headless --gstreamer --extreme-pipeline \
  --duration-ms 4000 --out artifacts/evidence

if [ -n "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]; then
  target/debug/encode-panel --ui-smoke-ms 3500 \
    --screenshot artifacts/evidence/ui-smoke.png
fi

"$ROOT/scripts/index_evidence.sh"
