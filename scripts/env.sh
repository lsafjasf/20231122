#!/bin/sh
# Source this for running compiled binaries without cargo.
export SYSROOT=/home/hj/sysroot/root
export PATH="/home/hj/.cargo/bin:$SYSROOT/usr/bin:$PATH"
export PKG_CONFIG="$SYSROOT/usr/bin/pkg-config"
export PKG_CONFIG_SYSROOT_DIR="$SYSROOT"
export PKG_CONFIG_PATH="$SYSROOT/usr/lib/x86_64-linux-gnu/pkgconfig:$SYSROOT/usr/share/pkgconfig"
export LD_LIBRARY_PATH="$SYSROOT/usr/lib/x86_64-linux-gnu${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export GST_PLUGIN_PATH="$SYSROOT/usr/lib/x86_64-linux-gnu/gstreamer-1.0"
export GST_PLUGIN_SYSTEM_PATH=/usr/lib/x86_64-linux-gnu/gstreamer-1.0
export RUSTUP_HOME=/home/hj/.rustup
export CARGO_HOME=/home/hj/.cargo

