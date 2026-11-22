#!/usr/bin/env bash
# End-to-end demo of the pausable subtitle font coverage pipeline:
# analyze with an auto-pause, resume under simulated memory pressure
# (degraded path), then independently verify the exported records.
set -euo pipefail
cd "$(dirname "$0")/.."

BIN=target/debug/subtitle-font-coverage
OUT="${1:-/tmp/sfc-demo}"

cargo build
[ -d "$OUT" ] && rm -r "$OUT"

echo "== session 1: analyze, auto-pause after 8 cues =="
"$BIN" analyze testdata/sample.srt --out "$OUT" --pause-after 8

echo "== session 2: resume with low memory (degraded path) =="
SFC_FAKE_MEM_KB=1024 "$BIN" resume testdata/sample.srt --out "$OUT"

echo "== independent verification =="
"$BIN" verify --dir "$OUT"

echo "artifacts in $OUT: report.json report.md checkpoint.json evidence.jsonl"
