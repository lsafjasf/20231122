#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/env.sh

if [ ! -f artifacts/report.json ]; then
  cargo run -- demo
fi
exec cargo run -- ui
