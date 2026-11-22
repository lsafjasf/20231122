#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MEDIA_PREFIX="${ENCODE_PANEL_MEDIA_PREFIX:-/home/hj/env-media}"

export PATH="/home/hj/.cargo/bin:${MEDIA_PREFIX}/bin:/home/hj/local/usr/bin:${PATH}"
export LD_LIBRARY_PATH="/home/hj/local/usr/lib/x86_64-linux-gnu:${MEDIA_PREFIX}/lib:${LD_LIBRARY_PATH:-}"
export PKG_CONFIG_PATH="${MEDIA_PREFIX}/lib/pkgconfig:${MEDIA_PREFIX}/share/pkgconfig:${PKG_CONFIG_PATH:-}"
export PKG_CONFIG="${MEDIA_PREFIX}/bin/pkg-config"
export GST_PLUGIN_SYSTEM_PATH_1_0="${MEDIA_PREFIX}/lib/gstreamer-1.0"
export GDK_BACKEND="${GDK_BACKEND:-x11}"

cd "$ROOT"
