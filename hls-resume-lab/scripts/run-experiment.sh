#!/usr/bin/env bash
# Reproducible end-to-end experiment driver.
# Runs the two-version + truncated-control diagnosis, replays the journal,
# verifies cleanup, and leaves durable evidence under evidence/.
set -euo pipefail

LAB_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$LAB_ROOT"

# shellcheck source=env.sh
source "$LAB_ROOT/scripts/env.sh"

RUN_ID="${1:-run-$(date +%Y%m%d-%H%M%S)}"

cargo build

echo "=== 1. execute experiment ($RUN_ID) ==="
./target/debug/hls-lab run run-config.json "$RUN_ID"

echo
echo "=== 2. replay journal step by step ==="
./target/debug/hls-lab replay "evidence/$RUN_ID.journal.jsonl" | sed -n '1,6p'
echo "..."

echo
echo "=== 3. evidence inventory ==="
ls -la "evidence/$RUN_ID.journal.jsonl" "evidence/$RUN_ID.report.json"

echo
echo "=== 4. temp state cleanup check ==="
if [ -e "/tmp/hls-resume-lab-$RUN_ID" ]; then
  echo "ERROR: scratch directory still present"
  exit 1
else
  echo "scratch directory removed; durable evidence retained"
fi
