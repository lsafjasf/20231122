#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT/scripts/env.sh"
cd "$ROOT"

export GDK_BACKEND="${GDK_BACKEND:-x11}"
export GSK_RENDERER="${GSK_RENDERER:-cairo}"
export LIBGL_ALWAYS_SOFTWARE=1
cargo build
exec ./target/debug/bwf-replay-lab --screenshot "${1:-artifacts/bwf-replay-lab/ui.png}"

