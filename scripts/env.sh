#!/usr/bin/env bash
# Local sysroot toolchain. Source from the repository root:
#   source scripts/env.sh
export CARGO_HOME=/home/hj/.cargo
export RUSTUP_HOME=/home/hj/.rustup
export SYSROOT=/home/hj/sysroot
export PATH="$CARGO_HOME/bin:$SYSROOT/usr/bin:$PATH"
export LD_LIBRARY_PATH="$SYSROOT/root/usr/lib/x86_64-linux-gnu:/home/hj/env-media/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export PKG_CONFIG="$SYSROOT/usr/bin/pkgconf"
export PKG_CONFIG_SYSROOT_DIR="$SYSROOT/root"
export PKG_CONFIG_PATH="$SYSROOT/root/usr/lib/x86_64-linux-gnu/pkgconfig:$SYSROOT/root/usr/share/pkgconfig:/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig"
export GST_PLUGIN_PATH="$SYSROOT/root/usr/lib/x86_64-linux-gnu/gstreamer-1.0"
export GST_REGISTRY="/tmp/source-ledger-gst-registry.bin"
export XDG_DATA_DIRS="$SYSROOT/root/usr/share:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"
export GSK_RENDERER="${GSK_RENDERER:-cairo}"
unset http_proxy https_proxy HTTP_PROXY HTTPS_PROXY ALL_PROXY all_proxy
