#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/env.sh"

VERIFY_DIR="$ROOT/artifacts/verification"
mkdir -p "$VERIFY_DIR"

echo "[1/7] format check"
cargo fmt --check | tee "$VERIFY_DIR/fmt.log"

echo "[2/7] clippy"
cargo clippy --all-targets -- -D warnings | tee "$VERIFY_DIR/clippy.log"

echo "[3/7] unit tests (Rust + GStreamer render)"
cargo test | tee "$VERIFY_DIR/tests.log"

echo "[4/7] replayable experiment"
cargo run -- run --scenario experiments/audio-undo.undoexp --evidence artifacts/evidence \
    | tee "$VERIFY_DIR/experiment.log"

echo "[5/7] replay from retained JSONL evidence"
cargo run -- replay --evidence artifacts/evidence | tee "$VERIFY_DIR/replay.log"

echo "[6/7] GTK4 headless startup"
timeout 20s target/debug/audio-undo-lab ui --selftest | tee "$VERIFY_DIR/ui-selftest.log"

echo "[7/7] evidence and cleanup assertions"
{
    jq -e '.ok == true' artifacts/evidence/summary.json
    jq -e '.replay.ok == true' artifacts/evidence/summary.json
    jq -e '.cleanup.removed == true and .cleanup.exists_after == false' artifacts/evidence/summary.json
    jq -e 'select(.event == "step" and .result.status == "duplicate_ignored")' artifacts/evidence/events.jsonl >/dev/null
    jq -e 'select(.event == "object_complete" and .object_id == "after-exception-object" and .passed == true)' artifacts/evidence/events.jsonl >/dev/null
    test ! -e artifacts/tmp
    test -f artifacts/evidence/controls/snapshot_v1.json
    test -f artifacts/evidence/controls/snapshot_v2.json
    test -f artifacts/evidence/controls/snapshot_v1.wav
    test -f artifacts/evidence/controls/snapshot_v2.wav
} | tee "$VERIFY_DIR/assertions.log"

echo "verification passed"
