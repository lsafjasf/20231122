#!/usr/bin/env bash

set -u

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MEDIA_PREFIX="${MEDIA_PREFIX:-/home/hj/env-media}"
RUSTUP_HOME="${RUSTUP_HOME:-/home/hj/.rustup}"
CARGO_HOME="${CARGO_HOME:-/home/hj/.cargo}"
TOOLCHAIN_BIN="${TOOLCHAIN_BIN:-$RUSTUP_HOME/toolchains/stable-x86_64-unknown-linux-gnu/bin}"

export ROOT
export MEDIA_PREFIX
export RUSTUP_HOME
export CARGO_HOME
export PATH="$MEDIA_PREFIX/bin:$TOOLCHAIN_BIN:$CARGO_HOME/bin:$PATH"

export PKG_CONFIG_PATH="$MEDIA_PREFIX/lib/pkgconfig"
if [ -d /home/hj/sysroot/root/usr/lib/x86_64-linux-gnu/pkgconfig ]; then
    export PKG_CONFIG_PATH="$PKG_CONFIG_PATH:/home/hj/sysroot/root/usr/lib/x86_64-linux-gnu/pkgconfig"
    export PKG_CONFIG_PATH="$PKG_CONFIG_PATH:/home/hj/sysroot/root/usr/share/pkgconfig"
fi
unset PKG_CONFIG_SYSROOT_DIR

export LD_LIBRARY_PATH="$MEDIA_PREFIX/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export GST_PLUGIN_PATH="$MEDIA_PREFIX/lib/gstreamer-1.0"
export GST_REGISTRY="${GST_REGISTRY:-/tmp/audio-undo-gst-registry-${UID}.bin}"
export XDG_DATA_DIRS="$MEDIA_PREFIX/share:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"
export GSK_RENDERER="${GSK_RENDERER:-cairo}"

cd "$ROOT"
