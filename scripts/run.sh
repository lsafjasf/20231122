#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT/scripts/env.sh"

if [[ -z "${DISPLAY:-}" && -z "${WAYLAND_DISPLAY:-}" ]]; then
  echo "No display detected; running the evidence exporter instead." >&2
  exec cargo run --manifest-path "$ROOT/Cargo.toml" -- --headless
fi

export GDK_BACKEND="${GDK_BACKEND:-x11}"
export GSK_RENDERER="${GSK_RENDERER:-cairo}"
exec cargo run --manifest-path "$ROOT/Cargo.toml" --

