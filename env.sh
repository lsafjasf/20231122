#!/usr/bin/env bash
# Local toolchain environment (no root required). Source before cargo/run.
SR="$HOME/sysroot"
export PATH="$HOME/.cargo/bin:$PATH"
unset http_proxy https_proxy HTTP_PROXY HTTPS_PROXY ALL_PROXY all_proxy

export PKG_CONFIG="$SR/usr/bin/pkgconf"
export PKG_CONFIG_PATH="$SR/usr/lib/x86_64-linux-gnu/pkgconfig:$SR/usr/share/pkgconfig"
export PKG_CONFIG_SYSROOT_DIR="$SR"

export LD_LIBRARY_PATH="$SR/usr/lib/x86_64-linux-gnu${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export GST_PLUGIN_PATH="$SR/usr/lib/x86_64-linux-gnu/gstreamer-1.0"
export GST_REGISTRY="$HOME/.cache/noise-provenance-gst-registry.bin"
export GST_PLUGIN_SYSTEM_PATH=""
