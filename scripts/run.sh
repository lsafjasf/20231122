#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

if [[ -d "$ROOT/.sysroot/root/usr" ]]; then
  export PATH="$ROOT/.sysroot/root/usr/bin:$PATH"
  export LD_LIBRARY_PATH="$ROOT/.sysroot/root/usr/lib/x86_64-linux-gnu:$ROOT/.sysroot/root/lib/x86_64-linux-gnu:${LD_LIBRARY_PATH:-}"
  export PKG_CONFIG_PATH="$ROOT/.sysroot/root/usr/lib/x86_64-linux-gnu/pkgconfig:$ROOT/.sysroot/root/usr/share/pkgconfig"
  export PKG_CONFIG_SYSROOT_DIR="$ROOT/.sysroot/root"
  export GST_PLUGIN_PATH_1_0="$ROOT/.sysroot/root/usr/lib/x86_64-linux-gnu/gstreamer-1.0"
  export GST_PLUGIN_SYSTEM_PATH_1_0="$GST_PLUGIN_PATH_1_0"
  export XDG_DATA_DIRS="$ROOT/.sysroot/root/usr/share:/usr/share"
  export GSETTINGS_SCHEMA_DIR="$ROOT/.sysroot/root/usr/share/glib-2.0/schemas"
fi

CARGO_BIN="${CARGO_BIN:-cargo}"
if ! command -v "$CARGO_BIN" >/dev/null 2>&1; then
  CARGO_BIN="$HOME/.cargo/bin/cargo"
fi

exec "$CARGO_BIN" run "$@"
