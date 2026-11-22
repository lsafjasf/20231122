#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT/scripts/env.sh"

cargo test --all-targets
cargo run --quiet -- demo
cargo run --quiet -- verify
