#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MEDIA_PREFIX="${MEDIA_PREFIX:-/home/hj/env-media}"

export RUSTUP_HOME="${RUSTUP_HOME:-/home/hj/.rustup}"
export CARGO_HOME="${CARGO_HOME:-/home/hj/.cargo}"
export PATH="$CARGO_HOME/bin:$PATH"
export PKG_CONFIG="$MEDIA_PREFIX/bin/pkg-config"
export PKG_CONFIG_PATH="$MEDIA_PREFIX/lib/pkgconfig:${PKG_CONFIG_PATH:-}"
export LD_LIBRARY_PATH="$MEDIA_PREFIX/lib:${LD_LIBRARY_PATH:-}"
export FONTCONFIG_FILE="${FONTCONFIG_FILE:-$MEDIA_PREFIX/etc/fonts/fonts.conf}"

cd "$ROOT"
cargo test "$@"
